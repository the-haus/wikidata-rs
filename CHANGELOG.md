# Changelog

## 0.3.0 — unreleased

What Festimap needs to use the dumps to their full potential (W-8 to W-16
of its requests), each with a regression test. Throughput of the existing
paths is unchanged (bench/README.md).

### Merges and deletions (W-8)

- `RedirectIndex`: the item redirects of Wikidata's SQL dumps
  (`wikidatawiki-<date>-redirect.sql.gz` joined with `…-page.sql.gz`,
  files or URLs, gzip/bzip2/zstd/plain), streamed and parsed on all cores;
  build, save, load, `resolve(id)` (double redirects followed). Also
  `readRedirects()` (`{ from, to }` pairs) and `wikidata redirects`.
- With `redirects: index` and `ids` in the filter, a pass follows merges:
  the items merged into come through, `EntityRecord.mergedFrom` naming the
  requested ids, and the missing ids are sorted into `stats.merged`
  (`{ from, to, inDump }`), `stats.deleted` and `stats.newer` (above the
  dump's highest id). `missingIds` is unchanged.

### A place index (W-9)

- `PlaceIndex`: each item's countries, from the truthy P17, P131 and P159
  of every item, with the P131 chain followed up (cycle-safe, at most 24
  steps; a partial result is never cached). Built in one pass (also with
  the class index in the same pass: `buildPlaceIndex` / `buildClassIndex`
  on any task), saved as a small zstd file; `countryOf(id)`,
  `countriesOf(items)`, `wikidata places`.
- The `inCountry` filter (`{ inCountry: "Q55" }` or a list): by the item's
  P17, its P131 chain's country or its P159's. Items the index knows are
  decided from their id, before parsing; others (newer than the index) from
  their own claims, through the index.
- `languages` / `sitelinks` rules (`{ by: "P17", map, always }`) fall back to
  the place index for an item whose claims choose nothing.

### Revision-aware reads (W-10)

- `ids` can carry known revisions: `{ ids, revisions }` (typed arrays or
  arrays, in the same order) or a map `{ Q42: 2000000001 }`. An entity whose
  `lastrevid` is no newer is skipped before parsing (read from the line's
  tail), still counts as found (not in `missingIds`), and is counted in
  `stats.unchanged`.

### Updates between weekly dumps (W-11)

- Incremental dumps (`wikidatawiki-<date>-pages-meta-hist-incr.xml.bz2`)
  read like any dump, through every filter: each entity page once, at its
  newest revision, with `lastrevid` and `modified` added; wikitext pages
  are skipped and the day's merges come as `stats.redirects`. Detected
  from the decompressed data; a truncated export is an error (or, with
  `allowTruncated`, ends at its last whole page).
- `liveChanges()`: the entities edited on Wikidata as they are edited
  (EventStreams `recentchange`, `wikidatawiki`, namespace 0 by default),
  filtered natively by ids, resumable from an event id (`lastEventId`) or a
  time (`since`); dropped connections resume without gaps or repeats.
  `wikidata live`.

### Resumable passes (W-12)

- `checkpointEvery` and `onCheckpoint`: a checkpoint (a JSON-safe object)
  after every N chunks, called once every record before it was consumed;
  `resumeFrom: checkpoint` continues with exactly the records after it,
  with the counters and the found ids behind `missingIds` carried over.
  bzip2 (from its block, the stream CRC carried on) and plain JSON, files
  and URLs; the checkpoint records the input and the filter's fingerprint
  and refuses another. `stream.checkpoint` is the last one reached. The
  CLI: `cat --checkpoint FILE --resume FILE`.

### Downloads (W-13)

- `downloadDump()` and `wikidata download`: the newest complete dump (the
  newest date whose `sha1sums.txt` lists the file; entity dumps have no
  `dumpstatus.json`) or a given date, in ranges from the base and every
  mirror at once, written into one preallocated `<name>.part` (no join),
  resumable from its `.part.state`, SHA-1 verified in file order as it
  downloads, and renamed to its final name only once verified.

### Mirrors (W-14)

- `mirrors` (and `connections`) on every input that is a URL: the file is
  read in ranges from all servers in parallel, in bounded memory, into the
  bzip2 decoder in order; every range is checked (`Content-Range`,
  `If-Range`), a dropped one resumed, and a failing mirror's ranges go to
  the others. Two servers capped at 3 MB/s each read a file in half the
  time of one. The CLI: `--mirror URL`, `--connections N`.

### Qualifiers by property (W-15)

- `qualifiers: ["P585", "P580", "P582", "P407"]` keeps only those
  qualifiers, skipped natively while parsing. `--qualifiers` in the CLI.

### Labels of referenced items (W-16)

- `referencedLabels: { languages, output? }`: the labels of the items the
  matching entities' kept claims (and qualifiers) refer to, and only those.
  Items after their first reference are taken during the pass; for the
  others (most: new items refer to old ones), only the chunks holding them
  are read again (bzip2 blocks and plain JSON; gzip and zstd are read again
  whole). `stats.referencedLabels` or a JSON file; `stats.labels` says how.
  The CLI: `cat --referenced-labels en,nl --labels-output FILE`.

### Node.js

- The ESM build now exports every native function (a test checks it).
- A new example, `refresh-known.ts`: known revisions, merges, checkpoints,
  on weekly and incremental dumps.

### Rust

- `wikidata-dump`: `ResumePoint`, `Block`, `BlockStart`,
  `ReadOptions::resume`, `Chunk::resume`, `Source::open_at`,
  `bz2::Scanner::resume`, `bz2::Blocks::{resumed, last_start}`; modules
  `sql`, `xml`, `sse`, `multi`, `download`; `HttpOptions::{mirrors,
  connections, range_size}`; `ReadStats::{xml, redirects}`.
- `wikidata-filter`: `PlaceIndex`, `RedirectIndex`, `Indexes` and
  `Filter::compile_with`; `Filter::{InCountry, Revisions, ItemRevisions}`,
  `Filter::{fingerprint, parse_json_with_revisions, needs_place_index}`;
  `CompiledFilter::{unchanged, merged_from, is_target, misses,
  fingerprint}`.
- `wikidata-export`: `Checkpoint`, `Event::Checkpoint`, `build_place_index`,
  `build_redirect_index`, `live_changes`, `labels`;
  `ScanOptions::{place_index, checkpoint_every, resume}`;
  `EntityOptions::referenced_labels`; `ByClaim::places`;
  `Record::merged_from`; `ScanStats::{unchanged, misses, redirects,
  place_index, labels, referenced_labels}`.
- `wikidata-model`: `ModelOptions::qualifier_properties`, `lastrevid`.
- Breaking: struct literals of `ScanOptions`, `EntityOptions`, `ByClaim`,
  `Record`, `ModelOptions`, `HttpOptions`, `ReadOptions` and `Chunk` need
  the new fields (or `..Default::default()`); matches on `Event` need
  `Event::Checkpoint`; `Splitter` takes `Block`s.

## 0.2.1 — unreleased

### Fixed

- Node.js on macOS: loaded in one process with `@pestle-rs/node` (in either
  order), the process crashed with a segfault (exit 139). Each bundled its
  own mimalloc, which on macOS keeps every thread's heap in the same fixed
  TLS slots, so each took the other's heaps. The binding now uses the
  system allocator on macOS (mimalloc stays on Linux and Windows, where
  each copy has its own thread-local storage). `test/coexist.test.ts`
  loads two copies of the addon in one process.

### Project

- The release workflow keeps its artifacts for a day (the organisation's
  artifact storage is shared; 0.2.0's release run stopped at a full quota)
  and checks every platform package's `repository` before publishing.
- README, RELEASING.md and the Node.js guide reorganised.

## 0.2.0 — 2026-10-20

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
