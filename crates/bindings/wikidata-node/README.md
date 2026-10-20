# @wikidata-rs/node

[![npm version](https://img.shields.io/npm/v/@wikidata-rs/node.svg)](https://www.npmjs.com/package/@wikidata-rs/node)
[![Node.js](https://img.shields.io/node/v/@wikidata-rs/node.svg)](#requirements)
[![platforms](https://img.shields.io/badge/platforms-linux%20%7C%20macOS%20%7C%20windows-blue.svg)](#supported-platforms)
[![types](https://img.shields.io/npm/types/@wikidata-rs/node.svg)](#typescript)
[![license](https://img.shields.io/npm/l/@wikidata-rs/node.svg)](#license)

**Fast, streaming Wikidata dumps for Node.js.** Read `wikidata-*-all.json.bz2`
from a file or a URL on every core with bounded memory, filter before
parsing, and get typed, normalised entities, compact subsets, a class
hierarchy and file statistics. The engine is written in Rust and exposed
through N-API, and prebuilt binaries are provided for eight platforms.

```ts
import { openDump } from "@wikidata-rs/node";

const dump = await openDump("https://dumps.wikimedia.org/wikidatawiki/entities/latest-all.json.bz2");
for await (const e of dump.entities({ filter: { claim: "P17=Q55" }, languages: ["nl", "en", "mul"] })) {
  e.id;           // "Q1072"
  e.label;        // { text: "Pinkpop", language: "nl" }
  e.instanceOf;   // ["Q868557"]
  e.coordinates;  // { lat: 50.9, lon: 6.03 }
  e.externalIds;  // { musicbrainzArtist: "…", osmNode: "…" }
  e.entity;       // the full, typed entity (claims, labels, sitelinks, …), parsed on first access
}
```

## Contents

- [Highlights](#highlights)
- [Installation](#installation)
  - [Requirements](#requirements)
  - [Supported platforms](#supported-platforms)
- [Versioning and releases](#versioning-and-releases)
- [Performance](#performance)
- [Guide](#guide)
  - [Entities](#entities) · [Filters](#filters) · [Subclasses](#subclasses) · [Subsets](#subsets)
  - [File statistics](#file-statistics) · [URLs](#urls) · [Streams](#streams) · [Memory](#memory)
- [API reference](#api-reference)
- [TypeScript](#typescript)
- [Errors](#errors)
- [Troubleshooting](#troubleshooting)
- [Building from source](#building-from-source)
- [Examples](#examples)
- [Changelog](#changelog) · [License](#license)

## Highlights

- **Fast.** bzip2 is decoded on every core by a block decoder twice as
  fast as libbzip2: the first 50 MB of the dump in 0.6 s on 10 cores,
  12× faster than `bzip2 -dc | node`.
- **Filters before parsing.** Ids, types, properties, claim values,
  instance of a class (and its subclasses), sitelinks, combined with
  `all`, `any` and `not`. Most entities are rejected from their raw bytes
  and never parsed.
- **Organised entities.** Labels and descriptions in your languages with a
  fallback chain, claims as typed values (times at their precision,
  quantities with units, coordinates, Commons files, …), ranks,
  qualifiers, references, sitelink URLs.
- **Derived fields without `JSON.parse`.** `instanceOf`, `country`,
  `coordinates`, `inception`, `dissolved`, `image`, `website` and
  `externalIds` (MusicBrainz, Discogs, Spotify, OpenStreetMap, GeoNames,
  ISNI, VIAF, …) arrive as columnar data; the full entity is parsed only
  when you read it.
- **Subsets.** `extract()` keeps a small `.jsonl.zst` of what you need
  instead of the 103 GB dump, readable again by `openDump`.
- **Files or URLs.** Downloads stream without a temporary file and resume
  dropped connections.
- **Bounded memory.** The reader never gets more than `readAhead` batches
  ahead of your code; `AbortSignal` stops it and its threads.

## Installation

```sh
npm install @wikidata-rs/node
# or
yarn add @wikidata-rs/node
# or
pnpm add @wikidata-rs/node
```

The package ships no install scripts and needs no compiler. The prebuilt
binary for your platform is a separate package,
`@wikidata-rs/node-<platform>`, which the package manager installs
automatically through `optionalDependencies`. Only the one matching your
system is downloaded.

### Requirements

| | Supported | Notes |
|---|---|---|
| Node.js | **18 or newer** | N-API version 6. Tested on Node 20, 22 and 24 in CI; every release is also installed and run on the oldest supported Node on every platform before it is published (18; 20 on Windows arm64). |
| Module systems | ESM and CommonJS | `import` loads an ESM build and `require()` a CommonJS one; both bundle (see [Bundlers](#bundlers-esbuild-vite-webpack-nextjs)). |
| TypeScript | optional | Declarations are bundled; no `@types` package is needed. |

### Supported platforms

| OS | Architecture | C library | Binary package | Minimum OS |
|---|---|---|---|---|
| Linux | x64 | glibc | `@wikidata-rs/node-linux-x64-gnu` | glibc 2.31 (Debian 11, Ubuntu 20.04, RHEL 9, Amazon Linux 2023) |
| Linux | arm64 | glibc | `@wikidata-rs/node-linux-arm64-gnu` | glibc 2.31 |
| Linux | x64 | musl | `@wikidata-rs/node-linux-x64-musl` | Alpine 3.x |
| Linux | arm64 | musl | `@wikidata-rs/node-linux-arm64-musl` | Alpine 3.x |
| macOS | x64 (Intel) | — | `@wikidata-rs/node-darwin-x64` | macOS 10.15 |
| macOS | arm64 (Apple Silicon) | — | `@wikidata-rs/node-darwin-arm64` | macOS 11 |
| Windows | x64 | MSVC | `@wikidata-rs/node-win32-x64-msvc` | Windows 10 |
| Windows | arm64 | MSVC | `@wikidata-rs/node-win32-arm64-msvc` | Windows 10; Node.js 20 or newer |

Every binary is built natively on its own OS and architecture, and the
full test suite runs against it before release. For any other platform,
see [Building from source](#building-from-source).

## Versioning and releases

- **Semantic versioning.** While the package is at `0.x`, a minor release
  (`0.1` → `0.2`) may contain breaking changes and a patch release will
  not. From `1.0.0` on, breaking changes happen only in major releases.
  The entity JSON shape is part of the API.
- **Dist-tags.** Stable releases are published under `latest`, and
  prereleases (such as `0.2.0-rc.1`) under `next`.
- **One version for everything.** The root package and the platform
  packages always share a version, and the version matches the `wikidata`
  CLI and the Rust crates built from the same release tag.
- **Provenance.** Releases are built and published from CI with
  [npm provenance](https://docs.npmjs.com/generating-provenance-statements).
- **Release notes** are in the [changelog](https://github.com/the-haus/wikidata-rs/blob/main/CHANGELOG.md).

At runtime, `version()` returns the version of the native library that was
loaded.

### Bundled components

The native library is self-contained and links nothing from the system
beyond the C runtime: a native bzip2 block decoder (with libbzip2's Rust
port for the rare randomised blocks), zlib-rs, zstd 1.5, rustls for HTTPS
(with the Mozilla root certificates), and mimalloc as its allocator.

## Performance

Measured on an Apple M1 Max (10 cores, 32 GiB, macOS 26, Node 24) while
other jobs were running (load average 3–7), on the first 50 MB of
`wikidata-20261012-all.json.bz2` (520 MB of JSON, 10,989 entities) and on
400,000 small entities (448 MB of JSON). Wall time and peak memory, best
of three.

| Task | @wikidata-rs/node | `bzip2 -dc \| node` |
|---|---|---|
| Entities with `P17 = Q55`, summary fields (bzip2 head) | **0.59 s** · 272 MB | 6.55 s · 155 MB (substring test + `JSON.parse`) |
| The same on one thread | 3.72 s · 111 MB | |
| Every small entity, summary fields | **0.22 s** (1.8 M entities/s) | 1.79 s (`JSON.parse` per line) |
| Every small entity, full `entity` | 1.61 s (250 k entities/s) | |

The full entity is bounded by `JSON.parse` on the JS thread; choose
`languages` and `properties` to make it smaller, or use the summary
fields. Benchmarks for the CLI and the method are in the repository's
[`bench/`](https://github.com/the-haus/wikidata-rs/tree/main/bench).

## Guide

### Entities

`dump.entities(options)` streams the matching entities in dump order as
`EntityRecord`s:

| Field | From | |
|---|---|---|
| `id`, `type` | | `"Q42"`, `"item"` (`property`, `lexeme`, `mediainfo`, `entity-schema`) |
| `label`, `description` | `labels`, `descriptions` | `{ text, language }` in the first language of `languages` that has one (default `en`, then `mul`) |
| `instanceOf`, `subclassOf`, `country` | P31, P279, P17 | item ids |
| `coordinates` | P625 | `{ lat, lon }` |
| `inception`, `dissolved` | P571, P576 | years |
| `image` | P18 | a Commons file name |
| `website` | P856 | |
| `externalIds` | see `EXTERNAL_IDS` | `{ musicbrainzArtist, discogsArtist, osmRelation, geonames, viaf, … }` |
| `matchedBy` | the filter | the [named filters](#named-filters) that match the entity, `[]` without names |
| `entity` | | the full `Entity`, parsed on first access |
| `json` | | its JSON text |

The derived fields come from the best-ranked ("truthy") statements and are
computed whatever `properties` keeps. Options shape the full entity:

```ts
dump.entities({
  languages: ["nl", "en", "mul"], // labels/descriptions/aliases kept, and the label fallback order
  sitelinks: ["nlwiki", "enwiki"], // default: all
  properties: ["P31", "P17", "P625", "P571"], // claims kept (default: all)
  qualifiers: true,                // default
  references: false,               // default
  deprecated: false,               // default: deprecated statements are dropped
  bestRank: false,                 // true: only each property's preferred statements, when it has any
  json: true,                      // false: summary fields only (faster)
});
```

Languages and sitelinks can also be chosen per entity, from the values of
its claims: each venue's label in its own country's language, and English,
without keeping every language of every country for every entity.

```ts
dump.entities({
  languages: { by: "P17", map: { Q55: ["nl"], Q183: ["de"], Q31: ["nl", "fr"] }, always: ["en", "mul"] },
  sitelinks: { by: "P17", map: { Q55: ["nlwiki"], Q183: ["dewiki"] }, always: ["enwiki"] },
});
// A Dutch item keeps nl, en, mul (label: nl first); a German one de, en, mul;
// an item without a mapped country en, mul.
```

`by` takes one property or several (`["P17", "P495"]`); the values are read
from the truthy statements, whether or not `properties` keeps the claims.

Every claim value carries a `type`:

```ts
const e = record.entity;
for (const s of e.claims.P571 ?? []) {
  if (s.value.type === "time") s.value.iso; // "1830-10-04", "1830-10" or "1830", by precision
}
e.claims.P1082?.[0]?.value; // { type: "quantity", amount: 11697557, unit: null }
e.claims.P625?.[0]?.value;  // { type: "coordinate", lat, lon, precision, globe: "Q2" }
e.claims.P18?.[0]?.value;   // { type: "media", file: "Foo.jpg", url: "https://commons.wikimedia.org/wiki/File:Foo.jpg" }
e.sitelinks.nlwiki;         // { title: "België", url: "https://nl.wikipedia.org/wiki/Belgi%C3%AB", badges: [] }
```

The value types are `entity`, `string`, `externalId`, `url`, `media`,
`monolingual`, `time`, `quantity`, `coordinate`, `math`,
`musicalNotation`, `geoShape`, `tabularData`, `someValue` and `noValue`;
see the repository's
[COMPATIBILITY.md](https://github.com/the-haus/wikidata-rs/blob/main/docs/COMPATIBILITY.md).

### Filters

```ts
{ ids: ["Q42", "P31"] }                         // these entities
{ type: "item" }                                // or ["property", "lexeme"]
{ hasProperty: ["P434", "P1953"] }              // a statement on any of these
{ claim: "P17=Q55" }                            // a statement with this value (or "P17=Q55,Q31"; "P17": any)
{ claim: { property: "P297", values: ["NL", "BE"] } }
{ instanceOf: ["Q5"] }                          // P31 one of these
{ instanceOf: { classes: ["Q868557"], subclasses: true } } // or a subclass (needs classIndex)
{ sitelink: "nlwiki" }
{ any: [{ claim: "P17=Q55" }, { hasProperty: "P434" }] }
[{ type: "item" }, { not: { sitelink: "enwiki" } }] // an array, or several keys: all of them
```

Claim filters ignore deprecated statements. Most lines are decided from
their raw bytes: a claim filter rejects every line that does not contain
`"P17":[` and `"id":"Q55"`, without parsing it.

#### Large `ids` filters

A long list of items is cheaper as a typed array of item numbers (`42` for
`Q42`) than as strings: it crosses to the native side without being written
out as JSON (a million ids cost ~0.1 MB of JS heap instead of ~36 MB).
`Uint32Array` and `Int32Array` are passed without a copy; other typed
arrays are converted.

```ts
const known = Uint32Array.from(knownQids, (q) => Number(q.slice(1)));
dump.entities({ filter: { ids: known } });
```

#### Named filters

Name the branches of one pass, and each entity says which of them it
matched, every one (not only the first):

```ts
const stream = dump.entities({
  filter: {
    any: {
      venue: { instanceOf: { classes: ["Q8719053"], subclasses: true } },
      festival: { instanceOf: { classes: ["Q868557"], subclasses: true } },
      artist: { hasProperty: "P434" },
      known: { ids: known },
    },
  },
  classIndex,
});
for await (const e of stream) e.matchedBy; // ["venue", "known"]
```

`any` and `all` take an object of named filters, and any filter can carry
a `name` (`{ name: "dutch", claim: "P17=Q55" }`). A named filter is
reported wherever it is in the filter, when the entity matches the whole
filter. `matchedBy` lists them in the order the filter names them.

#### Ids the dump lacks

With `ids` in the filter (anywhere in it), `stats.missingIds` lists the
requested ids the dump does not hold: deleted, or merged into another item
(follow the redirect with the Wikidata API). It is only set after a
complete pass, not one stopped by `limit`, `close()`, a signal, or
truncated with `allowTruncated`.

```ts
const stream = dump.entities({ filter: { ids: known } });
for await (const e of stream) refresh(e);
const { missingIds } = await stream.stats; // ["Q123", …] or [] (undefined if cut short)
```

The missing items also come as `stats.missingItemNumbers` (a `Uint32Array`);
`missingIds` is made from it only when read, so a pass that misses many ids
costs no more JS heap than its `ids` did.

### Subclasses

"Instance of music festival, or of any subclass of it" needs the class
hierarchy. Build it in one pass over a dump (as long as reading it; only
the P279 claims are parsed), and save it (tens of MB for all of Wikidata):

```ts
import { ClassIndex, openDump } from "@wikidata-rs/node";

const dump = await openDump("wikidata-20261012-all.json.bz2");
const classIndex = await dump.classIndex();
classIndex.save("classes.wdc");
// later: const classIndex = await ClassIndex.load("classes.wdc");

classIndex.subclassesOf("Q868557"); // ["Q868557", …every subclass]
classIndex.isSubclassOf("Q99000001", "Q1656682");
for await (const e of dump.entities({
  filter: { instanceOf: { classes: ["Q868557"], subclasses: true } },
  classIndex,
})) {
  // …
}
```

`ClassIndex.build(pathOrUrl, options)` opens a dump and builds its index in
one call.

Building the index costs a pass of its own. To build it as a by-product of
a pass you make anyway, give that pass `buildClassIndex`
(`entities()`, `extract()` and `fileInfo()` take it). The index is saved
when the pass ends without being stopped, and `stats.classIndex` reports
`{ path, classes, edges }`. The file is identical to `ClassIndex.build` on
the same dump, and is written to a temporary name first, so it exists only
when it is whole. The pass's own filter cannot use it yet; the next pass can.

```ts
const stats = await dump.extract({
  output: "subset.jsonl.zst",
  filter: { hasProperty: "P434" },
  buildClassIndex: "wikidata-20261012-classes.wdc",
});
stats.classIndex; // { path: "wikidata-20261012-classes.wdc", classes: …, edges: … }
// the next pass: classIndex: await ClassIndex.load("wikidata-20261012-classes.wdc")
```

### Subsets

Keep the part of Wikidata you need instead of the 103 GB dump:

```ts
const stats = await dump.extract({
  output: "subset.jsonl.zst",
  filter: { any: [{ claim: "P17=Q55" }, { hasProperty: "P434" }] },
  onProgress: (p) => console.log(`${(100 * (p.fraction ?? 0)).toFixed(1)}%, ${p.mbPerSecondIn.toFixed(0)} MB/s`),
});
const subset = await openDump("subset.jsonl.zst"); // reads like the dump, with any options
```

Extracts copy the dump's original lines (so they can be read again with
any options). The output format follows the file name: `.jsonl.zst`
(recommended: as small as bzip2, written 4.5× and read back 3× faster),
`.json.bz2` (Wikimedia's own layout; any tool reads it), `.json.gz`,
`.jsonl.gz`, `.json`, `.jsonl`. Compression runs on every core; `level`
sets it.

### File statistics

```ts
const info = await dump.fileInfo();
info.types;    // { items, properties, lexemes, mediainfo, entitySchemas }
info.maxIds;   // the highest id number per type
info.largest;  // { id, bytes }
info.bytesIn; info.bytesOut; info.blocks; info.elapsedMs;
```

### URLs

`openDump(url)` streams `http://` and `https://` URLs without a temporary
file. When the connection drops, it reconnects with a `Range` request (and
`If-Range`, so a file that changed on the server is an error, not a
corrupt read). `userAgent` sets the `User-Agent` on every request,
reconnects included (default `wikidata-rs/<version> (https://github.com/the-haus/wikidata-rs)`;
Wikimedia asks for a descriptive one with contact details), and `retries`
the reconnect attempts per position (default 8; 0 fails on the first
dropped connection). `openDump` takes them, and so do the functions that
open a dump themselves:

```ts
const http = { userAgent: "Festimap/1.0 (ops@example.org)", retries: 20 };
streamEntities(url, { ...http, filter });
await extract(url, { ...http, output: "subset.jsonl.zst" });
await fileInfo(url, http);
await ClassIndex.build(url, http);
```

A truncated download is an error; `allowTruncated: true` reads a partial
file up to its last whole entity and sets `stats.truncated`.

### Streams

`entities()` returns an `EntityStream`:

- `for await (const e of stream)` or `for await (const batch of stream.batches())`
- `stream.toArray()`, `stream.map(fn)`, `stream.filter(fn)`, `stream.toReadable()`
- `stream.stats`: a promise of the counters (`entities`, `matched`,
  `parsed`, `bytesIn`, `bytesOut`, `types`, `stopped`, `truncated`, …,
  and `missingIds` and `classIndex` when they apply), resolved when the
  stream ends
- `stream.close()`: stop early (breaking out of a loop does this too)
- options `limit`, `onProgress`, `signal` (an `AbortSignal`), `batchSize`
  (default 1024) and `readAhead` (default 2), `threads` (default: all
  cores)

Progress events carry `bytesIn`, `bytesTotal`, `fraction`, `bytesOut`,
`entities`, `matched`, `elapsedMs`, `mbPerSecondIn`, `mbPerSecondOut` and
`entitiesPerSecond`. Nothing is read until iteration starts. A stream can
be iterated once.

### Memory

The reader holds at most a few blocks per thread (bzip2 blocks are
900 kB, ~6 MB of tables per decoding thread) and `readAhead` batches of
entities; peak memory is ~200–300 MB on 10 cores whatever the dump's
size. Filters cost about 2 bytes per id for `ids`, and a class index
about 12 bytes per subclass-of edge.

## API reference

| Export | Description |
|---|---|
| `openDump(pathOrUrl, OpenOptions?)` → `Promise<DumpReader>` | Open a dump (looks at its first bytes). |
| `DumpReader` | `.input`, `.compression`, `.size`, `.entities(opts)`, `.extract(opts)`, `.fileInfo(opts)`, `.classIndex(opts)` |
| `streamEntities(pathOrUrl, EntityOptions & OpenOptions)` | Entities, opening the dump when iteration starts. |
| `extract(pathOrUrl, ExtractOptions & OpenOptions)` → `Promise<ExtractStats>` | A subset. |
| `fileInfo(pathOrUrl, CommonOptions & OpenOptions)` → `Promise<FileInfo>` | What a dump holds. |
| `ClassIndex` | `.build(pathOrUrl, ClassIndexOptions & OpenOptions)`, `.load(path)`, `.save(path)`, `.size`, `.edges`, `.source`, `.parents(id)`, `.subclassesOf(ids)`, `.isSubclassOf(id, ancestor)` |
| `EntityStream<T>` | See [Streams](#streams). |
| `EXTERNAL_IDS`, `externalIdUrl(key, value)` | The external identifier table and URLs. |
| `version()` | The version of the loaded native library. |
| `WikidataError` | The error class. |

## TypeScript

Type declarations are bundled, and every option and field is documented in
them. The main types are:

- **Records**: `EntityRecord`, `Text`, `ExternalIdKey`.
- **Entities**: `Entity`, `Statement`, `Snaks`, `Sitelink`, `Form`,
  `Sense`, `Rank`, `EntityType`, and `Value` = the union of `TimeValue`,
  `QuantityValue`, `CoordinateValue` and the other value shapes.
- **Filters**: `Filter`, `ItemNumbers`.
- **Options**: `OpenOptions`, `CommonOptions`, `EntityOptions` (with
  `ByClaim`), `ExtractOptions`, `ClassIndexOptions`.
- **Results**: `ScanStats` (with `TypeCounts`), `ExtractStats`, `FileInfo`.
- **Progress**: `Progress`.

## Errors

Invalid options, bad filters, unreadable, corrupt or truncated dumps and
HTTP errors reject with a `WikidataError`, either from `openDump` or from
the first iteration of a stream. An aborted stream rejects with the
signal's reason.

```ts
import { openDump, WikidataError } from "@wikidata-rs/node";

try {
  for await (const e of (await openDump("dump.json.bz2")).entities({ filter: { claim: "P17=Q55" } })) {
    // …
  }
} catch (err) {
  if (err instanceof WikidataError) console.error("could not read the dump:", err.message);
  else throw err;
}
```

## Troubleshooting

### No native binary found

`@wikidata-rs/node: no native binary found for <platform>-<arch>` means
the platform package was not installed. Common causes:

- **Optional dependencies were skipped.** For example, `npm install
  --omit=optional`, `--no-optional`, or `optional=false` in `.npmrc`.
  Install without that option.
- **`node_modules` came from another platform.** For example, it was
  installed on macOS and copied into a Linux Docker image, or installed on
  glibc and run on Alpine. Install inside the image or on the target
  system instead.
- **The platform is not supported.** See [Supported platforms](#supported-platforms)
  and [Building from source](#building-from-source).

### `GLIBC_2.xx` not found

The Linux distribution is older than glibc 2.31. Upgrade the base image
(Debian 11, Ubuntu 20.04 or later), or use an Alpine image, which gets the
musl build.

### Bundlers (esbuild, Vite, webpack, Next.js)

The package bundles for Node.js targets without being marked external: the
ESM build (`import`) and the CommonJS build (`require()`) both load the
native addon at run time with a `require` that bundlers leave alone. This
is tested with esbuild (ESM and CommonJS output) and Vite's SSR build.
Other bundlers are not tested; if one tries to bundle the `.node` file,
mark the package external.
The addon itself is not bundled. At run time it is found in the platform
package (`@wikidata-rs/node-<platform>`) in a `node_modules` beside or above
the bundle, or at the path in `WIKIDATA_RS_NODE_BINARY`:

```sh
# e.g. a Docker image with the bundle and only the binary
WIKIDATA_RS_NODE_BINARY=/app/wikidata-node.linux-x64-gnu.node node dist/ingest.mjs
```

Tools that locate the package's directory can resolve
`@wikidata-rs/node/package.json`. Marking the package external still works
too (`serverExternalPackages` in Next.js).

### Two copies of the package

`import` and `require()` load separate builds. A process that does both
(an ESM app using a CommonJS library that also requires this package) has
two copies of its classes, so `err instanceof WikidataError` is false for
an error thrown by the other copy, and a `ClassIndex` from one cannot be
passed to the other. Check `err.name === "WikidataError"` instead, and use
one module system for the package within a process. The native library is
loaded once either way.

## Building from source

You need Rust 1.87 or newer, Node.js 18 or newer, and a C compiler (zstd
and mimalloc are compiled from vendored sources). From the root of the
repository:

```sh
corepack enable
yarn install
cd crates/bindings/wikidata-node
yarn build        # native addon (release) + TypeScript
yarn test         # vitest
```

`yarn build` writes `wikidata-node.<platform>.node` next to the package,
and the loader picks it up before looking for an installed platform
package.

## Examples

Runnable scripts are in [`examples/`](https://github.com/the-haus/wikidata-rs/tree/main/crates/bindings/wikidata-node/examples):

| Script | What it does |
|---|---|
| `music-festivals.mjs` | Music festivals (and every subclass) in a country, as CSV, with a saved class index. |
| `weekly-subset.mjs` | A `.jsonl.zst` subset from the latest dump, with progress. |
| `weekly-refresh.mjs` | One pass for artists, festivals and known ids: named branches, item-number ids, labels by country, missing ids, and the class index built on the first run. |
| `places.mjs` | Places with coordinates in a country, as GeoJSON lines. |
| `external-ids.mjs` | Items mapped to MusicBrainz, Discogs and Spotify, with URLs. |
| `file-info.mjs` | What a dump holds. |

## Changelog

See [CHANGELOG.md](https://github.com/the-haus/wikidata-rs/blob/main/CHANGELOG.md) for the release history.

## License

FTWPL; see [LICENSE](LICENSE). Wikidata's data is CC0.
