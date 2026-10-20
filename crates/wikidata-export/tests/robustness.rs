mod common;

use std::io::{BufRead, BufReader, Read, Write};

use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use common::*;
use wikidata_export::{entities, Dump, DumpError, EntityOptions, ExportError, Flow, HttpOptions};

fn read_err(path: &std::path::Path, allow: bool) -> Result<(usize, bool), ExportError> {
    let dump = Dump::open(path)?;
    let mut opts = EntityOptions::default();
    opts.scan.allow_truncated = allow;
    let mut n = 0;
    let stats = entities(&dump, &opts, &mut |e| {
        if let wikidata_export::Event::Records(rs) = e {
            n += rs.len();
        }
        Flow::Continue
    })?;
    Ok((n, stats.truncated))
}

fn truncated(r: Result<(usize, bool), ExportError>) -> bool {
    matches!(r, Err(ExportError::Dump(DumpError::Truncated(_))))
}

#[test]
fn truncated_inputs_fail() {
    let d = tmp();
    let data = sample();
    // Plain JSON cut inside a line, and at a line end.
    let cut = data.len() / 2;
    let p = write(&d, "cut.json", &data[..cut]);
    assert!(truncated(read_err(&p, false)));
    let line_end = data[..cut].iter().rposition(|&b| b == b'\n').unwrap() + 1;
    let p = write(&d, "cut2.json", &data[..line_end]);
    assert!(truncated(read_err(&p, false)));
    // ... and with allow_truncated, the whole lines before the cut.
    let (n, t) = read_err(&p, true).unwrap();
    assert!(t && n > 5 && n < 41, "{n}");

    // bzip2, gzip and zstd cut short.
    let bz = bz2(&data, 1);
    let p = write(&d, "cut.json.bz2", &bz[..bz.len() - 20]);
    assert!(truncated(read_err(&p, false)));
    for name in ["x.json.gz", "x.json.zst"] {
        let full = d.join(name);
        let mut w = wikidata_dump::DumpWriter::create(&full, &Default::default()).unwrap();
        w.write_all(&data).unwrap();
        w.finish().unwrap();
        let bytes = std::fs::read(&full).unwrap();
        let p = write(&d, &format!("cut-{name}"), &bytes[..bytes.len() * 2 / 3]);
        let r = read_err(&p, false);
        assert!(r.is_err(), "{name}: {:?}", r.ok());
    }
}

#[test]
fn garbage_fails() {
    let d = tmp();
    assert!(read_err(&write(&d, "g.json", b"<html>nope</html>\n"), false).is_err());
    assert!(read_err(&write(&d, "e.json", b""), false).is_err());
    assert!(read_err(
        &write(&d, "bad.json", b"[\n{\"type\":\"item\",\"id\":\"Q1\",\"labels\":{\n]\n"),
        false
    )
    .is_err());
    let mut bz = bz2(&sample(), 1);
    let mid = bz.len() / 2;
    bz[mid] ^= 0xff;
    assert!(read_err(&write(&d, "bad.json.bz2", &bz), false).is_err());
    assert!(Dump::open(d.join("missing.json")).is_err());
}

/// A tiny HTTP server with Range support. The first `drops` responses
/// close the connection after `cut` body bytes.
fn serve(body: Vec<u8>, drops: usize, cut: usize) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let reqs = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { return };
            let n = reqs.fetch_add(1, Ordering::SeqCst);
            let body = body.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut start = 0usize;
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap() == 0 || h == "\r\n" {
                        break;
                    }
                    let lower = h.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("range: bytes=") {
                        start = v.trim().trim_end_matches('-').parse().unwrap();
                    }
                }
                if first.contains("/missing") {
                    let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    return;
                }
                let rest = &body[start..];
                let head = if start > 0 {
                    format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n",
                    rest.len(),
                    start,
                    body.len() - 1,
                    body.len()
                )
                } else {
                    format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"v1\"\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                    rest.len()
                )
                };
                let _ = s.write_all(head.as_bytes());
                // The first request is the probe for the format; then drops.
                if n >= 1 && n <= drops {
                    let _ = s.write_all(&rest[..cut.min(rest.len())]);
                } else {
                    let _ = s.write_all(rest);
                }
                let _ = s.flush();
                let _ = s.shutdown(std::net::Shutdown::Both);
            });
        }
    });
    (format!("http://{addr}"), requests)
}

#[test]
fn http_streams_and_resumes() {
    let body = bz2(&sample(), 1);
    let (base, requests) = serve(body.clone(), 2, body.len() / 3);
    let dump = Dump::open(format!("{base}/wikidata.json.bz2")).unwrap();
    assert_eq!(dump.len(), Some(body.len() as u64));
    let (rs, stats) = collect(&dump, &EntityOptions::default());
    assert_eq!(rs.len(), 41);
    assert_eq!(stats.bytes_in, body.len() as u64);
    // probe + first read + two resumes
    assert!(
        requests.load(Ordering::SeqCst) >= 4,
        "{}",
        requests.load(Ordering::SeqCst)
    );

    let r = Dump::open_with(format!("{base}/missing.json"), HttpOptions::default());
    assert!(r.is_err());
}

#[test]
fn http_truncated_body_is_an_error() {
    // The server never sends a body byte after the probe: retries run out.
    let body = bz2(&sample(), 1);
    let (base, _) = serve(body.clone(), usize::MAX, 0);
    let dump = Dump::open_with(
        format!("{base}/w.json.bz2"),
        HttpOptions {
            retries: 2,
            ..Default::default()
        },
    )
    .unwrap();
    let mut opts = EntityOptions::default();
    opts.scan.threads = 2;
    let r = entities(&dump, &opts, &mut |_| Flow::Continue);
    assert!(r.is_err());
}

#[test]
fn reads_from_any_reader() {
    // A source over a reader (stdin-like).
    let data = sample();
    let source = wikidata_dump::Source::from_reader(std::io::Cursor::new(data.clone()), None);
    let chunks = wikidata_dump::Chunks::new(source, wikidata_dump::Compression::None, &Default::default());
    let mut n = 0;
    for c in chunks {
        n += c.unwrap().lines().count();
    }
    assert_eq!(n, 41);
    let mut buf = Vec::new();
    std::io::Cursor::new(&data).read_to_end(&mut buf).unwrap();
}
