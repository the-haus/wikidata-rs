// What a weekly refresh uses: known revisions, checkpoints, merges,
// labels of referenced items, qualifiers by property.

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { extract, PlaceIndex, readRedirects, RedirectIndex, streamEntities, type Checkpoint } from "../ts/index.js";
import { bigDump, fixture, sample, sampleRevisions, tmp } from "./helpers.js";

describe("revisions", () => {
  it("skip entities that did not change, which still count as found", async () => {
    const { ids, revisions } = sampleRevisions();
    const s = streamEntities(sample, { filter: { ids, revisions }, json: false });
    expect(await s.toArray()).toEqual([]);
    const stats = await s.stats;
    expect(stats.missingIds).toEqual([]);
    expect(stats.unchanged).toBe(ids.length);
    expect(stats.parsed).toBe(0);

    // One revision behind: only that entity.
    const older = Float64Array.from(revisions);
    older[ids.indexOf("Q42")]! -= 1;
    const got = await streamEntities(sample, { filter: { ids, revisions: older }, json: false }).toArray();
    expect(got.map((e) => e.id)).toEqual(["Q42"]);

    // Item numbers with BigInt revisions, and the map form.
    const items = ids.filter((id) => id.startsWith("Q"));
    const numbers = Uint32Array.from(items, (id) => Number(id.slice(1)));
    const big = BigUint64Array.from(items, (id) => BigInt(revisions[ids.indexOf(id)]!));
    const none = streamEntities(sample, { filter: { ids: numbers, revisions: big } });
    expect(await none.toArray()).toEqual([]);
    const map = new Map([
      ["Q42", revisions[ids.indexOf("Q42")]!],
      ["Q31", 1],
    ]);
    const changed = await streamEntities(sample, { filter: { ids: map }, json: false }).toArray();
    expect(changed.map((e) => e.id)).toEqual(["Q31"]);
    await expect(streamEntities(sample, { filter: { ids: ["Q1"], revisions: [1, 2] } }).toArray()).rejects.toThrow(
      /revisions/,
    );
  });
});

describe("checkpoints", () => {
  it("resume a stopped pass with exactly the records after the last one consumed", async () => {
    const dir = tmp();
    const plain = bigDump(dir, 20_000);
    const bz2 = join(dir, "big.json.bz2");
    await extract(plain, { output: bz2, level: 1 });
    const filter = { any: [{ claim: "P17=Q55" }, { ids: ["Q42", "Q999"] }] };
    for (const input of [plain, bz2]) {
      const all: string[] = [];
      const full = streamEntities(input, { filter, json: false });
      for await (const e of full) all.push(e.id);
      expect((await full.stats).missingIds).toEqual(["Q999"]);

      const head: string[] = [];
      let last: Checkpoint | undefined;
      const stream = streamEntities(input, {
        filter,
        json: false,
        batchSize: 64,
        checkpointEvery: 1,
        onCheckpoint: (cp) => {
          // Every record before it was consumed.
          expect(cp.matched).toBe(head.length);
          last = cp;
        },
      });
      for await (const e of stream) {
        head.push(e.id);
        if (head.length >= all.length / 2) break;
      }
      expect(last).toBeDefined();
      expect(stream.checkpoint).toEqual(last);
      if (input === plain) {
        // The first unread line starts at the offset.
        const text = readFileSync(plain);
        expect(text[last!.offset - 1]).toBe(0x0a);
        expect(text.subarray(last!.offset, last!.offset + 2).toString()).toBe('{"');
      }
      // Through JSON, as a caller stores it.
      const cp = JSON.parse(JSON.stringify(last)) as Checkpoint;
      const resumed = streamEntities(input, { filter, json: false, resumeFrom: cp });
      const rest = (await resumed.toArray()).map((e) => e.id);
      expect([...head.slice(0, cp.matched), ...rest]).toEqual(all);
      const stats = await resumed.stats;
      expect(stats.missingIds).toEqual(["Q999"]);
      expect(stats.matched).toBe(all.length);
      // Not with another filter.
      await expect(streamEntities(input, { filter: { claim: "P17=Q31" }, resumeFrom: cp }).toArray()).rejects.toThrow(
        /another filter/,
      );
    }
  });
});

describe("redirects", () => {
  it("are read from the SQL dumps, and sort the misses", async () => {
    const options = { redirects: fixture("wikidatawiki-redirect.sql"), pages: fixture("wikidatawiki-page.sql") };
    const pairs = [];
    for await (const p of readRedirects(options)) pairs.push(p);
    expect(pairs).toEqual([
      { from: "Q1", to: "Q42" },
      { from: "Q7", to: "Q55" },
      { from: "Q8", to: "Q99999999" },
      { from: "Q9", to: "Q10" },
      { from: "Q10", to: "Q42" },
    ]);
    const index = await RedirectIndex.build(options);
    expect(index.size).toBe(5);
    expect(index.resolve("Q9")).toBe("Q42");
    expect(index.resolve("Q42")).toBeNull();
    const saved = join(tmp(), "redirects.wdr");
    index.save(saved);
    const redirects = await RedirectIndex.load(saved);

    const s = streamEntities(sample, { filter: { ids: ["Q1", "Q7", "Q8", "Q3", "Q999999999"] }, redirects, json: false });
    const got = (await s.toArray()).map((e) => [e.id, e.mergedFrom]);
    expect(got).toEqual([
      ["Q55", ["Q7"]],
      ["Q42", ["Q1"]],
    ]);
    const stats = await s.stats;
    expect(stats.merged).toEqual([
      { from: "Q1", to: "Q42", inDump: true },
      { from: "Q7", to: "Q55", inDump: true },
      { from: "Q8", to: "Q99999999", inDump: false },
    ]);
    expect(stats.deleted).toEqual(["Q3"]);
    expect(stats.newer).toEqual(["Q999999999"]);
  });
});

