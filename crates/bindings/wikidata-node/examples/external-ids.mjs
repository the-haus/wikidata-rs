// Map Wikidata items to other databases: one JSON line per item that has
// any of the given identifiers (default: MusicBrainz artist, Discogs
// artist, Spotify artist), with their URLs.
//
//   node external-ids.mjs DUMP [musicbrainzArtist,discogsArtist,spotifyArtist]

import { EXTERNAL_IDS, externalIdUrl, openDump } from "@wikidata-rs/node";

const [input, keyList = "musicbrainzArtist,discogsArtist,spotifyArtist"] = process.argv.slice(2);
const keys = keyList.split(",");
const properties = keys.map((k) => {
  const row = EXTERNAL_IDS.find((r) => r.key === k);
  if (!row) throw new Error(`unknown key ${k}; known: ${EXTERNAL_IDS.map((r) => r.key).join(", ")}`);
  return row.property;
});
const dump = await openDump(input);
for await (const e of dump.entities({ filter: { hasProperty: properties }, languages: ["en", "mul"], json: false })) {
  const ids = {};
  for (const k of keys) {
    const v = e.externalIds[k];
    if (v) ids[k] = { id: v, url: externalIdUrl(k, v) };
  }
  console.log(JSON.stringify({ id: e.id, label: e.label?.text ?? null, ids }));
}
