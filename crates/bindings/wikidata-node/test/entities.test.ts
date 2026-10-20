import { describe, expect, it } from "vitest";

import {
  EXTERNAL_IDS,
  externalIdUrl,
  openDump,
  streamEntities,
  version,
  WikidataError,
  type EntityRecord,
  type TimeValue,
} from "../ts/index.js";
import { sample, sampleIds } from "./helpers.js";

async function all(options = {}): Promise<Map<string, EntityRecord>> {
  const dump = await openDump(sample);
  const out = new Map<string, EntityRecord>();
  for await (const e of dump.entities(options)) out.set(e.id, e);
  return out;
}

describe("open", () => {
  it("opens a dump and tells what it is", async () => {
    const dump = await openDump(sample);
    expect(dump.compression).toBe("none");
    expect(dump.size).toBeGreaterThan(10_000);
    expect(dump.input).toBe(sample);
    expect(version()).toMatch(/^\d+\.\d+\.\d+/);
  });

  it("rejects missing files", async () => {
    await expect(openDump("/nonexistent/dump.json.bz2")).rejects.toBeInstanceOf(WikidataError);
  });

  it("knows the external id table", () => {
    expect(EXTERNAL_IDS.find((r) => r.key === "musicbrainzArtist")?.property).toBe("P434");
    expect(externalIdUrl("osmRelation", "47796")).toBe("https://www.openstreetmap.org/relation/47796");
    expect(externalIdUrl("isni", "0000 0001 2103 2683")).toBe("https://isni.org/isni/0000000121032683");
  });
});

