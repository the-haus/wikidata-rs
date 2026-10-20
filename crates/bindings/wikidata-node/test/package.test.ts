// The package as consumers resolve it: CommonJS and ESM entry points,
// `./package.json`, and bundling with esbuild and Vite (no `external`).

import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { sample, tmp } from "./helpers.js";

const pkg = resolve(__dirname, "..");
/** The dev build next to the package (what the loaders find first). */
const binary = (): string => {
  const triple = `${process.platform}-${process.arch}`;
  const names = ["gnu", "musl", "msvc"].map((abi) => `wikidata-node.${triple}-${abi}.node`).concat(`wikidata-node.${triple}.node`);
  const found = names.map((n) => join(pkg, n)).find((p) => existsSync(p));
  if (!found) throw new Error("no dev build of the native module");
  return found;
};

/** Run a script with node, from a directory outside the package. */
function run(file: string, env: Record<string, string> = {}): string {
  const r = spawnSync(process.execPath, [file, sample], { encoding: "utf8", timeout: 60_000, env: { ...process.env, ...env } });
  expect(r.stderr).toBe("");
  expect(r.status).toBe(0);
  return r.stdout.trim();
}

const APP = `
import { streamEntities, version, WikidataError } from "@wikidata-rs/node";
streamEntities(process.argv[2], { filter: { any: { nl: { claim: "P17=Q55" } } } })
  .toArray()
  .then((all) => console.log(JSON.stringify([typeof version(), typeof WikidataError, all.length, all[0].matchedBy])));
`;
const WANT = JSON.stringify(["string", "function", 9, ["nl"]]);

describe("package", () => {
  let src: string;
  let out: string;
  beforeAll(() => {
    // Inside the package, so that `@wikidata-rs/node` resolves to it.
    src = mkdtempSync(join(pkg, ".bundle-"));
    writeFileSync(join(src, "app.mjs"), APP);
    out = tmp();
  });
  afterAll(() => rmSync(src, { recursive: true, force: true }));

  it("exports CommonJS, ESM and ./package.json", () => {
    const r = createRequire(join(pkg, "..", "..", "..", "package.json"));
    // Real paths: Windows may spell the drive or the junction differently.
    const real = (p: string): string => realpathSync.native(p);
    expect(real(r.resolve("@wikidata-rs/node/package.json"))).toBe(real(join(pkg, "package.json")));
    expect(real(r.resolve("@wikidata-rs/node"))).toBe(real(join(pkg, "dist", "index.js")));
    const manifest = JSON.parse(readFileSync(join(pkg, "package.json"), "utf8")) as {
      exports: Record<string, { import: { types: string; default: string } }>;
    };
    expect(manifest.exports["."]!.import.default).toBe("./dist/esm/index.js");
    expect(readFileSync(join(pkg, "dist", "esm", "index.d.ts"), "utf8")).toContain("streamEntities");
    // Node's own ESM resolution picks the ESM build.
    copyFileSync(join(src, "app.mjs"), join(src, "native-esm.mjs"));
    writeFileSync(join(src, "where.mjs"), `console.log(import.meta.resolve("@wikidata-rs/node"));`);
    expect(run(join(src, "native-esm.mjs"))).toBe(WANT);
    expect(run(join(src, "where.mjs"))).toMatch(/dist\/esm\/index\.js$/);
  });

  it("bundles with esbuild, as ESM and as CommonJS", async () => {
    const esbuild = await import("esbuild");
    for (const format of ["esm", "cjs"] as const) {
      const file = join(out, `esbuild.${format === "esm" ? "mjs" : "cjs"}`);
      const r = await esbuild.build({
        entryPoints: [join(src, "app.mjs")],
        bundle: true,
        platform: "node",
        format,
        outfile: file,
        logLevel: "silent",
      });
      expect(r.errors).toEqual([]);
      expect(readFileSync(file, "utf8")).not.toMatch(/from "@wikidata-rs\/node"|require\("@wikidata-rs\/node"\)/);
      // Away from node_modules, the binary is named by the environment.
      expect(run(file, { WIKIDATA_RS_NODE_BINARY: binary() })).toBe(WANT);
    }
  });

  it("bundles with Vite", async () => {
    const vite = await import("vite");
    await vite.build({
      root: src,
      logLevel: "silent",
      configFile: false,
      ssr: { noExternal: true, target: "node" },
      build: {
        ssr: join(src, "app.mjs"),
        outDir: join(out, "vite"),
        emptyOutDir: true,
        rollupOptions: { output: { format: "esm", entryFileNames: "app.mjs" } },
      },
    });
    const file = join(out, "vite", "app.mjs");
    expect(readFileSync(file, "utf8")).not.toMatch(/from "@wikidata-rs\/node"/);
    expect(run(file, { WIKIDATA_RS_NODE_BINARY: binary() })).toBe(WANT);
  });

  it("names the override in the error when no binary is found", () => {
    const r = spawnSync(process.execPath, ["-e", `require(${JSON.stringify(join(pkg, "dist", "index.js"))})`], {
      encoding: "utf8",
      env: { ...process.env, WIKIDATA_RS_NODE_BINARY: join(out, "missing.node") },
    });
    expect(r.status).not.toBe(0);
    expect(r.stderr).toMatch(/missing\.node/);
  });
});
