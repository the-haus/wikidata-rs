# wikidata-filter

Filters for Wikidata dumps that reject lines before parsing them.

```rust
use wikidata_filter::{ClassIndex, Filter};

let index = ClassIndex::from_edges(vec![(99, 868557), (868557, 132241)], "example");
let f = Filter::parse_json(r#"{"all":[
    {"instanceOf":{"classes":["Q132241"],"subclasses":true}},
    {"claim":"P17=Q55"}
]}"#)?
.compile(Some(&index))?;
let line = br#"{"type":"item","id":"Q1","claims":{}}"#;
assert!(!f.matches_line(line, wikidata_model::peek(line).as_ref())?);
# Ok::<(), wikidata_filter::FilterError>(())
```

- `ids`, `type`, `hasProperty`, `claim`, `instanceOf` (with subclasses),
  `sitelink`, with `all` / `any` / `not`, from JSON or built in Rust.
  `ids` also takes item numbers (`Filter::ItemNumbers`, or
  `{"ids": {"$items": k}}` with `Filter::from_json_with`).
- Named filters (`{"any": {"venue": …}}`, `{"name": "venue", …}`):
  `matched_names` tells which of them an entity matches.
- Decided first from raw bytes (the type and id at the head of the line,
  memchr/Aho–Corasick needles for claims and sitelinks), three-valued;
  the rest after a probe parse of only the filter's properties and sites.
- `IdSet`: a Roaring-like id set, ~2 bytes an id, shared by every entity
  kind (`id_key`).
- `ClassIndex`: the P279 hierarchy as CSR arrays, closure queries both
  ways, saved as a small zstd-compressed file.
