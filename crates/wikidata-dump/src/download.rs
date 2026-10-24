//! Downloading a dump: the newest complete one, from several mirrors at
//! once, resumable, verified.
//!
//! 1. The newest complete dump is the newest date whose
//!    `wikidata-<date>-sha1sums.txt` lists the file (Wikimedia writes the
//!    checksums when a dump is done; the entity dumps have no
//!    `dumpstatus.json`).
//! 2. The file is split into ranges, fetched from every mirror in
//!    parallel (a few connections each; the mirrors cap bandwidth per
//!    client, so two mirrors are about twice as fast as one).
//! 3. Ranges are written at their place in one preallocated
//!    `<name>.part` file (no join afterwards, which would write the
//!    103 GB a second time), and the ranges done are recorded in
//!    `<name>.part.state`, so a stopped or killed download resumes.
//! 4. The SHA-1 is computed in file order as the ranges arrive, and
//!    compared with the published one.
//! 5. Only then is the file renamed to its final name: a reader never sees
//!    a partial file.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
#[cfg(not(any(unix, windows)))]
compile_error!("downloads need positional file writes (unix or windows)");
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use sha1::{Digest, Sha1};

use crate::multi::{agent, fetch_range, probe};
use crate::{DumpError, HttpOptions, Result};

/// Wikimedia's entity dumps.
pub const ENTITY_DUMPS: &str = "https://dumps.wikimedia.org/wikidatawiki/entities/";

/// Wikimedia's dumps of Wikidata's database (XML and SQL tables).
pub const WIKI_DUMPS: &str = "https://dumps.wikimedia.org/wikidatawiki/";

/// Options for [`download`].
#[derive(Clone, Debug)]
pub struct DownloadOptions {
    /// The dumps' directory (default [`ENTITY_DUMPS`]).
    pub base: String,
    /// The same directory on mirrors
    /// (`https://dumps.wikimedia.your.org/wikidatawiki/entities/`).
    pub mirrors: Vec<String>,
    /// The dump's date (`20261012`); `None`: the newest complete one.
    pub date: Option<String>,
    /// Which file of the dump (default `all.json.bz2`, i.e.
    /// `wikidata-<date>-all.json.bz2`).
    pub file: String,
    /// Where to put it.
    pub dir: PathBuf,
    /// User agent, connections per server, range size, retries.
    pub http: HttpOptions,
    /// Check the SHA-1 (default true).
    pub verify: bool,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        DownloadOptions {
            base: ENTITY_DUMPS.into(),
            mirrors: Vec::new(),
            date: None,
            file: "all.json.bz2".into(),
            dir: PathBuf::from("."),
            http: HttpOptions::default(),
            verify: true,
        }
    }
}

/// Progress of a download.
#[derive(Clone, Debug, Default)]
pub struct DownloadProgress {
    /// The file being downloaded.
    pub file: String,
    /// Bytes on disk (resumed ones included).
    pub bytes_done: u64,
    /// The file's size.
    pub bytes_total: u64,
    /// Bytes fetched by this run.
    pub bytes_fetched: u64,
    /// Bytes hashed so far.
    pub bytes_hashed: u64,
    /// Bytes fetched by this run from each server, in order (the base,
    /// then the mirrors).
    pub per_server: Vec<u64>,
    /// Time since the start.
    pub elapsed_ms: u64,
}

/// What [`download`] did.
#[derive(Clone, Debug, Default)]
pub struct DownloadStats {
    /// The file.
    pub path: PathBuf,
    /// The dump's date.
    pub date: String,
    /// Its size.
    pub bytes: u64,
    /// Bytes fetched by this run.
    pub bytes_fetched: u64,
    /// Bytes that were already there (a resumed download).
    pub bytes_resumed: u64,
    /// Bytes fetched from each server (the base, then the mirrors).
    pub per_server: Vec<u64>,
    /// The verified SHA-1 (hex), when verified.
    pub sha1: Option<String>,
    /// The file was already complete: nothing was fetched.
    pub already_done: bool,
    /// Wall time.
    pub elapsed_ms: u64,
}

fn get_text(agent: &ureq::Agent, url: &str) -> io::Result<Option<String>> {
    let resp = agent.get(url).call().map_err(io::Error::other)?;
    match resp.status().as_u16() {
        200 => {
            let (_, body) = resp.into_parts();
            let mut s = String::new();
            body.into_reader().take(16 << 20).read_to_string(&mut s)?;
            Ok(Some(s))
        }
        404 => Ok(None),
        s => Err(io::Error::other(format!("{url}: HTTP status {s}"))),
    }
}

