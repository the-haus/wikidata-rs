mod common;

use std::path::{Path, PathBuf};

use common::*;
use wikidata_export::{Dump, EntityOptions, Filter, Redirect};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/incremental.xml")
}

fn read(
    path: &Path,
    filter: Option<&str>,
    allow_truncated: bool,
) -> Result<(Vec<wikidata_export::Record>, wikidata_export::ScanStats), wikidata_export::ExportError> {
    let dump = Dump::open(path)?;
    let mut opts = EntityOptions::default();
    opts.scan.allow_truncated = allow_truncated;
    if let Some(f) = filter {
        opts.scan.filter = Some(Filter::parse_json(f).unwrap().compile(None).unwrap().into());
    }
    let mut out = Vec::new();
    let stats = wikidata_export::entities(&dump, &opts, &mut |e| {
        if let wikidata_export::Event::Records(rs) = e {
            out.extend(rs);
        }
        wikidata_export::Flow::Continue
    })?;
    Ok((out, stats))
}

#[test]
fn each_changed_entity_once_at_its_newest_revision() {
    let d = tmp();
    let xml = std::fs::read(fixture()).unwrap();
    for path in [fixture(), write(&d, "incr.xml.bz2", &bz2(&xml, 1))] {
        let (rs, stats) = read(&path, None, false).unwrap();
        // Not the discussion page, the redirect or the deleted revision.
        assert_eq!(ids(&rs), ["Q42", "Q1072", "P31", "L7"], "{}", path.display());
        let q42 = &rs[0].entity;
        assert_eq!(q42.labels.get("en").unwrap(), "Douglas Adams (newest)");
        assert_eq!(q42.lastrevid, Some(2_200_000_003));
        assert_eq!(q42.modified.as_deref(), Some("2026-10-22T10:03:00Z"));
        assert_eq!(q42.derived.external_ids[0].0, "musicbrainzArtist");
        assert_eq!(
            stats.redirects,
            Some(vec![Redirect {
                from: "Q7".into(),
                to: "Q55".into()
            }])
        );
        assert_eq!(stats.entities, 4);
    }
    // Through the usual filters, revisions included.
    let (rs, _) = read(&fixture(), Some(r#"{"claim":"P17=Q55"}"#), false).unwrap();
    assert_eq!(ids(&rs), ["Q1072"]);
    let (rs, stats) = read(&fixture(), Some(r#"{"ids":{"Q42":2200000003,"Q1072":1}}"#), false).unwrap();
    assert_eq!(ids(&rs), ["Q1072"]);
    assert_eq!(stats.unchanged, 1);
}

#[test]
fn a_truncated_export_fails() {
    let d = tmp();
    let xml = std::fs::read_to_string(fixture()).unwrap();
    // Cut inside the Property:P31 page.
    let cut = xml.find("<title>Property:P31").unwrap() + 40;
    let p = write(&d, "cut.xml", &xml.as_bytes()[..cut]);
    let err = read(&p, None, false).unwrap_err();
    assert!(err.to_string().contains("truncated"), "{err}");
    let (rs, stats) = read(&p, None, true).unwrap();
    assert_eq!(ids(&rs), ["Q42", "Q1072"]);
    assert!(stats.truncated);
    // HTML is still not a dump.
    let p = write(&d, "page.html", b"<html><body>nope</body></html>\n");
    assert!(read(&p, None, false).is_err());
}
