import { execFileSync } from "node:child_process";
import { readFileSync, statSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { extract, fileInfo, openDump, type Progress } from "../ts/index.js";
import { bigDump, hasBzip2, sample, sampleLines, tmp } from "./helpers.js";

async function ids(path: string): Promise<string[]> {
  const out: string[] = [];
  for await (const e of (await openDump(path)).entities({ json: false })) out.push(e.id);
  return out;
}

describe("extract", () => {
  it("writes subsets that read back like the dump", async () => {
    const dir = tmp();
    const filter = { any: [{ claim: "P17=Q55" }, { type: "lexeme" as const }] };
    const want = ["Q55", "Q1072", "Q1074", "L7", "Q5000", "Q5004", "Q5008", "Q5012", "Q5016", "Q5020", "L8"];
    for (const [name, format, compression] of [
      ["subset.jsonl.zst", "jsonLines", "zstd"],
      ["subset.json.bz2", "jsonArray", "bzip2"],
      ["subset.json.gz", "jsonArray", "gzip"],
      ["subset.json", "jsonArray", "none"],
    ] as const) {
      const out = join(dir, name);
      const stats = await extract(sample, { output: out, filter });
      expect(stats.matched).toBe(11);
      expect(stats.entities).toBe(41);
      expect(stats.format).toBe(format);
      expect(stats.outputCompression).toBe(compression);
      expect(stats.bytesWritten).toBe(statSync(out).size);
      expect(await ids(out)).toEqual(want);
      const back = await openDump(out);
      expect(back.compression).toBe(compression);
    }
    // The plain array is the dump's own layout, with the original lines.
    const text = readFileSync(join(dir, "subset.json"), "utf8");
    const lines = sampleLines();
    expect(text.startsWith("[\n")).toBe(true);
    expect(text.endsWith("\n]\n")).toBe(true);
    expect(text.split("\n")[1]).toBe(lines[1] + ",");
    if (hasBzip2) {
      const plain = execFileSync("bzip2", ["-dc", join(dir, "subset.json.bz2")], { encoding: "utf8" });
      expect(plain).toBe(text);
    }
  });

  it("reports progress and stops on abort", async () => {
    const dir = tmp();
    const big = bigDump(dir, 41 * 400);
    const progress: Progress[] = [];
    const stats = await extract(big, { output: join(dir, "all.jsonl.zst"), onProgress: (p) => progress.push(p) });
    expect(stats.matched).toBe(41 * 400);
    expect(progress.at(-1)!.matched).toBe(41 * 400);
    const ac = new AbortController();
    ac.abort(new Error("no thanks"));
    await expect(extract(big, { output: join(dir, "never.jsonl.zst"), signal: ac.signal })).rejects.toThrow("no thanks");
    await expect(extract(big, { output: join(dir, "x.jsonl"), filter: { hasProperty: "nope" } })).rejects.toThrow();
  });
});

describe("fileInfo", () => {
  it("counts what a dump holds", async () => {
    const info = await fileInfo(sample);
    expect(info.entities).toBe(41);
    expect(info.types).toEqual({ items: 37, properties: 2, lexemes: 2, mediainfo: 0, entitySchemas: 0 });
    expect(info.maxIds.item).toBe(99000001);
    expect(info.largest.id).toBe("Q31");
    expect(info.size).toBe(statSync(sample).size);
    expect(info.compression).toBe("none");
    const limited = await fileInfo(sample, { limit: 5 });
    expect(limited.matched).toBe(5);
    expect(limited.stopped).toBe(true);
  });
});
