import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

/** The repository's sample dump (41 entities; see scripts/make-sample-dump.mjs). */
export const sample = join(here, "..", "..", "..", "..", "fixtures", "sample.json");

/** The sample's entity lines, without commas. */
export function sampleLines(): string[] {
  return readFileSync(sample, "utf8")
    .split("\n")
    .filter((l) => l.startsWith("{"))
    .map((l) => (l.endsWith(",") ? l.slice(0, -1) : l));
}

export const sampleIds = (): string[] => sampleLines().map((l) => (JSON.parse(l) as { id: string }).id);

/** A fresh temporary directory. */
export const tmp = (): string => mkdtempSync(join(tmpdir(), "wikidata-node-"));

/** A dump of `n` entities: the sample's lines, repeated. */
export function bigDump(dir: string, n: number, name = "big.json"): string {
  const lines = sampleLines();
  const out: string[] = [];
  for (let i = 0; i < n; i++) out.push(lines[i % lines.length]!);
  const p = join(dir, name);
  writeFileSync(p, "[\n" + out.join(",\n") + "\n]\n");
  return p;
}

/** Whether the system's bzip2 is installed. */
export const hasBzip2 = (() => {
  try {
    execFileSync("bzip2", ["--help"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
})();