fn dir_url(base: &str) -> String {
    if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    }
}

/// The file name of `file` in the dump of `date`.
pub fn dump_file_name(date: &str, file: &str) -> String {
    format!("wikidata-{date}-{file}")
}

/// The published SHA-1 of `file` in the dump of `date`, when the dump is
/// complete.
pub fn published_sha1(base: &str, date: &str, file: &str, http: &HttpOptions) -> Result<Option<String>> {
    let agent = agent(http);
    let url = format!("{}{date}/wikidata-{date}-sha1sums.txt", dir_url(base));
    let Some(text) = get_text(&agent, &url).map_err(|e| DumpError::Http(e.to_string()))? else {
        return Ok(None);
    };
    let name = dump_file_name(date, file);
    Ok(text.lines().find_map(|l| {
        let (sum, n) = l.split_once(char::is_whitespace)?;
        (n.trim() == name).then(|| sum.trim().to_ascii_lowercase())
    }))
}

/// The newest dump of `base` whose checksums list `file`, and the
/// published SHA-1.
pub fn latest_dump(base: &str, file: &str, http: &HttpOptions) -> Result<(String, String)> {
    let agent = agent(http);
    // The last few: an older dump than that is not "the newest".
    for date in dates(&agent, base)?.iter().take(6) {
        if let Some(sum) = published_sha1(base, date, file, http)? {
            return Ok((date.clone(), sum));
        }
    }
    Err(DumpError::Http(format!(
        "{base}: no complete dump of {file} among the newest dates"
    )))
}

/// A file of a dump, as its status lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DumpFile {
    /// The SQL table (`redirect`), or the entity dump's file
    /// (`all.json.bz2`).
    pub part: String,
    /// The file name (`wikidatawiki-20261001-redirect.sql.gz`).
    pub name: String,
    /// Its URL on the base.
    pub url: String,
    /// Its size, when the status gives it.
    pub size: Option<u64>,
    /// Its published SHA-1.
    pub sha1: String,
}

/// A complete dump: its date and the files asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DumpStatus {
    /// `20261001`.
    pub date: String,
    /// The files, in the order asked.
    pub files: Vec<DumpFile>,
}

/// The dates of a dumps directory, newest first.
fn dates(agent: &ureq::Agent, base: &str) -> Result<Vec<String>> {
    let index = get_text(agent, &dir_url(base))
        .map_err(|e| DumpError::Http(e.to_string()))?
        .ok_or_else(|| DumpError::Http(format!("{base}: not found")))?;
    let mut dates: Vec<String> = index
        .split("href=\"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .filter_map(|h| h.strip_suffix('/'))
        .filter(|d| d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_string)
        .collect();
    dates.sort_unstable();
    dates.dedup();
    dates.reverse();
    Ok(dates)
}

/// The SQL tables of the database dump of `date` (`["redirect",
/// "page"]`), from its `dumpstatus.json`, when every one of them is done.
pub fn sql_dump_status(base: &str, date: &str, tables: &[String], http: &HttpOptions) -> Result<Option<DumpStatus>> {
    let agent = agent(http);
    let url = format!("{}{date}/dumpstatus.json", dir_url(base));
    let Some(text) = get_text(&agent, &url).map_err(|e| DumpError::Http(e.to_string()))? else {
        return Ok(None);
    };
    let status: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| DumpError::Format(format!("{url}: {e}")))?;
    let mut files = Vec::new();
    for table in tables {
        let job = &status["jobs"][format!("{table}table")];
        if job["status"] != "done" {
            return Ok(None);
        }
        let Some((name, f)) = job["files"].as_object().and_then(|fs| fs.iter().next()) else {
            return Ok(None);
        };
        let host = dir_url(base);
        // The status's URLs are absolute paths on the dumps server.
        let url = match f["url"].as_str() {
            Some(u) if u.starts_with('/') => {
                let origin = host.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
                format!("{origin}{u}")
            }
            _ => format!("{host}{date}/{name}"),
        };
        files.push(DumpFile {
            part: table.clone(),
            name: name.clone(),
            url,
            size: f["size"].as_u64(),
            sha1: f["sha1"].as_str().unwrap_or_default().to_ascii_lowercase(),
        });
    }
    Ok(Some(DumpStatus {
        date: date.to_string(),
        files,
    }))
}

