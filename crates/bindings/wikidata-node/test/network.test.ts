// Mirrors, downloads and the live stream, against local servers.

import { createHash } from "node:crypto";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { afterAll, describe, expect, it } from "vitest";

import { downloadDump, liveChanges, streamEntities, type DownloadProgress } from "../ts/index.js";
import { sample, tmp } from "./helpers.js";

const servers: Server[] = [];
afterAll(() => {
  for (const s of servers) s.close();
});

/** Serve files by path with `Range` support; `rate` caps the server's bytes per second. */
async function files(map: Record<string, Buffer>, rate = 0): Promise<string> {
  let next = Date.now();
  const server = createServer(async (req: IncomingMessage, res: ServerResponse) => {
    const body = map[req.url ?? ""];
    if (!body) {
      res.writeHead(404).end();
      return;
    }
    const m = /bytes=(\d+)-(\d*)/.exec(req.headers.range ?? "");
    const a = m ? Number(m[1]) : 0;
    const b = m && m[2] ? Math.min(Number(m[2]), body.length - 1) : body.length - 1;
    const headers: Record<string, string | number> = { "content-length": b + 1 - a, etag: '"v1"', "accept-ranges": "bytes" };
    if (m) headers["content-range"] = `bytes ${a}-${b}/${body.length}`;
    res.writeHead(m ? 206 : 200, headers);
    for (let at = a; at <= b; at += 16384) {
      const chunk = body.subarray(at, Math.min(at + 16384, b + 1));
      if (rate > 0) {
        const start = Math.max(next, Date.now());
        next = start + (chunk.length / rate) * 1000;
        const wait = start - Date.now();
        if (wait > 0) await new Promise((r) => setTimeout(r, wait));
      }
      if (!res.write(chunk)) await new Promise((r) => res.once("drain", r));
    }
    res.end();
  });
  servers.push(server);
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  return `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
}

describe("mirrors", () => {
  it("read one dump from two servers at once, identically", async () => {
    const body = readFileSync(sample);
    const a = await files({ "/s.json": body });
    const b = await files({ "/s.json": body });
    const one = await streamEntities(`${a}/s.json`, { json: false }).toArray();
    const two = await streamEntities(`${a}/s.json`, { mirrors: [`${b}/s.json`], json: false }).toArray();
    expect(two.map((e) => e.id)).toEqual(one.map((e) => e.id));
    expect(two.length).toBe(41);
  });
});

describe("downloadDump", () => {
  it("downloads the newest complete dump from two mirrors, verified, and resumes", async () => {
    const body = Buffer.alloc(3 << 20);
    for (let i = 0; i < body.length; i++) body[i] = (i * 2654435761) >>> 24;
    const sha1 = createHash("sha1").update(body).digest("hex");
    const site = {
      "/entities/": Buffer.from('<a href="20261012/">20261012/</a> <a href="20261019/">20261019/</a>'),
      "/entities/20261012/wikidata-20261012-sha1sums.txt": Buffer.from(`${sha1}  wikidata-20261012-all.json.bz2\n`),
      "/entities/20261019/wikidata-20261019-sha1sums.txt": Buffer.from("x  wikidata-20261019-all.json.gz\n"),
      "/entities/20261012/wikidata-20261012-all.json.bz2": body,
    };
    const a = await files(site, 2 << 20);
    const b = await files(site, 2 << 20);
    const dir = tmp();
    const options = {
      dir,
      base: `${a}/entities/`,
      mirrors: [`${b}/entities/`],
      rangeSize: 256 << 10,
    };
    // Stopped half way…
    const ac = new AbortController();
    let seen: DownloadProgress | undefined;
    await expect(
      downloadDump({
        ...options,
        signal: ac.signal,
        onProgress: (p) => {
          seen = p;
          if (p.bytesDone > 1 << 20) ac.abort(new Error("stop"));
        },
      }),
    ).rejects.toThrow("stop");
    expect(seen!.bytesTotal).toBe(body.length);
    const final = join(dir, "wikidata-20261012-all.json.bz2");
    expect(existsSync(final)).toBe(false);
    // …and resumed.
    const r = await downloadDump(options);
    expect(r.path).toBe(final);
    expect(r.sha1).toBe(sha1);
    expect(r.bytesResumed).toBeGreaterThan(0);
    expect(r.perServer.every((n) => n > 0)).toBe(true);
    expect(readFileSync(final).equals(body)).toBe(true);
    expect((await downloadDump(options)).alreadyDone).toBe(true);
  });
});

describe("liveChanges", () => {
  it("streams the changes to the asked items, resuming after the last one", async () => {
    let connections = 0;
    const event = (i: number): string =>
      `event: message\nid: ${i}\ndata: ${JSON.stringify({
        wiki: i % 3 === 0 ? "enwiki" : "wikidatawiki",
        namespace: 0,
        title: `Q${40 + (i % 4)}`,
        type: "edit",
        timestamp: 1792800000 + i,
        user: "Example",
        bot: false,
        comment: "",
        revision: { old: 1000 + i - 1, new: 1000 + i },
      })}\n\n`;
    const server = createServer((req, res) => {
      connections++;
      const last = Number(req.headers["last-event-id"] ?? 0);
      res.writeHead(200, { "content-type": "text/event-stream" });
      // The first connection drops after a few events.
      const end = connections === 1 ? last + 5 : 30;
      for (let i = last + 1; i <= end; i++) res.write(event(i));
      if (connections === 1) res.destroy();
    });
    servers.push(server);
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}/v2/stream/recentchange`;
    const want = Array.from({ length: 30 }, (_, k) => k + 1).filter((i) => i % 3 !== 0 && 40 + (i % 4) === 42);
    const got: number[] = [];
    const stream = liveChanges({ url, ids: ["Q42"] });
    for await (const c of stream) {
      expect(c.id).toBe("Q42");
      got.push(Number(c.eventId));
      if (got.length === want.length) break;
    }
    expect(got).toEqual(want);
    expect(connections).toBeGreaterThanOrEqual(2);
    // From a stored event id, by item number.
    const after: number[] = [];
    for await (const c of liveChanges({ url, ids: Uint32Array.of(42), lastEventId: "20" })) {
      after.push(Number(c.eventId));
      if (after.length === want.filter((i) => i > 20).length) break;
    }
    expect(after).toEqual(want.filter((i) => i > 20));
  });
});
