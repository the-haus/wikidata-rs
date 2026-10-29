mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::*;
use wikidata_export::{
    build_class_index, build_place_index, ClassIndex, Dump, EntityOptions, Filter, Flow, Indexes, PlaceIndex,
    ScanOptions,
};

fn places_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/places.json")
}

fn index_of(path: &Path) -> PlaceIndex {
    let dump = Dump::open(path).unwrap();
    build_place_index(&dump, &ScanOptions::default(), &mut |_| Flow::Continue)
        .unwrap()
        .0
}

fn in_country(index: Arc<PlaceIndex>, countries: &str) -> Vec<String> {
    let dump = Dump::open(places_path().as_path()).unwrap();
    let mut opts = EntityOptions::default();
    let f = Filter::parse_json(&format!(r#"{{"inCountry":{countries}}}"#)).unwrap();
    opts.scan.filter = Some(
        f.compile_with(&Indexes {
            places: Some(index),
            ..Default::default()
        })
        .unwrap()
        .into(),
    );
    ids(&collect(&dump, &opts).0)
}

#[test]
fn countries_through_the_p131_chain() {
    let index = index_of(&places_path());
    assert_eq!(
        index.country_of("Q9003"),
        ["Q55"],
        "venue → village → municipality → NL"
    );
    assert_eq!(index.country_of("Q9002"), ["Q55"]);
    assert_eq!(
        index.country_of("Q9004"),
        ["Q55", "Q145"],
        "P17, then the headquarters' country"
    );
    assert_eq!(index.country_of("Q9009"), ["Q55"], "the deprecated P131 does not count");
    assert_eq!(index.country_of("Q9013"), ["Q31"]);
    // The cycle ends, and gives no country.
    for id in ["Q9006", "Q9007", "Q9008", "Q9011", "Q9012"] {
        assert!(index.country_of(id).is_empty(), "{id}");
    }

    let index = Arc::new(index);
    assert_eq!(
        in_country(index.clone(), r#""Q55""#),
        ["Q55", "Q9001", "Q9002", "Q9003", "Q9004", "Q9009"]
    );
    assert_eq!(in_country(index.clone(), r#"["Q145"]"#), ["Q145", "Q9004", "Q9005"]);
    assert_eq!(
        in_country(index.clone(), r#"["Q31","Q145"]"#),
        ["Q31", "Q145", "Q9004", "Q9005", "Q9013", "Q9014"]
    );
    // Without an index, the filter cannot compile.
    assert!(Filter::parse_json(r#"{"inCountry":"Q55"}"#)
        .unwrap()
        .compile(None)
        .is_err());
}

#[test]
fn items_the_index_lacks_are_decided_from_their_claims() {
    // An index built without the venue and the festival (an older dump).
    let d = tmp();
    let text = std::fs::read_to_string(places_path()).unwrap();
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| !l.contains(r#""id":"Q9003""#) && !l.contains(r#""id":"Q9009""#))
        .collect();
    let mut older = kept.join("\n");
    older = older.replace("},\n]", "}\n]");
    let older = write(&d, "older.json", older.as_bytes());
    let index = index_of(&older);
    assert!(index.country_of("Q9003").is_empty());
    let got = in_country(Arc::new(index), r#""Q55""#);
    assert!(
        got.contains(&"Q9003".to_string()) && got.contains(&"Q9009".to_string()),
        "{got:?}"
    );
}

#[test]
fn both_indexes_in_one_pass() {
    let d = tmp();
    let dump = Dump::open(places_path().as_path()).unwrap();
    let opts = ScanOptions {
        place_index: Some(d.join("places.wdp")),
        ..Default::default()
    };
    let (classes, stats) = build_class_index(&dump, &opts, &mut |_| Flow::Continue).unwrap();
    assert!(classes.is_empty(), "no P279 here");
    let saved = stats.place_index.unwrap();
    assert_eq!(saved.items, index_of(&places_path()).len() as u64);
    let loaded = PlaceIndex::load(d.join("places.wdp")).unwrap();
    assert_eq!(loaded.country_of("Q9003"), ["Q55"]);
    assert_eq!(loaded.source(), "places.json");

    // And as a by-product of an entities pass.
    let mut eopts = EntityOptions::default();
    eopts.scan.place_index = Some(d.join("again.wdp"));
    eopts.scan.class_index = Some(d.join("classes.wdc"));
    let (_, stats) = collect(&dump, &eopts);
    assert!(stats.place_index.is_some() && stats.class_index.is_some());
    assert_eq!(
        PlaceIndex::load(d.join("again.wdp")).unwrap().country_of("Q9004"),
        ["Q55", "Q145"]
    );
    assert!(ClassIndex::load(d.join("classes.wdc")).is_ok());
}

#[test]
fn languages_by_country_through_the_place_index() {
    let dump = Dump::open(places_path().as_path()).unwrap();
    let rule = |places| wikidata_export::ByClaim {
        properties: vec![wikidata_model::Pid(17)],
        map: [("Q55".to_string(), vec!["nl".to_string()])].into(),
        always: vec!["en".into()],
        places,
        all_for: None,
    };
    let opts = |places| EntityOptions {
        languages_by: Some(rule(places)),
        ..Default::default()
    };
    let langs = |rs: &[wikidata_export::Record], id: &str| -> Vec<String> {
        let e = &rs.iter().find(|r| r.entity.id == id).unwrap().entity;
        e.labels.iter().map(|(l, _)| l.clone()).collect()
    };
    // P17 only: the village names its place, not its country.
    let (rs, _) = collect(&dump, &opts(None));
    assert_eq!(langs(&rs, "Q9001"), ["nl", "en"]);
    assert_eq!(langs(&rs, "Q9002"), ["en"]);
    let (rs, _) = collect(&dump, &opts(Some(Arc::new(index_of(&places_path())))));
    assert_eq!(langs(&rs, "Q9002"), ["nl", "en"]);
    assert_eq!(
        rs.iter()
            .find(|r| r.entity.id == "Q9002")
            .unwrap()
            .entity
            .label
            .as_ref()
            .unwrap()
            .text,
        "Schaesberg"
    );
    assert_eq!(langs(&rs, "Q9005"), ["en"], "London is not Dutch");
}

/// An item newer than the place index gets its country's languages from
/// its P131, as `inCountry` places it (Festimap's W-18).
#[test]
fn languages_for_items_newer_than_the_index() {
    let d = tmp();
    let text = std::fs::read_to_string(places_path()).unwrap();
    let older: Vec<&str> = text.lines().filter(|l| !l.contains(r#""id":"Q9009""#)).collect();
    let older = write(&d, "older.json", older.join("\n").replace("},\n]", "}\n]").as_bytes());
    let index = Arc::new(index_of(&older));
    assert!(index.country_of("Q9009").is_empty());
    let dump = Dump::open(places_path().as_path()).unwrap();
    let mut opts = EntityOptions {
        languages_by: Some(wikidata_export::ByClaim {
            properties: vec![wikidata_model::Pid(17)],
            map: [("Q55".to_string(), vec!["nl".to_string()])].into(),
            always: vec!["en".into()],
            places: Some(index.clone()),
            all_for: None,
        }),
        ..Default::default()
    };
    // P131 is read for the rule, though `properties` does not keep it.
    opts.model.properties = Some(vec![wikidata_model::Pid(31)]);
    let f = Filter::parse_json(r#"{"inCountry":"Q55"}"#).unwrap();
    opts.scan.filter = Some(
        f.compile_with(&Indexes {
            places: Some(index),
            ..Default::default()
        })
        .unwrap()
        .into(),
    );
    let (rs, _) = collect(&dump, &opts);
    // The festival (P131 Schaesberg, no P17) is newer than the index.
    let fest = &rs
        .iter()
        .find(|r| r.entity.id == "Q9009")
        .expect("matched by inCountry")
        .entity;
    assert_eq!(
        fest.labels.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(),
        ["nl", "en"]
    );
    assert_eq!(fest.label.as_ref().unwrap().language, "nl");
    assert!(
        fest.claims.get(&wikidata_model::Pid(131)).is_none(),
        "P131 read, not kept"
    );
}

/// Every Wikipedia for the items of some classes, the country's wiki
/// still chosen through the place index for both kinds (Festimap's W-21).
#[test]
fn every_wikipedia_for_artists_with_the_place_index() {
    let d = tmp();
    let item = |id: &str, class: &str| {
        format!(
            r#"{{"type":"item","id":"{id}","claims":{{"P31":[{{"mainsnak":{{"snaktype":"value","property":"P31","datavalue":{{"value":{{"entity-type":"item","id":"{class}"}},"type":"wikibase-entityid"}},"datatype":"wikibase-item"}},"type":"statement","rank":"normal"}}],"P131":[{{"mainsnak":{{"snaktype":"value","property":"P131","datavalue":{{"value":{{"entity-type":"item","id":"Q9002"}},"type":"wikibase-entityid"}},"datatype":"wikibase-item"}},"type":"statement","rank":"normal"}}]}},"sitelinks":{{"dewiki":{{"site":"dewiki","title":"A"}},"nlwiki":{{"site":"nlwiki","title":"B"}},"enwiki":{{"site":"enwiki","title":"C"}},"commonswiki":{{"site":"commonswiki","title":"D"}},"frwikivoyage":{{"site":"frwikivoyage","title":"E"}}}}}}"#
        )
    };
    let path = write(
        &d,
        "people.json",
        format!("[\n{},\n{}\n]\n", item("Q9100", "Q5"), item("Q9101", "Q8719053")).as_bytes(),
    );
    let rule = wikidata_export::ByClaim {
        properties: vec![wikidata_model::Pid(17)],
        map: [("Q55".to_string(), vec!["nlwiki".to_string()])].into(),
        always: vec!["enwiki".into()],
        places: Some(Arc::new(index_of(&places_path()))),
        all_for: Some(wikidata_model::SitesFor {
            instance_of: vec!["Q5".into(), "Q215380".into()],
            groups: vec!["wikipedia".into()],
        }),
    };
    let mut opts = EntityOptions {
        sitelinks_by: Some(rule),
        ..Default::default()
    };
    // P31 and P131 are read for the rule, though not kept.
    opts.model.properties = Some(vec![wikidata_model::Pid(17)]);
    opts.model.sitelink_counts = true;
    let (rs, _) = collect(&Dump::open(path.as_path()).unwrap(), &opts);
    let get = |id: &str| &rs.iter().find(|r| r.entity.id == id).unwrap().entity;
    let sites = |id: &str| get(id).sitelinks.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>();
    assert_eq!(sites("Q9100"), ["dewiki", "nlwiki", "enwiki"]);
    assert_eq!(sites("Q9101"), ["nlwiki", "enwiki"]);
    for id in ["Q9100", "Q9101"] {
        assert!(get(id).claims.is_empty());
        assert_eq!((get(id).sitelink_count, get(id).wikipedia_count), (Some(5), Some(3)));
    }
    // A rule for languages takes no classes.
    let bad = EntityOptions {
        languages_by: opts.sitelinks_by.clone(),
        ..Default::default()
    };
    assert!(wikidata_export::entities(&Dump::open(path.as_path()).unwrap(), &bad, &mut |_| Flow::Continue).is_err());
}