/// The newest database dump whose SQL `tables` are all done.
pub fn latest_sql_dump(base: &str, tables: &[String], http: &HttpOptions) -> Result<DumpStatus> {
    let agent = agent(http);
    // The last few: an older dump than that is not "the newest".
    for date in dates(&agent, base)?.iter().take(6) {
        if let Some(s) = sql_dump_status(base, date, tables, http)? {
            return Ok(s);
        }
    }
    Err(DumpError::Http(format!(
        "{base}: no dump among the newest dates has done the tables {}",
        tables.join(", ")
    )))
}

/// Options for [`download_sql`].
#[derive(Clone, Debug)]
pub struct SqlDownloadOptions {
    /// The database dumps' directory (default [`WIKI_DUMPS`]).
    pub base: String,
    /// The same directory on mirrors.
    pub mirrors: Vec<String>,
    /// The dump's date; `None`: the newest whose tables are done.
    pub date: Option<String>,
    /// The tables (default `redirect` and `page`, what the redirect
    /// index needs).
    pub tables: Vec<String>,
    /// Where to put them.
    pub dir: PathBuf,
    /// User agent, connections per server, range size, retries.
    pub http: HttpOptions,
    /// Check the SHA-1s (default true).
    pub verify: bool,
}

impl Default for SqlDownloadOptions {
    fn default() -> Self {
        SqlDownloadOptions {
            base: WIKI_DUMPS.into(),
            mirrors: Vec::new(),
            date: None,
            tables: vec!["redirect".into(), "page".into()],
            dir: PathBuf::from("."),
            http: HttpOptions::default(),
            verify: true,
        }
    }
}

/// Download SQL tables of a database dump (`wikidatawiki-<date>-redirect.sql.gz`,
/// `…-page.sql.gz`), each as [`download`] does a dump: from the base and
/// the mirrors at once, resumable, SHA-1 verified (from the dump's
/// `dumpstatus.json`), renamed into place when verified.
pub fn download_sql(
    opts: &SqlDownloadOptions,
    progress: &mut dyn FnMut(&DownloadProgress) -> bool,
) -> Result<Vec<DownloadStats>> {
    let status = match &opts.date {
        Some(d) => sql_dump_status(&opts.base, d, &opts.tables, &opts.http)?.ok_or_else(|| {
            DumpError::Http(format!(
                "the database dump of {d} has not done the tables {}",
                opts.tables.join(", ")
            ))
        })?,
        None => latest_sql_dump(&opts.base, &opts.tables, &opts.http)?,
    };
    let mut out = Vec::new();
    for f in &status.files {
        let urls = std::iter::once(&opts.base)
            .chain(&opts.mirrors)
            .map(|b| format!("{}{}/{}", dir_url(b), status.date, f.name))
            .collect();
        out.push(fetch(
            &Fetch {
                urls,
                name: f.name.clone(),
                date: status.date.clone(),
                sha1: opts.verify.then(|| f.sha1.clone()),
                dir: &opts.dir,
                http: &opts.http,
                start: Instant::now(),
            },
            progress,
        )?);
    }
    Ok(out)
}

/// The newest complete entity dump of `file` (see [`latest_dump`]), as a
/// [`DumpStatus`].
pub fn latest_entity_dump(base: &str, file: &str, http: &HttpOptions) -> Result<DumpStatus> {
    let (date, sha1) = latest_dump(base, file, http)?;
    let name = dump_file_name(&date, file);
    Ok(DumpStatus {
        files: vec![DumpFile {
            part: file.to_string(),
            url: format!("{}{date}/{name}", dir_url(base)),
            name,
            size: None,
            sha1,
        }],
        date,
    })
}

/// The ranges done, saved beside the partial file.
struct StateFile {
    path: PathBuf,
    size: u64,
    range: u64,
    sha1: String,
    done: Vec<bool>,
}

