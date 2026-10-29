import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";

import { beforeAll, describe, expect, it } from "vitest";

import { ClassIndex, extract, fileInfo, openDump, streamEntities, type Filter } from "../ts/index.js";
import { bigDump, sample, sampleIds, tmp } from "./helpers.js";

describe("named filters", () => {
  it("report every named branch that matched", async () => {
    const filter: Filter = {
      any: {
        country: { instanceOf: "Q6256" },
        dutch: { claim: "P17=Q55" },
        artist: { hasProperty: "P434" },
      },
    };
    const got = new Map<string, string[]>();
    for await (const e of streamEntities(sample, { filter, json: false })) got.set(e.id, e.matchedBy);
    expect(got.get("Q55")).toEqual(["country", "dutch"]);
    expect(got.get("Q31")).toEqual(["country"]);
    // In the order the filter names them.
    expect(got.get("Q1072")).toEqual(["dutch", "artist"]);
    expect(got.get("Q42")).toEqual(["artist"]);
    // Also when the kept properties leave out what the branches read.
    const pruned = await streamEntities(sample, { filter, properties: ["P1"] }).toArray();
    expect(pruned.find((e) => e.id === "Q55")!.matchedBy).toEqual(["country", "dutch"]);
    expect(pruned.find((e) => e.id === "Q55")!.toJSON().matchedBy).toEqual(["country", "dutch"]);
  });

  it("take a name on any filter, and leave unnamed filters as they were", async () => {
    const named = await streamEntities(sample, {
      filter: { any: [{ name: "nl", ids: "Q55" }, { ids: ["Q42", "Q55"] }] },
    }).toArray();
    expect(named.map((e) => [e.id, e.matchedBy])).toEqual([
      ["Q55", ["nl"]],
      ["Q42", []],
    ]);
    const plain = await streamEntities(sample, { filter: { any: [{ ids: "Q55" }, { ids: "Q42" }] } }).toArray();
    expect(plain.map((e) => e.matchedBy)).toEqual([[], []]);
  });
});

describe("ids as item numbers", () => {
  it("match the same entities as the string form", async () => {
    const items = sampleIds().filter((id) => id.startsWith("Q"));
    const numbers = Uint32Array.from(items.slice(0, 7), (id) => Number(id.slice(1)));
    const strings = items.slice(0, 7);
    const want = await streamEntities(sample, { filter: { ids: strings }, json: false }).toArray();
    for (const ids of [numbers, Int32Array.from(numbers), Float64Array.from(numbers), BigUint64Array.from(numbers, BigInt)]) {
      const got = await streamEntities(sample, { filter: { any: [{ type: "lexeme" }, { ids }] }, json: false }).toArray();
      expect(got.map((e) => e.id)).toEqual(sampleIds().filter((id) => strings.includes(id) || id.startsWith("L")));
    }
    expect(want.map((e) => e.id)).toEqual(strings);
  });

  it("reject what is not an item number", async () => {
    await expect(streamEntities(sample, { filter: { ids: Int32Array.of(-1) } }).toArray()).rejects.toThrow(/item number/);
    await expect(streamEntities(sample, { filter: { ids: Float64Array.of(1.5) } }).toArray()).rejects.toThrow(/item number/);
    await expect(
      streamEntities(sample, { filter: { hasProperty: Uint32Array.of(31) as unknown as string } }).toArray(),
    ).rejects.toThrow(/only accepted for ids/);
  });

  it("cost little heap for a million ids", () => {
    const dir = tmp();
    const big = bigDump(dir, 2000);
    const entry = resolve(__dirname, "..", "dist", "index.js");
    const script = `
      const { openDump } = require(${JSON.stringify(entry)});
      (async () => {
        const n = 1_000_000;
        const numbers = new Uint32Array(n);
        for (let i = 0; i < n; i++) numbers[i] = 10_000_000 + i * 7;
        numbers.set([55, 42, 1072], 0);
        const dump = await openDump(${JSON.stringify(big)});
        async function run(ids) {
          gc();
          const before = process.memoryUsage().heapUsed;
          const stream = dump.entities({ filter: { ids }, json: false });
          const it = stream[Symbol.asyncIterator]();
          const first = await it.next();
          const growth = process.memoryUsage().heapUsed - before;
          const seen = [first.value.id];
          for (let r = await it.next(); !r.done; r = await it.next()) seen.push(r.value.id);
          const stats = await stream.stats;
          gc();
          // Holding the stats, with nearly a million missing ids.
          const held = process.memoryUsage().heapUsed - before;
          return { growth, held, seen, missing: stats.missingItemNumbers.length };
        }
        const typed = await run(numbers);
        const strings = await run(Array.from(numbers, (q) => "Q" + q));
        console.log(JSON.stringify({ typed: typed.growth, held: typed.held, missing: typed.missing, strings: strings.growth, same: JSON.stringify(typed.seen) === JSON.stringify(strings.seen), n: typed.seen.length }));
      })();
    `;
    const r = spawnSync(process.execPath, ["--expose-gc", "-e", script], { encoding: "utf8", timeout: 60_000 });
    expect(r.stderr).toBe("");
    const out = JSON.parse(r.stdout) as { typed: number; held: number; missing: number; strings: number; same: boolean; n: number };
    expect(out.missing).toBe(1_000_000 - 3);
    expect(out.held).toBeLessThan(5e6);
    expect(out.same).toBe(true);
    const ids = sampleIds();
    const want = Array.from({ length: 2000 }, (_, i) => ids[i % ids.length]!).filter((id) => ["Q55", "Q42", "Q1072"].includes(id));
    expect(out.n).toBe(want.length);
    expect(out.typed).toBeLessThan(5e6);
    // The measurement sees the string form's cost.
    expect(out.strings).toBeGreaterThan(5e6);
  });
});

