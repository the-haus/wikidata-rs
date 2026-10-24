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
  `inCountry` (with a `PlaceIndex`), `sitelink`, with `all` / `any` /
  `not`, from JSON or built in Rust. `ids` also takes item numbers
  (`Filter::ItemNumbers`, or `{"ids": {"$items": k}}` with
  `Filter::from_json_with`), and known revisions (`Filter::Revisions`,
  `{"ids": …, "revisions": …}` or `{"ids": {"Q42": 2000000001}}`): an
  entity no newer is rejected from its line's tail.
- `Filter::compile_with(&Indexes { classes, places, redirects })`: with a
  `RedirectIndex`, `ids` follow merges (`merged_from`) and `misses` sorts
  the ids a pass did not find into merged, deleted and newer.
- Named filters (`{"any": {"venue": …}}`, `{"name": "venue", …}`):
  `matched_names` tells which of them an entity matches.
- Decided first from raw bytes (the type and id at the head of the line,
  memchr/Aho–Corasick needles for claims and sitelinks), three-valued;
  the rest after a probe parse of only the filter's properties and sites.
- `IdSet`: a Roaring-like id set, ~2 bytes an id, shared by every entity
  kind (`id_key`).
- `ClassIndex`: the P279 hierarchy as CSR arrays, closure queries both
  ways, saved as a small zstd-compressed file.
- `PlaceIndex`: each item's countries through P17, the P131 chain
  (cycle-safe, depth-limited) and P159, as shared country sets, saved as a
  small zstd-compressed file.
- `RedirectIndex`: item redirects (merges) as sorted arrays, double
  redirects followed.
- `Filter::fingerprint`: a stable hash, for checkpoints.
