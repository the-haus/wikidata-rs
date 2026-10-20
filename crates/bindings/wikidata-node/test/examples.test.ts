import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { beforeAll, describe, expect, it } from "vitest";

import { openDump } from "../ts/index.js";
import { sample } from "./helpers.js";

const root = resolve(__dirname, "..");
let work: string;

/** Run an example with `@wikidata-rs/node` resolving to this package. */
function run(example: string, args: string[]) {
  const r = spawnSync(process.execPath, [join(work, "examples", example), ...args], {
    encoding: "utf8",
    cwd: work,
    timeout: 60_000,
  });
  if (r.status !== 0) throw new Error(`${example} failed: ${r.stderr}`);
  return r;
}

beforeAll(() => {
  work = mkdtempSync(join(tmpdir(), "wikidata-node-examples-"));
  mkdirSync(join(work, "node_modules", "@wikidata-rs"), { recursive: true });
  symlinkSync(root, join(work, "node_modules", "@wikidata-rs", "node"), "junction");
  symlinkSync(join(root, "examples"), join(work, "examples"), "junction");
});

describe("examples", () => {
  it("music-festivals", () => {
    const index = join(work, "classes.wdc");
    const lines = run("music-festivals.mjs", [sample, "Q55", index]).stdout.trim().split("\n");
    expect(lines[0]).toBe("id,label,lat,lon,inception,website,musicbrainz");
    expect(lines.slice(1)).toEqual([
      "Q1072,Pinkpop,50.9,6.03,1970,https://www.pinkpop.nl/,e4b4b7e5-5c2c-4f8a-a8a5-0b2b0b6a8b2e",
    ]);
    // Again, with the saved index.
    expect(run("music-festivals.mjs", [sample, "Q31", index]).stdout).toContain("Q1073,Rock Werchter");
  });

  it("weekly-subset", async () => {
    const out = join(work, "nl.jsonl.zst");
    const r = JSON.parse(run("weekly-subset.mjs", [sample, out, '{"claim":"P17=Q55"}']).stdout);
    expect(r.matched).toBe(9);
    const ids: string[] = [];
    for await (const e of (await openDump(out)).entities({ json: false })) ids.push(e.id);
    expect(ids).toHaveLength(9);
  });

  it("external-ids", () => {
    const lines = run("external-ids.mjs", [sample]).stdout.trim().split("\n").map((l) => JSON.parse(l));
    expect(lines[0]).toEqual({
      id: "Q42",
      label: "Douglas Adams",
      ids: {
        musicbrainzArtist: {
          id: "e9ed318d-8cc5-4cf8-ab77-505e39ab6ea4",
          url: "https://musicbrainz.org/artist/e9ed318d-8cc5-4cf8-ab77-505e39ab6ea4",
        },
        discogsArtist: { id: "66941", url: "https://www.discogs.com/artist/66941" },
      },
    });
  });

  it("file-info", () => {
    const info = JSON.parse(run("file-info.mjs", [sample]).stdout);
    expect(info.entities).toBe(41);
  });

  it("places", () => {
    const features = run("places.mjs", [sample]).stdout.trim().split("\n").map((l) => JSON.parse(l));
    expect(features.map((f) => f.id)).toEqual(["Q55", "Q1072"]);
    expect(features[1].geometry.coordinates).toEqual([6.03, 50.9]);
  });
});
