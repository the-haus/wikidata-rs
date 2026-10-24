# Architecture

## The pipeline

Every task (entities, extract, class index, file statistics) is one pass
over a dump, in stages that each run in parallel or pipelined, with file
order restored only where it matters:

```text
 read ─▶ scan ─▶ decode blocks ─▶ check CRCs ─▶ cut lines ─▶ filter, parse, ─▶ sink
 (file   (bit     (N workers,      join false     (stitch the   normalise,        (CLI
  or      magics)  any order)       magics,        line across   serialise         writer,
  HTTP)                             in order)      blocks)       (N workers)       Node batches)
 └──── feeder thread ────┘ └─ pool A ─┘ └────── feeder of pool B ─────┘ └─ pool B ─┘ └ caller ┘
```

Both pools are `parallel::ordered_map`s (as in tilemaker-rs): a feeder
thread produces items, N workers transform them, and results come back in
input order. A permit channel caps the items in flight or waiting at 2N,
so a slow consumer slows the reader rather than buffering the file; a
panic in a worker is re-raised on the consumer. Dropping a map cancels
and joins its threads (a chain of maps unwinds stage by stage), so a
stopped or aborted task leaves no thread behind (`live_threads()` counts
them; the tests check it).

## Reading (`wikidata-dump`)

`Dump::open` takes a path or a URL and looks at the first bytes for the
compression (`BZh`, gzip, zstd, or plain JSON).

**HTTP** inputs stream through ureq (rustls) with the User-Agent
`wikidata-rs/<version> (https://github.com/the-haus/wikidata-rs)`. When
the connection drops, the reader reconnects with `Range: bytes=N-` and
`If-Range` (the first response's `ETag` or `Last-Modified`), and checks
the `Content-Range`, so a resumed download can never splice two versions
of a file. Reconnects back off and give up after `retries` attempts
without progress. The bytes read are counted for progress.

**bzip2.** Wikimedia's dump is a single bzip2 stream (one `BZh9` header
at byte 0, checked on the 2026-10-12 dump) of ~900 kB blocks. Blocks are
bit-aligned and their lengths are recorded nowhere, but each block is
self-contained (its own Huffman tables and CRC), so:

1. `bz2::Scanner` reads the input in 4 MB pieces and finds every block
   magic (`0x314159265359`) and end-of-stream magic (`0x177245385090`)
   at any bit offset. A 256-entry table on the second byte of each 8-byte
   window says which of the 16 shift/magic combinations can match there,
   so most bytes cost one table load (~850 MB/s on one core, ~9× what
   decoding needs). The bits between two magics are copied to byte
   alignment as a `Piece`.
2. Workers decode block pieces (`bzdecode`, below) in any order.
3. `bz2::Blocks` takes the results in order, checks each stream's
   combined CRC (`crc = rotl(crc, 1) ^ block_crc`) against its
   end-of-stream marker, and yields the bytes.

