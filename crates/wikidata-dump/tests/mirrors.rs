//! Reading from several mirrors at once (`HttpOptions::mirrors`).

mod common;

use std::io::Read;
use std::sync::Arc;
use std::time::Instant;

use common::*;
use wikidata_dump::multi::MultiReader;
use wikidata_dump::{HttpOptions, Input, Source};

const MB: u64 = 1 << 20;

fn opts(mirrors: Vec<String>) -> HttpOptions {
    HttpOptions {
        mirrors,
        connections: Some(2),
        range_size: 256 << 10,
        retries: 2,
        ..Default::default()
    }
}

fn read_all(url: &str, o: &HttpOptions) -> (Vec<u8>, f64) {
    let t = Instant::now();
    let mut s = Source::open(&Input::parse(url), o).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    (out, t.elapsed().as_secs_f64())
}

#[test]
fn two_capped_mirrors_read_about_twice_as_fast() {
    let body = Arc::new(noise(6 * MB as usize));
    let cap = Server {
        rate: 3 * MB,
        ..Default::default()
    };
    let a = serve(body.clone(), "dump.json.bz2", cap.clone());
    let b = serve(body.clone(), "dump.json.bz2", cap);
    // One server (the cap is per server, whatever the connections).
    let (one, t1) = read_all(&a.url, &HttpOptions::default());
    assert_eq!(one, *body);
    let (two, t2) = read_all(&a.url, &opts(vec![b.url.clone()]));
    assert_eq!(two, *body, "identical to a single-source read");
    eprintln!("one mirror {t1:.2} s, two mirrors {t2:.2} s");
    assert!(t1 > 1.6, "the cap: {t1:.2} s");
    assert!(t2 < t1 * 0.7, "two mirrors: {t2:.2} s, one: {t1:.2} s");
    assert!(
        b.requests.load(std::sync::atomic::Ordering::SeqCst) > 3,
        "both mirrors served ranges"
    );
}

#[test]
fn memory_stays_bounded_and_offsets_work() {
    let body = Arc::new(noise(5 * MB as usize + 12_345));
    let a = serve(body.clone(), "x", Server::default());
    let b = serve(body.clone(), "x", Server::default());
    let o = opts(vec![b.url.clone()]);
    let urls = vec![a.url.clone(), b.url.clone()];
    let mut r = MultiReader::open(&urls, 0, &o).unwrap();
    let mut buf = vec![0u8; 1000];
    // Read slowly: the mirrors may only get a window ahead.
    r.read_exact(&mut buf).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let window = 2 * 2 * 2 * (256 << 10);
    assert!(r.peak_buffered() <= window, "{} > {window}", r.peak_buffered());
    let mut rest = Vec::new();
    r.read_to_end(&mut rest).unwrap();
    assert_eq!([&buf[..], &rest[..]].concat(), *body);
    // From an offset in the middle of a range.
    let mut r = MultiReader::open(&urls, 1_000_003, &o).unwrap();
    let mut tail = Vec::new();
    r.read_to_end(&mut tail).unwrap();
    assert_eq!(tail, body[1_000_003..]);
}

#[test]
fn failing_mirrors_hand_over_and_disagreeing_ones_are_refused() {
    let body = Arc::new(noise(3 * MB as usize));
    let good = serve(body.clone(), "x", Server::default());
    // Ranges cut short are resumed.
    let flaky = serve(
        body.clone(),
        "x",
        Server {
            drops: 3,
            cut: 1000,
            ..Default::default()
        },
    );
    let (got, _) = read_all(&good.url, &opts(vec![flaky.url.clone()]));
    assert_eq!(got, *body);
    // A mirror that stops answering: the other one reads everything.
    let broken = Server {
        fail_with: Some(503),
        ..Default::default()
    };
    let urls = [good.url.clone(), serve(body.clone(), "x", Server::default()).url];
    let dead = serve(body.clone(), "x", broken);
    let mut r = MultiReader::open(&urls, 0, &opts(vec![])).unwrap();
    let mut out = Vec::new();
    r.read_to_end(&mut out).unwrap();
    assert_eq!(out, *body);
    assert!(MultiReader::open(&[good.url.clone(), dead.url.clone()], 0, &opts(vec![])).is_err());
    let other = serve(Arc::new(noise(100)), "x", Server::default());
    let err = MultiReader::open(&[good.url.clone(), other.url], 0, &opts(vec![]))
        .err()
        .unwrap();
    assert!(err.to_string().contains("disagree"), "{err}");
}