describe("missing ids", () => {
  it("are reported after a complete pass", async () => {
    const stream = streamEntities(sample, { filter: { ids: ["Q1", "Q999999999"] } });
    expect(await stream.toArray()).toEqual([]);
    expect((await stream.stats).missingIds).toEqual(["Q1", "Q999999999"]);
    expect((await stream.stats).missingItemNumbers).toEqual(Uint32Array.of(1, 999999999));
    const others = streamEntities(sample, { filter: { ids: ["P31", "P404", "L404", "Q404"] } });
    await others.toArray();
    expect((await others.stats).missingIds).toEqual(["Q404", "P404", "L404"]);
    expect(Object.keys(await others.stats)).toContain("missingIds");
    const mixed = streamEntities(sample, { filter: { any: [{ type: "lexeme" }, { ids: Uint32Array.of(55, 999999999) }] } });
    await mixed.toArray();
    expect((await mixed.stats).missingIds).toEqual(["Q999999999"]);
    expect((await fileInfo(sample, { filter: { ids: ["Q31", "Q404"] } })).missingIds).toEqual(["Q404"]);
  });

  it("are not reported for a pass cut short", async () => {
    const limited = streamEntities(sample, { filter: { ids: ["Q31", "Q55", "Q999999999"] }, limit: 1 });
    await limited.toArray();
    expect((await limited.stats).stopped).toBe(true);
    expect((await limited.stats).missingIds).toBeUndefined();
    // Q31 is every 41st of 20,500 entities: closing after the first one
    // stops the pass while the producer waits for credits.
    const closed = streamEntities(bigDump(tmp(), 20_500), { filter: { ids: ["Q31", "Q999999999"] }, batchSize: 1 });
    for await (const e of closed) {
      void e;
      break;
    }
    expect((await closed.stats).stopped).toBe(true);
    expect((await closed.stats).missingIds).toBeUndefined();
    const plain = streamEntities(sample, { filter: { type: "item" } });
    await plain.toArray();
    expect((await plain.stats).missingIds).toBeUndefined();
  });
});

