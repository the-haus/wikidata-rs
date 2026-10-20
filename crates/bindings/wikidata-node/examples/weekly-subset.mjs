// Keep a small subset of Wikidata on disk instead of the 100 GB dump:
// stream the latest dump (a path or URL) and write the entities a filter
// selects to a .jsonl.zst that reads back like the dump.
//
//   node weekly-subset.mjs DUMP OUT.jsonl.zst 'FILTER_JSON'
//   node weekly-subset.mjs https://dumps.wikimedia.org/wikidatawiki/entities/latest-all.json.bz2 \
//     nl.jsonl.zst '{"any":[{"claim":"P17=Q55"},{"hasProperty":"P434"}]}'

import { extract } from "@wikidata-rs/node";

const [input, output, filter] = process.argv.slice(2);
if (!input || !output) {
  console.error("usage: node weekly-subset.mjs DUMP OUT.jsonl.zst [FILTER_JSON]");
  process.exit(1);
}
let last = 0;
const stats = await extract(input, {
  output,
  ...(filter ? { filter: JSON.parse(filter) } : {}),
  onProgress: (p) => {
    if (process.stderr.isTTY && p.elapsedMs - last > 1000) {
      last = p.elapsedMs;
      const pct = p.fraction == null ? "" : ` ${(p.fraction * 100).toFixed(1)}%`;
      process.stderr.write(`\r${pct} ${p.entities} entities, ${p.matched} kept, ${p.mbPerSecondIn.toFixed(1)} MB/s`);
    }
  },
});
if (process.stderr.isTTY) process.stderr.write("\n");
console.log(JSON.stringify({ matched: stats.matched, entities: stats.entities, bytesWritten: stats.bytesWritten }));
