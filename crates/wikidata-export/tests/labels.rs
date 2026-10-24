mod common;

use std::path::{Path, PathBuf};

use common::*;
use wikidata_export::{Dump, EntityOptions, Filter, LabelOptions, ScanStats};
use wikidata_model::Pid;

fn places_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/places.json")
}

fn festival(path: &Path) -> (Vec<String>, ScanStats) {
    let dump = Dump::open(path).unwrap();
    let mut opts = EntityOptions {
        referenced_labels: Some(LabelOptions {
            languages: vec!["en".into(), "nl".into()],
        }),
        ..Default::default()
    };
    opts.model.properties = Some(vec![Pid(131), Pid(136)]);
    opts.scan.filter = Some(
        Filter::parse_json(r#"{"ids":"Q9009"}"#)
            .unwrap()
            .compile(None)
            .unwrap()
            .into(),
    );
    let (rs, stats) = collect(&dump, &opts);
    (ids(&rs), stats)
}

fn labels(stats: &ScanStats) -> Vec<(u32, Vec<(String, String)>)> {
    stats.referenced_labels.as_ref().unwrap().labels.clone()
}

fn want() -> Vec<(u32, Vec<(String, String)>)> {
    let l = |en: &str, nl: &str| vec![("en".to_string(), en.to_string()), ("nl".to_string(), nl.to_string())];
    vec![
        (7411, l("Dutch", "Nederlands")),
        (9002, l("Schaesberg", "Schaesberg")),
        (9011, l("rock music", "rockmuziek")),
    ]
}

#[test]
fn labels_of_referenced_items_only() {
    let (got, stats) = festival(&places_path());
    assert_eq!(got, ["Q9009"]);
    // The village (before the festival), the genre and the qualifier's
    // language (after it); not the deprecated P131, not P31 (not kept),
    // and nothing unreferenced.
    assert_eq!(labels(&stats), want());
    let s = stats.labels.unwrap();
    assert_eq!((s.referenced, s.labelled), (3, 3));
    assert_eq!(s.during_pass, 2, "the two after the festival");
    let json = serde_json::to_value(stats.referenced_labels.as_ref().unwrap()).unwrap();
    assert_eq!(json["Q9011"]["nl"], "rockmuziek");
    assert!(json.get("Q9012").is_none());
}

/// The places fixture with filler between the village and the festival.
fn spread() -> Vec<u8> {
    let text = std::fs::read_to_string(places_path()).unwrap();
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| l.trim_end_matches(',').to_string())
        .collect();
    let fest = lines.iter().position(|l| l.contains(r#""id":"Q9009""#)).unwrap();
    let fest = lines.remove(fest);
    let filler = sample_lines();
    for copy in 1..150 {
        for l in &filler {
            let l = String::from_utf8_lossy(l).replacen("\"id\":\"Q", &format!("\"id\":\"Q{copy}00000"), 1);
            lines.push(l);
        }
    }
    lines.push(fest);
    format!("[\n{}\n]\n", lines.join(",\n")).into_bytes()
}

#[test]
fn only_the_chunks_holding_missing_items_are_read_again() {
    let d = tmp();
    let text = spread();
    for name in ["spread.json.bz2", "spread.json", "spread.json.gz"] {
        let p = d.join(name);
        match name {
            "spread.json.bz2" => std::fs::write(&p, bz2(&text, 1)).unwrap(),
            "spread.json" => std::fs::write(&p, &text).unwrap(),
            _ => {
                let mut w = wikidata_dump::DumpWriter::create(&p, &Default::default()).unwrap();
                std::io::Write::write_all(&mut w, &text).unwrap();
                w.finish().unwrap();
            }
        }
        let (got, stats) = festival(&p);
        assert_eq!(got, ["Q9009"], "{name}");
        assert_eq!(labels(&stats), want(), "{name}");
        let s = stats.labels.unwrap();
        assert!(s.chunks >= 2, "{name}: {s:?}");
        if name.ends_with(".gz") {
            assert_eq!(s.chunks_reread, s.chunks, "{name}: read again whole");
        } else {
            // The first chunk, and the last: the festival (Q9009) is in the
            // same range of 256 ids as the village (Q9002).
            assert_eq!(s.chunks_reread, 2, "{name}: {s:?}");
        }
    }
}