describe("languages and sitelinks by claim", () => {
  it("keep each entity's own languages", async () => {
    const stream = streamEntities(sample, {
      filter: { ids: ["Q31", "Q55", "Q42"] },
      properties: ["P31"],
      languages: { by: "P17", map: { Q55: ["nl"], Q31: ["fr", "de"] }, always: ["en", "mul"] },
      sitelinks: { by: ["P17"], map: { Q55: ["nlwiki"] }, always: ["enwiki"] },
    });
    const got = new Map((await stream.toArray()).map((e) => [e.id, e]));
    const nl = got.get("Q55")!;
    expect(Object.keys(nl.entity.labels)).toEqual(["nl", "en", "mul"]);
    expect(nl.label?.language).toBe("nl");
    expect(Object.keys(nl.entity.sitelinks)).toEqual(["nlwiki", "enwiki"]);
    expect(Object.keys(nl.entity.claims)).toEqual(["P31"]);
    const be = got.get("Q31")!;
    expect(Object.keys(be.entity.labels)).toEqual(["fr", "de", "en", "mul"]);
    expect(be.label?.language).toBe("fr");
    expect(Object.keys(be.entity.sitelinks)).toEqual(["enwiki"]);
    expect(Object.keys(got.get("Q42")!.entity.labels)).toEqual(["en"]);
  });

  it("reject malformed rules", async () => {
    await expect(
      streamEntities(sample, { languages: { by: "Q17", map: {} } }).toArray(),
    ).rejects.toThrow(/not a property id/);
    await expect(
      streamEntities(sample, { languages: { map: {} } as never }).toArray(),
    ).rejects.toThrow(/by/);
    await expect(
      streamEntities(sample, { languages: { by: "P17", map: {}, allFor: { instanceOf: "Q5" } } }).toArray(),
    ).rejects.toThrow(/only a sitelinks rule/);
    await expect(
      streamEntities(sample, { sitelinks: { allFor: { instanceOf: "Q5", group: "wikipedias" as never } } }).toArray(),
    ).rejects.toThrow(/unknown site group "wikipedias"/);
    await expect(
      streamEntities(sample, { sitelinks: { allFor: { instanceOf: "P31" } } }).toArray(),
    ).rejects.toThrow(/not an item id/);
  });
});

