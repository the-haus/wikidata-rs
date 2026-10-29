# wikidata-model

Wikidata entities as typed, normalised Rust values, parsed from dump
lines.

```rust
use wikidata_model::{parse_entity, Compiled, ModelOptions, Pid, Value};

let opts = Compiled::new(ModelOptions {
    languages: Some(vec!["nl".into(), "en".into(), "mul".into()]),
    properties: Some(vec![Pid(31)]),
    ..Default::default()
});
let line = br#"{"type":"item","id":"Q42","labels":{"en":{"language":"en","value":"Douglas Adams"}},"claims":{"P31":[{"mainsnak":{"snaktype":"value","property":"P31","datavalue":{"value":{"entity-type":"item","numeric-id":5,"id":"Q5"},"type":"wikibase-entityid"},"datatype":"wikibase-item"},"type":"statement","rank":"normal"}]}}"#;
let e = parse_entity(line, &opts)?;
assert_eq!(e.label.as_ref().unwrap().language, "en"); // nl → en → mul
assert_eq!(e.statements(Pid(31))[0].value, Value::Entity { id: "Q5".into() });
assert_eq!(e.derived.instance_of, ["Q5"]);
println!("{}", e.to_json());
# Ok::<(), wikidata_model::ParseError>(())
```

- Serde seeds driven by the options: unwanted languages, properties,
  sites and references are skipped while parsing, never allocated.
  Runs on serde_json (exact floats) or sonic-rs, picked by the options.
- Typed values: entity references, strings, external ids, URLs, Commons
  files with page URLs, monolingual texts, times with precision-aware
  `iso` dates and calendars, quantities with units and bounds,
  coordinates, maths, musical notation, geo-shapes, tabular data,
  `somevalue`/`novalue`.
- Ranks (deprecated dropped by default, `best_rank`), qualifiers (all, or
  only `qualifier_properties`), references, sitelinks with URLs, lexemes,
  properties.
- Sitelinks: the `sitelinks` sites for every entity and, with
  `sitelinks_for` (`SitesFor`), the sites of some groups (every Wikipedia)
  for the items of some classes, decided from P31 before the sitelinks are
  read; `sitelink_counts` counts them all, and the Wikipedia ones, the
  skipped ones included (`sitelink_count`, `wikipedia_count`). `sites`
  has the site groups (`site_group`, `is_wikipedia`) and page URLs.
- Derived fields from truthy statements: `instance_of`, `subclass_of`,
  `country`, `coordinates`, `inception`, `dissolved`, `image`, `website`,
  `external_ids` (the `EXTERNAL_IDS` table).
- `peek`: an entity's type and id from its line's first bytes;
  `lastrevid` from its tail.
- A stable JSON shape (`Entity::write_json`).
