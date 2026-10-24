// Refresh the items an app already has: only those that changed since its
// copy reach JS, merges and deletions are told apart, and a stopped pass
// resumes where it was. Works on the weekly dump and on the daily
// incremental dumps (wikidatawiki-<date>-pages-meta-hist-incr.xml.bz2).
//
//   npx tsx refresh-known.ts DUMP known.json [redirects.wdr] [checkpoint.json]
//
// known.json maps each item to the revision the app has: {"Q42": 2000000001}.
// redirects.wdr comes from `wikidata redirects --redirect-sql … --page-sql …`
// (or RedirectIndex.build). With checkpoint.json, the pass saves where it
// is every ~900 MB of JSON and continues from there when run again.

import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";

import { RedirectIndex, streamEntities, type Checkpoint } from "@wikidata-rs/node";

const [input, knownFile, redirectsFile, checkpointFile] = process.argv.slice(2);
if (!input || !knownFile) {
  console.error("usage: npx tsx refresh-known.ts DUMP known.json [redirects.wdr] [checkpoint.json]");
  process.exit(1);
}
const known = new Map(Object.entries(JSON.parse(readFileSync(knownFile, "utf8")) as Record<string, number>));
const resumeFrom =
  checkpointFile && existsSync(checkpointFile) ? (JSON.parse(readFileSync(checkpointFile, "utf8")) as Checkpoint) : undefined;

const stream = streamEntities(input, {
  // Entities no newer than the known revision are skipped before parsing.
  filter: { ids: known },
  ...(redirectsFile ? { redirects: await RedirectIndex.load(redirectsFile) } : {}),
  ...(checkpointFile
    ? {
        checkpointEvery: 1000,
        onCheckpoint: (cp: Checkpoint) => writeFileSync(checkpointFile, JSON.stringify(cp)),
        ...(resumeFrom ? { resumeFrom } : {}),
      }
    : {}),
  languages: ["en"],
  userAgent: "wikidata-rs refresh-known example (https://github.com/the-haus/wikidata-rs)",
});
for await (const e of stream) {
  console.log(JSON.stringify({ id: e.id, revision: e.entity.lastrevid, label: e.label?.text ?? null, mergedFrom: e.mergedFrom }));
}
const stats = await stream.stats;
if (checkpointFile) rmSync(checkpointFile, { force: true });
console.error(
  JSON.stringify({
    changed: stats.matched,
    unchanged: stats.unchanged,
    merged: stats.merged ?? [],
    deleted: stats.deleted ?? [],
    newer: stats.newer ?? [],
    // Incremental dumps: the day's merges.
    mergedToday: stats.redirects ?? [],
  }),
);
