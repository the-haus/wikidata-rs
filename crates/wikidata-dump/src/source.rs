//! Where dump bytes come from: a local file or an HTTP(S) URL.
//!
//! Both are read sequentially in large blocks. HTTP downloads stream (no
//! temporary file) and resume with a `Range` request when the connection
//! drops, checking that the file did not change in between (`If-Range`
//! with the `ETag` or `Last-Modified` of the first response).

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::{DumpError, Result};

/// The `User-Agent` sent with HTTP requests (Wikimedia asks for one that
/// names the tool and how to reach its authors).
pub const USER_AGENT: &str = concat!(
    "wikidata-rs/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/the-haus/wikidata-rs)"
);

/// A dump location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// A local file.
    Path(PathBuf),
    /// An `http://` or `https://` URL.
    Url(String),
}

impl Input {
    /// A URL when `s` starts with `http://` or `https://`, a path otherwise.
    pub fn parse(s: &str) -> Input {
        if s.starts_with("http://") || s.starts_with("https://") {
            Input::Url(s.to_string())
        } else {
            Input::Path(PathBuf::from(s))
        }
    }

    /// The path or URL, for messages.
    pub fn display(&self) -> String {
        match self {
            Input::Path(p) => p.display().to_string(),
            Input::Url(u) => u.clone(),
        }
    }

    /// The file name (the last path or URL segment).
    pub fn file_name(&self) -> String {
        match self {
            Input::Path(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Input::Url(u) => {
                let u = u.split(['?', '#']).next().unwrap_or(u);
                u.rsplit('/').next().unwrap_or(u).to_string()
            }
        }
    }
}

impl From<&str> for Input {
    fn from(s: &str) -> Self {
        Input::parse(s)
    }
}

impl From<String> for Input {
    fn from(s: String) -> Self {
        Input::parse(&s)
    }
}

impl From<&Path> for Input {
    fn from(p: &Path) -> Self {
        Input::Path(p.to_path_buf())
    }
}

impl From<PathBuf> for Input {
    fn from(p: PathBuf) -> Self {
        Input::Path(p)
    }
}

/// Options for HTTP inputs.
#[derive(Clone, Debug)]
pub struct HttpOptions {
    /// `User-Agent` header (default [`USER_AGENT`]).
    pub user_agent: String,
    /// Reconnect attempts after a dropped connection, per position.
    pub retries: u32,
    /// Timeout for connecting and for each read.
    pub timeout: Duration,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            user_agent: USER_AGENT.to_string(),
            retries: 8,
            timeout: Duration::from_secs(60),
        }
    }
}

/// Bytes read from the source so far, shared with progress reporting.
#[derive(Clone, Debug, Default)]
pub struct Counter(Arc<AtomicU64>);

impl Counter {
    /// The current count.
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    fn add(&self, n: u64) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }
}

/// An open source: a sequential reader, its size when known, and a
/// counter of the bytes read from it.
pub struct Source {
    reader: Box<dyn Read + Send>,
    counter: Counter,
    len: Option<u64>,
}

impl Source {
    /// Open `input` for reading from the start.
    pub fn open(input: &Input, http: &HttpOptions) -> Result<Source> {
        match input {
            Input::Path(p) => {
                let f = File::open(p).map_err(|e| DumpError::Io(format!("{}: {e}", p.display())))?;
                let len = f.metadata().ok().map(|m| m.len());
                Ok(Source {
                    reader: Box::new(f),
                    counter: Counter::default(),
                    len,
                })
            }
            Input::Url(url) => {
                let r = HttpReader::open(url, http.clone(), 0)?;
                let len = r.total;
                Ok(Source {
                    reader: Box::new(r),
                    counter: Counter::default(),
                    len,
                })
            }
        }
    }

    /// A source over any reader (stdin, an in-memory buffer, …).
    pub fn from_reader(reader: impl Read + Send + 'static, len: Option<u64>) -> Source {
        Source {
            reader: Box::new(reader),
            counter: Counter::default(),
            len,
        }
    }

    /// The size in bytes, when known.
    pub fn len(&self) -> Option<u64> {
        self.len
    }

    /// Whether the source is known to be empty.
    pub fn is_empty(&self) -> bool {
        self.len == Some(0)
    }

