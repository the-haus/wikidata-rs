# wikidata-dump

Read Wikidata JSON dumps fast: files or URLs, bzip2 on every core, in
file order, with bounded memory.

```rust,no_run
use wikidata_dump::{Dump, ReadOptions};

let dump = Dump::open("https://dumps.wikimedia.org/wikidatawiki/entities/latest-all.json.bz2")?;
let mut entities = 0u64;
for chunk in dump.chunks(&ReadOptions::default())? {
    for line in chunk?.lines() {   // one entity's JSON, without the trailing comma
        entities += 1;
    }
}
# Ok::<(), wikidata_dump::DumpError>(())
```

- bzip2, gzip, zstd (single or multi-stream/member/frame), plain JSON and
  JSON lines, detected from the first bytes.
- bzip2 decoded in parallel: `bz2::Scanner` finds the bit-aligned block
  boundaries of a single stream, `bzdecode` decodes blocks about twice as
  fast as libbzip2 (eight inverse-BWT chains at once, a run-aware table
  build, table-driven Huffman, hardware CRC-32), `bz2::Blocks` checks the
  stream CRCs and undoes false magic matches. ~99 MB/s of bzip2 (~1 GB/s
  of JSON) on 10 Apple M1 Max cores.
- HTTP(S) streamed with ureq/rustls and resumed with `Range` + `If-Range`;
  with `HttpOptions::mirrors`, read in ranges from several servers at once
  (`multi::MultiReader`), in order and bounded memory.
- `Chunk`s hold whole lines; only the line across two blocks is copied.
  Each carries a `ResumePoint` (bzip2: its block's bit offset and the
  stream CRC so far; plain JSON: a byte), from which
  `ReadOptions::resume` reads on.
- MediaWiki XML exports (the daily incremental dumps) are detected and
  read as entity lines (`xml::XmlEntities`): each page's newest revision,
  with `lastrevid` and `modified`; redirects are reported.
- `sql`: MediaWiki's SQL dumps (`page`, `redirect`, …), their rows
  parsed statement by statement. `sse`: server-sent events (EventStreams)
  resumed with `Last-Event-ID`. `download`: the newest complete dump,
  segmented across mirrors into one preallocated file, resumable, SHA-1
  verified.
- A truncated input is an error (`DumpError::Truncated`), unless
  `allow_truncated` is set.
- `parallel::ordered_map`: order-preserving parallel map with a bounded
  window, cancelled and joined on drop.
- `DumpWriter`: bzip2/gzip/zstd output compressed on every core as
  independent streams, members or frames.
