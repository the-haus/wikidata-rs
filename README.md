<div align="center">

<a href="https://github.com/the-haus/wikidata-rs">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/wikidata-rs-mark-dark.svg">
    <img alt="wikidata-rs" src="docs/assets/wikidata-rs-mark.svg" width="96" height="96">
  </picture>
</a>

# wikidata-rs

**Streaming Wikidata dumps, fast, in Rust.**

Read the 103 GB Wikidata JSON dump from a file or straight from a URL, filter it before parsing, and get typed, normalised entities or compact subsets, on every core, in bounded memory.

<p>
  <a href="https://github.com/the-haus/wikidata-rs/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/the-haus/wikidata-rs/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://github.com/the-haus/wikidata-rs/actions/workflows/node.yml"><img alt="Node" src="https://github.com/the-haus/wikidata-rs/actions/workflows/node.yml/badge.svg"></a>
  <a href="https://github.com/the-haus/wikidata-rs/releases"><img alt="Release" src="https://img.shields.io/github/v/release/the-haus/wikidata-rs?include_prereleases&label=release&color=24456b"></a>
  <a href="https://www.npmjs.com/package/@wikidata-rs/node"><img alt="npm" src="https://img.shields.io/npm/v/@wikidata-rs/node?label=npm&logo=npm"></a>
  <a href="#development"><img alt="Rust 1.87+" src="https://img.shields.io/badge/rust-1.87%2B-orange?logo=rust"></a>
  <a href="LICENCE.txt"><img alt="License: FTWPL" src="https://img.shields.io/badge/license-FTWPL-lightgrey"></a>
</p>

<sub>CLI · Rust crates · Node.js &nbsp;·&nbsp; parallel bzip2 · gzip · zstd &nbsp;·&nbsp; files or resumable HTTP &nbsp;·&nbsp; filters before parsing &nbsp;·&nbsp; bounded memory</sub>

</div>

---

## Overview

wikidata-rs reads Wikidata's JSON dumps (`wikidata-*-all.json.bz2`, 103 GB
compressed) from a file or straight from a URL and turns them into
well-organised data:

- bzip2 decoded on every core, with a block decoder twice as fast as
  libbzip2;
- filters that decide most entities from their raw bytes, before any
  parsing: ids, types, properties, claim values, instance of a class or
  any of its subclasses, sitelinks;
- typed, normalised entities: labels in your languages with a fallback
  chain, claims as typed values (times at their precision, quantities with
  units, coordinates, …), ranks, qualifiers, references, and derived fields
  such as `instanceOf`, `country`, `coordinates`, `inception` and
  `externalIds`;
- compact subsets (`.jsonl.zst`) that read back like the dump itself;
- refreshes that touch only what changed: known revisions skip unchanged
  items before parsing, merges and deletions come from the SQL dumps, a
  stopped pass resumes from a checkpoint, and the labels of referenced
  items come with the pass;
- a place index that finds every item in a country through its P131 chain;
- the daily incremental dumps and live changes, between weekly dumps;
- several mirrors read at once, and verified, resumable downloads;
- bounded memory, and a truncated input is always an error, never a silent
  end.

Reach for it when you're building:

- **Knowledge-graph imports**: the entities of one country, one class
  (with its subclasses) or one set of ids, out of the full dump, in minutes
- **Weekly refreshes**: a small `.jsonl.zst` subset kept instead of the
  dump, refreshed from the latest dump by URL without a temporary file
- **Entity linking**: labels and aliases in your languages, sitelinks, and
  external identifiers (MusicBrainz, Discogs, OpenStreetMap, GeoNames, …)
  with their URLs
- **Pipelines in Node.js**: the same engine behind an async iterator, with
  backpressure, on a worker pool that leaves the JS thread free

