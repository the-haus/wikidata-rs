mod common;

use common::*;
use wikidata_export::{Dump, EntityOptions, Filter};

/// Every entity's id and current `lastrevid`.
fn current() -> (Vec<String>, Vec<u64>) {
    sample_lines()
        .iter()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_slice(l).unwrap();
            (v["id"].as_str().unwrap().to_string(), v["lastrevid"].as_u64().unwrap())
        })
        .unzip()
}

fn run(filter: Filter) -> (Vec<String>, wikidata_export::ScanStats) {
    let dump = Dump::open(sample_path()).unwrap();
    let mut opts = EntityOptions::default();
    opts.scan.filter = Some(filter.compile(None).unwrap().into());
    let (rs, stats) = collect(&dump, &opts);
    (ids(&rs), stats)
}

#[test]
fn unchanged_entities_are_skipped_but_found() {
    let (all, revisions) = current();
    let (got, stats) = run(Filter::Revisions {
        ids: all.clone(),
        revisions: revisions.clone(),
    });
    assert!(got.is_empty(), "{got:?}");
    assert_eq!(stats.missing_ids, Some(vec![]));
    assert_eq!(stats.unchanged, all.len() as u64);
    assert_eq!(stats.parsed, 0, "skipped before any parse");

    // One entity one revision behind: only that one.
    let mut older = revisions.clone();
    let i = all.iter().position(|id| id == "Q42").unwrap();
    older[i] -= 1;
    let (got, stats) = run(Filter::Revisions {
        ids: all.clone(),
        revisions: older,
    });
    assert_eq!(got, ["Q42"]);
    assert_eq!(stats.unchanged, all.len() as u64 - 1);
    assert_eq!(stats.missing_ids, Some(vec![]));

    // Item numbers, plus one the dump lacks.
    let (items, revs): (Vec<u32>, Vec<u64>) = all
        .iter()
        .zip(&revisions)
        .filter(|(id, _)| id.starts_with('Q'))
        .map(|(id, &r)| (id[1..].parse().unwrap(), r))
        .chain([(123_456_789, 1)])
        .unzip();
    let (got, stats) = run(Filter::ItemRevisions { items, revisions: revs });
    assert!(got.is_empty());
    assert_eq!(stats.missing_ids, Some(vec!["Q123456789".to_string()]));
}
