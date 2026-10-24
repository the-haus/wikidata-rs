// Two native addons in one process, each with its own allocator: here two
// copies of this one (a second path is a second dlopen), as an app has when
// it loads @wikidata-rs/node beside @pestle-rs/node. mimalloc on macOS keeps
// its heaps in fixed slots of the process, so two copies of it crashed
// (exit 139).

import { spawnSync } from "node:child_process";
import { copyFileSync, readdirSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";

import { expect, it } from "vitest";

import { sample, tmp } from "./helpers.js";

const pkg = resolve(__dirname, "..");
const binary = readdirSync(pkg).find((f) => /^wikidata-node\..+\.node$/.test(f));

it.skipIf(!binary)("two copies of the addon live in one process", () => {
  const dir = tmp();
  try {
    const copy = join(dir, "copy.node");
    copyFileSync(join(pkg, binary!), copy);
    const script = `
      const a = require(process.argv[1]), b = require(process.argv[2]);
      let n = 0;
      for (let i = 0; i < 200; i++) for (const m of [a, b]) n += m.externalIdTable().length;
      Promise.all([a, b, a, b].map((m) => m.open(process.argv[3]))).then(() => console.log(n > 0));
    `;
    const run = spawnSync(process.execPath, ["-e", script, join(pkg, binary!), copy, sample], { encoding: "utf8" });
    expect({ status: run.status, signal: run.signal, stderr: run.stderr }).toEqual({ status: 0, signal: null, stderr: "" });
    expect(run.stdout.trim()).toBe("true");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