Everything is available in three ways: the `wikidata` command line tool,
a set of Rust crates, and a Node.js package
([`@wikidata-rs/node`](crates/bindings/wikidata-node)). It is a sibling of
[tilemaker-rs](https://github.com/the-haus/tilemaker-rs) and
[pestle-rs](https://github.com/the-haus/pestle-rs), and follows the same
design.

## Contents

- [Features](#features)
- [Installation](#installation)
- [Quick start](#quick-start)
- [The output](#the-output)
- [Verified](#verified)
- [Performance](#performance)
- [Project layout](#project-layout)
- [Development](#development)
- [Limitations](#limitations)
- [Documentation](#documentation)
- [CI/CD](#cicd)
- [Contributing](#contributing)
- [License](#license)

## Features

| | |
|---|---|
| **Fast reading** | bzip2 is cut at its bit-aligned block boundaries and the blocks are decoded on every core by a decoder that is 1.9× faster than libbzip2, in file order. gzip and zstd decode on one thread, pipelined with everything else; plain JSON and JSON lines are read as they are. |
| **Files or URLs** | A path, or an `http(s)://` URL that is streamed (no temporary file) and resumed with `Range` requests when the connection drops. |
| **Filters before parsing** | `ids` (a compact id set; from Node, a typed array of item numbers; with known revisions, unchanged entities are skipped), `type`, `hasProperty`, `claim` (`P17=Q55`), `instanceOf` (with subclasses, through a class index built in one pass, or as a by-product of any other pass), `inCountry` (P17, the P131 chain or P159, through a place index), `sitelink`, combined with `all` / `any` / `not`. Named branches report which of them each entity matched, and an `ids` filter reports the ids the dump lacks: merged (with a redirect index from the SQL dumps), deleted or newer. Lines are rejected from their first bytes and SIMD substring search; only possible matches are parsed. |
| **Organised output** | A stable JSON shape with typed values, language fallback, sitelink URLs, rank handling and derived fields, computed in Rust. |
| **Subsets** | `extract` writes the matching entities' original lines as `.jsonl.zst` (recommended), `.json.bz2` (Wikimedia's own layout), `.json.gz`, … compressed on every core, readable by this package and by every common tool. |
| **Resumable** | Checkpoints every N bzip2 blocks; a pass resumes from one with exactly the records after it (counters and missing ids carried over). |
| **Between weekly dumps** | The daily incremental XML dumps read like a dump (each entity at its newest revision, merges reported); live changes from EventStreams, resumable from an event id. |
| **Referenced labels** | The labels of the items the matches refer to (genres, places, operators), collected during the pass, and afterwards from only the blocks that hold the rest. |
| **Mirrors and downloads** | A URL is read in ranges from several mirrors at once (each caps bandwidth per client); `download` keeps a local copy: newest complete dump, segmented across mirrors into one preallocated file, resumable, SHA-1 verified, renamed into place when done. |
| **Streaming by design** | Every stage runs in parallel or pipelined; a slow consumer slows the reader instead of filling memory (credit-based flow control into Node.js). |

## Installation

### Command line

Prebuilt binaries for Linux (x64, arm64), macOS (Apple Silicon, Intel) and
Windows (x64) are attached to every
[GitHub release](https://github.com/the-haus/wikidata-rs/releases) from v0.2.1 on, with
SHA-256 checksums:

```sh
VERSION=0.3.0
TARGET=aarch64-apple-darwin   # or x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu,
                              #    x86_64-apple-darwin, x86_64-pc-windows-msvc (.zip)
curl -fsSL "https://github.com/the-haus/wikidata-rs/releases/download/v$VERSION/wikidata-$VERSION-$TARGET.tar.gz" | tar xz
./wikidata-$VERSION-$TARGET/wikidata --version
```

To build the CLI with Cargo instead:

```sh
cargo install --git https://github.com/the-haus/wikidata-rs --tag v0.3.0 wikidata-cli
```

### Node.js

```sh
npm install @wikidata-rs/node
```

The package needs Node.js 18 or newer and has prebuilt binaries for Linux
(x64 and arm64, glibc and musl), macOS (x64 and arm64) and Windows (x64
and arm64). See the [package README](crates/bindings/wikidata-node) for
details.

### Rust

The crates are not on crates.io yet; depend on them from Git:

```toml
[dependencies]
# filters, entities, extracts, class index, file statistics
wikidata-export = { git = "https://github.com/the-haus/wikidata-rs", tag = "v0.3.0" }
# just the dump reader (parallel bzip2, HTTP, line chunks)
wikidata-dump = { git = "https://github.com/the-haus/wikidata-rs", tag = "v0.3.0" }
# just the entity model and parser
wikidata-model = { git = "https://github.com/the-haus/wikidata-rs", tag = "v0.3.0" }
```

## Quick start

### Command line

```sh
DUMP=https://dumps.wikimedia.org/wikidatawiki/entities/latest-all.json.bz2   # or a local file

# What a dump holds: entities by type, sizes, speed
wikidata info wikidata-20261012-all.json.bz2

# Normalised entities as JSON lines: Dutch places, labels in nl → en → mul
wikidata cat $DUMP --claim P17=Q55 --has P625 --languages nl,en,mul --properties P31,P17,P625 > nl-places.jsonl

# A small subset to keep instead of the 103 GB dump, read back like the dump
wikidata extract $DUMP --filter '{"any":[{"claim":"P17=Q55"},{"hasProperty":"P434"}]}' -o subset.jsonl.zst
wikidata cat subset.jsonl.zst --id Q42

# Instances of music festival or any of its subclasses (needs the class index)
wikidata classes $DUMP -o classes.wdc                                  # one pass; ~20 MB file
wikidata extract $DUMP --has P434 -o artists.jsonl.zst --build-classes classes.wdc  # or during another pass
wikidata cat subset.jsonl.zst --instance-of Q868557 --subclasses --classes classes.wdc

# Everything in the Netherlands, also what names only a village in P131
wikidata places $DUMP -o places.wdp --build-classes classes.wdc        # both indexes, one pass
wikidata cat $DUMP --in-country Q55 --places places.wdp > nl.jsonl

# Merges and deletions: the redirects of the SQL dumps of the same date
SQL=https://dumps.wikimedia.org/wikidatawiki/20261020/wikidatawiki-20261020
wikidata redirects --redirect-sql $SQL-redirect.sql.gz --page-sql $SQL-page.sql.gz -o redirects.wdr
wikidata cat $DUMP --revisions-file known.txt --redirects redirects.wdr > changed.jsonl  # stats: merged, deleted, newer

# A resumable pass, and the labels of the items the matches refer to
wikidata cat $DUMP --claim P17=Q55 --checkpoint pass.ckpt --referenced-labels en,nl --labels-output labels.json > nl.jsonl
wikidata cat $DUMP --claim P17=Q55 --resume pass.ckpt >> nl.jsonl        # after a stop: the rest

# Between weekly dumps: a day's incremental dump, and live changes
wikidata cat https://dumps.wikimedia.org/other/incr/wikidatawiki/20261022/wikidatawiki-20261022-pages-meta-hist-incr.xml.bz2 --revisions-file known.txt
wikidata live --id Q42,Q55 --last-event-id "$(tail -1 changes.jsonl | jq -r .eventId)" >> changes.jsonl

# A local copy: newest complete dump, from two servers, verified, resumable
wikidata download --dir /data/dumps --mirror https://dumps.wikimedia.your.org/wikidatawiki/entities/
```

Run `wikidata --help`, or `wikidata <command> --help`, for every option.

### Node.js

```ts
import { openDump, RedirectIndex } from "@wikidata-rs/node";

const dump = await openDump("https://dumps.wikimedia.org/wikidatawiki/entities/latest-all.json.bz2");
for await (const e of dump.entities({ filter: { claim: "P17=Q55" }, languages: ["nl", "en", "mul"] })) {
  e.label;        // { text: "Pinkpop", language: "nl" } — no JSON parsed for these
  e.coordinates;  // { lat: 50.9, lon: 6.03 }
  e.externalIds;  // { musicbrainzArtist: "…", osmNode: "…" }
  e.entity;       // the full, typed entity, parsed on first access
}

await dump.extract({ output: "nl.jsonl.zst", filter: { claim: "P17=Q55" } });

// A weekly refresh: only what changed, merges followed, resumable.
const redirects = await RedirectIndex.load("redirects.wdr");
const stream = dump.entities({
  filter: { ids: knownItems, revisions: knownRevisions },
  redirects,
  checkpointEvery: 1000,
  onCheckpoint: (cp) => save(cp),
  referencedLabels: { languages: ["en", "nl"] },
});
for await (const e of stream) update(e, e.mergedFrom);
const { unchanged, merged, deleted, newer, referencedLabels } = await stream.stats;
```

The [Node.js guide](crates/bindings/wikidata-node) covers every option, and
[`examples/`](crates/bindings/wikidata-node/examples) holds runnable TypeScript
(`npx tsx places.ts DUMP Q55`): festivals by class, weekly subsets and
refreshes, GeoJSON places, external ids, file statistics.

### Rust

```rust,no_run
use wikidata_export::{entities, Dump, EntityOptions, Event, Filter, Flow};

let dump = Dump::open("wikidata-20261012-all.json.bz2")?;
let mut opts = EntityOptions::default();
opts.scan.filter = Some(Filter::parse_claim("P17=Q55")?.compile(None)?.into());
opts.model.languages = Some(vec!["nl".into(), "en".into(), "mul".into()]);
entities(&dump, &opts, &mut |e| {
    if let Event::Records(records) = e {
        for r in records {
            println!("{} {:?} {:?}", r.entity.id, r.entity.label, r.entity.derived.coordinates);
        }
    }
    Flow::Continue
})?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## The output

One entity, as `wikidata cat` and `entity.json` write it (shortened):

```json
{
  "id": "Q31", "type": "item",
  "label": { "text": "België", "language": "nl" },
  "description": { "text": "land in West-Europa", "language": "nl" },
  "instanceOf": ["Q6256"], "subclassOf": [], "country": ["Q31"],
  "coordinates": { "lat": 50.641111, "lon": 4.668056 },
  "inception": 1830, "dissolved": null,
  "image": "Flag of Belgium (civil).svg", "website": "https://www.belgium.be/",
  "externalIds": { "osmRelation": "52411", "geonames": "2802361" },
  "labels": { "nl": "België", "en": "Belgium", "mul": "Belgium" },
  "descriptions": { "nl": "land in West-Europa", "en": "country in western Europe" },
  "aliases": { "nl": ["Koninkrijk België"], "en": ["Kingdom of Belgium", "BE"] },
  "sitelinks": { "nlwiki": { "title": "België", "url": "https://nl.wikipedia.org/wiki/Belgi%C3%AB", "badges": [] } },
  "claims": {
    "P31": [{ "value": { "type": "entity", "id": "Q6256" }, "rank": "preferred" }],
    "P571": [{ "value": { "type": "time", "time": "+1830-10-04T00:00:00Z", "iso": "1830-10-04", "precision": 11,
                          "precisionName": "day", "year": 1830, "month": 10, "day": 4, "calendar": "gregorian" },
               "rank": "normal" }],
    "P1082": [{ "value": { "type": "quantity", "amount": 11697557, "unit": null }, "rank": "preferred",
                "qualifiers": { "P585": [{ "type": "time", "iso": "2023-01-01", "…": "…" }] } }],
    "P625": [{ "value": { "type": "coordinate", "lat": 50.641111, "lon": 4.668056, "precision": 0.0000277778, "globe": "Q2" },
               "rank": "normal" }]
  },
  "lastrevid": 2000000001, "modified": "2026-10-01T12:00:00Z"
}
```

- Top-level keys are always present (`null`, `[]` or `{}` when empty).
- Every value carries its `type`: `entity`, `string`, `externalId`, `url`,
  `media` (file and Commons page URL), `monolingual`, `time`, `quantity`,
  `coordinate`, `math`, `musicalNotation`, `geoShape`, `tabularData`,
  `someValue`, `noValue`.
- Deprecated statements are dropped by default; `bestRank` keeps only the
  preferred statements of a property when there are any ("truthy").
- Derived fields come from the truthy statements and are computed whatever
  `properties` selects. The external identifiers collected into
  `externalIds` (MusicBrainz, Discogs, Spotify, OpenStreetMap, GeoNames,
  ISNI, VIAF, GND, …) are one table in
  [`external.rs`](crates/wikidata-model/src/external.rs), with their URL
  formats.

## Verified

- **bzip2**: the native block decoder agrees byte for byte with libbzip2
  on all 659 blocks of the first 50 MB of the 2026-10-12 dump, and on
  synthetic data built to stress it (all 256 byte values, long and short
  runs around the run-length thresholds, every compression level); every
  block's CRC is checked, and a randomised fuzz test flips bits without
  ever causing a panic. Multi-stream files (`pbzip2`, this package's own
  extracts) and false block-magic matches are handled.
- **Parsers**: serde_json and sonic-rs produce identical entities for every
  entity of the dump's head (and serde_json is configured for exact float
  parsing: its default is off by one ulp on some coordinates).
- **Filters**: every filter is checked before and after parsing on a
  sample dump that covers every datatype, ranks, qualifiers, references,
  `somevalue`/`novalue`, properties, lexemes and a class hierarchy.
- **Robustness**: truncated bzip2, gzip, zstd and plain inputs fail (or,
  with `--allow-truncated`, end at the last whole entity, flagged);
  corrupt blocks, garbage and dropped HTTP connections are tested; a
  stopped or aborted stream leaves no thread running.

## Performance

Measured on an Apple M1 Max (10 cores, 32 GiB, macOS 26) while other jobs
were using the machine (load average 3–7), so these are lower bounds.
The input is the first 50 MB of `wikidata-20261012-all.json.bz2` (520 MB
of JSON, 10,989 entities, 47 kB on average: the dump starts with large,
well-known items).
Wall time and peak memory from `/usr/bin/time`, best of three.

| Task | wikidata-rs | Baseline |
|---|---|---|
| Decompress and count entities | **0.53 s** (98.9 MB/s in, 980 MB/s of JSON) · 201 MB | `bzip2 -dc \| node`: 6.59 s · 157 MB |
| The same, with libbzip2 instead of the native decoder | 1.11 s (47.2 MB/s in) | |
| Entities in the Netherlands (`P17=Q55`), CLI | **0.54 s** · 221 MB | `bzip2 -dc \| node` (substring + `JSON.parse`): 6.55 s · 155 MB |
| Entities in the Netherlands, Node.js | **0.59 s** · 272 MB | |
| Parse and normalise 400,000 small entities (448 MB of JSON), CLI | **0.25 s** (1.6 M entities/s) | Node `JSON.parse` per line: 1.79 s |
| The same through Node.js, summary fields | **0.22 s** (1.8 M entities/s) | |

On one core the native bzip2 decoder runs at 148 MB/s of output (libbzip2:
76 MB/s; `bzip2 -dc`: 70 MB/s), and it scales to 10 cores with the same
ratio. At that rate a full pass over the 103 GB dump takes about 18
minutes on this machine (projected from the head; a full run was not
possible while the dump was still downloading). lbzip2 and pbzip2 were
not installed, so they are not compared. Every table, and how to
reproduce them, is in [bench/](bench/README.md).

## Project layout

A Cargo workspace of small, focused crates, with the language bindings
under `crates/bindings`:

| Crate | Purpose |
|---|---|
| [`wikidata-dump`](crates/wikidata-dump) | Reading dumps: files and resumable HTTP, the bzip2 block scanner and native block decoder, gzip/zstd/plain, line chunks in file order (`parallel::ordered_map`), and the parallel compressed writer. |
| [`wikidata-model`](crates/wikidata-model) | The normalised entity and its parser (serde seeds on serde_json or sonic-rs), typed values, time precision, sitelink URLs, derived fields and the external id table. |
| [`wikidata-filter`](crates/wikidata-filter) | Filter expressions, their evaluation before and after parsing, compact id sets and the class index. |
| [`wikidata-export`](crates/wikidata-export) | The engine: filtered entities on all cores, extracts, the class index pass and file statistics. |
| [`wikidata-cli`](crates/wikidata-cli) | The `wikidata` binary. |
| [`bindings/wikidata-node`](crates/bindings/wikidata-node) | The N-API binding, published as `@wikidata-rs/node`. |

## Development

**Prerequisites**

- Rust 1.87 or newer through `rustup` (stable is pinned in
  [`rust-toolchain.toml`](rust-toolchain.toml), with `rustfmt` and `clippy`)
- A C compiler (zstd and mimalloc are built from vendored sources)
- Node.js 20+ with Corepack for the Node binding (the published package
  runs on Node.js 18+)

**Clone and bootstrap**

```sh
git clone https://github.com/the-haus/wikidata-rs.git
cd wikidata-rs
cargo build --release -p wikidata-cli          # target/release/wikidata
cargo test --workspace --exclude wikidata-node # Rust tests

corepack enable
corepack yarn install --immutable
cd crates/bindings/wikidata-node
corepack yarn build && corepack yarn test      # the Node binding
corepack yarn smoke:install                    # pack, install into a fresh project, use from ESM, CJS and TypeScript
```

Before pushing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
node scripts/check-version.mjs
```

The release profile uses fat LTO and one codegen unit, and
[`.cargo/config.toml`](.cargo/config.toml) aligns loops to 64 bytes (the
bzip2 decoder's speed otherwise depends on where unrelated code puts its
loops). For a binary tuned to the machine that builds it, add
`RUSTFLAGS="-C target-cpu=native -C llvm-args=-align-loops=64"` (a
`RUSTFLAGS` variable replaces the config's flags; the published binaries
are portable and do not use `target-cpu`).

## Limitations

- **Lexemes and media info** are parsed (lemmas, forms, senses, claims)
  but have no derived fields beyond the item ones.
- **gzip and zstd** decode on one thread (here ~1 GB/s and ~2.7 GB/s of
  JSON, so parsing on the other cores keeps up): a gzip member or zstd
  frame cannot be split without an index. Extracts written here are cut
  into independent frames, but they are read sequentially too.
- **`references`** are kept as snaks per reference; their hashes and
  `snaks-order` are not.
- **Statement ids** (GUIDs) and qualifier order are not kept.
- **Redirects** are not in the JSON dumps; they come from the SQL dumps
  (`wikidata redirects`), or, for a day, from an incremental dump. The
  API's `missing` entities are not handled.
- **Checkpoints** exist for bzip2 and plain JSON inputs (gzip and zstd
  cannot be resumed from the middle), and not for incremental (XML) dumps.
- **Live changes** have no read timeout: a connection that stalls without
  closing is noticed only when the stream is closed.

See [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) for everything about
the dump format.

## Documentation

| Document | Contents |
|---|---|
| [Node.js package](crates/bindings/wikidata-node) | The full JavaScript and TypeScript API, with recipes and examples. |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | The pipeline, the bzip2 decoder, filters, parsing and flow control. |
| [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) | What of the dump format is covered, and how values are normalised. |
| Crates: [`wikidata-dump`](crates/wikidata-dump), [`wikidata-model`](crates/wikidata-model), [`wikidata-filter`](crates/wikidata-filter), [`wikidata-export`](crates/wikidata-export) | Each crate's purpose and API. |
| [Benchmarks](bench/README.md) | Method, results and how to reproduce them. |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Conventions, fixtures, the bzip2 decoder's checks. |
| [RELEASING.md](RELEASING.md) | Cutting a release, dry runs, secrets. |
| [Changelog](CHANGELOG.md) | Release history. |

## CI/CD

Every push and pull request runs the quality gate on Linux, macOS and
Windows; a `vX.Y.Z` tag publishes everything.

| Workflow | Purpose |
|----------|---------|
| [`ci`](.github/workflows/ci.yml) | `cargo fmt`, `clippy`, `cargo test` on Linux, macOS and Windows; version agreement, crate packaging, `actionlint` |
| [`node`](.github/workflows/node.yml) | Build, typecheck and test `@wikidata-rs/node` on three OSes and Node 20, 22 and 24; `npm pack`, then install the tarball into a fresh project |
| [`release`](.github/workflows/release.yml) | Tag-driven: versions agree with the tag, the full gate in release mode, CHANGELOG notes; npm, the `wikidata` CLI for five targets, and a GitHub release with checksums |
| [`release-node`](.github/workflows/release-node.yml) | Build the eight platform binaries natively (glibc, musl, macOS, Windows; x64 and arm64), test each where it was built, pack, install on every platform with Node 18, then publish |

See [RELEASING.md](RELEASING.md) for cutting a release.

## Contributing

Contributions are welcome; [CONTRIBUTING.md](CONTRIBUTING.md) has the
conventions (fixtures, the bzip2 decoder's checks, Conventional Commits).

- Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and the tests before pushing
- A fix comes with a regression test
- Regenerate `fixtures/sample.json` with `node scripts/make-sample-dump.mjs` rather than editing it

## License

Released under the FTWPL, like tilemaker-rs. See [LICENCE.txt](LICENCE.txt).
Wikidata's data is CC0.

---

<div align="center">
  <sub>
    <a href="https://www.npmjs.com/package/@wikidata-rs/node">npm</a> ·
    <a href="crates/bindings/wikidata-node">Node.js guide</a> ·
    <a href="docs/">Docs</a> ·
    <a href="https://github.com/the-haus/wikidata-rs/releases">Releases</a> ·
    <a href="https://github.com/the-haus/wikidata-rs/issues">Issues</a>
  </sub>
  <br>
  <sub>Built in Rust. The whole dump, on every core, in bounded memory.</sub>
</div>
