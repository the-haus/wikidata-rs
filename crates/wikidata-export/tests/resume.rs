mod common;

use std::sync::Arc;

use common::*;
use wikidata_export::{entities, Checkpoint, Dump, EntityOptions, Event, Filter, Flow, Record, ScanStats};

/// A dump of the sample's lines repeated `n` times over, each copy with
/// its own ids (`Q31` → `Q31`, `Q1000031`, …), so blocks fill up.
fn big(n: usize) -> Vec<u8> {
    let lines = sample_lines();
    let mut out = b"[\n".to_vec();
    for copy in 0..n {
        for (i, l) in lines.iter().enumerate() {
            let l = String::from_utf8_lossy(l);
            let l = if copy == 0 {
                l.into_owned()
            } else {
                l.replacen("\"id\":\"Q", &format!("\"id\":\"Q{copy}00000"), 1)
            };
            out.extend_from_slice(l.as_bytes());
            out.extend_from_slice(if copy + 1 == n && i + 1 == lines.len() {
                b"\n"
            } else {
                b",\n"
            });
        }
    }
    out.extend_from_slice(b"]\n");
    out
}

fn opts(filter: &str) -> EntityOptions {
    let mut o = EntityOptions::default();
    o.scan.filter = Some(Filter::parse_json(filter).unwrap().compile(None).unwrap().into());
    o.scan.threads = 3;
    o
}

/// Records and stats; with `stop_after`, the checkpoints seen until the
/// sink stopped after that many records.
fn run(dump: &Dump, o: &EntityOptions, stop_after: Option<usize>) -> (Vec<String>, ScanStats, Vec<Checkpoint>) {
    let mut ids: Vec<String> = Vec::new();
    let mut cps = Vec::new();
    let stats = entities(dump, o, &mut |e: Event<Record>| {
        match e {
            Event::Records(rs) => {
                ids.extend(rs.into_iter().map(|r| r.entity.id));
                if stop_after.is_some_and(|n| ids.len() >= n) {
                    return Flow::Stop;
                }
            }
            Event::Checkpoint(cp) => cps.push(*cp),
            Event::Progress(_) => {}
        }
        Flow::Continue
    })
    .unwrap();
    (ids, stats, cps)
}

#[test]
fn a_stopped_pass_resumes_from_its_last_checkpoint() {
    let d = tmp();
    let text = big(300);
    let inputs = [write(&d, "big.json.bz2", &bz2(&text, 1)), write(&d, "big.json", &text)];
    let filter = r#"{"any":[{"claim":"P17=Q55"},{"ids":["Q42","Q100000042","Q300000042","Q999"]}]}"#;
    for input in inputs {
        let dump = Dump::open(input.as_path()).unwrap();
        let (all, full, _) = run(&dump, &opts(filter), None);
        assert_eq!(full.missing_ids, Some(vec!["Q999".to_string()]));

        let mut o = opts(filter);
        o.scan.checkpoint_every = Some(1);
        let (head, stopped, cps) = run(&dump, &o, Some(all.len() / 2));
        assert!(stopped.stopped && stopped.missing_ids.is_none());
        let cp = cps.last().expect("checkpoints");
        assert!(!cps.is_empty(), "{}: {} checkpoints", input.display(), cps.len());
        let done = cp.matched as usize;
        assert_eq!(head[..done], all[..done]);

        // Through bytes, as a caller would store it.
        let cp = Checkpoint::from_bytes(&cp.to_bytes()).unwrap();
        let mut o = opts(filter);
        o.scan.resume = Some(Arc::new(cp.clone()));
        let (rest, resumed, _) = run(&dump, &o, None);
        assert_eq!(rest, all[done..], "{}", input.display());
        assert_eq!(resumed.matched, full.matched);
        assert_eq!(resumed.entities, full.entities);
        assert_eq!(resumed.missing_ids, full.missing_ids);

        // Not with another filter, nor on another input.
        let mut o = opts(r#"{"claim":"P17=Q31"}"#);
        o.scan.resume = Some(Arc::new(cp.clone()));
        let err = entities(&dump, &o, &mut |_| Flow::Continue).unwrap_err();
        assert!(err.to_string().contains("another filter"), "{err}");
        let other = Dump::open(sample_path()).unwrap();
        let mut o = opts(filter);
        o.scan.resume = Some(Arc::new(cp));
        assert!(entities(&other, &o, &mut |_| Flow::Continue).is_err());
    }
}

#[test]
fn gzip_has_no_checkpoints() {
    let d = tmp();
    let p = d.join("x.json.gz");
    let mut w = wikidata_dump::DumpWriter::create(&p, &Default::default()).unwrap();
    std::io::Write::write_all(&mut w, &big(50)).unwrap();
    w.finish().unwrap();
    let dump = Dump::open(p.as_path()).unwrap();
    let mut o = opts(r#"{"claim":"P17=Q55"}"#);
    o.scan.checkpoint_every = Some(1);
    let (ids, _, cps) = run(&dump, &o, None);
    assert!(!ids.is_empty());
    assert!(cps.is_empty());
}
