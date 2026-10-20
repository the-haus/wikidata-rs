import { join } from "node:path";

import { beforeAll, describe, expect, it } from "vitest";

import { ClassIndex, openDump, type Filter } from "../ts/index.js";
import { sample, tmp } from "./helpers.js";

async function ids(filter: Filter, classIndex?: ClassIndex): Promise<string[]> {
  const dump = await openDump(sample);
  const out: string[] = [];
  const options = classIndex ? { filter, classIndex } : { filter };
  for await (const e of dump.entities({ ...options, json: false })) out.push(e.id);
  return out;
}

describe("filters", () => {
  it("ids, types, properties, claims, sitelinks", async () => {
    expect(await ids({ ids: ["Q42", "P31", "L7", "Q404"] })).toEqual(["Q42", "P31", "L7"]);
    expect(await ids({ type: "lexeme" })).toEqual(["L7", "L8"]);
    expect(await ids({ type: ["property"] })).toEqual(["P31", "P625"]);
    const nl = await ids({ claim: "P17=Q55" });
    expect(nl.slice(0, 3)).toEqual(["Q55", "Q1072", "Q1074"]);
    expect(nl).toHaveLength(9);
    expect(await ids({ claim: { property: "P297", values: ["NL", "BE"] } })).toEqual(["Q31", "Q55"]);
    expect(await ids({ claim: { P1448: ["Koninkrijk België"] } })).toEqual(["Q31"]);
    expect(await ids([{ hasProperty: "P434" }, { type: "item" }])).toEqual([
      "Q42",
      "Q1072",
      "Q5000",
      "Q5005",
      "Q5010",
      "Q5015",
      "Q5020",
    ]);
    expect(await ids({ instanceOf: ["Q6256"] })).toEqual(["Q31", "Q55"]);
    expect(await ids({ sitelink: "nlwiki", not: { instanceOf: "Q6256" } })).toEqual(["Q42", "Q1072", "Q1073"]);
    expect(await ids({ any: [{ ids: "Q2003" }, { claim: "P297=NL" }] })).toEqual(["Q55", "Q2003"]);
  });

  it("rejects bad filters", async () => {
    const dump = await openDump(sample);
    await expect(dump.entities({ filter: { bogus: 1 } as unknown as Filter }).toArray()).rejects.toThrow(/unknown filter/);
    await expect(dump.entities({ filter: { hasProperty: "Q1" } }).toArray()).rejects.toThrow(/property/);
    await expect(
      dump.entities({ filter: { instanceOf: { classes: ["Q868557"], subclasses: true } } }).toArray(),
    ).rejects.toThrow(/class index/);
  });
});

describe("class index", () => {
  let index: ClassIndex;
  beforeAll(async () => {
    const dump = await openDump(sample);
    index = await dump.classIndex();
  });

  it("is built from P279", () => {
    expect(index.edges).toBe(3);
    expect(index.source).toBe("sample.json");
    expect(index.parents("Q99000001")).toEqual(["Q868557"]);
    expect(index.subclassesOf("Q132241")).toEqual(["Q868557", "Q132241", "Q99000001"].sort((a, b) => Number(a.slice(1)) - Number(b.slice(1))));
    expect(index.isSubclassOf("Q99000001", "Q1656682")).toBe(true);
    expect(index.isSubclassOf("Q1656682", "Q99000001")).toBe(false);
    expect(() => index.parents("P31")).toThrow();
  });

  it("finds instances of subclasses", async () => {
    expect(await ids({ instanceOf: { classes: ["Q868557"], subclasses: true } }, index)).toEqual(["Q1072", "Q1073"]);
    expect(await ids({ instanceOf: { classes: "Q132241", subclasses: true } }, index)).toEqual(["Q1072", "Q1073", "Q1074"]);
    expect(await ids({ instanceOf: { classes: ["Q868557"] } }, index)).toEqual(["Q1073"]);
  });

  it("saves and loads", async () => {
    const p = join(tmp(), "classes.wdc");
    index.save(p);
    const loaded = await ClassIndex.load(p);
    expect(loaded.edges).toBe(3);
    expect(await ids({ instanceOf: { classes: ["Q868557"], subclasses: true } }, loaded)).toEqual(["Q1072", "Q1073"]);
    await expect(ClassIndex.load(sample)).rejects.toThrow(/class index/);
  });
});
