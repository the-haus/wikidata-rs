# Changelog

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
