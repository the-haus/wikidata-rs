//! A small HTTP file server for the tests: `Range` requests, an optional
//! bandwidth cap shared by all of a server's connections (as mirrors cap
//! each client), and faults.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct Server {
    /// Bytes per second for the whole server (0: no cap).
    pub rate: u64,
    /// Answer every request with this status.
    pub fail_with: Option<u16>,
    /// Flip a byte at this offset of the body.
    pub corrupt_at: Option<u64>,
    /// Close the connection after this many body bytes, for the first
    /// `drops` requests.
    pub drops: usize,
    pub cut: usize,
    /// ETag.
    pub etag: Option<String>,
}

pub struct Running {
    pub url: String,
    pub requests: Arc<AtomicUsize>,
}

/// Serve `body` at `/<name>`.
pub fn serve(body: Arc<Vec<u8>>, name: &str, cfg: Server) -> Running {
    let mut files = std::collections::HashMap::new();
    files.insert(format!("/{name}"), body);
    let r = serve_files(files, cfg);
    Running {
        url: format!("{}/{name}", r.url),
        requests: r.requests,
    }
}

/// Serve several files by path (`/a/b.txt`); the URL is the server's root
/// (without a trailing slash). The faults apply to the largest file only.
pub fn serve_files(files: std::collections::HashMap<String, Arc<Vec<u8>>>, cfg: Server) -> Running {
    let files = Arc::new(files);
    let largest = files.values().map(|b| b.len()).max().unwrap_or(0);
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let requests = Arc::new(AtomicUsize::new(0));
    let req = requests.clone();
    // When the cap allows the next byte out.
    let next = Arc::new(Mutex::new(Instant::now()));
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let (files, cfg, req, next) = (files.clone(), cfg.clone(), req.clone(), next.clone());
            std::thread::spawn(move || {
                let mut s = s;
                let mut r = BufReader::new(s.try_clone().unwrap());
                loop {
                    let mut range = None;
                    let mut if_range = None;
                    let mut first = String::new();
                    if r.read_line(&mut first).unwrap_or(0) == 0 {
                        return;
                    }
                    let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let body = files.get(&path).cloned();
                    let empty = Arc::new(Vec::new());
                    let body = body.as_ref().unwrap_or(&empty);
                    loop {
                        let mut line = String::new();
                        if r.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        if line == "\r\n" {
                            break;
                        }
                        let lower = line.to_ascii_lowercase();
                        if let Some(v) = lower.strip_prefix("range: bytes=") {
                            let (a, b) = v.trim().split_once('-').unwrap();
                            let a: u64 = a.parse().unwrap();
                            let last = (body.len() as u64).max(1) - 1;
                            let b: u64 = if b.is_empty() { last } else { b.parse().unwrap() };
                            range = Some((a, b.min(last)));
                        }
                        if let Some(v) = lower.strip_prefix("if-range:") {
                            if_range = Some(v.trim().to_string());
                        }
                    }
                    if !files.contains_key(&path) {
                        let _ = write!(s, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                        continue;
                    }
                    let faults = body.len() == largest;
                    let n = if faults {
                        req.fetch_add(1, Ordering::SeqCst)
                    } else {
                        usize::MAX
                    };
                    if let (Some(code), true) = (cfg.fail_with, faults) {
                        let _ = write!(s, "HTTP/1.1 {code} Nope\r\nContent-Length: 0\r\n\r\n");
                        continue;
                    }
                    let etag = cfg.etag.clone().unwrap_or_else(|| "\"v1\"".into());
                    let honour = if_range.is_none_or(|v| v == etag.to_ascii_lowercase());
                    let (a, b, status) = match range {
                        Some((a, b)) if honour => (a, b, "206 Partial Content"),
                        _ => (0, (body.len() as u64).max(1) - 1, "200 OK"),
                    };
                    let mut head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: {etag}\r\n",
                        if body.is_empty() { 0 } else { b + 1 - a }
                    );
                    if status.starts_with("206") {
                        head.push_str(&format!("Content-Range: bytes {a}-{b}/{}\r\n", body.len()));
                    }
                    head.push_str("\r\n");
                    if s.write_all(head.as_bytes()).is_err() {
                        return;
                    }
                    let mut part = if body.is_empty() {
                        Vec::new()
                    } else {
                        body[a as usize..=b as usize].to_vec()
                    };
                    if let (Some(c), true) = (cfg.corrupt_at, faults) {
                        if (a..=b).contains(&c) {
                            part[(c - a) as usize] ^= 0xff;
                        }
                    }
                    let limit = if faults && n < cfg.drops {
                        cfg.cut.min(part.len())
                    } else {
                        part.len()
                    };
                    for chunk in part[..limit].chunks(16 << 10) {
                        if cfg.rate > 0 {
                            let wait = {
                                let mut t = next.lock().unwrap();
                                let now = Instant::now();
                                let start = (*t).max(now);
                                *t = start + Duration::from_secs_f64(chunk.len() as f64 / cfg.rate as f64);
                                start.saturating_duration_since(now)
                            };
                            std::thread::sleep(wait);
                        }
                        if s.write_all(chunk).is_err() {
                            return;
                        }
                    }
                    if limit < part.len() {
                        return;
                    }
                }
            });
        }
    });
    Running { url, requests }
}

/// Bytes that do not repeat.
pub fn noise(n: usize) -> Vec<u8> {
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}
