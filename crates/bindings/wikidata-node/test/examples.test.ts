import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

import { beforeAll, describe, expect, it } from "vitest";

import { openDump } from "../ts/index.js";
import { sample } from "./helpers.js";

const root = resolve(__dirname, "..");
// The examples are TypeScript, run as `npx tsx examples/places.ts` runs them.
const tsx = pathToFileURL(require.resolve("tsx")).href;
let work: string;

/** Run an example with `@wikidata-rs/node` resolving to this package. */
function run(example: string, args: string[]) {
  const r = spawnSync(process.execPath, ["--import", tsx, join(work, "examples", example), ...args], {
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
    const lines = run("music-festivals.ts", [sample, "Q55", index]).stdout.trim().split("\n");
    expect(lines[0]).toBe("id,label,lat,lon,inception,website,musicbrainz");
    expect(lines.slice(1)).toEqual([
      "Q1072,Pinkpop,50.9,6.03,1970,https://www.pinkpop.nl/,e4b4b7e5-5c2c-4f8a-a8a5-0b2b0b6a8b2e",
    ]);
    // Again, with the saved index.
    expect(run("music-festivals.ts", [sample, "Q31", index]).stdout).toContain("Q1073,Rock Werchter");
  });

  it("weekly-subset", async () => {
    const out = join(work, "nl.jsonl.zst");
    const r = JSON.parse(run("weekly-subset.ts", [sample, out, '{"claim":"P17=Q55"}']).stdout);
    expect(r.matched).toBe(9);
    const ids: string[] = [];
    for await (const e of (await openDump(out)).entities({ json: false })) ids.push(e.id);
    expect(ids).toHaveLength(9);
  });

  it("external-ids", () => {
    const lines = run("external-ids.ts", [sample]).stdout.trim().split("\n").map((l) => JSON.parse(l));
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
    const info = JSON.parse(run("file-info.ts", [sample]).stdout);
    expect(info.entities).toBe(41);
  });

  it("places", () => {
    const features = run("places.ts", [sample]).stdout.trim().split("\n").map((l) => JSON.parse(l));
    expect(features.map((f) => f.id)).toEqual(["Q55", "Q1072"]);
    expect(features[1].geometry.coordinates).toEqual([6.03, 50.9]);
  });
  it("weekly-refresh", () => {
    const index = join(work, "weekly-classes.wdc");
    const known = join(work, "known.txt");
    writeFileSync(known, "Q55\nQ999999999\n");
    const records = (out: string) =>
      new Map(out.trim().split("\n").map((l) => {
        const r = JSON.parse(l) as { id: string; language: string | null; as: string[] };
        return [r.id, r] as const;
      }));
    // First run: no index yet, so it is built during the pass.
    const first = run("weekly-refresh.ts", [sample, index, known]);
    expect(JSON.parse(first.stderr)).toEqual({ matched: 9, missing: ["Q999999999"], classIndex: `built ${index}` });
    expect(records(first.stdout).get("Q1072")!.as).toEqual(["artist"]);
    expect(records(first.stdout).get("Q55")).toEqual({ id: "Q55", label: "Nederland", language: "nl", as: ["known"] });
    // Second run: subclasses of music festival through the saved index.
    const second = run("weekly-refresh.ts", [sample, index, known]);
    expect(JSON.parse(second.stderr).classIndex).toBe("loaded");
    expect(records(second.stdout).get("Q1072")!.as).toEqual(["artist", "festival"]);
    expect(records(second.stdout).get("Q42")!.language).toBe("en");
  });
});
