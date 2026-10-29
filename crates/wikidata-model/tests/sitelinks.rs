//! Sitelink counts and sites kept for the items of some classes.

use wikidata_model::{parse_entity, parse_entity_with, Backend, Compiled, Entity, ModelOptions, SitesFor};

fn item(id: &str, p31: &[(&str, &str)], sites: &[&str], claims_first: bool) -> String {
    let statements: Vec<String> = p31
        .iter()
        .map(|(q, rank)| {
            format!(
                r#"{{"mainsnak":{{"snaktype":"value","property":"P31","datavalue":{{"value":{{"entity-type":"item","id":"{q}"}},"type":"wikibase-entityid"}},"datatype":"wikibase-item"}},"type":"statement","rank":"{rank}"}}"#
            )
        })
        .collect();
    let claims = format!(r#""claims":{{"P31":[{}]}}"#, statements.join(","));
    let links: Vec<String> = sites
        .iter()
        .map(|s| format!(r#""{s}":{{"site":"{s}","title":"T {s}","badges":[]}}"#))
        .collect();
    let sitelinks = format!(r#""sitelinks":{{{}}}"#, links.join(","));
    let (a, b) = if claims_first {
        (claims, sitelinks)
    } else {
        (sitelinks, claims)
    };
    format!(r#"{{"type":"item","id":"{id}","labels":{{}},{a},{b}}}"#)
}

const ARTIST_SITES: [&str; 7] = [
    "enwiki",
    "nlwiki",
    "dewiki",
    "zh_yuewiki",
    "commonswiki",
    "enwikiquote",
    "wikidatawiki",
];

fn sites(e: &Entity) -> Vec<&str> {
    e.sitelinks.iter().map(|(s, _)| s.as_str()).collect()
}

fn parse_all(line: &str, c: &Compiled) -> Vec<Entity> {
    let mut out = vec![parse_entity_with(line.as_bytes(), c, Backend::SerdeJson).unwrap()];
    #[cfg(feature = "sonic")]
    out.push(parse_entity_with(line.as_bytes(), c, Backend::Sonic).unwrap());
    out
}

#[test]
fn counts_include_the_pruned_sitelinks() {
    let c = Compiled::new(ModelOptions {
        sitelinks: Some(vec!["enwiki".into()]),
        sitelink_counts: true,
        ..Default::default()
    });
    let line = item("Q1", &[("Q5", "normal")], &ARTIST_SITES, true);
    for e in parse_all(&line, &c) {
        assert_eq!(sites(&e), ["enwiki"]);
        assert_eq!(e.sitelink_count, Some(7));
        // enwiki, nlwiki, dewiki, zh_yuewiki.
        assert_eq!(e.wikipedia_count, Some(4));
        let json = e.to_json();
        assert!(json.contains(r#""sitelinkCount":7,"wikipediaCount":4"#), "{json}");
    }
    // None kept: still counted.
    let none = Compiled::new(ModelOptions {
        sitelinks: Some(Vec::new()),
        sitelink_counts: true,
        ..Default::default()
    });
    let e = parse_entity(line.as_bytes(), &none).unwrap();
    assert!(e.sitelinks.is_empty());
    assert_eq!((e.sitelink_count, e.wikipedia_count), (Some(7), Some(4)));
    // No sitelinks, or `[]`: zero.
    let e = parse_entity(br#"{"type":"item","id":"Q2","sitelinks":[]}"#, &none).unwrap();
    assert_eq!((e.sitelink_count, e.wikipedia_count), (Some(0), Some(0)));
    let e = parse_entity(br#"{"type":"item","id":"Q2"}"#, &none).unwrap();
    assert_eq!((e.sitelink_count, e.wikipedia_count), (Some(0), Some(0)));
    // Off by default, and then not in the JSON.
    let e = parse_entity(line.as_bytes(), &Compiled::default()).unwrap();
    assert_eq!((e.sitelink_count, e.wikipedia_count), (None, None));
    assert!(!e.to_json().contains("sitelinkCount"));
}

#[test]
fn every_wikipedia_for_the_listed_classes() {
    let c = Compiled::new(ModelOptions {
        sitelinks: Some(vec!["nlwiki".into(), "enwiki".into()]),
        sitelinks_for: Some(SitesFor {
            instance_of: vec!["Q5".into(), "Q215380".into()],
            groups: vec!["wikipedia".into()],
        }),
        ..Default::default()
    });
    for claims_first in [true, false] {
        let artist = item("Q1", &[("Q215380", "normal")], &ARTIST_SITES, claims_first);
        for e in parse_all(&artist, &c) {
            assert_eq!(
                sites(&e),
                ["enwiki", "nlwiki", "dewiki", "zh_yuewiki"],
                "{claims_first}"
            );
        }
        let city = item("Q2", &[("Q515", "normal")], &ARTIST_SITES, claims_first);
        for e in parse_all(&city, &c) {
            assert_eq!(sites(&e), ["enwiki", "nlwiki"], "{claims_first}");
        }
        // A deprecated P31 does not count, nor a normal one beside a
        // preferred one.
        let deprecated = item("Q3", &[("Q5", "deprecated")], &ARTIST_SITES, claims_first);
        assert_eq!(
            sites(&parse_entity(deprecated.as_bytes(), &c).unwrap()),
            ["enwiki", "nlwiki"]
        );
        let outranked = item(
            "Q4",
            &[("Q515", "preferred"), ("Q5", "normal")],
            &ARTIST_SITES,
            claims_first,
        );
        assert_eq!(
            sites(&parse_entity(outranked.as_bytes(), &c).unwrap()),
            ["enwiki", "nlwiki"]
        );
    }
    // P31 is read for the rule even when its claims are not kept.
    let pruned = Compiled::new(ModelOptions {
        sitelinks: Some(vec!["enwiki".into()]),
        properties: Some(Vec::new()),
        derived: false,
        sitelinks_for: Some(SitesFor {
            instance_of: vec!["Q5".into()],
            groups: vec!["wikipedia".into(), "wikiquote".into(), "special".into()],
        }),
        ..Default::default()
    });
    let e = parse_entity(item("Q1", &[("Q5", "normal")], &ARTIST_SITES, true).as_bytes(), &pruned).unwrap();
    assert!(e.claims.is_empty());
    assert_eq!(sites(&e), ARTIST_SITES);
    // No groups: every site.
    let all = Compiled::new(ModelOptions {
        sitelinks: Some(Vec::new()),
        sitelinks_for: Some(SitesFor {
            instance_of: vec!["Q5".into()],
            groups: Vec::new(),
        }),
        ..Default::default()
    });
    let e = parse_entity(item("Q1", &[("Q5", "normal")], &ARTIST_SITES, false).as_bytes(), &all).unwrap();
    assert_eq!(sites(&e), ARTIST_SITES);
}

#[test]
fn site_groups_are_checked() {
    let ok = SitesFor {
        instance_of: vec!["Q5".into()],
        groups: vec!["wikipedia".into(), "special".into()],
    };
    assert!(ok.validate().is_ok());
    let bad = SitesFor {
        groups: vec!["wikipedias".into()],
        ..ok
    };
    assert!(bad.validate().unwrap_err().contains("wikipedias"));
}