impl StateFile {
    fn load(path: &Path, size: u64, range: u64, sha1: &str) -> Option<Vec<bool>> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut lines = text.lines();
        if lines.next()? != "wikidata-rs download 1" {
            return None;
        }
        let field = |l: Option<&str>, k: &str| l?.strip_prefix(k).map(str::to_string);
        if field(lines.next(), "size ")? != size.to_string()
            || field(lines.next(), "range ")? != range.to_string()
            || field(lines.next(), "sha1 ")? != sha1
        {
            return None;
        }
        let mut done = vec![false; size.div_ceil(range) as usize];
        for run in field(lines.next(), "done ")?.split(',').filter(|r| !r.is_empty()) {
            let (a, b) = run.split_once('-').unwrap_or((run, run));
            let (a, b): (usize, usize) = (a.parse().ok()?, b.parse().ok()?);
            for d in done.get_mut(a..=b)? {
                *d = true;
            }
        }
        Some(done)
    }

    fn save(&self) -> io::Result<()> {
        let mut runs = Vec::new();
        let mut i = 0;
        while i < self.done.len() {
            if !self.done[i] {
                i += 1;
                continue;
            }
            let a = i;
            while i < self.done.len() && self.done[i] {
                i += 1;
            }
            runs.push(format!("{a}-{}", i - 1));
        }
        let text = format!(
            "wikidata-rs download 1\nsize {}\nrange {}\nsha1 {}\ndone {}\n",
            self.size,
            self.range,
            self.sha1,
            runs.join(",")
        );
        let mut tmp = self.path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(unix)]
fn write_at(f: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(f, buf, offset)
}

#[cfg(windows)]
fn write_at(f: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = std::os::windows::fs::FileExt::seek_write(f, buf, offset)?;
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(f, buf, offset)
}

#[cfg(windows)]
fn read_at(f: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = std::os::windows::fs::FileExt::seek_read(f, buf, offset)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf = &mut buf[n..];
        offset += n as u64;
    }
    Ok(())
}

struct Shared {
    state: Mutex<Work>,
    cv: Condvar,
    stop: AtomicBool,
    fetched: Vec<AtomicU64>,
}

struct Work {
    /// Ranges not started, in order.
    todo: std::collections::VecDeque<u64>,
    done: Vec<bool>,
    /// Ranges below this are all done.
    frontier: u64,
    error: Option<String>,
    /// Servers with a connection still working.
    alive: usize,
}

/// Download a dump (see the module docs). `progress` is called about
/// every 200 ms; returning `false` stops the download (it resumes on the
/// next call).
pub fn download(opts: &DownloadOptions, progress: &mut dyn FnMut(&DownloadProgress) -> bool) -> Result<DownloadStats> {
    let start = Instant::now();
    let (date, sha1) = match &opts.date {
        Some(d) if !opts.verify => (d.clone(), String::new()),
        Some(d) => {
            let sum = published_sha1(&opts.base, d, &opts.file, &opts.http)?.ok_or_else(|| {
                DumpError::Http(format!(
                    "the dump of {d} is not complete (its checksums do not list {})",
                    opts.file
                ))
            })?;
            (d.clone(), sum)
        }
        None => latest_dump(&opts.base, &opts.file, &opts.http)?,
    };
    let name = dump_file_name(&date, &opts.file);
    let urls = std::iter::once(&opts.base)
        .chain(&opts.mirrors)
        .map(|b| format!("{}{date}/{name}", dir_url(b)))
        .collect();
    fetch(
        &Fetch {
            urls,
            name,
            date,
            sha1: opts.verify.then_some(sha1),
            dir: &opts.dir,
            http: &opts.http,
            start,
        },
        progress,
    )
}

/// One file to fetch and verify (see the module docs).
struct Fetch<'a> {
    /// Its URL on the base, then on the mirrors.
    urls: Vec<String>,
    name: String,
    date: String,
    /// The published SHA-1 (`None`: not checked).
    sha1: Option<String>,
    dir: &'a Path,
    http: &'a HttpOptions,
    start: Instant,
}

