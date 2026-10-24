mod common;

use common::*;
use wikidata_export::{
    build_class_index, file_info, Dump, EntityOptions, Filter, Flow, ModelOptions, ScanOptions, WriteOptions,
};
use wikidata_model::{Pid, Rank, Value};

fn by_id<'a>(rs: &'a [wikidata_export::Record], id: &str) -> &'a wikidata_model::Entity {
    &rs.iter()
        .find(|r| r.entity.id == id)
        .unwrap_or_else(|| panic!("{id} missing"))
        .entity
}

#[test]
fn every_format_reads_the_same() {
    let d = tmp();
    let data = sample();
    let lines = sample_lines();
    let want: Vec<String> = lines
        .iter()
        .map(|l| wikidata_model::peek(l).unwrap().id.to_string())
        .collect();
    assert_eq!(want.len(), 41);

    // Several bzip2 streams, cut at arbitrary bytes (not at lines).
    let mut multi = Vec::new();
    for part in data.chunks(data.len() / 3 + 1) {
        multi.extend(bz2(part, 1));
    }
    let mut jsonl = Vec::new();
    for l in &lines {
        jsonl.extend_from_slice(l);
        jsonl.push(b'\n');
    }
    let mut inputs = vec![
        write(&d, "plain.json", &data),
        write(&d, "one.json.bz2", &bz2(&data, 9)),
        write(&d, "multi.json.bz2", &multi),
        write(&d, "lines.jsonl", &jsonl),
        // CRLF line endings (a dump saved or checked out on Windows).
        write(&d, "crlf.json", &crlf(&data)),
        write(&d, "crlf.json.bz2", &bz2(&crlf(&data), 9)),
        write(&d, "crlf.jsonl", &crlf(&jsonl)),
    ];
    for name in ["c.json.gz", "c.json.zst", "c.jsonl.zst"] {
        let p = d.join(name);
        let mut w = wikidata_dump::DumpWriter::create(&p, &WriteOptions::default()).unwrap();
        std::io::Write::write_all(&mut w, if name.contains("jsonl") { &jsonl } else { &data }).unwrap();
        w.finish().unwrap();
        inputs.push(p);
    }
    for p in inputs {
        let dump = Dump::open(p.as_path()).unwrap();
        for threads in [1, 3] {
            let mut opts = EntityOptions::default();
            opts.scan.threads = threads;
            let (rs, stats) = collect(&dump, &opts);
            assert_eq!(ids(&rs), want, "{}", p.display());
            assert_eq!(stats.entities, 41);
            assert!(!stats.truncated);
        }
    }
}

