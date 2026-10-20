// Run the benchmarks: wall time, CPU time and peak RSS (from
// /usr/bin/time) of each tool on the dump head, written to results.json
// and printed as Markdown.
//
//   node run.mjs [--head path/to/head.json.bz2] [--group decode,filter,parse] [--runs 3]
//
// The input is the first 50 MB of a Wikidata dump
// (`curl -r 0-52428799 -o head.json.bz2 https://dumps.wikimedia.org/…/wikidata-…-all.json.bz2`);
// it is truncated mid-block, so every tool reads it with --allow-truncated
// (and bzip2 reports the end as an error, which is ignored). Tools: our
// CLI (target/release/wikidata: `cargo build --release` first), our Node
// package (`yarn build` first), and `bzip2 -dc | node` line filtering.
// lbzip2 and pbzip2 are used when on $PATH.

import { spawnSync } from "node:child_process";
import { cpus } from "node:os";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 ? process.argv[i + 1] : fallback;
};
const groups = arg("group", "decode,filter,parse,small").split(",");
const runs = Number(arg("runs", "3"));
const head = resolve(arg("head", process.env.WIKIDATA_BENCH_HEAD ?? join(here, ".work/head.json.bz2")));
if (!existsSync(head)) {
  console.error(`no dump head at ${head} (see the comment at the top of this file)`);
  process.exit(1);
}
const work = join(here, ".work");
mkdirSync(work, { recursive: true });
const cli = join(root, "target/release/wikidata");
const all = cpus().length;
const which = (cmd) => spawnSync("sh", ["-c", `command -v ${cmd}`]).status === 0;

// The decompressed head, for the parse group.
const plain = join(work, "head.json");
if (groups.includes("parse") && !existsSync(plain)) {
  spawnSync("sh", ["-c", `bzip2 -dc '${head}' > '${plain}' 2>/dev/null`]);
}
// Many small entities (the repository's sample, repeated), like most of
// the real dump, to measure the cost per entity.
const small = join(work, "small.json");
const SMALL_N = 400_000;
if (groups.includes("small") && !existsSync(small)) {
  const lines = readFileSync(join(root, "fixtures/sample.json"), "utf8")
    .split("\n")
    .filter((l) => l.startsWith("{"))
    .map((l) => (l.endsWith(",") ? l.slice(0, -1) : l));
  const out = [];
  for (let i = 0; i < SMALL_N; i++) out.push(lines[i % lines.length]);
  writeFileSync(small, "[\n" + out.join(",\n") + "\n]\n");
}
const sizeIn = readFileSync(head).length;
const sizeOut = groups.includes("parse") ? readFileSync(plain).length : 0;

/** Run a shell command under /usr/bin/time; best wall time of `runs`. */
function measure(cmd) {
  let best;
  for (let i = 0; i < runs; i++) {
    const r = spawnSync("/usr/bin/time", ["-l", "sh", "-c", `${cmd} > '${work}/out.txt'`], {
      encoding: "utf8",
      maxBuffer: 1 << 26,
    });
    const err = r.stderr;
    const num = (re) => Number(re.exec(err)?.[1] ?? NaN);
    const m = {
      wall: num(/([\d.]+) real/),
      user: num(/([\d.]+) user/),
      sys: num(/([\d.]+) sys/),
      rssMB: num(/(\d+)\s+maximum resident set size/) / 1e6,
      out: readFileSync(join(work, "out.txt"), "utf8").trim().split("\n").slice(-1)[0]?.slice(0, 200),
    };
    if (!best || m.wall < best.wall) best = m;
  }
  return best;
}

const tests = [];
const threadCounts = [1, 4, all];
const node = (script, ...a) => `node '${join(here, "tasks", script)}' ${a.map((x) => `'${x}'`).join(" ")}`;
const filter = '{"claim":"P17=Q55"}';

