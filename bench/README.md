# Benchmarks

`run.mjs` times our CLI and Node package against plain JavaScript reading
the dump through `bzip2 -dc` (the usual way: decompress with the system
tool, split lines and `JSON.parse` in Node). Each run is measured with
`/usr/bin/time -l` (wall time, CPU time, peak resident memory); every
command runs three times and the best is kept. lbzip2 and pbzip2 are used
when they are on `$PATH`.

```sh
# The first 50 MB of a dump (one range request; the file is truncated mid-block,
# so every tool reads it with --allow-truncated / allowTruncated)
mkdir -p bench/.work
curl -r 0-52428799 -o bench/.work/head.json.bz2 \
  https://dumps.wikimedia.org/wikidatawiki/entities/20261012/wikidata-20261012-all.json.bz2

cargo build --release -p wikidata-cli                 # target/release/wikidata
(cd crates/bindings/wikidata-node && corepack yarn build)
node bench/run.mjs [--group decode,filter,parse,small] [--runs 3]

# The bzip2 decoders alone, block by block, checked against each other:
cargo run --release -p wikidata-dump --example bz2_compare -- bench/.work/head.json.bz2
# serde_json against sonic-rs on the same lines (needs the decompressed head):
bzip2 -dc bench/.work/head.json.bz2 > bench/.work/head.json
cargo run --release -p wikidata-model --example parse_bench -- bench/.work/head.json
```

Results go to `results.json` (the last run) and are printed as the tables
below. `tasks/node-entities.mjs` streams entities through the Node package
and reads their fields; `tasks/baseline-filter.mjs` is the plain
JavaScript reader.

Data: the first 50 MB of `wikidata-20261012-all.json.bz2` (659 bzip2
blocks, 520 MB of JSON, 10,989 entities of 47 kB on average: the dump
starts with large, well-known items); and, for the cost per entity, the
repository's sample dump repeated to 400,000 entities of ~1.5 kB.

Machine: Apple M1 Max (8 performance and 2 efficiency cores), 32 GiB,
macOS 26, SSD; Node 24.11; Rust 1.95. Other jobs were running at the same
time (load average 3–4 for the tables below; a second run at load 7–9
measured the 10-thread rows 20–30% slower: decode 0.68 s, filter 0.62 s),
so these are lower bounds. The full dump could not be measured: it was
still downloading.

## Results

### Decompress and count entities (52 MB of bzip2, 10,989 entities)

| Tool | Wall | CPU | MB/s in | JSON MB/s | Entities/s | Peak RSS |
|---|---:|---:|---:|---:|---:|---:|
| wikidata info (1 thread) | 3.51 s | 3.6 s | 14.9 | 148 | 3,131 | 51 MB |
| wikidata info (4 threads) | 1.01 s | 4.1 s | 51.9 | 515 | 10,880 | 101 MB |
| wikidata info (10 threads) | **0.53 s** | 4.8 s | **98.9** | **981** | 20,734 | 201 MB |
| wikidata info, libbzip2 decoder (10 threads) | 1.11 s | 9.1 s | 47.2 | 469 | 9,900 | 168 MB |
| bzip2 -dc \| node (count lines) | 6.59 s | 7.3 s | 8.0 | 79 | 1,668 | 157 MB |

### Entities with P17 = Q55 (the Netherlands), from the bzip2 head

59 entities match; 73 of the 10,989 lines are parsed, the rest are
rejected from their bytes.

| Tool | Wall | CPU | MB/s in | JSON MB/s | Entities/s | Peak RSS |
|---|---:|---:|---:|---:|---:|---:|
| wikidata cat --claim P17=Q55 (1 thread) | 3.47 s | 3.6 s | 15.1 | 150 | 3,167 | 59 MB |
| wikidata cat --claim P17=Q55 (4 threads) | 1.03 s | 4.2 s | 50.9 | 505 | 10,669 | 128 MB |
| wikidata cat --claim P17=Q55 (10 threads) | **0.54 s** | 4.9 s | **97.1** | 963 | 20,350 | 221 MB |
| @wikidata-rs/node, summary fields (1 thread) | 3.72 s | 3.8 s | 14.1 | 140 | 2,954 | 111 MB |
| @wikidata-rs/node, summary fields (4 threads) | 1.04 s | 4.2 s | 50.4 | 500 | 10,566 | 172 MB |
| @wikidata-rs/node, summary fields (10 threads) | **0.59 s** | 5.0 s | 88.9 | 882 | 18,625 | 272 MB |
| @wikidata-rs/node, full entity (10 threads) | 0.59 s | 5.0 s | 88.9 | 882 | 18,625 | 288 MB |
| bzip2 -dc \| node (substring test + JSON.parse) | 6.55 s | 7.6 s | 8.0 | 79 | 1,678 | 155 MB |

### Parse and normalise every entity (520 MB of plain JSON)

