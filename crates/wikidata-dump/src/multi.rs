//! Reading one file from several mirrors at once.
//!
//! Wikimedia's dump servers and their mirrors cap bandwidth per client,
//! not per connection (dumps.wikimedia.org: ~5.5–8.6 MB/s, two
//! connections allowed; your.org: ~7 MB/s), so one connection reads a
//! 103 GB dump in about four hours. [`MultiReader`] splits the file into
//! ranges and fetches them from every mirror in parallel, a few
//! connections each, handing the bytes on in file order: throughput is
//! about the sum of the mirrors' caps.
//!
//! Memory is bounded: ranges are fetched at most `window` ahead of the
//! one being read. Every range request is checked (`Content-Range`, and
//! `If-Range` with that mirror's validator), a dropped range is resumed
//! where it stopped, and a mirror that keeps failing hands its ranges to
//! the others. The mirrors must agree on the file's size.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::{DumpError, HttpOptions, Result};

/// A mirror's size and validator (from a one-byte range request).
pub(crate) fn probe(agent: &ureq::Agent, url: &str) -> io::Result<(u64, Option<String>)> {
    let resp = agent
        .get(url)
        .header("Range", "bytes=0-0")
        .call()
        .map_err(io::Error::other)?;
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let validator = header("etag").or_else(|| header("last-modified"));
    match resp.status().as_u16() {
        206 => {
            let range = header("content-range").unwrap_or_default();
            let total = range.rsplit('/').next().and_then(|t| t.parse().ok());
            total
                .map(|t| (t, validator))
                .ok_or_else(|| io::Error::other(format!("no size in Content-Range {range:?}")))
        }
        200 => Err(io::Error::other("the server does not support range requests")),
        s => Err(io::Error::other(format!("HTTP status {s}"))),
    }
}

pub(crate) fn agent(opts: &HttpOptions) -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(opts.user_agent.as_str())
        .timeout_connect(Some(opts.timeout))
        .timeout_recv_response(Some(opts.timeout))
        .timeout_recv_body(None)
        .http_status_as_error(false)
        .build()
        .into()
}

/// Fetch bytes `[from, to)` of `url` into `out` (appending), resuming a
/// dropped connection where it stopped; `cancel` ends it early.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fetch_range(
    agent: &ureq::Agent,
    url: &str,
    validator: Option<&str>,
    from: u64,
    to: u64,
    retries: u32,
    cancel: &AtomicBool,
    out: &mut dyn FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let mut at = from;
    let mut attempts = 0u32;
    let mut buf = vec![0u8; 256 << 10];
    while at < to {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("cancelled"));
        }
        let result = (|| -> io::Result<()> {
            let mut req = agent.get(url).header("Range", format!("bytes={at}-{}", to - 1));
            if let Some(v) = validator {
                req = req.header("If-Range", v);
            }
            let resp = req.call().map_err(io::Error::other)?;
            match resp.status().as_u16() {
                206 => {}
                200 => return Err(io::Error::other("the file changed on the server (If-Range)")),
                s => return Err(io::Error::other(format!("HTTP status {s}"))),
            }
            let range = resp
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let start = range
                .strip_prefix("bytes ")
                .and_then(|r| r.split('-').next())
                .and_then(|s| s.parse::<u64>().ok());
            if start != Some(at) {
                return Err(io::Error::other(format!("wrong range {range:?} (wanted {at})")));
            }
            let (_, body) = resp.into_parts();
            let mut r = body.into_reader();
            while at < to {
                if cancel.load(Ordering::Relaxed) {
                    return Err(io::Error::other("cancelled"));
                }
                let want = buf.len().min((to - at) as usize);
                let n = r.read(&mut buf[..want])?;
                if n == 0 {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the connection closed"));
                }
                out(&buf[..n])?;
                at += n as u64;
                attempts = 0;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {}
            Err(e) if e.to_string().contains("changed on the server") => return Err(e),
            Err(e) if cancel.load(Ordering::Relaxed) => return Err(e),
            Err(e) => {
                if attempts >= retries {
                    return Err(io::Error::other(format!("{url}: bytes {at}–{to}: {e}")));
                }
                attempts += 1;
                std::thread::sleep(Duration::from_millis(200 * u64::from(attempts.min(10))));
            }
        }
    }
    Ok(())
}

struct State {
    /// The next range not handed to a worker yet.
    next: u64,
    /// Ranges given back by a failing mirror.
    retry: VecDeque<u64>,
    /// The range the reader waits for; workers stay within `window` of it.
    head: u64,
    /// Fetched ranges not read yet.
    done: BTreeMap<u64, Vec<u8>>,
    /// Bytes in `done` and being fetched.
    buffered: u64,
    /// Mirrors still working.
    alive: usize,
    error: Option<String>,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    cancel: AtomicBool,
    peak: AtomicU64,
}

/// Reads one file from several URLs (see the module docs).
pub struct MultiReader {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    total: u64,
    ranges: u64,
    current: Vec<u8>,
    pos: usize,
}

