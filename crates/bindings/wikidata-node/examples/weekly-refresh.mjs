// One pass over a new dump for a music app: artists, festivals and the
// items it already knows, each record saying which of them it is, labels
// in the item's own country's language, the ids the dump no longer has,
// and, on the first run, the class index for the next one.
//
//   node weekly-refresh.mjs DUMP classes.wdc [known-ids.txt]
//
// The first run (no classes.wdc yet) matches festivals by their direct
// class and builds classes.wdc as it goes; later runs load it and match
// every subclass of music festival too.

import { existsSync, readFileSync } from "node:fs";

import { ClassIndex, streamEntities } from "@wikidata-rs/node";

const [input, indexFile, knownFile] = process.argv.slice(2);
if (!input || !indexFile) {
  console.error("usage: node weekly-refresh.mjs DUMP classes.wdc [known-ids.txt]");
  process.exit(1);
}
const haveIndex = existsSync(indexFile);
const known = Uint32Array.from(knownFile ? (readFileSync(knownFile, "utf8").match(/Q\d+/g) ?? []) : [], (q) =>
  Number(q.slice(1)),
);

const branches = {
  artist: { hasProperty: "P434" },
  festival: haveIndex ? { instanceOf: { classes: ["Q868557"], subclasses: true } } : { instanceOf: ["Q868557"] },
};
if (known.length > 0) branches.known = { ids: known };

const stream = streamEntities(input, {
  filter: { any: branches },
  ...(haveIndex ? { classIndex: await ClassIndex.load(indexFile) } : { buildClassIndex: indexFile }),
  languages: { by: "P17", map: { Q55: ["nl"], Q31: ["nl", "fr"], Q183: ["de"], Q142: ["fr"] }, always: ["en", "mul"] },
  json: false,
  // Wikimedia asks for a descriptive User-Agent when reading its dumps by URL.
  userAgent: "wikidata-rs weekly-refresh example (https://github.com/the-haus/wikidata-rs)",
});
for await (const e of stream) {
  console.log(JSON.stringify({ id: e.id, label: e.label?.text ?? null, language: e.label?.language ?? null, as: e.matchedBy }));
}
const stats = await stream.stats;
console.error(
  JSON.stringify({
    matched: stats.matched,
    missing: stats.missingIds ?? [],
    classIndex: stats.classIndex ? `built ${stats.classIndex.path}` : "loaded",
  }),
);
