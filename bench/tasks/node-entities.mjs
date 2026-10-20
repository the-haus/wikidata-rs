// @wikidata-rs/node: stream matching entities and touch their fields.
//
//   node node-entities.mjs DUMP THREADS FILTER_JSON|- [summary|entity] [languages]

import { openDump } from "@wikidata-rs/node";

const [input, threads, filter, mode = "summary", languages] = process.argv.slice(2);
const dump = await openDump(input);
const opts = { threads: Number(threads), allowTruncated: true };
if (filter && filter !== "-") opts.filter = JSON.parse(filter);
if (languages) opts.languages = languages.split(",");
if (mode === "summary") opts.json = false;
let n = 0;
let chars = 0;
const stream = dump.entities(opts);
for await (const batch of stream.batches()) {
  for (const e of batch) {
    n++;
    chars += e.id.length + (e.label?.text.length ?? 0) + e.instanceOf.length;
    if (mode === "entity") chars += Object.keys(e.entity.claims).length;
  }
}
const stats = await stream.stats;
console.log(JSON.stringify({ matched: n, entities: stats.entities, chars }));