if (groups.includes("decode")) {
  for (const t of threadCounts) {
    tests.push(["decode", `wikidata info (${t} threads)`, `'${cli}' info '${head}' --allow-truncated --json --threads ${t}`]);
  }
  tests.push(["decode", `wikidata info, libbzip2 decoder (${all} threads)`, `WIKIDATA_BZ2_DECODER=libbzip2 '${cli}' info '${head}' --allow-truncated --json --threads ${all}`]);
  tests.push(["decode", "bzip2 -dc | node (count lines)", `bzip2 -dc '${head}' 2>/dev/null | ${node("baseline-filter.mjs", "count")}`]);
  if (which("lbzip2")) {
    tests.push(["decode", `lbzip2 -dc -n ${all} | node (count lines)`, `lbzip2 -dc -n ${all} '${head}' 2>/dev/null | ${node("baseline-filter.mjs", "count")}`]);
  }
  if (which("pbzip2")) {
    tests.push(["decode", `pbzip2 -dc | node (count lines)`, `pbzip2 -dc '${head}' 2>/dev/null | ${node("baseline-filter.mjs", "count")}`]);
  }
}
if (groups.includes("filter")) {
  for (const t of threadCounts) {
    tests.push(["filter", `wikidata cat --claim P17=Q55 (${t} threads)`, `'${cli}' cat '${head}' -q --allow-truncated --claim P17=Q55 --languages nl,en,mul --threads ${t} | wc -l`]);
  }
  for (const t of threadCounts) {
    tests.push(["filter", `@wikidata-rs/node, summary (${t} threads)`, node("node-entities.mjs", head, t, filter, "summary", "nl,en,mul")]);
  }
  tests.push(["filter", `@wikidata-rs/node, full entity (${all} threads)`, node("node-entities.mjs", head, all, filter, "entity", "nl,en,mul")]);
  tests.push(["filter", "bzip2 -dc | node (substring + JSON.parse)", `bzip2 -dc '${head}' 2>/dev/null | ${node("baseline-filter.mjs", "claim")}`]);
  if (which("lbzip2")) {
    tests.push(["filter", `lbzip2 -dc | node (substring + JSON.parse)`, `lbzip2 -dc -n ${all} '${head}' 2>/dev/null | ${node("baseline-filter.mjs", "claim")}`]);
  }
}
if (groups.includes("parse")) {
  for (const t of threadCounts) {
    tests.push(["parse", `wikidata cat, every entity (${t} threads)`, `'${cli}' cat '${plain}' -q --allow-truncated --languages nl,en,mul --threads ${t} | wc -c`]);
  }
  tests.push(["parse", `wikidata cat, every entity, all languages (${all} threads)`, `'${cli}' cat '${plain}' -q --allow-truncated --threads ${all} | wc -c`]);
  tests.push(["parse", `@wikidata-rs/node, summary (${all} threads)`, node("node-entities.mjs", plain, all, "-", "summary", "nl,en,mul")]);
  tests.push(["parse", `@wikidata-rs/node, full entity (${all} threads)`, node("node-entities.mjs", plain, all, "-", "entity", "nl,en,mul")]);
  tests.push(["parse", "node JSON.parse every line", `cat '${plain}' | ${node("baseline-filter.mjs", "parse")}`]);
}

if (groups.includes("small")) {
  for (const t of [1, all]) {
    tests.push(["small", `wikidata cat, every entity (${t} threads)`, `'${cli}' cat '${small}' -q --languages nl,en,mul --threads ${t} | wc -c`]);
  }
  tests.push(["small", `@wikidata-rs/node, summary (${all} threads)`, node("node-entities.mjs", small, all, "-", "summary", "nl,en,mul")]);
  tests.push(["small", `@wikidata-rs/node, full entity (${all} threads)`, node("node-entities.mjs", small, all, "-", "entity", "nl,en,mul")]);
  tests.push(["small", "node JSON.parse every line", `cat '${small}' | ${node("baseline-filter.mjs", "parse")}`]);
}

const results = [];
for (const [group, name, cmd] of tests) {
  process.stderr.write(`${group}: ${name} … `);
  const m = measure(cmd);
  process.stderr.write(`${m.wall.toFixed(2)} s\n`);
  results.push({ group, name, ...m });
}

const sizeSmall = groups.includes("small") ? readFileSync(small).length : 0;
const input = { decode: sizeIn, filter: sizeIn, parse: sizeOut, small: sizeSmall };
const json = { decode: 520.2e6, filter: 520.2e6, parse: sizeOut, small: sizeSmall };
const count = { decode: 10989, filter: 10989, parse: 10989, small: SMALL_N };
const lines = [];
const titles = {
  decode: `Decompress and count entities (${(sizeIn / 1e6).toFixed(0)} MB of bzip2, 10,989 entities)`,
  filter: "Entities with P17 = Q55 (country: the Netherlands), from the bzip2 head",
  parse: `Parse and normalise every entity (${(sizeOut / 1e6).toFixed(0)} MB of plain JSON)`,
  small: `Parse and normalise ${SMALL_N.toLocaleString("en")} small entities (${(sizeSmall / 1e6).toFixed(0)} MB of plain JSON, ~1.5 kB each)`,
};
for (const g of groups) {
  lines.push(`### ${titles[g]}`, "", "| Tool | Wall | CPU | MB/s in | JSON MB/s | Entities/s | Peak RSS |", "|---|---:|---:|---:|---:|---:|---:|");
  for (const r of results.filter((r) => r.group === g)) {
    lines.push(
      `| ${r.name} | ${r.wall.toFixed(2)} s | ${(r.user + r.sys).toFixed(1)} s | ${(input[g] / 1e6 / r.wall).toFixed(1)} | ${Math.round(json[g] / 1e6 / r.wall)} | ${Math.round(count[g] / r.wall).toLocaleString("en")} | ${Math.round(r.rssMB)} MB |`,
    );
  }
  lines.push("");
}
writeFileSync(join(here, "results.json"), JSON.stringify({ date: new Date().toISOString(), cpus: all, results }, null, 2));
console.log(lines.join("\n"));