| Tool | Wall | CPU | MB/s in | JSON MB/s | Entities/s | Peak RSS |
|---|---:|---:|---:|---:|---:|---:|
| wikidata cat, nl/en/mul (1 thread) | 1.13 s | 1.2 s | 460 | 460 | 9,725 | 65 MB |
| wikidata cat, nl/en/mul (4 threads) | 0.31 s | 1.3 s | 1,678 | 1,678 | 35,448 | 138 MB |
| wikidata cat, nl/en/mul (10 threads) | **0.17 s** | 1.5 s | **3,060** | 3,060 | 64,641 | 249 MB |
| wikidata cat, all languages (10 threads) | 0.16 s | 1.4 s | 3,251 | 3,251 | 68,681 | 272 MB |
| @wikidata-rs/node, summary fields (10 threads) | 0.18 s | 1.3 s | 2,890 | 2,890 | 61,050 | 277 MB |
| @wikidata-rs/node, full entity (10 threads) | 1.10 s | 2.6 s | 473 | 473 | 9,990 | 779 MB |
| node JSON.parse every line | 2.97 s | 3.2 s | 175 | 175 | 3,700 | 309 MB |

### Parse and normalise 400,000 small entities (448 MB of plain JSON, ~1.5 kB each)

| Tool | Wall | CPU | MB/s in | Entities/s | Peak RSS |
|---|---:|---:|---:|---:|---:|
| wikidata cat, nl/en/mul (1 thread) | 1.50 s | 1.7 s | 299 | 266,667 | 62 MB |
| wikidata cat, nl/en/mul (10 threads) | **0.25 s** | 1.9 s | 1,793 | **1,600,000** | 277 MB |
| @wikidata-rs/node, summary fields (10 threads) | **0.22 s** | 1.6 s | 2,038 | **1,818,182** | 294 MB |
| @wikidata-rs/node, full entity (10 threads) | 1.61 s | 3.5 s | 278 | 248,447 | 561 MB |
| node JSON.parse every line | 1.79 s | 1.9 s | 250 | 223,464 | 95 MB |

### The bzip2 decoders, one core

`bz2_compare` on the 659 blocks of the head (each block decoded by both,
the outputs compared):

| Decoder | Time | MB/s of JSON |
|---|---:|---:|
| wikidata-dump `bzdecode` | 3.5 s | 148 |
| libbzip2 (the `bzip2` crate's Rust port; the C library measured the same) | 6.8 s | 76 |
| `bzip2 -dc` (system, 1.0.8) | 7.4 s | 70 |

### JSON parsers, one core

`parse_bench` on the head's 10,989 lines, checking that both produce the
same entities:

| Options | serde_json | sonic-rs |
|---|---:|---:|
| everything, with references | 373 MB/s | **497 MB/s** |
| defaults (all languages, no references) | 449 MB/s | **537 MB/s** |
| nl, en, mul + 6 properties | **757 MB/s** | 651 MB/s |
| ids and derived fields only | **786 MB/s** | 666 MB/s |

The parser is picked by the options: sonic-rs for unrestricted ones,
serde_json for selective ones.

### Subset formats

The whole head (10,989 entities) extracted to each format, then read back
with `wikidata info` (10 threads):

| Output | Written in | Size | Read back in |
|---|---:|---:|---:|
| `.jsonl.zst` (level 9, 8 MB frames) | 1.2 s | 52 MB | **0.19 s** |
| `.json.bz2` (level 9, one stream per block) | 5.5 s | 52 MB | 0.62 s |
| `.jsonl.gz` (level 6) | 1.0 s | 74 MB | 0.49 s |
| `.jsonl` | 0.8 s | 520 MB | 0.09 s |

### 0.2.0 against 0.1.0 (the head, best of 5 interleaved runs, load 4–5)

| Task | 0.1.0 | 0.2.0 |
|---|---:|---:|
| `wikidata info` | 0.582 s | 0.576 s |
| `cat --claim P17=Q55` | 0.610 s | 0.596 s |
| `cat --ids-file` (1M ids, 995,000 of them missing) | 0.644 s | 0.653 s |
| the class index and `info` | 1.242 s (`classes`, then `info`) | 0.567 s (`info --build-classes`) |

Tracking the ids an `ids` filter misses and reading P279 edges for
`--build-classes` cost nothing measurable here (differences are within
the noise), and the index built during `info` is byte-identical to
`wikidata classes`. Named filters cost only for the entities that match:
four branches, one of them (`sitelink: enwiki`) matching 10,441 of the
10,989 entities, took 742 ms through Node against 716 ms for the same
filter unnamed (+3.7%).

## Reading the numbers

- Everything that reads bzip2 is bound by decompression: filtering and
  parsing the matches cost almost nothing on top (`info` and `cat
  --claim` take the same time).
- The native decoder is twice as fast as libbzip2 on one core and on ten.
  At ~99 MB/s of input, a pass over the full 103 GB dump takes about
  18 minutes on this machine (projected, not measured).
- Through Node.js, the summary fields cost nothing extra; the full entity
  is bounded by `JSON.parse` on the JS thread, so keep only the languages
  and properties you need.
