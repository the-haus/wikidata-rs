//! Named filters, missing ids, per-entity languages and sitelinks, and the
//! class index built during another task.

mod common;

use std::sync::Arc;

use common::*;
use wikidata_export::{
    build_class_index, entities, extract, file_info, ByClaim, Dump, EntityOptions, Event, ExtractOptions, Filter, Flow,
    ModelOptions, ScanOptions,
};
use wikidata_model::Pid;

fn scan(filter: &str) -> ScanOptions {
    ScanOptions {
        filter: Some(Arc::new(Filter::parse_json(filter).unwrap().compile(None).unwrap())),
        ..Default::default()
    }
}

#[test]
fn named_filters_report_every_branch_that_matched() {
    let dump = Dump::open(sample_path()).unwrap();
    let opts = EntityOptions {
        scan: scan(r#"{"any":{"country":{"instanceOf":"Q6256"},"dutch":{"claim":"P17=Q55"},"q42":{"ids":"Q42"}}}"#),
        // Claims of P17 are not kept, so the branches cannot be read off
        // the output's parse.
        model: ModelOptions {
            properties: Some(vec![Pid(1)]),
            ..Default::default()
        },
        ..Default::default()
    };
    let names = opts.scan.filter.as_ref().unwrap().names().to_vec();
    assert_eq!(names, ["country", "dutch", "q42"]);
    let (rs, _) = collect(&dump, &opts);
    let by = |id: &str| -> Vec<&str> {
        let r = rs.iter().find(|r| r.entity.id == id).unwrap();
        r.matched_by.iter().map(|&i| names[i as usize].as_str()).collect()
    };
    assert_eq!(by("Q55"), ["country", "dutch"]);
    assert_eq!(by("Q31"), ["country"]);
    assert_eq!(by("Q1072"), ["dutch"]);
    assert_eq!(by("Q42"), ["q42"]);

    let (rs, _) = collect(
        &dump,
        &EntityOptions {
            scan: scan(r#"{"any":[{"instanceOf":"Q6256"},{"ids":"Q42"}]}"#),
            ..Default::default()
        },
    );
    assert_eq!(ids(&rs), ["Q31", "Q55", "Q42"]);
    assert!(rs.iter().all(|r| r.matched_by.is_empty()));
}

#[test]
fn missing_ids_after_a_complete_scan_only() {
    let dump = Dump::open(sample_path()).unwrap();
    let (_, stats) = collect(
        &dump,
        &EntityOptions {
            scan: scan(r#"{"any":[{"ids":["Q1","Q42","Q999999999"]},{"ids":"L7"},{"claim":"P17=Q55"}]}"#),
            ..Default::default()
        },
    );
    assert_eq!(stats.missing_ids.unwrap(), ["Q1", "Q999999999"]);

    let mut opts = EntityOptions {
        scan: scan(r#"{"ids":["Q1","Q999999999"]}"#),
        ..Default::default()
    };
    // Q1 is not in the sample either.
    let (rs, stats) = collect(&dump, &opts);
    assert!(rs.is_empty());
    assert_eq!(stats.missing_ids.unwrap(), ["Q1", "Q999999999"]);
    // Cut short: nothing can be said.
    opts.scan = scan(r#"{"ids":["Q31","Q999999999"]}"#);
    opts.scan.limit = Some(1);
    let (_, stats) = collect(&dump, &opts);
    assert!(stats.stopped);
    assert!(stats.missing_ids.is_none());
    // No ids in the filter: no report.
    let (_, stats) = collect(
        &dump,
        &EntityOptions {
            scan: scan(r#"{"type":"item"}"#),
            ..Default::default()
        },
    );
    assert!(stats.missing_ids.is_none());
    // The other tasks report them too.
    let info = file_info(&dump, &scan(r#"{"ids":["Q31","Q404"]}"#), &mut |_| Flow::Continue).unwrap();
    assert_eq!(info.scan.missing_ids.unwrap(), ["Q404"]);
}

#[test]
fn languages_and_sitelinks_by_country() {
    let dump = Dump::open(sample_path()).unwrap();
    let rule = |map: &[(&str, &[&str])], always: &[&str]| ByClaim {
        properties: vec![Pid(17)],
        map: map
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect(),
        always: always.iter().map(|s| s.to_string()).collect(),
    };
    let opts = EntityOptions {
        scan: scan(r#"{"ids":["Q31","Q55","Q42"]}"#),
        model: ModelOptions {
            // P17 is read for the rules but not kept.
            properties: Some(vec![Pid(31)]),
            ..Default::default()
        },
        languages_by: Some(rule(&[("Q55", &["nl"]), ("Q31", &["fr", "de"])], &["en", "mul"])),
        sitelinks_by: Some(rule(&[("Q55", &["nlwiki"])], &["enwiki"])),
        ..Default::default()
    };
    let (rs, _) = collect(&dump, &opts);
    let get = |id: &str| &rs.iter().find(|r| r.entity.id == id).unwrap().entity;
    let langs = |m: &wikidata_model::Map<String, String>| m.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>();
    let sites = |e: &wikidata_model::Entity| e.sitelinks.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>();

    let nl = get("Q55");
    assert_eq!(langs(&nl.labels), ["nl", "en", "mul"]);
    assert_eq!(nl.label.as_ref().unwrap().language, "nl");
    assert_eq!(sites(nl), ["nlwiki", "enwiki"]);
    assert_eq!(nl.claims.iter().map(|(p, _)| *p).collect::<Vec<_>>(), [Pid(31)]);
    assert_eq!(nl.derived.country, ["Q55"]);

    // Belgium's P17 = Q55 is deprecated: only Q31 counts.
    let be = get("Q31");
    assert_eq!(langs(&be.labels), ["fr", "de", "en", "mul"]);
    assert_eq!(be.label.as_ref().unwrap().language, "fr");
    assert_eq!(be.description.as_ref().unwrap().language, "en");
    assert_eq!(sites(be), ["enwiki"]);

    // No country: `always` only.
    let person = get("Q42");
    assert_eq!(langs(&person.labels), ["en"]);
    assert_eq!(sites(person), ["enwiki"]);
}

#[test]
fn the_class_index_is_built_during_another_task() {
    let d = tmp();
    let dump = Dump::open(sample_path()).unwrap();
    let (index, _) = build_class_index(&dump, &ScanOptions::default(), &mut |_| Flow::Continue).unwrap();
    let alone = d.join("alone.wdc");
    index.save(&alone).unwrap();

    let during = d.join("during.wdc");
    let mut opts = EntityOptions {
        scan: scan(r#"{"claim":"P17=Q55"}"#),
        ..Default::default()
    };
    opts.scan.class_index = Some(during.clone());
    let (rs, stats) = collect(&dump, &opts);
    assert_eq!(rs.len(), 9);
    let saved = stats.class_index.unwrap();
    assert_eq!((saved.classes, saved.edges), (index.len() as u64, index.edges() as u64));
    assert_eq!(std::fs::read(&during).unwrap(), std::fs::read(&alone).unwrap());

    // Extract and file info build it too.
    let during = d.join("extract.wdc");
    let mut x = ExtractOptions {
        scan: scan(r#"{"type":"lexeme"}"#),
        ..Default::default()
    };
    x.scan.class_index = Some(during.clone());
    let stats = extract(&dump, &x, d.join("lexemes.jsonl"), &mut |_| Flow::Continue).unwrap();
    assert!(stats.scan.class_index.is_some());
    assert_eq!(std::fs::read(&during).unwrap(), std::fs::read(&alone).unwrap());
    let info = file_info(
        &dump,
        &ScanOptions {
            class_index: Some(d.join("info.wdc")),
            ..Default::default()
        },
        &mut |_| Flow::Continue,
    )
    .unwrap();
    assert!(info.scan.class_index.is_some());
    assert_eq!(
        std::fs::read(d.join("info.wdc")).unwrap(),
        std::fs::read(&alone).unwrap()
    );

    // A stopped scan saves nothing.
    let stopped = d.join("stopped.wdc");
    let stats = entities(
        &dump,
        &EntityOptions {
            scan: ScanOptions {
                class_index: Some(stopped.clone()),
                ..Default::default()
            },
            ..Default::default()
        },
        &mut |e| match e {
            Event::Records(_) => Flow::Stop,
            Event::Progress(_) => Flow::Continue,
        },
    )
    .unwrap();
    assert!(stats.stopped);
    assert!(stats.class_index.is_none());
    assert!(!stopped.exists());
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 5, "no temporary files left");
}
