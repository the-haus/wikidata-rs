mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::*;
use wikidata_export::{
    build_redirect_index, Dump, EntityOptions, Filter, Flow, Indexes, Input, Merged, RedirectIndex, RedirectOptions,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(name)
}

fn gzip(path: &Path, dir: &Path) -> PathBuf {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(&std::fs::read(path).unwrap()).unwrap();
    write(
        dir,
        &format!("{}.gz", path.file_name().unwrap().to_string_lossy()),
        &e.finish().unwrap(),
    )
}

fn index(redirects: &Path, pages: &Path) -> RedirectIndex {
    build_redirect_index(
        &Input::from(redirects),
        &Input::from(pages),
        &RedirectOptions::default(),
        &mut |_| Flow::Continue,
    )
    .unwrap()
    .0
}

#[test]
fn redirects_from_the_sql_dumps() {
    let d = tmp();
    let plain = index(&fixture("wikidatawiki-redirect.sql"), &fixture("wikidatawiki-page.sql"));
    let gz = index(
        &gzip(&fixture("wikidatawiki-redirect.sql"), &d),
        &gzip(&fixture("wikidatawiki-page.sql"), &d),
    );
    for r in [&plain, &gz] {
        assert_eq!(
            r.pairs().collect::<Vec<_>>(),
            [(1, 42), (7, 55), (8, 99_999_999), (9, 10), (10, 42)],
            "items only, not the property or the talk page"
        );
        assert_eq!(r.resolve_id("Q1").as_deref(), Some("Q42"));
        assert_eq!(r.resolve_id("Q9").as_deref(), Some("Q42"), "a double redirect");
        assert_eq!(r.resolve_id("Q42"), None);
    }
    assert_eq!(plain.source(), "wikidatawiki-redirect.sql + wikidatawiki-page.sql");
    let saved = d.join("redirects.wdr");
    plain.save(&saved).unwrap();
    assert_eq!(RedirectIndex::load(&saved).unwrap().len(), 5);
    // The wrong table is an error, not an empty index.
    let wrong = build_redirect_index(
        &Input::from(fixture("wikidatawiki-page.sql").as_path()),
        &Input::from(fixture("wikidatawiki-page.sql").as_path()),
        &RedirectOptions::default(),
        &mut |_| Flow::Continue,
    );
    assert!(wrong.is_err());
}

#[test]
fn misses_are_merged_deleted_or_newer() {
    let redirects = Arc::new(index(
        &fixture("wikidatawiki-redirect.sql"),
        &fixture("wikidatawiki-page.sql"),
    ));
    let dump = Dump::open(sample_path()).unwrap();
    let filter = Filter::parse_json(r#"{"ids":["Q1","Q7","Q8","Q9","Q3","Q999999999","Q31"]}"#).unwrap();
    let mut opts = EntityOptions::default();
    opts.scan.filter = Some(
        filter
            .compile_with(&Indexes {
                redirects: Some(redirects),
                ..Default::default()
            })
            .unwrap()
            .into(),
    );
    let (rs, stats) = collect(&dump, &opts);
    // The items merged into come through, saying what was merged in.
    let got: Vec<(String, Vec<String>)> = rs
        .iter()
        .map(|r| (r.entity.id.clone(), r.merged_from.clone()))
        .collect();
    assert_eq!(
        got,
        [
            ("Q31".to_string(), vec![]),
            ("Q55".into(), vec!["Q7".to_string()]),
            ("Q42".into(), vec!["Q1".to_string(), "Q9".into()]),
        ]
    );
    let m = stats.misses.unwrap();
    let merged = |from: &str, to: &str, in_dump| Merged {
        from: from.into(),
        to: to.into(),
        in_dump,
    };
    assert_eq!(
        m.merged,
        [
            merged("Q1", "Q42", true),
            merged("Q7", "Q55", true),
            merged("Q8", "Q99999999", false),
            merged("Q9", "Q42", true)
        ]
    );
    assert_eq!(m.deleted, ["Q3"]);
    assert_eq!(m.newer, ["Q999999999"], "above the dump's highest item, Q99000001");
    assert_eq!(stats.missing_ids.unwrap(), ["Q1", "Q3", "Q7", "Q8", "Q9", "Q999999999"]);
    let json = serde_json::to_value(wikidata_export::ScanStats {
        misses: Some(m),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(json["merged"][0]["inDump"], true);
    assert_eq!(json["newer"][0], "Q999999999");
}
