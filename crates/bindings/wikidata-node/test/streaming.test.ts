import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { Writable } from "node:stream";
import { pipeline } from "node:stream/promises";

import { beforeAll, describe, expect, it } from "vitest";

import { activeThreads, EntityStream, openDump, WikidataError, type EntityRecord, type Progress } from "../ts/index.js";
import { bigDump, sampleIds, tmp } from "./helpers.js";

const N = 20_500;
let big: string;
beforeAll(() => {
  big = bigDump(tmp(), N);
});

describe("streaming", () => {
  it("keeps dump order across many chunks and batches", async () => {
    const dump = await openDump(big);
    const stream = dump.entities({ batchSize: 1000, threads: 4, json: false });
    const want = sampleIds();
    let n = 0;
    let batches = 0;
    for await (const batch of stream.batches()) {
      batches++;
      expect(batch.length).toBeLessThanOrEqual(1000);
      for (const e of batch) {
        expect(e.id).toBe(want[n % want.length]);
        n++;
      }
    }
    expect(n).toBe(N);
    expect(batches).toBeGreaterThanOrEqual(21);
    const stats = await stream.stats;
    expect(stats.entities).toBe(N);
    expect(stats.matched).toBe(N);
    expect(stats.stopped).toBe(false);
    expect(stats.types.items + stats.types.properties + stats.types.lexemes).toBe(N);
  });

  it("stops when the loop breaks, and leaves no threads", async () => {
    const dump = await openDump(big);
    const stream = dump.entities({ batchSize: 10 });
    let n = 0;
    for await (const e of stream) {
      void e;
      if (++n === 25) break;
    }
    const stats = await stream.stats;
    expect(stats.stopped).toBe(true);
    expect(stats.matched).toBeLessThan(N);
    expect(activeThreads()).toBe(0);
  });

  it("closes before starting", async () => {
    const dump = await openDump(big);
    const stream = dump.entities();
    stream.close();
    expect(await stream.toArray()).toEqual([]);
    expect((await stream.stats).stopped).toBe(true);
  });

  it("can only be iterated once", async () => {
    const dump = await openDump(big);
    const stream = dump.entities({ limit: 3 });
    expect(await stream.toArray()).toHaveLength(3);
    await expect(stream.toArray()).rejects.toBeInstanceOf(WikidataError);
  });

  it("aborts with a signal", async () => {
    const dump = await openDump(big);
    const ac = new AbortController();
    const stream = dump.entities({ batchSize: 100, signal: ac.signal });
    let n = 0;
    await expect(
      (async () => {
        for await (const e of stream) {
          void e;
          if (++n === 150) ac.abort(new Error("enough"));
        }
      })(),
    ).rejects.toThrow("enough");
    await expect(stream.stats).rejects.toThrow("enough");
    const pre = new AbortController();
    pre.abort();
    await expect(dump.entities({ signal: pre.signal }).toArray()).rejects.toThrow();
    // The native side winds down on its own thread.
    for (let i = 0; i < 50 && activeThreads() > 0; i++) await new Promise((r) => setTimeout(r, 20));
    expect(activeThreads()).toBe(0);
  });

  it("does not run ahead of a slow consumer", async () => {
    const dump = await openDump(big);
    const progress: Progress[] = [];
    const stream = dump.entities({
      batchSize: 100,
      readAhead: 1,
      threads: 1,
      json: false,
      onProgress: (p) => progress.push(p),
    });
    const it = stream[Symbol.asyncIterator]();
    await it.next();
    await new Promise((r) => setTimeout(r, 300));
    // With one batch of read-ahead, the producer stops within the first
    // 4 MB chunks, far from the end.
    const matched = Math.max(0, ...progress.map((p) => p.matched));
    expect(matched).toBeLessThan(N / 2);
    await it.return?.();
    expect((await stream.stats).stopped).toBe(true);
  });

  it("limits", async () => {
    const dump = await openDump(big);
    const stream = dump.entities({ limit: 1234, batchSize: 100 });
    expect(await stream.toArray()).toHaveLength(1234);
    const stats = await stream.stats;
    expect(stats.matched).toBe(1234);
    expect(stats.stopped).toBe(true);
  });

  it("runs several streams at once", async () => {
    const dump = await openDump(big);
    const [a, b, c] = await Promise.all([
      dump.entities({ filter: { type: "lexeme" }, json: false }).toArray(),
      dump.entities({ filter: { claim: "P17=Q55" }, json: false }).toArray(),
      dump.entities({ filter: { ids: "Q42" }, json: false }).toArray(),
    ]);
    const rounds = N / 41;
    expect(a).toHaveLength(2 * rounds);
    expect(b).toHaveLength(9 * rounds);
    expect(c).toHaveLength(rounds);
  });

  it("maps and filters lazily, and pipes with backpressure", async () => {
    const dump = await openDump(big);
    const out: string[] = [];
    for await (const s of dump.entities({ limit: 41 }).map((e) => `${e.type}:${e.id}`)) out.push(s);
    expect(out[0]).toBe("item:Q31");
    expect(out).toHaveLength(41);
    const lexemes: EntityRecord[] = [];
    for await (const e of dump.entities({ limit: 41 }).filter((e) => e.type === "lexeme")) lexemes.push(e);
    expect(lexemes.map((e) => e.id)).toEqual(["L7", "L8"]);
    let n = 0;
    await pipeline(
      dump.entities({ filter: { claim: "P17=Q55" } }).toReadable(),
      new Writable({
        objectMode: true,
        highWaterMark: 4,
        write(_e: EntityRecord, _enc, cb) {
          n++;
          setImmediate(cb);
        },
      }),
    );
    expect(n).toBe((9 * N) / 41);
  });

  it("reports progress with rates", async () => {
    const dump = await openDump(big);
    const progress: Progress[] = [];
    await dump.entities({ onProgress: (p) => progress.push(p), json: false }).toArray();
    const last = progress.at(-1)!;
    expect(last.entities).toBe(N);
    expect(last.bytesTotal).toBe(dump.size);
    expect(last.fraction).toBe(1);
    expect(last.mbPerSecondOut).toBeGreaterThan(0);
    expect(last.entitiesPerSecond).toBeGreaterThan(0);
  });

  it("surfaces errors thrown by the consumer and stops", async () => {
    const dump = await openDump(big);
    const stream = dump.entities({ batchSize: 10 });
    await expect(
      (async () => {
        for await (const e of stream) {
          void e;
          throw new Error("consumer failed");
        }
      })(),
    ).rejects.toThrow("consumer failed");
    expect((await stream.stats).stopped).toBe(true);
  });

  it("is an EntityStream", async () => {
    const dump = await openDump(big);
    const s = dump.entities();
    expect(s).toBeInstanceOf(EntityStream);
    s.close();
  });

  it("lets the process exit after a stream ends or is abandoned", () => {
    const entry = resolve(__dirname, "..", "dist", "index.js");
    const script = `
      const { openDump } = require(${JSON.stringify(entry)});
      (async () => {
        const dump = await openDump(${JSON.stringify(big)});
        for await (const e of dump.entities({ batchSize: 5 })) { break; }
        const all = await dump.entities({ filter: { type: "lexeme" } }).toArray();
        console.log(all.length);
        dump.entities();
        const it = dump.entities({ batchSize: 5, readAhead: 1 })[Symbol.asyncIterator]();
        await it.next();
        const closed = dump.entities({ batchSize: 5 });
        for await (const e of closed) { break; }
        console.log((await closed.stats).stopped);
      })();
    `;
    const r = spawnSync(process.execPath, ["-e", script], { encoding: "utf8", timeout: 60_000 });
    expect(r.error).toBeUndefined();
    expect(r.status).toBe(0);
    expect(r.stdout.trim().split("\n")).toEqual([String((2 * N) / 41), "true"]);
  });
});