A magic can also occur by chance inside compressed data (once in 2^48 bit
positions; ~0.003 expected matches in the whole dump). Such a false start
cuts one block in two pieces that both fail to decode; `Blocks` then joins
a failed piece with the following ones and decodes again. Multi-stream
files (`pbzip2`, and this package's extracts) work the same way: the
pieces between an end-of-stream marker and the next block carry the next
stream's header and are skipped.

**The block decoder** (`bzdecode`) decodes a block straight from its bits
and is about twice as fast as libbzip2 (one core, the dump's head: 148 vs
76 MB/s of output; ten cores: 99 vs 47 MB/s of input). Where the time
went (one core, the dump's head: a straightforward port of libbzip2's
algorithm first, 7.1 s, then this decoder, 3.5 s), and what changed:

| Phase | Time | Here |
|---|---|---|
| Inverse BWT walk | 3.49 → 1.25 s | Following the T-vector is a chain of dependent loads that miss the cache (3.6 MB for a block). Eight chains are followed at once: from the text's start and from seven evenly spaced indices, whose text positions are unknown. Start indices are marked in a spare bit of the table; each chain stops where another began, and the segments are joined in cycle order. The table and footprint stay libbzip2's; eight loads are in flight instead of one (~3× faster). An earlier version that walked from both ends with a second (LF) table was faster on one core and slower on four: the doubled footprint no longer fit in the shared L2. |
| Building the T-vector | 1.04 → 0.61 s | Runs of a byte (common in the BWT's last column) are placed together, so the per-byte bucket counter is not read and written back for every byte, and the table is written with plain stores (`T[j] = i << 8 \| F[j]`, using `F[j] = L[i]`). ~1.7× faster. |
| Huffman and move-to-front | 1.58 → 1.49 s | A 10-bit lookup table per Huffman group, read from a 64-bit buffer kept in a register (refilled eight bytes at a time); small move-to-front indices shift one 8-byte word. Bounded by unpredictable branches per symbol. |
| CRC and run-length decoding | 0.60 → 0.13 s | The CRC uses the CPU's CRC-32 instructions (CRC-32/BZIP2 is zlib's CRC-32 with every bit order reversed); runs of four are found eight positions at a time. ~4× faster. |

Every block's CRC is checked, so a decoding error cannot pass silently.
Randomised blocks (written only by bzip2 before 0.9.5, 1999) go to
libbzip2 (the `bzip2` crate's Rust port); `WIKIDATA_BZ2_DECODER=libbzip2`
uses it for everything, for comparisons.

**gzip** (flate2 with zlib-rs) and **zstd** decode on the feeder thread:
neither format can be split without an index, so decompression is
sequential there, pipelined with the parallel work on the lines (~1 and
~2.7 GB/s of JSON, enough to keep the parsers busy). Plain JSON is read in
4 MB chunks.

**Lines.** `lines::Splitter` turns decompressed blocks into `Chunk`s of
whole lines: the line that straddles two blocks is stitched into the next
chunk's `prefix` (only that line is copied, never a block), and a block
without a line end (an entity larger than a block) is carried over whole.
It also checks the framing: a dump that starts with `[` must end with `]`
(else it was truncated), and a JSON-lines file is read line by line.
Chunks then go to the workers of the second pool.

## Filtering (`wikidata-filter`)

A filter is evaluated in two steps:

1. **Before parsing**, on the raw line. `wikidata_model::peek` reads the
   type and id from the line's head (`{"type":"item","id":"Q31",…`;
   other key orders fall back to a scan of the top level), which decides
   `ids` (an `IdSet`: tilemaker-rs's Roaring-like set, ~2 bytes an id) and
   `type` exactly. The other filters look for byte strings that must
   occur when they match — `"P17":[` for a claim on P17, `"id":"Q55"` for
   its value, `"site":"nlwiki"` for a sitelink — with memchr's SIMD
   substring search, or one Aho–Corasick automaton for many values (the
   thousands of classes of an `instanceOf` with subclasses). The answer
   is yes, no or maybe (three-valued logic through `all`/`any`/`not`).
   On the dump's head, `P17=Q55` decides 99.3% of the lines here.
2. **After a probe parse**, for the maybes: only the filter's properties
   and sites are read, everything else is skipped. When the output
   options read at least as much, the probe and the output share one
   parse.

**Named filters** (`{"any": {"venue": …}}`, `{"name": "venue", …}`) are
nodes that pass their child's answer through. For an entity the whole
filter matched, every named node is evaluated again, from its raw-bytes
answer or, for a maybe, on the parse the match already made (or one probe
parse), so `matchedBy` lists every named branch, not the first that
decided `any`.

**Missing ids.** Every `IdSet` of the filter is also checked against each
line's id on the workers, before the filter runs, and the keys it holds
are collected. After a complete pass (not stopped, not truncated) the
requested ids that were never seen are reported as `missing_ids`.

**The class index** is built in one pass that parses only the lines
containing `"P279":[`, and only their P279 claims: the truthy subclass-of
edges (what SPARQL's `wdt:P279*` follows) as CSR arrays (~12 bytes an
edge), saved as a zstd-compressed file. `instanceOf` with subclasses
expands its classes through the reversed edges (cycles are fine) into a
set of item numbers, then checks P31. Any other task builds the same index
as a by-product (`ScanOptions::class_index`): the workers read the P279
edges of every line before filtering it, and the index is saved, through a
temporary file, when the pass ends without being stopped.

## Parsing (`wikidata-model`)

The parser is a set of serde `DeserializeSeed`s driven by the compiled
options. Map keys are compared as borrowed strings, and whatever is not
wanted (other languages, properties, sites, references) is skipped as it
is read, never allocated. `datavalue.value` comes before its `type` (and
the snak's `datatype` after both), so a value is first read into a small
untyped form and typed once the snak is complete.

The seeds run on serde_json or sonic-rs. Measured on the head of the dump
(one thread): sonic-rs parses whole entities ~20% faster (537 vs 449 MB/s),
serde_json skips ~15% faster when a few languages and properties are kept
(757 vs 651 MB/s). The default picks by the options. serde_json is built
with `float_roundtrip`: its default float parsing is off by one ulp on
some coordinates (both parsers then agree on every entity of the head).

Derived fields are computed from the truthy statements of their
properties whatever `properties` keeps (those claims are parsed without
qualifiers then). Serialising to JSON happens on the workers too.

The CLI and the Node binding use mimalloc as the global allocator: 12–20%
faster on parse-heavy work than the system allocator. The Node binding
uses the system allocator on macOS: there mimalloc keeps each thread's heap
in fixed TLS slots of the process, so two addons that bundle it (this one
and @pestle-rs/node) crash when loaded together. A library loaded into
someone else's process must not assume it is the only one.

## Writing (`wikidata-dump::write`)

`DumpWriter` cuts its input into chunks (bzip2: 899 kB, one block; gzip
4 MB; zstd 8 MB) and compresses each one on a worker as an independent
bzip2 stream, gzip member or zstd frame, written in order. Concatenated
streams, members and frames are valid files for every common tool.

Extracts copy the matching entities' original lines (no normalisation),
so they can be read back with any options. The recommended form is
`.jsonl.zst`: on the dump's head (all 10,989 entities) it is as small as
bzip2 at level 9 (52 MB), written 4.5× faster (1.2 vs 5.5 s) and read
back 3× faster (0.19 vs 0.62 s, with this package's parallel bzip2).

## Node.js binding

Each task runs on its own thread and sends events through a napi
threadsafe function. Entities are nested and varied, so they cross as
JSON text: all of a batch's JSON in one string with UTF-16 offsets,
parsed in JS only for the entities whose `entity` is read. The fields
every consumer reads (id, type, label, description, instance of, subclass
of, country, coordinates, years, image, website, external ids) cross as
typed arrays plus one string of short strings, so they are usable without
any `JSON.parse`; a record is two fields (batch and index) with getters
on its prototype. A batch costs about twenty napi calls whatever its size.

Measured (400,000 entities of ~1.5 kB, 10 threads): the summary fields
reach JS at 1.8 M entities/s (the CLI writes 1.6 M/s); the full entity is
bounded by `JSON.parse` on the JS thread (~250 k/s), so select the
languages and properties you need.

Flow control is credit-based, as in tilemaker-node: the producer may send
a batch only with a credit, and JS grants one per batch it takes, so at
most `readAhead` batches are queued. The producer waits on its own
thread, never on the event loop or the libuv pool. The threadsafe
function keeps the event loop alive only while JS awaits a batch (or the
end of a closed stream); streams dropped unfinished are closed when
collected. `AbortSignal`s close the stream; the native side then stops
at its next chunk and joins its threads.

Class indexes held by JS are referred to from task options by a registry
id (napi objects cannot carry references to native classes).
