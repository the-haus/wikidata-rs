//! The live stream, against a local EventStreams-like server that drops
//! connections (one of them in the middle of an event).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use wikidata_export::{live_changes, Change, Flow, HttpOptions, LiveOptions};
use wikidata_filter::{id_key, IdSetBuilder};

const EVENTS: u64 = 30;

fn event(i: u64) -> String {
    let wiki = if i.is_multiple_of(3) { "enwiki" } else { "wikidatawiki" };
    let ns = if i.is_multiple_of(5) { 1 } else { 0 };
    let title = format!("Q{}", 40 + i % 4);
    let data = format!(
        r#"{{"$schema":"/mediawiki/recentchange/1.0.0","id":{i},"type":"edit","namespace":{ns},"title":"{title}","comment":"/* wbsetclaim */","timestamp":{},"user":"Example","bot":false,"revision":{{"old":{},"new":{}}},"wiki":"{wiki}"}}"#,
        1_792_800_000 + i,
        1000 + i - 1,
        1000 + i
    );
    format!("event: message\nid: {i}\ndata: {data}\n\n")
}

/// Serves events after `Last-Event-ID`; the first connections end after
/// 7 events, the second one in the middle of the 8th.
fn serve() -> (String, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v2/stream/recentchange", l.local_addr().unwrap());
    let conns = Arc::new(AtomicUsize::new(0));
    let c = conns.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            let n = c.fetch_add(1, Ordering::SeqCst);
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut last = 0u64;
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("last-event-id:") {
                    last = v.trim().parse().unwrap();
                }
            }
            let _ =
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n:ok\n\n");
            for i in last + 1..=EVENTS {
                let e = event(i);
                if n < 3 && i == last + 8 {
                    let _ = s.write_all(&e.as_bytes()[..e.len() / 2]);
                    break;
                }
                let _ = s.write_all(e.as_bytes());
                if n < 3 && i == last + 7 && n != 1 {
                    break;
                }
            }
            if n >= 3 {
                // Then nothing: a quiet stream, until the client goes.
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        }
    });
    (url, conns)
}

fn wanted(after: u64) -> Vec<u64> {
    (after + 1..=EVENTS)
        .filter(|i| !i.is_multiple_of(3) && !i.is_multiple_of(5) && matches!(40 + i % 4, 42 | 43))
        .collect()
}

fn run(url: &str, last: Option<&str>) -> (Vec<Change>, wikidata_export::LiveStats) {
    let ids = IdSetBuilder::new();
    ids.insert(id_key("Q42").unwrap());
    ids.insert(id_key("Q43").unwrap());
    let opts = LiveOptions {
        url: url.to_string(),
        ids: Some(Arc::new(ids.freeze())),
        last_event_id: last.map(str::to_string),
        http: HttpOptions {
            retries: 5,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut got = Vec::new();
    let stats = live_changes(&opts, &|| false, &mut |cs| {
        got.extend(cs);
        if got
            .last()
            .is_some_and(|c| c.event_id == wanted(0).last().unwrap().to_string())
        {
            Flow::Stop
        } else {
            Flow::Continue
        }
    })
    .unwrap();
    (got, stats)
}

#[test]
fn resumes_after_reconnects_without_gaps_or_repeats() {
    let (url, conns) = serve();
    let (got, stats) = run(&url, None);
    let ids: Vec<u64> = got.iter().map(|c| c.event_id.parse().unwrap()).collect();
    assert_eq!(ids, wanted(0));
    assert!(conns.load(Ordering::SeqCst) >= 3, "the server dropped connections");
    assert!(stats.reconnects >= 2, "{stats:?}");
    let c = &got[0];
    assert_eq!(
        (c.id.as_str(), c.kind.as_str(), c.revision, c.old_revision),
        ("Q42", "edit", Some(1002), Some(1001))
    );

    // From a stored event id.
    let (url, _) = serve();
    let (got, _) = run(&url, Some("20"));
    let ids: Vec<u64> = got.iter().map(|c| c.event_id.parse().unwrap()).collect();
    assert_eq!(ids, wanted(20));
}

#[test]
fn stops_while_the_stream_is_quiet() {
    let (url, conns) = serve();
    // Use up the dropping connections first.
    conns.store(3, Ordering::SeqCst);
    let opts = LiveOptions {
        url,
        last_event_id: Some(EVENTS.to_string()),
        ..Default::default()
    };
    let t = std::time::Instant::now();
    let stop = || t.elapsed() > std::time::Duration::from_millis(300);
    let stats = live_changes(&opts, &stop, &mut |_| Flow::Continue).unwrap();
    assert_eq!(stats.changes, 0);
    assert!(t.elapsed() < std::time::Duration::from_millis(1500));
}
