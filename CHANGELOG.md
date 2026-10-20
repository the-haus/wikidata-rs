# Changelog

## 0.2.0 — unreleased

What Festimap asked for, each with a regression test.

### Fixed

- bzip2: a block whose text is periodic (one pattern repeated end to end)
  was rejected as "the BWT is not one cycle". Its T-vector is several
  cycles; the text is origPtr's cycle repeated to the block's length, as
  libbzip2 decodes it. The 20261012 Wikidata dump has such a block at
  byte 547,678,139, so 0.1.0 could not read past it.

### Node.js

- `streamEntities`, `extract`, `fileInfo` and the new `ClassIndex.build`
  take `userAgent` and `retries` (`OpenOptions`), like `openDump`; the
  `User-Agent` goes out on every request, reconnects included, and
  `retries: 0` fails on the first dropped connection.
- `ids` takes a typed array of item numbers (`Uint32Array.of(42)` for
  `Q42`), passed to the native side without being written out as strings:
  a million ids cost ~0.1 MB of JS heap instead of ~36 MB. `Uint32Array`
  and `Int32Array` are not copied.
- Named filters: `{ any: { venue: …, festival: … } }`, or a `name` on any
  filter. `EntityRecord.matchedBy` lists every named filter the entity
  matches (`[]` without names), in the order the filter names them.
- `stats.missingIds`: with `ids` in the filter, the requested ids the dump
  does not hold (deleted or merged), after a complete pass only (not one
  cut short by `limit`, `close()`, a signal or truncation). The missing
  items also come as `stats.missingItemNumbers` (a `Uint32Array`), and
  `missingIds` is made from it when read, so many misses cost little heap.
- `languages` and `sitelinks` take a rule that chooses them per entity by
  its claims: `{ by: "P17", map: { Q55: ["nl"], Q183: ["de"] }, always: ["en", "mul"] }`.
  The chosen languages are also the entity's label fallback chain.
- `buildClassIndex: "classes.wdc"` on `entities()`, `extract()` or
  `fileInfo()` builds the class index during that pass and saves it when
  the pass completes (`stats.classIndex`), identical to `ClassIndex.build`,
  so a new dump costs one pass instead of two.
- An ESM build beside the CommonJS one (`exports` with `import` and
  `require` conditions, each with its own types), and
  `@wikidata-rs/node/package.json` is exported. The package bundles with
  esbuild (ESM and CommonJS output) and Vite without being marked
  external; the native addon is loaded at run time from the platform
  package, or from `WIKIDATA_RS_NODE_BINARY`.
- A new example, `weekly-refresh.mjs`: one pass with named branches,
  item-number ids, labels by country, missing ids and the class index.
- Invalid options now fail the stream when it starts, like a bad filter,
  instead of throwing from `entities()` / `streamEntities()`.

### CLI

- `--build-classes FILE` on `info`, `cat` and `extract` builds the class
  index during that pass (as `wikidata classes -o FILE` would).

### Rust

- `wikidata-filter`: `Filter::Named` and `CompiledFilter::{names,
  matched_names, check_line}`; `Filter::ItemNumbers`,
  `Filter::from_json_with` and `Filter::parse_json_with`; `parse_json`
  keeps the text's key order (named filters are reported in it);
  `CompiledFilter::{has_ids, requests, missing_ids}` replace `id_count`;
  `IdSet::iter` and `id_from_key`.
- `wikidata-export`: `Record::matched_by`; `EntityOptions::{languages_by,
  sitelinks_by}` (`ByClaim`); `ScanOptions::class_index` and
  `ScanStats::{missing_ids, class_index}`.
- `wikidata-model`: `EntityType::from_code`.
- Breaking: struct literals of `ScanOptions`, `EntityOptions` and `Record`
  need the new fields (or `..Default::default()`).

## 0.1.0 — 2026-10-20

First release.

`@wikidata-rs/node` 0.1.0 was released locally for six platforms
(macOS arm64/x64, Linux arm64/x64 on glibc 2.31+ and on musl); the
Windows packages follow from CI.

### Reading

- Wikidata JSON dumps from a file or an `http(s)://` URL: bzip2 (single
  or multi-stream), gzip, zstd, plain JSON and JSON lines. HTTP streams
  without a temporary file and resumes dropped connections with `Range`
  and `If-Range`; the User-Agent names the tool.
- bzip2 on every core: the single stream of Wikimedia's dumps is cut at
  its bit-aligned block boundaries, blocks are decoded in parallel and
  come back in file order, stream CRCs are checked, and false block-magic
  matches are undone.
- A native bzip2 block decoder, ~1.9× faster than libbzip2 on one core and
  on ten (the inverse BWT followed as eight chains at once, a run-aware
  T-vector build, table-driven Huffman decoding, hardware CRC-32).
- A truncated input is an error; `allowTruncated` / `--allow-truncated`
  reads a partial download up to its last whole entity and says so.

### Filtering

- `ids`, `type`, `hasProperty`, `claim` (`P17=Q55`), `instanceOf` (with
  subclasses), `sitelink`, combined with `all` / `any` / `not`, as JSON or
  CLI flags. Lines are decided from their first bytes and SIMD substring
  search where possible, then by a probe parse of only what the filter
  reads.
- The class index: the truthy P279 hierarchy in one pass, saved as a
  small file, for "instance of a subclass of …" without SPARQL.

### Output

- Typed, normalised entities: languages with a fallback chain, sitelinks
  with URLs, claims as typed values (times at their precision, quantities,
  coordinates, Commons files, …), ranks (deprecated dropped, `bestRank`),
  qualifiers, references (optional), lexemes and properties.
- Derived fields: `instanceOf`, `subclassOf`, `country`, `coordinates`,
  `inception`, `dissolved`, `image`, `website`, `externalIds` (one table
  of 38 identifiers, with URL formats).
- Extracts: the matching entities' original lines as `.jsonl.zst`
  (recommended), `.json.bz2`, `.json.gz`, … compressed on every core,
  readable by this package and by common tools.
- File statistics: entities by type, highest ids, largest entity, sizes,
  bzip2 blocks and streams.

### Interfaces

- The `wikidata` CLI: `info`, `cat`, `extract`, `classes`.
- `@wikidata-rs/node`: `openDump`, `entities()` (async iteration with
  credit-based backpressure, summary fields without `JSON.parse`, the full
  entity on demand), `extract()`, `fileInfo()`, `classIndex()`,
  `ClassIndex.load()`, progress events with rates, `AbortSignal`s.
- Rust crates: `wikidata-dump`, `wikidata-model`, `wikidata-filter`,
  `wikidata-export`.
