// Write fixtures/sample.json: a small Wikidata dump that covers every
// datatype, ranks, qualifiers, references, somevalue/novalue, a property, a
// lexeme, empty term lists, multi-byte UTF-8 and a class hierarchy. It is
// written the way Wikimedia's dumper writes (PHP json_encode: non-ASCII as
// \uXXXX, slashes as \/, one entity per line with a trailing comma).
//
//   node scripts/make-sample-dump.mjs

import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

function php(value) {
  return JSON.stringify(value)
    .replace(/[\u007f-￿]/g, (c) => "\\u" + c.charCodeAt(0).toString(16).padStart(4, "0"))
    .replace(/\//g, "\\/");
}

const WD = "http://www.wikidata.org/entity/";
const terms = (o) => Object.fromEntries(Object.entries(o).map(([l, v]) => [l, { language: l, value: v }]));
const aliases = (o) =>
  Object.fromEntries(Object.entries(o).map(([l, vs]) => [l, vs.map((v) => ({ language: l, value: v }))]));
const sitelinks = (o) => Object.fromEntries(Object.entries(o).map(([s, t]) => [s, { site: s, title: t, badges: [] }]));

let guid = 0;
function snak(property, datatype, value, type) {
  if (value === "somevalue" || value === "novalue") return { snaktype: value, property, datatype };
  return { snaktype: "value", property, datavalue: { value, type }, datatype };
}
const item = (q) => ({ "entity-type": "item", "numeric-id": Number(q.slice(1)), id: q });
const V = {
  item: (p, q) => snak(p, "wikibase-item", item(q), "wikibase-entityid"),
  prop: (p, id) =>
    snak(p, "wikibase-property", { "entity-type": "property", "numeric-id": Number(id.slice(1)), id }, "wikibase-entityid"),
  str: (p, s) => snak(p, "string", s, "string"),
  ext: (p, s) => snak(p, "external-id", s, "string"),
  url: (p, s) => snak(p, "url", s, "string"),
  media: (p, s) => snak(p, "commonsMedia", s, "string"),
  math: (p, s) => snak(p, "math", s, "string"),
  music: (p, s) => snak(p, "musical-notation", s, "string"),
  shape: (p, s) => snak(p, "geo-shape", s, "string"),
  tab: (p, s) => snak(p, "tabular-data", s, "string"),
  text: (p, text, language) => snak(p, "monolingualtext", { text, language }, "monolingualtext"),
  time: (p, time, precision, cal = "Q1985727") =>
    snak(p, "time", { time, timezone: 0, before: 0, after: 0, precision, calendarmodel: WD + cal }, "time"),
  qty: (p, amount, unit, lower, upper) => {
    const v = { amount, unit: unit ? WD + unit : "1" };
    if (lower !== undefined) Object.assign(v, { upperBound: upper, lowerBound: lower });
    return snak(p, "quantity", v, "quantity");
  },
  coord: (p, latitude, longitude, precision = 0.0001, globe = "Q2") =>
    snak(p, "globe-coordinate", { latitude, longitude, altitude: null, precision, globe: WD + globe }, "globecoordinate"),
  lexeme: (p, id) =>
    snak(p, "wikibase-lexeme", { "entity-type": "lexeme", "numeric-id": Number(id.slice(1)), id }, "wikibase-entityid"),
  form: (p, id) => snak(p, "wikibase-form", { "entity-type": "form", id }, "wikibase-entityid"),
  sense: (p, id) => snak(p, "wikibase-sense", { "entity-type": "sense", id }, "wikibase-entityid"),
  schema: (p, id) => snak(p, "entity-schema", { "entity-type": "entity-schema", id }, "wikibase-entityid"),
  some: (p, dt = "wikibase-item") => snak(p, dt, "somevalue"),
  none: (p, dt = "wikibase-item") => snak(p, dt, "novalue"),
};

function st(mainsnak, { rank = "normal", qualifiers, references } = {}) {
  const s = { mainsnak, type: "statement" };
  if (qualifiers) {
    s.qualifiers = {};
    for (const q of qualifiers) (s.qualifiers[q.property] ??= []).push(q);
    s["qualifiers-order"] = Object.keys(s.qualifiers);
  }
  s.id = `${mainsnak.property}$${(++guid).toString(16).padStart(8, "0")}-0000-4000-8000-000000000000`;
  s.rank = rank;
  if (references) {
    s.references = references.map((snaks) => {
      const r = { hash: (guid * 7919).toString(16).padStart(40, "0"), snaks: {} };
      for (const sn of snaks) (r.snaks[sn.property] ??= []).push(sn);
      r["snaks-order"] = Object.keys(r.snaks);
      return r;
    });
  }
  return s;
}

function claims(list) {
  const out = {};
  for (const s of list) (out[s.mainsnak.property] ??= []).push(s);
  return out;
}

let rev = 2_000_000_000;
function entity(id, { type = "item", labels = {}, descriptions = {}, al = {}, cl = [], links = {}, extra = {} } = {}) {
  const e = { type, id };
  Object.assign(e, extra.before ?? {});
  e.labels = Object.keys(labels).length ? terms(labels) : [];
  e.descriptions = Object.keys(descriptions).length ? terms(descriptions) : [];
  e.aliases = Object.keys(al).length ? aliases(al) : [];
  e.claims = cl.length ? claims(cl) : [];
  if (type === "item") e.sitelinks = Object.keys(links).length ? sitelinks(links) : [];
  Object.assign(e, extra.after ?? {});
  e.pageid = Number(id.replace(/\D/g, "")) + 100;
  e.ns = type === "property" ? 120 : type === "lexeme" ? 146 : 0;
  e.title = type === "property" ? `Property:${id}` : type === "lexeme" ? `Lexeme:${id}` : id;
  e.lastrevid = ++rev;
  e.modified = "2026-10-01T12:00:00Z";
  return e;
}

const entities = [
  entity("Q31", {
    labels: { nl: "België", en: "Belgium", fr: "Belgique", de: "Belgien", mul: "Belgium", ja: "ベルギー" },
    descriptions: { nl: "land in West-Europa", en: "country in western Europe" },
    al: { nl: ["Koninkrijk België"], en: ["Kingdom of Belgium", "BE"] },
    links: { nlwiki: "België", enwiki: "Belgium", commonswiki: "Category:Belgium", zh_yuewiki: "比利時" },
    cl: [
      st(V.item("P31", "Q6256"), { rank: "preferred" }),
      st(V.item("P31", "Q3624078")),
      st(V.item("P17", "Q31")),
      st(V.item("P17", "Q55"), { rank: "deprecated" }),
      st(V.coord("P625", 50.641111, 4.668056, 0.0000277778)),
      st(V.time("P571", "+1830-10-04T00:00:00Z", 11), {
        references: [[V.item("P248", "Q36578"), V.url("P854", "https://example.org/belgium")]],
      }),
      st(V.media("P18", "Flag of Belgium (civil).svg")),
      st(V.url("P856", "https://www.belgium.be/")),
      st(V.qty("P1082", "+11697557", null), {
        rank: "preferred",
        qualifiers: [V.time("P585", "+2023-01-01T00:00:00Z", 11)],
        references: [[V.item("P248", "Q12483"), V.time("P813", "+2024-05-01T00:00:00Z", 11)]],
      }),
      st(V.qty("P1082", "+11000000", null), { qualifiers: [V.time("P585", "+2010-00-00T00:00:00Z", 9)] }),
      st(V.ext("P297", "BE")),
      st(V.text("P1448", "Royaume de Belgique", "fr")),
      st(V.text("P1448", "Koninkrijk België", "nl")),
      st(V.ext("P402", "52411")),
      st(V.ext("P1566", "2802361")),
      st(V.item("P30", "Q46")),
      st(V.shape("P3896", "Data:Belgium.map")),
      st(V.tab("P4179", "Data:Belgium population.tab")),
      st(V.qty("P2046", "+30689", "Q712226", "+30688", "+30690")),
    ],
  }),
  entity("Q55", {
    labels: { nl: "Nederland", en: "Netherlands", mul: "Nederland" },
    descriptions: { en: "country in northwestern Europe" },
    links: { nlwiki: "Nederland", enwiki: "Netherlands" },
    cl: [
      st(V.item("P31", "Q6256")),
      st(V.item("P17", "Q55")),
      st(V.coord("P625", 52.316667, 5.55)),
      st(V.time("P571", "+1815-03-16T00:00:00Z", 11)),
      st(V.ext("P297", "NL")),
      st(V.ext("P402", "47796")),
    ],
  }),
  entity("Q42", {
    labels: { en: "Douglas Adams", nl: "Douglas Adams", ja: "ダグラス・アダムズ", ru: "Дуглас Адамс" },
    descriptions: { en: "English writer and humourist (1952–2001)" },
    al: { en: ["Douglas Noël Adams", "DNA 😀"] },
    links: { enwiki: "Douglas Adams", nlwiki: "Douglas Adams" },
    cl: [
      st(V.item("P31", "Q5")),
      st(V.item("P27", "Q145")),
      st(V.time("P569", "+1952-03-11T00:00:00Z", 11)),
      st(V.time("P570", "+2001-05-11T00:00:00Z", 11)),
      st(V.ext("P434", "e9ed318d-8cc5-4cf8-ab77-505e39ab6ea4")),
      st(V.ext("P214", "113230702")),
      st(V.ext("P213", "0000 0000 8045 6315")),
      st(V.ext("P1953", "66941")),
      st(V.qty("P2048", "+1.96", "Q11573", "+1.95", "+1.97")),
      st(V.some("P40")),
      st(V.none("P26")),
      st(V.media("P18", "Douglas adams portrait cropped.jpg"), { rank: "deprecated" }),
      st(V.ext("P2002", "douglasadams")),
    ],
  }),
  entity("Q868557", {
    labels: { en: "music festival", nl: "muziekfestival" },
    cl: [st(V.item("P279", "Q132241")), st(V.item("P31", "Q16889133"))],
    links: { enwiki: "Music festival" },
  }),
  entity("Q132241", {
    labels: { en: "festival", nl: "festival" },
    cl: [st(V.item("P279", "Q1656682"))],
  }),
  entity("Q1656682", { labels: { en: "event", nl: "evenement" } }),
  entity("Q99000001", {
    labels: { en: "rock festival", nl: "rockfestival" },
    cl: [
      st(V.item("P279", "Q868557"), { rank: "preferred" }),
      st(V.item("P279", "Q5"), { rank: "deprecated" }),
      st(V.item("P279", "Q1656682")),
    ],
  }),
  entity("Q1072", {
    labels: { nl: "Pinkpop", en: "Pinkpop Festival" },
    descriptions: { nl: "muziekfestival in Landgraaf" },
    links: { nlwiki: "Pinkpop", enwiki: "Pinkpop Festival" },
    cl: [
      st(V.item("P31", "Q99000001")),
      st(V.item("P17", "Q55")),
      st(V.coord("P625", 50.9, 6.03)),
      st(V.time("P571", "+1970-05-18T00:00:00Z", 11)),
      st(V.url("P856", "https://www.pinkpop.nl/")),
      st(V.ext("P434", "e4b4b7e5-5c2c-4f8a-a8a5-0b2b0b6a8b2e")),
      st(V.ext("P11693", "123456789")),
    ],
  }),
  entity("Q1073", {
    labels: { nl: "Rock Werchter" },
    cl: [st(V.item("P31", "Q868557")), st(V.item("P17", "Q31")), st(V.time("P571", "+1976-00-00T00:00:00Z", 9))],
    links: { nlwiki: "Rock Werchter" },
  }),
  entity("Q1074", {
    labels: { en: "Dissolved Festival" },
    cl: [
      st(V.item("P31", "Q132241")),
      st(V.item("P17", "Q55")),
      st(V.time("P571", "+1990-00-00T00:00:00Z", 8)),
      st(V.time("P576", "+2005-06-00T00:00:00Z", 10)),
    ],
  }),
  entity("Q2001", {
    labels: { en: "Battle of Marathon" },
    cl: [st(V.item("P31", "Q178561")), st(V.time("P585", "-0490-09-12T00:00:00Z", 11, "Q1985786"))],
  }),
  entity("Q2002", {
    labels: { en: "mass–energy equivalence" },
    cl: [
      st(V.math("P2534", "E=mc^2")),
      st(V.music("P6604", "\\relative c' { c d e f g }")),
      st(V.str("P1545", "1")),
      st(V.prop("P1659", "P31")),
      st(V.lexeme("P5137", "L7")),
      st(V.form("P5830", "L7-F1")),
      st(V.sense("P5972", "L7-S1")),
      st(V.schema("P12861", "E10")),
      st(V.coord("P625", 18.65, -71.25, null, "Q111")),
    ],
  }),
  entity("Q2003", { labels: {}, cl: [] }),
  entity("P31", {
    type: "property",
    labels: { en: "instance of", nl: "is een" },
    descriptions: { en: "that class of which this subject is a particular example and member" },
    extra: { before: { datatype: "wikibase-item" } },
    cl: [st(V.ext("P1630", "https://example.org/$1"))],
  }),
  entity("P625", {
    type: "property",
    labels: { en: "coordinate location" },
    extra: { before: { datatype: "globe-coordinate" } },
  }),
  entity("L7", {
    type: "lexeme",
    extra: {
      after: {
        lemmas: terms({ en: "cat" }),
        lexicalCategory: "Q1084",
        language: "Q1860",
        forms: [
          {
            id: "L7-F1",
            representations: terms({ en: "cats" }),
            grammaticalFeatures: ["Q146786"],
            claims: claims([st(V.str("P898", "kæts"))]),
          },
        ],
        senses: [
          {
            id: "L7-S1",
            glosses: terms({ en: "domesticated feline", nl: "huiskat" }),
            claims: claims([st(V.item("P5137", "Q146"))]),
          },
        ],
      },
    },
  }),
];

// Filler: people and places, to give filters something to reject.
for (let i = 0; i < 24; i++) {
  const q = `Q${5000 + i}`;
  const cl = [st(V.item("P31", i % 3 === 0 ? "Q5" : "Q515"))];
  if (i % 4 === 0) cl.push(st(V.item("P17", "Q55")));
  if (i % 4 === 1) cl.push(st(V.item("P17", "Q31")));
  if (i % 5 === 0) cl.push(st(V.ext("P434", `00000000-0000-4000-8000-${String(i).padStart(12, "0")}`)));
  entities.push(
    entity(q, {
      labels: { en: `Filler ${i}`, nl: `Vulling ${i}` },
      descriptions: i % 2 ? { en: "a made-up entity" } : {},
      cl,
      links: i % 6 === 0 ? { enwiki: `Filler ${i}` } : {},
    }),
  );
}

// Lexeme lines in lexeme dumps start with "pageid"; keep one that way.
const odd = entity("L8", {
  type: "lexeme",
  extra: { after: { lemmas: terms({ nl: "kat" }), lexicalCategory: "Q1084", language: "Q7411", forms: [], senses: [] } },
});
const reordered = { pageid: odd.pageid, ns: odd.ns, title: odd.title, ...odd };
entities.push(reordered);

const lines = entities.map(php);
writeFileSync(join(root, "fixtures/sample.json"), "[\n" + lines.join(",\n") + "\n]\n");
console.log(`fixtures/sample.json: ${lines.length} entities`);