/// Fetch a file in ranges from every server, resumably, verify it, and
/// rename it into place.
fn fetch(job: &Fetch<'_>, progress: &mut dyn FnMut(&DownloadProgress) -> bool) -> Result<DownloadStats> {
    let start = job.start;
    let (name, date, urls) = (job.name.clone(), job.date.clone(), &job.urls);
    let sha1 = job.sha1.clone().unwrap_or_default();
    let verify = job.sha1.is_some();
    let path = job.dir.join(&name);
    let mut stats = DownloadStats {
        path: path.clone(),
        date: date.clone(),
        ..Default::default()
    };
    if path.exists() {
        stats.bytes = std::fs::metadata(&path)?.len();
        stats.already_done = true;
        stats.elapsed_ms = start.elapsed().as_millis() as u64;
        return Ok(stats);
    }
    std::fs::create_dir_all(job.dir)?;
    let agent = agent(job.http);
    let mut size = None;
    let mut servers = Vec::new();
    for url in urls {
        match probe(&agent, url) {
            Ok((len, validator)) => {
                if size.is_some_and(|s| s != len) {
                    return Err(DumpError::Http(format!(
                        "the mirrors disagree on the size of {name}: {url} has {len} bytes, {} has {}",
                        urls[0],
                        size.unwrap()
                    )));
                }
                size = Some(len);
                servers.push((url.clone(), validator));
            }
            // A mirror without the file (not synced yet) is left out.
            Err(e) if url != &urls[0] => {
                let _ = e;
            }
            Err(e) => return Err(DumpError::Http(format!("{url}: {e}"))),
        }
    }
    let size = size.unwrap_or(0);
    stats.bytes = size;
    let range = job.http.range_size.max(64 << 10);
    let ranges = size.div_ceil(range);
    let part = job.dir.join(format!("{name}.part"));
    let state_path = job.dir.join(format!("{name}.part.state"));
    let done = match StateFile::load(&state_path, size, range, &sha1) {
        Some(done) if part.exists() => done,
        _ => vec![false; ranges as usize],
    };
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&part)?;
    if file.metadata()?.len() != size {
        file.set_len(size)?;
    }
    let file = Arc::new(file);
    stats.bytes_resumed = done.iter().filter(|&&d| d).count() as u64 * range;
    stats.bytes_resumed = stats.bytes_resumed.min(size);
    let mut state = StateFile {
        path: state_path.clone(),
        size,
        range,
        sha1: sha1.clone(),
        done: done.clone(),
    };
    state.save()?;

    let connections = job.http.connections.unwrap_or(2).max(1);
    // Ranges are fetched at most this far ahead of the first one missing,
    // so the file is written (and hashed) nearly in order.
    let window = (servers.len() * connections * 4) as u64;
    let shared = Arc::new(Shared {
        state: Mutex::new(Work {
            todo: (0..ranges).filter(|&i| !done[i as usize]).collect(),
            frontier: done.iter().position(|d| !d).unwrap_or(done.len()) as u64,
            done,
            error: None,
            alive: servers.len(),
        }),
        cv: Condvar::new(),
        stop: AtomicBool::new(false),
        fetched: servers.iter().map(|_| AtomicU64::new(0)).collect(),
    });
    let mut threads = Vec::new();
    for (s, (url, validator)) in servers.iter().enumerate() {
        let left = Arc::new(AtomicU64::new(connections as u64));
        for _ in 0..connections {
            let (shared, file, agent, url, validator, left) = (
                shared.clone(),
                file.clone(),
                agent.clone(),
                url.clone(),
                validator.clone(),
                left.clone(),
            );
            let retries = job.http.retries;
            threads.push(std::thread::spawn(move || {
                fetch_ranges(
                    &shared,
                    s,
                    &file,
                    &agent,
                    &url,
                    validator.as_deref(),
                    range,
                    size,
                    window,
                    retries,
                );
                if left.fetch_sub(1, Ordering::SeqCst) == 1 {
                    shared.state.lock().unwrap().alive -= 1;
                    shared.cv.notify_all();
                }
            }));
        }
    }

    // Hash in order, following the frontier; report progress; save state.
    let mut hasher = Sha1::new();
    let mut hashed = 0u64;
    let mut buf = vec![0u8; 4 << 20];
    let mut last_save = Instant::now();
    let mut last_report = Instant::now();
    let result: Result<()> = loop {
        let (frontier, finished, failed, done_now) = {
            let mut w = shared.state.lock().unwrap();
            if hashed >= w.frontier.saturating_mul(range).min(size) && w.error.is_none() && w.alive > 0 {
                w = shared.cv.wait_timeout(w, Duration::from_millis(200)).unwrap().0;
            }
            let all = w.frontier >= ranges;
            (
                w.frontier.saturating_mul(range).min(size),
                all,
                w.error
                    .clone()
                    .or_else(|| (w.alive == 0 && !all).then(|| "every server failed".to_string())),
                (last_save.elapsed() > Duration::from_secs(2)).then(|| w.done.clone()),
            )
        };
        if let Some(d) = done_now {
            state.done = d;
            if let Err(e) = state.save() {
                break Err(e.into());
            }
            last_save = Instant::now();
        }
        let mut read_error = None;
        while verify && hashed < frontier {
            let n = buf.len().min((frontier - hashed) as usize);
            if let Err(e) = read_at(&file, &mut buf[..n], hashed) {
                read_error = Some(e);
                break;
            }
            hasher.update(&buf[..n]);
            hashed += n as u64;
            if last_report.elapsed() > Duration::from_millis(200) {
                break;
            }
        }
        if let Some(e) = read_error {
            break Err(e.into());
        }
        if !verify {
            hashed = frontier;
        }
        if last_report.elapsed() > Duration::from_millis(200) || (finished && hashed >= size) {
            last_report = Instant::now();
            let per_server: Vec<u64> = shared.fetched.iter().map(|f| f.load(Ordering::Relaxed)).collect();
            let fetched: u64 = per_server.iter().sum();
            let p = DownloadProgress {
                file: name.clone(),
                bytes_done: (stats.bytes_resumed + fetched).min(size),
                bytes_total: size,
                bytes_fetched: fetched,
                bytes_hashed: hashed,
                per_server,
                elapsed_ms: start.elapsed().as_millis() as u64,
            };
            if !progress(&p) {
                break Err(DumpError::Io(
                    "the download was stopped (it resumes when run again)".into(),
                ));
            }
        }
        if let Some(e) = failed {
            break Err(DumpError::Http(e));
        }
        if finished && hashed >= size {
            break Ok(());
        }
    };
    shared.stop.store(true, Ordering::Relaxed);
    shared.cv.notify_all();
    for t in threads {
        let _ = t.join();
    }
    state.done = shared.state.lock().unwrap().done.clone();
    let _ = state.save();
    stats.per_server = shared.fetched.iter().map(|f| f.load(Ordering::Relaxed)).collect();
    stats.bytes_fetched = stats.per_server.iter().sum();
    result?;
    file.sync_all()?;
    drop(file);
    if verify {
        let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if got != sha1 {
            // Start over next time: which range is wrong is unknown.
            let _ = std::fs::remove_file(&state_path);
            let _ = std::fs::remove_file(&part);
            return Err(DumpError::Format(format!(
                "{name}: SHA-1 mismatch (got {got}, published {sha1}); the partial file was removed"
            )));
        }
        stats.sha1 = Some(got);
    }
    std::fs::rename(&part, &path)?;
    let _ = std::fs::remove_file(&state_path);
    stats.elapsed_ms = start.elapsed().as_millis() as u64;
    Ok(stats)
}