describe("entities", () => {
  it("streams every entity in dump order", async () => {
    const ids: string[] = [];
    for await (const e of streamEntities(sample)) ids.push(e.id);
    expect(ids).toEqual(sampleIds());
    expect(ids).toHaveLength(41);
  });

  it("gives the summary fields without parsing JSON", async () => {
    const m = await all({ languages: ["nl", "en", "mul"] });
    const be = m.get("Q31")!;
    expect(be.type).toBe("item");
    expect(be.label).toEqual({ text: "België", language: "nl" });
    expect(be.description).toEqual({ text: "land in West-Europa", language: "nl" });
    expect(be.instanceOf).toEqual(["Q6256"]); // truthy: the preferred one only
    expect(be.country).toEqual(["Q31"]); // the deprecated Q55 is not truthy
    expect(be.coordinates).toEqual({ lat: 50.641111, lon: 4.668056 });
    expect(be.inception).toBe(1830);
    expect(be.dissolved).toBeNull();
    expect(be.image).toBe("Flag of Belgium (civil).svg");
    expect(be.website).toBe("https://www.belgium.be/");
    expect(be.externalIds).toEqual({ osmRelation: "52411", geonames: "2802361" });

    const da = m.get("Q42")!;
    expect(da.label?.text).toBe("Douglas Adams");
    expect(da.image).toBeNull(); // deprecated
    expect(da.externalIds.musicbrainzArtist).toBe("e9ed318d-8cc5-4cf8-ab77-505e39ab6ea4");
    expect(m.get("Q1074")!.label).toEqual({ text: "Dissolved Festival", language: "en" });
    expect(m.get("Q1074")!.dissolved).toBe(2005);
    expect(m.get("Q99000001")!.subclassOf).toEqual(["Q868557"]);
    expect(m.get("Q2003")!.label).toBeNull();
    expect(m.get("P31")!.type).toBe("property");
    expect(m.get("L7")!.type).toBe("lexeme");
    expect(JSON.parse(JSON.stringify(be))).toMatchObject({ id: "Q31", inception: 1830 });
  });

  it("parses the full, typed entity on demand", async () => {
    const m = await all({ references: true });
    const be = m.get("Q31")!.entity;
    expect(be.id).toBe("Q31");
    expect(be.labels.ja).toBe("ベルギー");
    expect(be.sitelinks.nlwiki).toEqual({
      title: "België",
      url: "https://nl.wikipedia.org/wiki/Belgi%C3%AB",
      badges: [],
    });
    const types = new Set(Object.values(be.claims).flatMap((sts) => sts.map((s) => s.value.type)));
    for (const t of ["entity", "coordinate", "time", "media", "url", "quantity", "externalId", "monolingual", "geoShape", "tabularData"]) {
      expect(types.has(t as never), t).toBe(true);
    }
    const inception = be.claims.P571![0]!;
    expect(inception.value).toMatchObject({ type: "time", iso: "1830-10-04", precisionName: "day", calendar: "gregorian" });
    expect(inception.references![0]!.P854![0]).toEqual({ type: "url", value: "https://example.org/belgium" });
    const pop = be.claims.P1082![0]!;
    expect(pop.rank).toBe("preferred");
    expect(pop.value).toEqual({ type: "quantity", amount: 11697557, unit: null });
    expect((pop.qualifiers!.P585![0] as TimeValue).iso).toBe("2023-01-01");
    expect(be.claims.P2046![0]!.value).toEqual({ type: "quantity", amount: 30689, unit: "Q712226", lower: 30688, upper: 30690 });
    expect(be.claims.P17).toHaveLength(1);

    const da = m.get("Q42")!.entity;
    expect(da.aliases.en).toContain("DNA 😀");
    expect(da.claims.P40![0]!.value).toEqual({ type: "someValue" });
    expect(da.claims.P26![0]!.value).toEqual({ type: "noValue" });

    const misc = m.get("Q2002")!.entity.claims;
    expect(misc.P2534![0]!.value).toEqual({ type: "math", value: "E=mc^2" });
    expect(misc.P6604![0]!.value).toEqual({ type: "musicalNotation", value: "\\relative c' { c d e f g }" });
    expect(misc.P5830![0]!.value).toEqual({ type: "entity", id: "L7-F1" });
    expect(misc.P12861![0]!.value).toEqual({ type: "entity", id: "E10" });
    expect(misc.P625![0]!.value).toMatchObject({ type: "coordinate", globe: "Q111" });

    const battle = m.get("Q2001")!.entity.claims.P585![0]!.value as TimeValue;
    expect([battle.iso, battle.year, battle.calendar]).toEqual(["-0490-09-12", -490, "julian"]);

    const cat = m.get("L7")!.entity;
    expect(cat.lemmas).toEqual({ en: "cat" });
    expect(cat.forms![0]!.representations).toEqual({ en: "cats" });
    expect(cat.senses![0]!.glosses).toEqual({ en: "domesticated feline", nl: "huiskat" });
    expect(m.get("P31")!.entity.datatype).toBe("wikibase-item");
    expect(m.get("L8")!.entity.language).toBe("Q7411");
  });

  it("keeps what it is asked for", async () => {
    const m = await all({
      languages: ["fr"],
      sitelinks: ["enwiki"],
      properties: ["P31"],
      bestRank: true,
      aliases: false,
      qualifiers: false,
    });
    const be = m.get("Q31")!.entity;
    expect(Object.keys(be.labels)).toEqual(["fr"]);
    expect(be.label).toEqual({ text: "Belgique", language: "fr" });
    expect(Object.keys(be.sitelinks)).toEqual(["enwiki"]);
    expect(Object.keys(be.claims)).toEqual(["P31"]);
    expect(be.claims.P31).toHaveLength(1);
    expect(be.aliases).toEqual({});
    // Derived fields are computed whatever `properties` says.
    expect(be.country).toEqual(["Q31"]);
    expect(m.get("Q42")!.label).toBeNull();

    const d = await all({ deprecated: true, properties: ["P17"] });
    expect(d.get("Q31")!.entity.claims.P17).toHaveLength(2);
  });

  it("sends only the summary with json: false", async () => {
    const m = await all({ json: false });
    const be = m.get("Q31")!;
    expect(be.label?.text).toBe("Belgium");
    expect(be.json).toBe("");
    expect(() => be.entity).toThrow(/json: false/);
  });
});