    /// The counter of bytes read.
    pub fn counter(&self) -> Counter {
        self.counter.clone()
    }
}

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.reader.read(buf)?;
        self.counter.add(n as u64);
        Ok(n)
    }
}

/// Read until `buf` is full or the end of input; returns the bytes read.
pub(crate) fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// A streaming HTTP body that resumes with `Range` requests.
struct HttpReader {
    url: String,
    opts: HttpOptions,
    agent: ureq::Agent,
    body: Option<Box<dyn Read + Send>>,
    pos: u64,
    total: Option<u64>,
    validator: Option<String>,
}

impl HttpReader {
    fn agent(opts: &HttpOptions) -> ureq::Agent {
        let config = ureq::Agent::config_builder()
            .user_agent(opts.user_agent.as_str())
            .timeout_connect(Some(opts.timeout))
            .timeout_recv_response(Some(opts.timeout))
            .timeout_recv_body(None)
            .http_status_as_error(false)
            .build();
        config.into()
    }

    fn open(url: &str, opts: HttpOptions, pos: u64) -> Result<HttpReader> {
        let agent = Self::agent(&opts);
        let mut r = HttpReader {
            url: url.to_string(),
            opts,
            agent,
            body: None,
            pos,
            total: None,
            validator: None,
        };
        r.connect().map_err(|e| DumpError::Http(format!("{url}: {e}")))?;
        Ok(r)
    }

    fn connect(&mut self) -> io::Result<()> {
        let mut req = self.agent.get(&self.url);
        if self.pos > 0 {
            req = req.header("Range", format!("bytes={}-", self.pos));
            if let Some(v) = &self.validator {
                req = req.header("If-Range", v.as_str());
            }
        }
        let resp = req.call().map_err(io::Error::other)?;
        let status = resp.status().as_u16();
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        match (self.pos, status) {
            (0, 200) => {
                self.total = header("content-length").and_then(|v| v.parse().ok());
                self.validator = header("etag").or_else(|| header("last-modified"));
            }
            (p, 206) if p > 0 => {
                let range = header("content-range").unwrap_or_default();
                let start = range
                    .strip_prefix("bytes ")
                    .and_then(|r| r.split('-').next())
                    .and_then(|s| s.parse::<u64>().ok());
                if start != Some(p) {
                    return Err(io::Error::other(format!(
                        "the server resumed at the wrong position ({range:?}, wanted {p})"
                    )));
                }
            }
            (p, 200) if p > 0 => {
                return Err(io::Error::other(
                    "the file changed on the server (or it does not support ranges); cannot resume",
                ));
            }
            (_, s) => return Err(io::Error::other(format!("HTTP status {s}"))),
        }
        let (_, body) = resp.into_parts();
        self.body = Some(Box::new(body.into_reader()));
        Ok(())
    }
}

impl Read for HttpReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut attempts = 0;
        loop {
            let result = match self.body.as_mut() {
                Some(b) => b.read(buf),
                None => Err(io::Error::other("not connected")),
            };
            match result {
                Ok(0) if self.total.is_some_and(|t| self.pos < t) => {
                    // The connection closed early: resume.
                }
                Ok(n) => {
                    self.pos += n as u64;
                    return Ok(n);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if attempts >= self.opts.retries => {
                    return Err(io::Error::other(format!(
                        "{}: download failed at byte {} after {attempts} retries: {e}",
                        self.url, self.pos
                    )));
                }
                Err(_) => {}
            }
            if attempts >= self.opts.retries {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("{}: the connection kept closing at byte {}", self.url, self.pos),
                ));
            }
            attempts += 1;
            self.body = None;
            std::thread::sleep(Duration::from_millis(200 * u64::from(attempts.min(10))));
            if let Err(e) = self.connect() {
                if attempts >= self.opts.retries {
                    return Err(e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_kinds() {
        assert_eq!(Input::parse("https://x/y.bz2"), Input::Url("https://x/y.bz2".into()));
        assert_eq!(Input::parse("/tmp/a.json"), Input::Path("/tmp/a.json".into()));
        assert_eq!(Input::parse("https://x/dir/y.json.bz2?a=1").file_name(), "y.json.bz2");
        assert!(USER_AGENT.starts_with("wikidata-rs/"));
    }
}
