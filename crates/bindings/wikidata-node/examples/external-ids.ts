// Map Wikidata items to other databases: one JSON line per item that has
// any of the given identifiers (default: MusicBrainz artist, Discogs
// artist, Spotify artist), with their URLs.
//
//   npx tsx external-ids.ts DUMP [musicbrainzArtist,discogsArtist,spotifyArtist]

import { EXTERNAL_IDS, externalIdUrl, openDump, type ExternalIdKey } from "@wikidata-rs/node";

const [input, keyList = "musicbrainzArtist,discogsArtist,spotifyArtist"] = process.argv.slice(2);
if (!input) throw new Error("usage: npx tsx external-ids.ts DUMP [keys]");
const rows = keyList.split(",").map((k) => {
  const row = EXTERNAL_IDS.find((r) => r.key === k);
  if (!row) throw new Error(`unknown key ${k}; known: ${EXTERNAL_IDS.map((r) => r.key).join(", ")}`);
  return row;
});
const keys: ExternalIdKey[] = rows.map((r) => r.key);
const properties = rows.map((r) => r.property);
const dump = await openDump(input);
for await (const e of dump.entities({ filter: { hasProperty: properties }, languages: ["en", "mul"], json: false })) {
  const ids: Partial<Record<ExternalIdKey, { id: string; url: string | null }>> = {};
  for (const k of keys) {
    const v = e.externalIds[k];
    if (v) ids[k] = { id: v, url: externalIdUrl(k, v) };
  }
  console.log(JSON.stringify({ id: e.id, label: e.label?.text ?? null, ids }));
}
