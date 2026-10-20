import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { extract, openDump, streamEntities, WikidataError } from "../ts/index.js";
import { sample, tmp } from "./helpers.js";

describe("robustness", () => {
  let dir: string;
  let bz2: Buffer;
  beforeAll(async () => {
    dir = tmp();
    await extract(sample, { output: join(dir, "sample.json.bz2") });
    bz2 = readFileSync(join(dir, "sample.json.bz2"));
  });

  it("fails on truncated dumps, or reads them up to the cut when asked", async () => {
    const text = readFileSync(sample);
    const cut = text.subarray(0, text.lastIndexOf("\n", text.length / 2) + 1);
    const p = join(dir, "cut.json");
    writeFileSync(p, cut);
    await expect(streamEntities(p).toArray()).rejects.toThrow(/truncated/);
    const s = streamEntities(p, { allowTruncated: true });
    const got = await s.toArray();
    expect(got.length).toBeGreaterThan(5);
    expect(got.length).toBeLessThan(41);
    expect((await s.stats).truncated).toBe(true);

    const pb = join(dir, "cut.json.bz2");
    writeFileSync(pb, bz2.subarray(0, bz2.length - 30));
    await expect(streamEntities(pb).toArray()).rejects.toThrow(/truncated/);
  });

  it("fails on garbage and corrupt input", async () => {
    const g = join(dir, "garbage.json");
    writeFileSync(g, "<html>not a dump</html>\n");
    await expect(streamEntities(g).toArray()).rejects.toBeInstanceOf(WikidataError);
    const c = Buffer.from(bz2);
    c[c.length >> 1] ^= 0xff;
    const pc = join(dir, "corrupt.json.bz2");
    writeFileSync(pc, c);
    await expect(streamEntities(pc).toArray()).rejects.toThrow();
  });

  it("rejects bad options", async () => {
    const dump = await openDump(sample);
    await expect(dump.entities({ batchSize: 0 }).toArray()).rejects.toThrow(/batchSize/);
    await expect(dump.entities({ readAhead: 0 }).toArray()).rejects.toThrow(/readAhead/);
    await expect(dump.entities({ properties: ["Q5"] }).toArray()).rejects.toThrow(/property/);
    await expect(dump.entities({ limit: -1 }).toArray()).rejects.toThrow(/limit/);
  });

  it("fails the stream, not the process, when onProgress throws", async () => {
    const dump = await openDump(sample);
    const stream = dump.entities({
      onProgress: () => {
        throw new Error("progress handler broke");
      },
    });
    await expect(stream.toArray()).rejects.toThrow("progress handler broke");
  });

  it("honours an abort that happens while the stream is starting", async () => {
    const ac = new AbortController();
    const stream = streamEntities(sample, { signal: ac.signal });
    const pending = stream.toArray();
    ac.abort(new Error("changed my mind"));
    await expect(pending).rejects.toThrow("changed my mind");
  });
});

describe("http", () => {
  let server: Server;
  let base: string;
  let requests = 0;
  let body: Buffer;
  /** Responses that close the connection early, after this many bytes. */
  let dropsLeft = 0;
  beforeAll(async () => {
    const dir = tmp();
    await extract(sample, { output: join(dir, "s.json.bz2") });
    body = readFileSync(join(dir, "s.json.bz2"));
    server = createServer((req, res) => {
      requests++;
      if (req.url?.includes("missing")) {
        res.writeHead(404).end();
        return;
      }
      const m = /bytes=(\d+)-/.exec(req.headers.range ?? "");
      const start = m ? Number(m[1]) : 0;
      const rest = body.subarray(start);
      res.writeHead(m ? 206 : 200, {
        "content-length": rest.length,
        etag: '"v1"',
        ...(m ? { "content-range": `bytes ${start}-${body.length - 1}/${body.length}` } : {}),
      });
      if (requests > 1 && dropsLeft > 0) {
        dropsLeft--;
        res.write(rest.subarray(0, Math.min(500, rest.length)), () => res.destroy());
      } else {
        res.end(rest);
      }
    });
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });
  afterAll(() => {
    server.close();
  });

  it("streams a dump from a URL, resuming dropped connections", async () => {
    requests = 0;
    dropsLeft = 2;
    const dump = await openDump(`${base}/wikidata-test-all.json.bz2`);
    expect(dump.compression).toBe("bzip2");
    expect(dump.size).toBe(body.length);
    const stream = dump.entities({ json: false });
    const all = await stream.toArray();
    expect(all).toHaveLength(41);
    expect((await stream.stats).bytesIn).toBe(body.length);
    expect(requests).toBeGreaterThanOrEqual(4);
  });

  it("rejects missing URLs", async () => {
    await expect(openDump(`${base}/missing.json.bz2`)).rejects.toThrow(/404/);
  });
});