impl MultiReader {
    /// Read `urls` (the same file on each server) from byte `offset`, with
    /// `connections` connections per server.
    pub fn open(urls: &[String], offset: u64, opts: &HttpOptions) -> Result<MultiReader> {
        let agent = agent(opts);
        let mut size = None;
        let mut mirrors = Vec::new();
        for url in urls {
            let (len, validator) = probe(&agent, url).map_err(|e| DumpError::Http(format!("{url}: {e}")))?;
            if size.is_some_and(|s| s != len) {
                return Err(DumpError::Http(format!(
                    "the mirrors disagree on the size: {} has {len} bytes, {} has {}",
                    url,
                    urls[0],
                    size.unwrap()
                )));
            }
            size = Some(len);
            mirrors.push((url.clone(), validator));
        }
        let total = size.unwrap_or(0);
        let range = opts.range_size.max(64 << 10);
        let first = offset / range;
        let ranges = total.div_ceil(range);
        let connections = opts.connections.unwrap_or(2).max(1);
        let window = (mirrors.len() * connections * 2) as u64;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                next: first,
                retry: VecDeque::new(),
                head: first,
                done: BTreeMap::new(),
                buffered: 0,
                alive: mirrors.len(),
                error: None,
            }),
            cv: Condvar::new(),
            cancel: AtomicBool::new(false),
            peak: AtomicU64::new(0),
        });
        let mut threads = Vec::new();
        for (url, validator) in mirrors {
            // A mirror is given up when all its connections have failed.
            let failing = Arc::new(AtomicU64::new(0));
            for c in 0..connections {
                let (shared, agent, url, validator, failing) = (
                    shared.clone(),
                    agent.clone(),
                    url.clone(),
                    validator.clone(),
                    failing.clone(),
                );
                let retries = opts.retries;
                threads.push(
                    std::thread::Builder::new()
                        .name(format!("wikidata-mirror-{c}"))
                        .spawn(move || {
                            worker(
                                &shared,
                                &agent,
                                &url,
                                validator.as_deref(),
                                range,
                                total,
                                ranges,
                                window,
                                retries,
                            );
                            if failing.fetch_add(1, Ordering::SeqCst) + 1 == connections as u64 {
                                let mut s = shared.state.lock().unwrap();
                                s.alive -= 1;
                                shared.cv.notify_all();
                            }
                        })
                        .expect("spawn mirror thread"),
                );
            }
        }
        let mut r = MultiReader {
            shared,
            threads,
            total,
            ranges,
            current: Vec::new(),
            pos: 0,
        };
        // Skip to `offset` within its range.
        if offset < total {
            r.advance()?;
            r.pos = (offset - first * range) as usize;
        }
        Ok(r)
    }

    /// The file's size.
    pub fn len(&self) -> u64 {
        self.total
    }

    /// Whether the file is empty.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// The most bytes ever held at once (fetched or being fetched, not
    /// read yet).
    pub fn peak_buffered(&self) -> u64 {
        self.shared.peak.load(Ordering::Relaxed)
    }

    /// Wait for the next range; `false` at the end.
    fn advance(&mut self) -> Result<bool> {
        let mut s = self.shared.state.lock().unwrap();
        if s.head >= self.ranges {
            return Ok(false);
        }
        loop {
            let head = s.head;
            if let Some(data) = s.done.remove(&head) {
                s.buffered -= data.len() as u64;
                s.head += 1;
                self.shared.cv.notify_all();
                self.current = data;
                self.pos = 0;
                return Ok(true);
            }
            if let Some(e) = &s.error {
                return Err(DumpError::Http(e.clone()));
            }
            if s.alive == 0 {
                return Err(DumpError::Http("every mirror failed".into()));
            }
            s = self.shared.cv.wait(s).unwrap();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn worker(
    shared: &Shared,
    agent: &ureq::Agent,
    url: &str,
    validator: Option<&str>,
    range: u64,
    total: u64,
    ranges: u64,
    window: u64,
    retries: u32,
) {
    loop {
        let i = {
            let mut s = shared.state.lock().unwrap();
            loop {
                if shared.cancel.load(Ordering::Relaxed) || s.error.is_some() {
                    return;
                }
                if let Some(i) = s.retry.pop_front() {
                    break i;
                }
                if s.next < ranges && s.next < s.head + window {
                    s.next += 1;
                    break s.next - 1;
                }
                // Stay until everything was read: a failing mirror may
                // still give a range back.
                if s.head >= ranges {
                    return;
                }
                s = shared.cv.wait(s).unwrap();
            }
        };
        let (from, to) = (i * range, ((i + 1) * range).min(total));
        {
            let mut s = shared.state.lock().unwrap();
            s.buffered += to - from;
            shared.peak.fetch_max(s.buffered, Ordering::Relaxed);
        }
        let mut data = Vec::with_capacity((to - from) as usize);
        let r = fetch_range(agent, url, validator, from, to, retries, &shared.cancel, &mut |b| {
            data.extend_from_slice(b);
            Ok(())
        });
        let mut s = shared.state.lock().unwrap();
        match r {
            Ok(()) => {
                s.done.insert(i, data);
            }
            Err(e) => {
                s.buffered -= to - from;
                if e.to_string().contains("changed on the server") {
                    s.error = Some(format!("{url}: {e}"));
                } else {
                    // Another mirror takes it; this one is done.
                    s.retry.push_back(i);
                }
                shared.cv.notify_all();
                return;
            }
        }
        shared.cv.notify_all();
    }
}

impl Read for MultiReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.pos >= self.current.len() {
            if !self.advance().map_err(io::Error::other)? {
                return Ok(0);
            }
        }
        let n = buf.len().min(self.current.len() - self.pos);
        buf[..n].copy_from_slice(&self.current[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

impl Drop for MultiReader {
    fn drop(&mut self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
        self.shared.cv.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}
