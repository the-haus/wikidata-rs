// Places with coordinates in a country, as GeoJSON lines (P625 + P17),
// labelled in Dutch, falling back to English.
//
//   node places.mjs DUMP [COUNTRY=Q55] > places.geojsonseq

import { openDump } from "@wikidata-rs/node";

const [input, country = "Q55"] = process.argv.slice(2);
const dump = await openDump(input);
for await (const e of dump.entities({
  filter: [{ claim: `P17=${country}` }, { hasProperty: "P625" }],
  languages: ["nl", "en", "mul"],
  json: false,
})) {
  const c = e.coordinates;
  if (!c) continue;
  console.log(
    JSON.stringify({
      type: "Feature",
      id: e.id,
      geometry: { type: "Point", coordinates: [c.lon, c.lat] },
      properties: { name: e.label?.text ?? null, instanceOf: e.instanceOf, inception: e.inception },
    }),
  );
}
