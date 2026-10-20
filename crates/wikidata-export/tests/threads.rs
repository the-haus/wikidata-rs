//! In its own test binary, so no other test's threads are counted.

mod common;

use common::*;
use wikidata_export::{entities, Dump, EntityOptions, Event, Flow};

#[test]
fn stopping_leaves_no_threads() {
    let d = tmp();
    // Many bzip2 blocks, so the pipeline is busy when it is stopped.
    let lines = sample_lines();
    let mut data = b"[\n".to_vec();
    for i in 0..4000 {
        if i > 0 {
            data.extend(b",\n");
        }
        data.extend(&lines[i % lines.len()]);
    }
    data.extend(b"\n]\n");
    let p = write(&d, "many.json.bz2", &bz2(&data, 1));
    let dump = Dump::open(p.as_path()).unwrap();
    assert_eq!(wikidata_dump::live_threads(), 0);
    for _ in 0..3 {
        let mut seen = 0;
        let stats = entities(&dump, &EntityOptions::default(), &mut |e| {
            if let Event::Records(rs) = e {
                seen += rs.len();
                return Flow::Stop;
            }
            Flow::Continue
        })
        .unwrap();
        assert!(stats.stopped && seen > 0);
        assert_eq!(wikidata_dump::live_threads(), 0);
    }
    // And after a complete pass.
    let mut opts = EntityOptions::default();
    opts.scan.threads = 4;
    let (rs, _) = collect(&dump, &opts);
    assert_eq!(rs.len(), 4000);
    assert_eq!(wikidata_dump::live_threads(), 0);
}