describe("merges into known items", () => {
  it("come through even when the item merged into is unchanged", async () => {
    const redirects = await RedirectIndex.build({
      redirects: fixture("wikidatawiki-redirect.sql"),
      pages: fixture("wikidatawiki-page.sql"),
    });
    const { ids, revisions } = sampleRevisions();
    // Q1 was merged into Q42; Q42's revision is known and current.
    const filter = { ids: Uint32Array.of(1, 42), revisions: Float64Array.of(0, revisions[ids.indexOf("Q42")]!) };
    const s = streamEntities(sample, { filter, redirects, json: false });
    expect((await s.toArray()).map((e) => [e.id, e.mergedFrom])).toEqual([["Q42", ["Q1"]]]);
    expect((await s.stats).unchanged).toBe(0);
  });
});

describe("referenced labels", () => {
  it("are collected for the items the matches refer to, and only those", async () => {
    const s = streamEntities(fixture("places.json"), {
      filter: { ids: "Q9009" },
      properties: ["P131", "P136"],
      referencedLabels: { languages: ["en", "nl"] },
    });
    expect((await s.toArray()).map((e) => e.id)).toEqual(["Q9009"]);
    const stats = await s.stats;
    expect(stats.referencedLabels).toEqual({
      Q7411: { en: "Dutch", nl: "Nederlands" },
      Q9002: { en: "Schaesberg", nl: "Schaesberg" },
      Q9011: { en: "rock music", nl: "rockmuziek" },
    });
    expect(stats.labels).toMatchObject({ referenced: 3, labelled: 3, duringPass: 2 });
    // Or to a file.
    const out = join(tmp(), "labels.json");
    const t = streamEntities(fixture("places.json"), {
      filter: { ids: "Q9009" },
      properties: ["P131"],
      referencedLabels: { languages: ["nl"], output: out },
    });
    await t.toArray();
    expect((await t.stats).referencedLabels).toBeUndefined();
    expect(JSON.parse(readFileSync(out, "utf8"))).toEqual({ Q9002: { nl: "Schaesberg" } });
  });
});

describe("qualifiers", () => {
  it("can be kept by property", async () => {
    const [be] = await streamEntities(sample, { filter: { ids: "Q31" }, qualifiers: ["P585"] }).toArray();
    const pop = be!.entity.claims["P1082"]![0]!;
    expect(Object.keys(pop.qualifiers!)).toEqual(["P585"]);
    const [all] = await streamEntities(sample, { filter: { ids: "Q31" } }).toArray();
    expect(Object.keys(all!.entity.claims["P1082"]![0]!.qualifiers!)).toEqual(["P585", "P459"]);
  });
});

describe("places", () => {
  it("find items in a country through the P131 chain", async () => {
    const places = await PlaceIndex.build(fixture("places.json"));
    expect(places.countryOf("Q9003")).toEqual(["Q55"]);
    expect(places.countryOf("Q9004")).toEqual(["Q55", "Q145"]);
    expect(places.countryOf("Q9008")).toEqual([]);
    expect(places.countriesOf(Uint32Array.of(9003, 9013, 1))).toEqual([["Q55"], ["Q31"], []]);
    const file = join(tmp(), "places.wdp");
    places.save(file);
    const placeIndex = await PlaceIndex.load(file);
    expect(placeIndex.size).toBe(places.size);
    const nl = await streamEntities(fixture("places.json"), { filter: { inCountry: "Q55" }, placeIndex, json: false }).toArray();
    expect(nl.map((e) => e.id)).toEqual(["Q55", "Q9001", "Q9002", "Q9003", "Q9004", "Q9009"]);
    await expect(streamEntities(fixture("places.json"), { filter: { inCountry: "Q55" } }).toArray()).rejects.toThrow(
      /place index/,
    );
    // Languages by country, through the index.
    const [village] = await streamEntities(fixture("places.json"), {
      filter: { ids: "Q9002" },
      languages: { by: "P17", map: { Q55: ["nl"] }, always: ["en"] },
      placeIndex,
    }).toArray();
    expect(Object.keys(village!.entity.labels)).toEqual(["nl", "en"]);
    // Built as a by-product, with the class index in the same pass.
    const dir = tmp();
    const s = streamEntities(fixture("places.json"), {
      buildPlaceIndex: join(dir, "p.wdp"),
      buildClassIndex: join(dir, "c.wdc"),
      json: false,
    });
    await s.toArray();
    expect((await s.stats).placeIndex?.items).toBe(places.size);
  });
});

describe("incremental dumps", () => {
  it("read like a dump: each entity once, at its newest revision", async () => {
    const s = streamEntities(fixture("incremental.xml"));
    const got = await s.toArray();
    expect(got.map((e) => e.id)).toEqual(["Q42", "Q1072", "P31", "L7"]);
    expect(got[0]!.entity.labels["en"]).toBe("Douglas Adams (newest)");
    expect(got[0]!.entity.lastrevid).toBe(2_200_000_003);
    expect((await s.stats).redirects).toEqual([{ from: "Q7", to: "Q55" }]);
    const dir = tmp();
    const cut = readFileSync(fixture("incremental.xml"), "utf8");
    writeFileSync(join(dir, "cut.xml"), cut.slice(0, cut.indexOf("<title>Property:P31") + 30));
    await expect(streamEntities(join(dir, "cut.xml")).toArray()).rejects.toThrow(/truncated/);
  });
});