#[allow(clippy::too_many_arguments)]
fn fetch_ranges(
    shared: &Shared,
    server: usize,
    file: &File,
    agent: &ureq::Agent,
    url: &str,
    validator: Option<&str>,
    range: u64,
    size: u64,
    window: u64,
    retries: u32,
) {
    loop {
        let i = {
            let mut w = shared.state.lock().unwrap();
            loop {
                if shared.stop.load(Ordering::Relaxed) || w.error.is_some() {
                    return;
                }
                match w.todo.front() {
                    Some(&i) if i < w.frontier + window => {
                        w.todo.pop_front();
                        break i;
                    }
                    None if w.frontier >= w.done.len() as u64 => return,
                    _ => w = shared.cv.wait_timeout(w, Duration::from_millis(200)).unwrap().0,
                }
            }
        };
        let (from, to) = (i * range, ((i + 1) * range).min(size));
        let mut at = from;
        let r = fetch_range(agent, url, validator, from, to, retries, &shared.stop, &mut |b| {
            write_at(file, b, at)?;
            at += b.len() as u64;
            shared.fetched[server].fetch_add(b.len() as u64, Ordering::Relaxed);
            Ok(())
        });
        let mut w = shared.state.lock().unwrap();
        match r {
            Ok(()) => {
                w.done[i as usize] = true;
                while (w.frontier as usize) < w.done.len() && w.done[w.frontier as usize] {
                    w.frontier += 1;
                }
                shared.cv.notify_all();
            }
            Err(e) => {
                // Counted again when the range is fetched anew.
                shared.fetched[server].fetch_sub(at - from, Ordering::Relaxed);
                if e.to_string().contains("changed on the server") {
                    w.error = Some(format!("{url}: {e}"));
                } else {
                    // Another server takes the range; this one stops.
                    let pos = w.todo.partition_point(|&j| j < i);
                    w.todo.insert(pos, i);
                }
                shared.cv.notify_all();
                return;
            }
        }
    }
}
