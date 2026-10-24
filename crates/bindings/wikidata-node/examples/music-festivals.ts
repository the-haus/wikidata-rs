// Music festivals in a country, with every subclass of "music festival"
// (Q868557): build the class index once (or load a saved one), then
// stream the instances as CSV.
//
//   npx tsx music-festivals.ts DUMP [COUNTRY=Q55] [classes.wdc]

import { existsSync } from "node:fs";

import { ClassIndex, openDump } from "@wikidata-rs/node";

const [input, country = "Q55", indexFile] = process.argv.slice(2);
if (!input) {
  console.error("usage: npx tsx music-festivals.ts DUMP [COUNTRY] [classes.wdc]");
  process.exit(1);
}
const dump = await openDump(input);
let classIndex: ClassIndex;
if (indexFile && existsSync(indexFile)) {
  classIndex = await ClassIndex.load(indexFile);
} else {
  classIndex = await dump.classIndex();
  if (indexFile) classIndex.save(indexFile);
}
const csv = (v: unknown): string => (v == null ? "" : /[",\n]/.test(String(v)) ? `"${String(v).replace(/"/g, '""')}"` : String(v));
console.log("id,label,lat,lon,inception,website,musicbrainz");
for await (const e of dump.entities({
  filter: [{ instanceOf: { classes: ["Q868557"], subclasses: true } }, { claim: `P17=${country}` }],
  classIndex,
  languages: ["nl", "en", "mul"],
  json: false,
})) {
  const c = e.coordinates;
  console.log(
    [e.id, e.label?.text, c?.lat, c?.lon, e.inception, e.website, e.externalIds.musicbrainzArtist].map(csv).join(","),
  );
}