/// `data` with every `\n` made `\r\n` (an LF fixture assumed).
fn crlf(data: &[u8]) -> Vec<u8> {
    assert!(!data.contains(&b'\r'));
    let mut out = Vec::with_capacity(data.len() + data.len() / 64);
    for &b in data {
        if b == b'\n' {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

#[test]
fn typed_values() {
    let dump = Dump::open(sample_path()).unwrap();
    let mut opts = EntityOptions::default();
    opts.model.references = true;
    opts.json = true;
    let (rs, _) = collect(&dump, &opts);
    let be = by_id(&rs, "Q31");

    let first = |p: u32| &be.statements(Pid(p))[0];
    assert_eq!(first(31).value, Value::Entity { id: "Q6256".into() });
    assert_eq!(first(31).rank, Rank::Preferred);
    match &first(625).value {
        Value::Coordinate(c) => {
            assert_eq!((c.lat, c.lon, c.globe.as_str()), (50.641111, 4.668056, "Q2"));
            assert_eq!(c.precision, Some(0.0000277778));
        }
        v => panic!("{v:?}"),
    }
    match &first(571).value {
        Value::Time(t) => {
            assert_eq!((t.iso.as_str(), t.year, t.precision_name), ("1830-10-04", 1830, "day"));
            assert_eq!(t.calendar, "gregorian");
        }
        v => panic!("{v:?}"),
    }
    assert_eq!(first(571).references.len(), 1);
    assert_eq!(
        first(571).references[0].get(&Pid(854)).unwrap()[0],
        Value::Url {
            value: "https://example.org/belgium".into()
        }
    );
    match &first(18).value {
        Value::Media { file, url } => {
            assert_eq!(file, "Flag of Belgium (civil).svg");
            assert_eq!(
                url,
                "https://commons.wikimedia.org/wiki/File:Flag_of_Belgium_(civil).svg"
            );
        }
        v => panic!("{v:?}"),
    }
    let pop = first(1082);
    match &pop.value {
        Value::Quantity(q) => assert_eq!((q.amount, q.unit.as_deref()), (11_697_557.0, None)),
        v => panic!("{v:?}"),
    }
    match &pop.qualifiers.get(&Pid(585)).unwrap()[0] {
        Value::Time(t) => assert_eq!(t.iso, "2023-01-01"),
        v => panic!("{v:?}"),
    }
    match &be.statements(Pid(1082))[1].qualifiers.get(&Pid(585)).unwrap()[0] {
        Value::Time(t) => assert_eq!((t.iso.as_str(), t.month), ("2010", None)),
        v => panic!("{v:?}"),
    }
    assert_eq!(first(297).value, Value::ExternalId { value: "BE".into() });
    assert_eq!(
        first(1448).value,
        Value::Monolingual {
            text: "Royaume de Belgique".into(),
            language: "fr".into()
        }
    );
    assert_eq!(
        be.statements(Pid(1448))[1].value,
        Value::Monolingual {
            text: "Koninkrijk België".into(),
            language: "nl".into()
        }
    );
    assert!(
        matches!(&first(3896).value, Value::GeoShape { value, url } if value == "Data:Belgium.map" && url.ends_with("Data:Belgium.map"))
    );
    assert!(matches!(&first(4179).value, Value::TabularData { value, .. } if value == "Data:Belgium population.tab"));
    match &first(2046).value {
        Value::Quantity(q) => {
            assert_eq!(q.unit.as_deref(), Some("Q712226"));
            assert_eq!((q.lower, q.upper), (Some(30688.0), Some(30690.0)));
        }
        v => panic!("{v:?}"),
    }
    // Deprecated statements are dropped by default.
    assert_eq!(be.statements(Pid(17)).len(), 1);
    assert_eq!(
        be.sitelinks.get("nlwiki").unwrap().url.as_deref(),
        Some("https://nl.wikipedia.org/wiki/Belgi%C3%AB")
    );
    assert_eq!(
        be.sitelinks.get("zh_yuewiki").unwrap().url.as_deref(),
        Some("https://zh-yue.wikipedia.org/wiki/%E6%AF%94%E5%88%A9%E6%99%82")
    );
    assert_eq!(be.labels.get("ja").map(String::as_str), Some("ベルギー"));

    let da = by_id(&rs, "Q42");
    assert_eq!(da.statements(Pid(40))[0].value, Value::SomeValue);
    assert_eq!(da.statements(Pid(26))[0].value, Value::NoValue);
    assert_eq!(da.aliases.get("en").unwrap()[1], "DNA 😀");
    match &da.statements(Pid(2048))[0].value {
        Value::Quantity(q) => assert_eq!((q.amount, q.unit.as_deref()), (1.96, Some("Q11573"))),
        v => panic!("{v:?}"),
    }

    let misc = by_id(&rs, "Q2002");
    let v = |p: u32| misc.statements(Pid(p))[0].value.clone();
    assert_eq!(v(2534), Value::Math { value: "E=mc^2".into() });
    assert_eq!(
        v(6604),
        Value::MusicalNotation {
            value: "\\relative c' { c d e f g }".into()
        }
    );
    assert_eq!(v(1545), Value::String { value: "1".into() });
    assert_eq!(v(1659), Value::Entity { id: "P31".into() });
    assert_eq!(v(5137), Value::Entity { id: "L7".into() });
    assert_eq!(v(5830), Value::Entity { id: "L7-F1".into() });
    assert_eq!(v(5972), Value::Entity { id: "L7-S1".into() });
    assert_eq!(v(12861), Value::Entity { id: "E10".into() });
    match v(625) {
        Value::Coordinate(c) => assert_eq!((c.globe.as_str(), c.precision), ("Q111", None)),
        v => panic!("{v:?}"),
    }

    let battle = by_id(&rs, "Q2001");
    match &battle.statements(Pid(585))[0].value {
        Value::Time(t) => assert_eq!(
            (t.iso.as_str(), t.year, t.calendar.as_str()),
            ("-0490-09-12", -490, "julian")
        ),
        v => panic!("{v:?}"),
    }

    let p31 = by_id(&rs, "P31");
    assert_eq!(p31.kind, wikidata_model::EntityType::Property);
    assert_eq!(p31.datatype.as_deref(), Some("wikibase-item"));

    let cat = by_id(&rs, "L7");
    let lx = cat.lexeme.as_ref().unwrap();
    assert_eq!(lx.lemmas.get("en").map(String::as_str), Some("cat"));
    assert_eq!(lx.lexical_category.as_deref(), Some("Q1084"));
    assert_eq!(lx.forms[0].representations.get("en").map(String::as_str), Some("cats"));
    assert_eq!(lx.forms[0].grammatical_features, ["Q146786"]);
    assert_eq!(
        lx.forms[0].claims.get(&Pid(898)).unwrap()[0].value,
        Value::String { value: "kæts".into() }
    );
    assert_eq!(lx.senses[0].glosses.len(), 2);
    let l8 = by_id(&rs, "L8");
    assert_eq!(l8.lexeme.as_ref().unwrap().language.as_deref(), Some("Q7411"));

    let empty = by_id(&rs, "Q2003");
    assert!(empty.labels.is_empty() && empty.claims.is_empty() && empty.label.is_none());

    // The JSON is the entity, with a stable shape.
    let r = rs.iter().find(|r| r.entity.id == "Q31").unwrap();
    let j: serde_json::Value = serde_json::from_slice(&r.json).unwrap();
    assert_eq!(j["id"], "Q31");
    assert_eq!(j["type"], "item");
    assert_eq!(j["claims"]["P31"][0]["value"]["type"], "entity");
    assert_eq!(j["claims"]["P571"][0]["value"]["iso"], "1830-10-04");
    assert_eq!(j["externalIds"]["osmRelation"], "52411");
    assert_eq!(j["coordinates"]["lat"], 50.641111);
    let empty_json: serde_json::Value =
        serde_json::from_slice(&rs.iter().find(|r| r.entity.id == "Q2003").unwrap().json).unwrap();
    for k in ["label", "coordinates", "inception", "image", "website"] {
        assert!(empty_json[k].is_null(), "{k}");
    }
    for k in ["instanceOf", "country"] {
        assert_eq!(empty_json[k], serde_json::json!([]));
    }
}

#[test]
fn languages_ranks_and_derived() {
    let dump = Dump::open(sample_path()).unwrap();
    let mut opts = EntityOptions {
        model: ModelOptions {
            languages: Some(vec!["nl".into(), "en".into(), "mul".into()]),
            sitelinks: Some(vec!["nlwiki".into()]),
            properties: Some(vec![Pid(31), Pid(17)]),
            best_rank: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let (rs, _) = collect(&dump, &opts);
    let be = by_id(&rs, "Q31");
    assert_eq!(be.label.as_ref().unwrap().text, "België");
    assert_eq!(be.label.as_ref().unwrap().language, "nl");
    assert_eq!(
        be.labels.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(),
        ["nl", "en", "mul"]
    );
    assert_eq!(be.sitelinks.len(), 1);
    assert_eq!(be.claims.len(), 2);
    // best rank: the preferred P31 only.
    assert_eq!(be.statements(Pid(31)).len(), 1);
    // Derived fields, whatever `properties` says.
    let d = &be.derived;
    assert_eq!(d.instance_of, ["Q6256"]);
    assert_eq!(d.country, ["Q31"]);
    assert_eq!(d.coordinates, Some((50.641111, 4.668056)));
    assert_eq!(d.inception, Some(1830));
    assert_eq!(d.image.as_deref(), Some("Flag of Belgium (civil).svg"));
    assert_eq!(d.website.as_deref(), Some("https://www.belgium.be/"));
    let keys: Vec<&str> = d.external_ids.iter().map(|(k, _)| *k).collect();
    assert_eq!(keys, ["osmRelation", "geonames"]);
    // Fallback to English.
    let gone = by_id(&rs, "Q1074");
    assert_eq!(gone.label.as_ref().unwrap().language, "en");
    assert_eq!(
        (gone.derived.inception, gone.derived.dissolved),
        (Some(1990), Some(2005))
    );
    let da = by_id(&rs, "Q42");
    assert_eq!(da.derived.image, None, "deprecated image");
    assert_eq!(
        da.derived.external_ids,
        vec![
            ("musicbrainzArtist", "e9ed318d-8cc5-4cf8-ab77-505e39ab6ea4".to_string()),
            ("discogsArtist", "66941".into()),
            ("isni", "0000 0000 8045 6315".into()),
            ("viaf", "113230702".into()),
            ("x", "douglasadams".into()),
        ]
    );
    // Subclass: truthy P279 (preferred only).
    assert_eq!(by_id(&rs, "Q99000001").derived.subclass_of, ["Q868557"]);

    // Keeping deprecated statements.
    opts.model.best_rank = false;
    opts.model.deprecated = true;
    let (rs, _) = collect(&dump, &opts);
    assert_eq!(by_id(&rs, "Q31").statements(Pid(17)).len(), 2);
    assert_eq!(by_id(&rs, "Q31").statements(Pid(31)).len(), 2);
}

fn filtered(filter: &str, classes: Option<&wikidata_export::ClassIndex>) -> Vec<String> {
    let dump = Dump::open(sample_path()).unwrap();
    let mut opts = EntityOptions::default();
    opts.scan.filter = Some(Filter::parse_json(filter).unwrap().compile(classes).unwrap().into());
    let (rs, stats) = collect(&dump, &opts);
    assert_eq!(stats.entities, 41);
    assert_eq!(stats.matched as usize, rs.len());
    ids(&rs)
}

#[test]
fn filters() {
    assert_eq!(
        filtered(r#"{"ids":["Q42","P31","L7","Q404"]}"#, None),
        ["Q42", "P31", "L7"]
    );
    assert_eq!(filtered(r#"{"type":"lexeme"}"#, None), ["L7", "L8"]);
    assert_eq!(filtered(r#"{"type":"property"}"#, None), ["P31", "P625"]);
    let nl = filtered(r#"{"claim":"P17=Q55"}"#, None);
    assert_eq!(&nl[..3], ["Q55", "Q1072", "Q1074"]);
    assert_eq!(nl.len(), 3 + 6);
    // Q31's P17=Q55 is deprecated.
    assert!(!nl.contains(&"Q31".to_string()));
    assert_eq!(
        filtered(r#"{"all":[{"hasProperty":"P434"},{"type":"item"}]}"#, None),
        ["Q42", "Q1072", "Q5000", "Q5005", "Q5010", "Q5015", "Q5020"]
    );
    assert_eq!(filtered(r#"{"instanceOf":["Q6256"]}"#, None), ["Q31", "Q55"]);
    assert_eq!(
        filtered(r#"{"sitelink":"nlwiki","not":{"instanceOf":["Q6256"]}}"#, None),
        ["Q42", "Q1072", "Q1073"]
    );
    assert_eq!(
        filtered(
            r#"{"any":[{"ids":"Q2003"},{"claim":{"property":"P297","values":["NL"]}}]}"#,
            None
        ),
        ["Q55", "Q2003"]
    );
    assert_eq!(
        filtered(r#"{"claim":{"P1448":["Royaume de Belgique"]}}"#, None),
        ["Q31"]
    );
    assert_eq!(filtered(r#"{"claim":{"P1448":["Koninkrijk België"]}}"#, None), ["Q31"]);
}

#[test]
fn class_index_and_subclasses() {
    let dump = Dump::open(sample_path()).unwrap();
    let (classes, stats) = build_class_index(&dump, &ScanOptions::default(), &mut |_| Flow::Continue).unwrap();
    assert_eq!(stats.entities, 41);
    // Q868557 → Q132241 → Q1656682; Q99000001 → Q868557 and Q1656682
    // (truthy: the preferred one only); the deprecated edge is ignored.
    assert_eq!(classes.parents(99000001), &[868557]);
    assert_eq!(classes.edges(), 3);
    let d = tmp();
    let path = d.join("classes.wdc");
    classes.save(&path).unwrap();
    let loaded = wikidata_export::ClassIndex::load(&path).unwrap();
    assert_eq!(loaded.source(), "sample.json");
    let music = filtered(
        r#"{"instanceOf":{"classes":["Q868557"],"subclasses":true}}"#,
        Some(&loaded),
    );
    assert_eq!(music, ["Q1072", "Q1073"]);
    let fests = filtered(
        r#"{"instanceOf":{"classes":["Q132241"],"subclasses":true}}"#,
        Some(&loaded),
    );
    assert_eq!(fests, ["Q1072", "Q1073", "Q1074"]);
    let direct = filtered(
        r#"{"instanceOf":{"classes":["Q868557"],"subclasses":false}}"#,
        Some(&loaded),
    );
    assert_eq!(direct, ["Q1073"]);
}

#[test]
fn limit_and_stop() {
    let dump = Dump::open(sample_path()).unwrap();
    let mut opts = EntityOptions::default();
    opts.scan.limit = Some(5);
    let (rs, stats) = collect(&dump, &opts);
    assert_eq!(rs.len(), 5);
    assert!(stats.stopped);

    let mut n = 0;
    let stats = wikidata_export::entities(&dump, &EntityOptions::default(), &mut |e| {
        if let wikidata_export::Event::Records(rs) = e {
            n += rs.len();
            return Flow::Stop;
        }
        Flow::Continue
    })
    .unwrap();
    assert!(stats.stopped);
    assert!(n >= 1);
}

#[test]
fn file_info_counts() {
    let d = tmp();
    let p = write(&d, "s.json.bz2", &bz2(&sample(), 9));
    let dump = Dump::open(p.as_path()).unwrap();
    let info = file_info(&dump, &ScanOptions::default(), &mut |_| Flow::Continue).unwrap();
    assert_eq!(info.scan.entities, 41);
    assert_eq!(info.scan.types.items, 37);
    assert_eq!(info.scan.types.properties, 2);
    assert_eq!(info.scan.types.lexemes, 2);
    assert_eq!(info.scan.compression, "bzip2");
    assert_eq!(info.scan.streams, 1);
    assert_eq!(info.scan.bytes_out, sample().len() as u64);
    assert_eq!(info.max_ids.item, 99000001);
    assert_eq!(info.max_ids.property, 625);
    assert_eq!(info.largest.id, "Q31");
    let j = serde_json::to_value(&info).unwrap();
    assert_eq!(j["types"]["items"], 37);
}

#[test]
fn last_progress_has_the_final_totals() {
    // However the scan ends (finished, a limit, the sink stopping), its last
    // progress event carries the totals of the returned stats.
    let dump = Dump::open(sample_path()).unwrap();
    for (limit, stop_after) in [(None, None), (Some(5), None), (None, Some(1))] {
        for threads in [1, 3] {
            let mut opts = EntityOptions::default();
            opts.scan.threads = threads;
            opts.scan.limit = limit;
            let mut last = None;
            let mut batches = 0;
            let stats = wikidata_export::entities(&dump, &opts, &mut |e| match e {
                wikidata_export::Event::Progress(p) => {
                    last = Some(p);
                    Flow::Continue
                }
                wikidata_export::Event::Records(_) => {
                    batches += 1;
                    if stop_after.is_some_and(|n| batches >= n) {
                        Flow::Stop
                    } else {
                        Flow::Continue
                    }
                }
                wikidata_export::Event::Checkpoint(_) => Flow::Continue,
            })
            .unwrap();
            let last = last.expect("a progress event");
            let case = format!("limit {limit:?}, stop after {stop_after:?}, {threads} threads");
            assert_eq!(last.entities, stats.entities, "{case}");
            assert_eq!(last.matched, stats.matched, "{case}");
            assert_eq!(last.bytes_out, stats.bytes_out, "{case}");
            assert_eq!(stats.stopped, limit.is_some() || stop_after.is_some(), "{case}");
            if limit.is_none() && stop_after.is_none() {
                assert_eq!(last.entities, 41);
                assert_eq!(last.bytes_in, stats.bytes_in);
            }
        }
    }
}

#[test]
fn qualifiers_by_property() {
    let dump = Dump::open(sample_path()).unwrap();
    let mut opts = EntityOptions::default();
    opts.scan.filter = Some(
        Filter::parse_json(r#"{"ids":"Q31"}"#)
            .unwrap()
            .compile(None)
            .unwrap()
            .into(),
    );
    let (rs, _) = collect(&dump, &opts);
    let all = &by_id(&rs, "Q31").statements(Pid(1082))[0].qualifiers;
    assert_eq!(all.iter().map(|(p, _)| p.0).collect::<Vec<_>>(), [585, 459]);

    opts.model.qualifier_properties = Some(vec![Pid(585)]);
    let (rs, _) = collect(&dump, &opts);
    let be = by_id(&rs, "Q31");
    let kept = &be.statements(Pid(1082))[0].qualifiers;
    assert_eq!(kept.iter().map(|(p, _)| p.0).collect::<Vec<_>>(), [585]);
    match &kept.get(&Pid(585)).unwrap()[0] {
        Value::Time(t) => assert_eq!(t.iso, "2023-01-01"),
        v => panic!("{v:?}"),
    }
    // References are not qualifiers: they are kept as asked.
    opts.model.references = true;
    let (rs, _) = collect(&dump, &opts);
    assert_eq!(by_id(&rs, "Q31").statements(Pid(1082))[0].references[0].len(), 2);
}
