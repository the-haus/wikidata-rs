# wikidata-rs

[![CI](https://github.com/the-haus/wikidata-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/the-haus/wikidata-rs/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/the-haus/wikidata-rs?include_prereleases)](https://github.com/the-haus/wikidata-rs/releases)
[![npm](https://img.shields.io/npm/v/@wikidata-rs/node.svg?label=npm)](https://www.npmjs.com/package/@wikidata-rs/node)
[![Rust](https://img.shields.io/badge/rust-1.87%2B-orange.svg)](#building-from-source)
[![License: FTWPL](https://img.shields.io/badge/license-FTWPL-lightgrey.svg)](LICENCE.txt)

**Streaming Wikidata dumps, fast, in Rust.**

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
- bounded memory, and a truncated input is always an error, never a silent
  end.

Everything is available in three ways: the `wikidata` command line tool,
a set of Rust crates, and a Node.js package
([`@wikidata-rs/node`](crates/bindings/wikidata-node)). It is a sibling of
[tilemaker-rs](https://github.com/the-haus/tilemaker-rs) and follows the
same design.

## Contents

- [Features](#features)
- [Installation](#installation)
- [Quick start](#quick-start)
- [The output](#the-output)
- [Verified](#verified)
- [Performance](#performance)
- [Project layout](#project-layout)
- [Building from source](#building-from-source)
- [Limitations](#limitations)
- [Documentation](#documentation)
- [Contributing and releases](#contributing-and-releases)
- [License](#license)

## Features

| | |
|---|---|
| **Fast reading** | bzip2 is cut at its bit-aligned block boundaries and the blocks are decoded on every core by a decoder that is 1.9× faster than libbzip2, in file order. gzip and zstd decode on one thread, pipelined with everything else; plain JSON and JSON lines are read as they are. |
| **Files or URLs** | A path, or an `http(s)://` URL that is streamed (no temporary file) and resumed with `Range` requests when the connection drops. |
| **Filters before parsing** | `ids` (a compact id set), `type`, `hasProperty`, `claim` (`P17=Q55`), `instanceOf` (with subclasses, through a class index built in one pass), `sitelink`, combined with `all` / `any` / `not`. Lines are rejected from their first bytes and SIMD substring search; only possible matches are parsed. |
| **Organised output** | A stable JSON shape with typed values, language fallback, sitelink URLs, rank handling and derived fields, computed in Rust. |
| **Subsets** | `extract` writes the matching entities' original lines as `.jsonl.zst` (recommended), `.json.bz2` (Wikimedia's own layout), `.json.gz`, … compressed on every core, readable by this package and by every common tool. |
| **Streaming by design** | Every stage runs in parallel or pipelined; a slow consumer slows the reader instead of filling memory (credit-based flow control into Node.js). |

## Installation

### Command line

Prebuilt binaries for Linux (x64, arm64), macOS (Apple Silicon, Intel) and
Windows (x64) are attached to every
[GitHub release](https://github.com/the-haus/wikidata-rs/releases), with
SHA-256 checksums:

```sh
VERSION=0.1.0
TARGET=aarch64-apple-darwin   # or x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu,
                              #    x86_64-apple-darwin, x86_64-pc-windows-msvc (.zip)
curl -fsSL "https://github.com/the-haus/wikidata-rs/releases/download/v$VERSION/wikidata-$VERSION-$TARGET.tar.gz" | tar xz
./wikidata-$VERSION-$TARGET/wikidata --version
```

To build the CLI with Cargo instead:

```sh
cargo install --git https://github.com/the-haus/wikidata-rs --tag v0.1.0 wikidata-cli
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
wikidata-export = { git = "https://github.com/the-haus/wikidata-rs", tag = "v0.1.0" }
# just the dump reader (parallel bzip2, HTTP, line chunks)
wikidata-dump = { git = "https://github.com/the-haus/wikidata-rs", tag = "v0.1.0" }
# just the entity model and parser
wikidata-model = { git = "https://github.com/the-haus/wikidata-rs", tag = "v0.1.0" }
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
wikidata cat subset.jsonl.zst --instance-of Q868557 --subclasses --classes classes.wdc
```

Run `wikidata --help`, or `wikidata <command> --help`, for every option.

### Node.js

```ts
import { openDump } from "@wikidata-rs/node";

const dump = await openDump("https://dumps.wikimedia.org/wikidatawiki/entities/latest-all.json.bz2");
for await (const e of dump.entities({ filter: { claim: "P17=Q55" }, languages: ["nl", "en", "mul"] })) {
  e.label;        // { text: "Pinkpop", language: "nl" } — no JSON parsed for these
  e.coordinates;  // { lat: 50.9, lon: 6.03 }
  e.externalIds;  // { musicbrainzArtist: "…", osmNode: "…" }
  e.entity;       // the full, typed entity, parsed on first access
}

await dump.extract({ output: "nl.jsonl.zst", filter: { claim: "P17=Q55" } });
```

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

## Building from source

Requirements: Rust 1.87 or newer, a C compiler (for zstd and mimalloc,
built from vendored sources), and Node.js 18 or newer for the Node
binding.

```sh
# The CLI
cargo build --release -p wikidata-cli          # target/release/wikidata

# Rust tests
cargo test --workspace --exclude wikidata-node

# The Node binding
corepack enable && yarn install
cd crates/bindings/wikidata-node
yarn build && yarn test
```

The release profile uses fat LTO and one codegen unit. For a binary tuned
to the machine that builds it, add `RUSTFLAGS="-C target-cpu=native"`
(the published binaries are portable and do not).

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
- **Wikidata's JSON quirks** that are not in the dumps (the API's
  `missing` entities, `redirects`) are not handled.

See [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) for everything about
the dump format.

## Documentation

| Document | Contents |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | The pipeline, the bzip2 decoder, filters, parsing and flow control. |
| [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) | What of the dump format is covered, and how values are normalised. |
| [Node.js package](crates/bindings/wikidata-node) | The full JavaScript and TypeScript API. |
| [Benchmarks](bench/README.md) | Method, results and how to reproduce them. |
| [Changelog](CHANGELOG.md) | Release history. |

## Contributing and releases

Development setup, conventions and the release process are described in
[CONTRIBUTING.md](CONTRIBUTING.md). Releases are cut from a `vX.Y.Z` tag.
A single workflow then publishes all of the following:

- the npm packages;
- the CLI binaries;
- a GitHub release with notes and checksums.

## License

Released under the FTWPL, like tilemaker-rs. See [LICENCE.txt](LICENCE.txt).
Wikidata's data is CC0.