// Festimap's W-20 and W-21.
describe("sitelink counts and every Wikipedia for some classes", () => {
  it("count the pruned sitelinks, on the record and the entity", async () => {
    const stream = streamEntities(sample, {
      filter: { type: "item" },
      sitelinks: ["enwiki"],
      sitelinkCounts: true,
    });
    const got = new Map((await stream.toArray()).map((e) => [e.id, e]));
    const be = got.get("Q31")!;
    // nlwiki, enwiki, commonswiki, zh_yuewiki.
    expect([be.sitelinkCount, be.wikipediaCount]).toEqual([4, 3]);
    expect([be.entity.sitelinkCount, be.entity.wikipediaCount]).toEqual([4, 3]);
    expect(Object.keys(be.entity.sitelinks)).toEqual(["enwiki"]);
    expect(be.toJSON()).toMatchObject({ sitelinkCount: 4, wikipediaCount: 3 });
    expect([got.get("Q42")!.sitelinkCount, got.get("Q42")!.wikipediaCount]).toEqual([2, 2]);
    expect([got.get("Q5001")!.sitelinkCount, got.get("Q5001")!.wikipediaCount]).toEqual([0, 0]);
    // Without the JSON too.
    const summary = await streamEntities(sample, {
      filter: { ids: ["Q31"] },
      sitelinks: [],
      sitelinkCounts: true,
      json: false,
    }).toArray();
    expect([summary[0]!.sitelinkCount, summary[0]!.wikipediaCount]).toEqual([4, 3]);
    // Off by default.
    const [plain] = await streamEntities(sample, { filter: { ids: ["Q31"] } }).toArray();
    expect([plain!.sitelinkCount, plain!.wikipediaCount]).toEqual([null, null]);
    expect(plain!.entity.sitelinkCount).toBeUndefined();
  });

  it("keep every Wikipedia of the listed classes only", async () => {
    const stream = streamEntities(sample, {
      filter: { ids: ["Q31", "Q55", "Q42"] },
      properties: ["P1"],
      sitelinks: {
        by: "P17",
        map: { Q55: ["nlwiki"] },
        always: ["enwiki"],
        allFor: { instanceOf: ["Q5", "Q215380"], group: "wikipedia" },
      },
      sitelinkCounts: true,
    });
    const got = new Map((await stream.toArray()).map((e) => [e.id, e]));
    // A human: every Wikipedia; countries: what their P17 chooses.
    expect(Object.keys(got.get("Q42")!.entity.sitelinks)).toEqual(["enwiki", "nlwiki"]);
    expect(got.get("Q42")!.entity.sitelinks.nlwiki!.title).toBeTruthy();
    expect(Object.keys(got.get("Q55")!.entity.sitelinks)).toEqual(["nlwiki", "enwiki"]);
    expect(Object.keys(got.get("Q31")!.entity.sitelinks)).toEqual(["enwiki"]);
    expect(got.get("Q31")!.wikipediaCount).toBe(3);
    // P31 and P17 are read for the rule, not kept.
    expect(got.get("Q42")!.entity.claims).toEqual({});
    // Without `by`: `always` for everyone, and the classes' sites.
    const bare = await streamEntities(sample, {
      filter: { ids: ["Q31", "Q42"] },
      sitelinks: { always: ["enwiki"], allFor: { instanceOf: "Q5" } },
    }).toArray();
    expect(bare.map((e) => [e.id, Object.keys(e.entity.sitelinks)])).toEqual([
      ["Q31", ["enwiki"]],
      ["Q42", ["enwiki", "nlwiki"]],
    ]);
  });
});

describe("the class index built during another pass", () => {
  let alone: string;
  let dir: string;
  beforeAll(async () => {
    dir = tmp();
    alone = join(dir, "alone.wdc");
    (await ClassIndex.build(sample)).save(alone);
  });

  it("is identical to ClassIndex.build", async () => {
    const out = join(dir, "classes.wdc");
    const stream = streamEntities(sample, { filter: { claim: "P17=Q55" }, buildClassIndex: out });
    expect(await stream.toArray()).toHaveLength(9);
    expect((await stream.stats).classIndex).toEqual({ path: out, classes: 3, edges: 3 });
    expect(readFileSync(out)).toEqual(readFileSync(alone));
    const loaded = await ClassIndex.load(out);
    const subs = await streamEntities(sample, {
      filter: { instanceOf: { classes: ["Q868557"], subclasses: true } },
      classIndex: loaded,
    }).toArray();
    expect(subs.map((e) => e.id)).toEqual(["Q1072", "Q1073"]);
  });

  it("comes from extract and fileInfo too, and not from a stopped pass", async () => {
    const x = join(dir, "extract.wdc");
    const stats = await extract(sample, { output: join(dir, "x.jsonl.zst"), filter: { type: "lexeme" }, buildClassIndex: x });
    expect(stats.classIndex?.edges).toBe(3);
    expect(readFileSync(x)).toEqual(readFileSync(alone));
    const info = await fileInfo(sample, { buildClassIndex: join(dir, "info.wdc") });
    expect(info.classIndex?.classes).toBe(3);
    const stopped = join(dir, "stopped.wdc");
    const limited = streamEntities(sample, { limit: 2, buildClassIndex: stopped });
    await limited.toArray();
    expect((await limited.stats).classIndex).toBeUndefined();
    expect(existsSync(stopped)).toBe(false);
  });
});

it("DumpReader methods take the same new options", async () => {
  const dump = await openDump(sample);
  const s = dump.entities({ filter: { any: { a: { ids: "Q55" } } } });
  expect((await s.toArray()).map((e) => e.matchedBy)).toEqual([["a"]]);
});
