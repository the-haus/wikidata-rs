/**
 * @wikidata-rs/node — stream Wikidata JSON dumps.
 *
 * ```ts
 * import { openDump } from "@wikidata-rs/node";
 *
 * const dump = await openDump("wikidata-20261012-all.json.bz2"); // or an https:// URL
 * for await (const e of dump.entities({ filter: { claim: "P17=Q55" }, languages: ["nl", "en", "mul"] })) {
 *   // e.id, e.label, e.instanceOf, e.coordinates, e.externalIds … without parsing JSON;
 *   // e.entity: the full, typed entity, parsed on first access
 * }
 * ```
 *
 * Decompression (bzip2 on all cores), filtering and parsing run on native
 * threads; the JS thread only receives finished batches. Memory is
 * bounded: the producer never gets more than `readAhead` batches ahead of
 * the consumer.
 */

import { fileURLToPath } from "node:url";

import { decodeEntities, EXTERNAL_ID_KEYS } from "./decode.js";
import * as native from "./native.js";
import type {
  NativeClassIndex,
  NativeJsonBatch,
  NativeOpenOptions,
  NativeOptions,
  NativePlaceIndex,
  NativeReader,
  NativeRedirectIndex,
} from "./native.js";
import { EntityStream, type StreamHooks } from "./stream.js";
import {
  WikidataError,
  type ClassIndexLike,
  type ClassIndexOptions,
  type CommonOptions,
  type DownloadOptions,
  type DownloadProgress,
  type DownloadResult,
  type DumpStatus,
  type LatestDumpOptions,
  type SqlDownloadOptions,
  type EntityOptions,
  type EntityRecord,
  type ExtractOptions,
  type ExtractStats,
  type ExternalIdKey,
  type FileInfo,
  type ItemNumbers,
  type LiveChange,
  type LiveOptions,
  type LiveStats,
  type OpenOptions,
  type PlaceIndexLike,
  type PlaceIndexOptions,
  type Progress,
  type RedirectIndexLike,
  type RedirectIndexOptions,
  type Revisions,
  type ScanStats,
} from "./types.js";

export * from "./types.js";
export { EntityStream } from "./stream.js";

const toPath = (p: string | URL): string => (p instanceof URL && p.protocol === "file:" ? fileURLToPath(p) : String(p));

/** Item numbers as a `Uint32Array` (`Uint32Array` and `Int32Array` without a copy). */
function itemNumbers(a: ItemNumbers): Uint32Array {
  const bad = (n: unknown): never => {
    throw new WikidataError(`ids: not an item number: ${String(n)}`);
  };
  if (a instanceof Uint32Array) return a;
  if (a instanceof Int32Array) {
    for (const n of a) if (n < 0) bad(n);
    return new Uint32Array(a.buffer, a.byteOffset, a.length);
  }
  const out = new Uint32Array(a.length);
  for (let i = 0; i < a.length; i++) {
    const n = Number(a[i]);
    if (!Number.isInteger(n) || n < 0 || n > 0xffffffff) bad(a[i]);
    out[i] = n;
  }
  return out;
}

/** Revision ids as a `Float64Array` (exact up to 2^53). */
function revisionNumbers(a: Revisions): Float64Array {
  if (a instanceof Float64Array) return a;
  const out = new Float64Array(a.length);
  for (let i = 0; i < a.length; i++) {
    const n = Number(a[i]);
    if (!Number.isSafeInteger(n) || n < 0) throw new WikidataError(`revisions: not a revision id: ${String(a[i])}`);
    out[i] = n;
  }
  return out;
}

/**
 * The filter as JSON, with the typed arrays of `ids` and `revisions`
 * passed aside (`{"$items": k}`, `{"$revisions": k}`), and `ids` maps as
 * objects.
 */
function filterOptions(filter: unknown, out: NativeOptions): void {
  const arrays: Uint32Array[] = [];
  const revisions: Float64Array[] = [];
  out.filterJson = JSON.stringify(filter, (key, value: unknown) => {
    if (key === "ids" && value instanceof Map) return Object.fromEntries(value);
    if (key === "revisions" && (Array.isArray(value) || ArrayBuffer.isView(value))) {
      revisions.push(revisionNumbers(value as Revisions));
      return { $revisions: revisions.length - 1 };
    }
    if (!ArrayBuffer.isView(value) || value instanceof DataView) return value;
    if (key !== "ids") throw new WikidataError(`${key}: a typed array is only accepted for ids and revisions`);
    arrays.push(itemNumbers(value as ItemNumbers));
    return { $items: arrays.length - 1 };
  });
  if (arrays.length > 0) out.idArrays = arrays;
  if (revisions.length > 0) out.revisionArrays = revisions;
}

function commonOptions(o: CommonOptions): NativeOptions {
  const out: NativeOptions = {};
  if (o.filter !== undefined) filterOptions(o.filter, out);
  if (o.classIndex !== undefined) out.classIndex = o.classIndex._id;
  if (o.placeIndex !== undefined) out.placeIndex = o.placeIndex._placeId;
  if (o.redirects !== undefined) out.redirectIndex = o.redirects._redirectId;
  if (o.buildPlaceIndex !== undefined) out.placeIndexOutput = toPath(o.buildPlaceIndex);
  if (o.threads !== undefined) out.threads = o.threads;
  if (o.limit !== undefined) out.limit = o.limit;
  if (o.allowTruncated !== undefined) out.allowTruncated = o.allowTruncated;
  if (o.buildClassIndex !== undefined) out.classIndexOutput = toPath(o.buildClassIndex);
  return out;
}

function openOptions(o: OpenOptions): NativeOpenOptions {
  const out: NativeOpenOptions = {};
  if (o.userAgent !== undefined) out.userAgent = o.userAgent;
  if (o.retries !== undefined) out.retries = o.retries;
  if (o.mirrors !== undefined) out.mirrors = [...o.mirrors];
  if (o.connections !== undefined) out.connections = o.connections;
  return out;
}

function entityOptions(o: EntityOptions): NativeOptions {
  const out = commonOptions(o);
  if (Array.isArray(o.languages)) out.languages = [...o.languages];
  else if (o.languages !== undefined) out.languagesByJson = JSON.stringify(o.languages);
  if (Array.isArray(o.sitelinks)) out.sitelinks = [...o.sitelinks];
  else if (o.sitelinks !== undefined) out.sitelinksByJson = JSON.stringify(o.sitelinks);
  if (o.properties !== undefined) out.properties = [...o.properties];
  if (o.labels !== undefined) out.labels = o.labels;
  if (o.descriptions !== undefined) out.descriptions = o.descriptions;
  if (o.aliases !== undefined) out.aliases = o.aliases;
  if (Array.isArray(o.qualifiers)) {
    out.qualifiers = true;
    out.qualifierProperties = [...o.qualifiers];
  } else if (o.qualifiers !== undefined) out.qualifiers = o.qualifiers;
  if (o.references !== undefined) out.references = o.references;
  if (o.deprecated !== undefined) out.deprecated = o.deprecated;
  if (o.bestRank !== undefined) out.bestRank = o.bestRank;
  if (o.json !== undefined) out.json = o.json;
  if (o.batchSize !== undefined) out.batchSize = o.batchSize;
  if (o.readAhead !== undefined) out.readAhead = o.readAhead;
  if (o.checkpointEvery !== undefined) out.checkpointEvery = o.checkpointEvery;
  if (o.resumeFrom !== undefined) out.resume = Buffer.from(o.resumeFrom.data, "base64");
  if (o.referencedLabels !== undefined) {
    const { languages, output } = o.referencedLabels;
    out.referencedLabelsJson = JSON.stringify(output === undefined ? { languages } : { languages, output: toPath(output) });
  }
  return out;
}

function entityHooks(o: EntityOptions): StreamHooks {
  return { onCheckpoint: o.onCheckpoint };
}

/** Run a task that sends no records; resolve with its end stats. */
async function task<S>(
  start: (cb: Parameters<NativeReader["extract"]>[1]) => ReturnType<NativeReader["extract"]> | Promise<ReturnType<NativeReader["extract"]>>,
  options: { onProgress?: ((p: Progress) => void) | undefined; signal?: AbortSignal | undefined },
  hooks: StreamHooks = {},
): Promise<S> {
  const stream = new EntityStream<never, S>(start, () => [], options.onProgress, options.signal, undefined, hooks);
  try {
    for await (const _ of stream) {
      void _;
    }
  } catch (e) {
    if (hooks.holdUntilEnd) await stream._ended();
    throw e;
  }
  return stream.stats;
}

/** A class index: the P279 (subclass of) hierarchy, for `instanceOf` filters with `subclasses: true`. */
export class ClassIndex implements ClassIndexLike {
  readonly #native: NativeClassIndex;

  /** @internal */
  constructor(n: NativeClassIndex) {
    this.#native = n;
  }

  /** Build the class index of a dump (a pass that parses only P279 claims; with `buildPlaceIndex`, the place index too). */
  static async build(input: string | URL, options: ClassIndexOptions & OpenOptions = {}): Promise<ClassIndex> {
    return (await openDump(input, options)).classIndex(options);
  }

  /** Load a file written by {@link ClassIndex.save} (or `wikidata classes -o`). */
  static async load(path: string | URL): Promise<ClassIndex> {
    try {
      return new ClassIndex(await native.loadClassIndex(toPath(path)));
    } catch (e) {
      throw new WikidataError(e instanceof Error ? e.message : String(e));
    }
  }

  /** @internal */
  get _id(): number {
    return this.#native.id;
  }

  /** Classes with at least one parent. */
  get size(): number {
    return this.#native.size;
  }

  /** Subclass-of edges. */
  get edges(): number {
    return this.#native.edges;
  }

  /** The dump it was built from. */
  get source(): string {
    return this.#native.source;
  }

  /** Direct parents of a class. */
  parents(id: string): string[] {
    return this.#native.parents(id);
  }

  /** The classes and all their subclasses. */
  subclassesOf(ids: string | string[]): string[] {
    return this.#native.subclassesOf(Array.isArray(ids) ? ids : [ids]);
  }

  /** Whether `id` is `ancestor` or one of its subclasses. */
  isSubclassOf(id: string, ancestor: string): boolean {
    return this.#native.isSubclassOf(id, ancestor);
  }

  /** Write to a file (zstd-compressed arrays, about 4 bytes an edge). */
  save(path: string | URL): void {
    this.#native.save(toPath(path));
  }
}

/**
 * A place index: each item's countries, by its P17, its P131 chain
 * (followed up, cycle-safe) and its P159. For `inCountry` filters, and
 * for `languages: { by: "P17", … }` rules (an item that names only its
 * place gets its country's languages).
 */
export class PlaceIndex implements PlaceIndexLike {
  readonly #native: NativePlaceIndex;

  /** @internal */
  constructor(n: NativePlaceIndex) {
    this.#native = n;
  }

  /** Build the place index of a dump (a pass that parses only P17, P131 and P159; with `buildClassIndex`, the class index too). */
  static async build(input: string | URL, options: PlaceIndexOptions & OpenOptions = {}): Promise<PlaceIndex> {
    return (await openDump(input, options)).placeIndex(options);
  }

  /** Load a file written by {@link PlaceIndex.save} (or `wikidata places -o`). */
  static async load(path: string | URL): Promise<PlaceIndex> {
    try {
      return new PlaceIndex(await native.loadPlaceIndex(toPath(path)));
    } catch (e) {
      throw new WikidataError(e instanceof Error ? e.message : String(e));
    }
  }

  /** @internal */
  get _placeId(): number {
    return this.#native.id;
  }

  /** Items with a country. */
  get size(): number {
    return this.#native.size;
  }

  /** Distinct sets of countries. */
  get sets(): number {
    return this.#native.sets;
  }

  /** Bytes in memory. */
  get heapBytes(): number {
    return this.#native.heapBytes;
  }

  /** The dump it was built from. */
  get source(): string {
    return this.#native.source;
  }

  /** The countries of an item: its P17, then through its P131 places, then its P159's (`[]` when unknown). */
  countryOf(id: string): string[] {
    return this.#native.countryOf(id);
  }

  /** {@link countryOf} for many items (by number) at once. */
  countriesOf(items: ItemNumbers): string[][] {
    const { offsets, countries } = this.#native.countriesOf(itemNumbers(items));
    const out: string[][] = [];
    for (let i = 0; i + 1 < offsets.length; i++) {
      const list: string[] = [];
      for (let k = offsets[i]!; k < offsets[i + 1]!; k++) list.push(`Q${countries[k]!}`);
      out.push(list);
    }
    return out;
  }

  /** Write to a file (zstd-compressed arrays). */
  save(path: string | URL): void {
    this.#native.save(toPath(path));
  }
}

/**
 * Item redirects (merges), from the SQL dumps
 * (`wikidatawiki-<date>-redirect.sql.gz` joined with `…-page.sql.gz`).
 * Pass it as `redirects` to follow merges in a pass.
 */
export class RedirectIndex implements RedirectIndexLike {
  readonly #native: NativeRedirectIndex;

  /** @internal */
  constructor(n: NativeRedirectIndex) {
    this.#native = n;
  }

  /** Build from the `redirect` and `page` SQL dumps (paths or URLs). */
  static async build(options: RedirectIndexOptions): Promise<RedirectIndex> {
    const stats = await task<{ token: number }>(
      (cb) => {
        const o: Parameters<typeof native.buildRedirects>[0] = {
          redirects: toPath(options.redirects),
          pages: toPath(options.pages),
          open: openOptions(options),
        };
        if (options.threads !== undefined) o.threads = options.threads;
        return native.buildRedirects(o, cb);
      },
      options,
    );
    if ((stats as { stopped?: boolean }).stopped) throw new WikidataError("building the redirect index was stopped");
    return new RedirectIndex(native.takeRedirectIndex(stats.token));
  }

  /** Load a file written by {@link RedirectIndex.save} (or `wikidata redirects -o`). */
  static async load(path: string | URL): Promise<RedirectIndex> {
    try {
      return new RedirectIndex(await native.loadRedirectIndex(toPath(path)));
    } catch (e) {
      throw new WikidataError(e instanceof Error ? e.message : String(e));
    }
  }

  /** @internal */
  get _redirectId(): number {
    return this.#native.id;
  }

  /** Redirects. */
  get size(): number {
    return this.#native.size;
  }

  /** The SQL dumps it was built from. */
  get source(): string {
    return this.#native.source;
  }

  /** Where an item was merged into (following double redirects); `null` when it is not a redirect. */
  resolve(id: string): string | null {
    return this.#native.resolve(id);
  }

  /** Every redirect, `{ from: "Q1", to: "Q2" }`, in `from` order. */
  *entries(): Generator<{ from: string; to: string }, void, undefined> {
    const { from, to } = this.#native.pairs();
    for (let i = 0; i < from.length; i++) yield { from: `Q${from[i]!}`, to: `Q${to[i]!}` };
  }

  /** Write to a file (zstd-compressed arrays). */
  save(path: string | URL): void {
    this.#native.save(toPath(path));
  }
}

/** The item redirects of the SQL dumps, `{ from, to }` (see {@link RedirectIndex.build}). */
export async function* readRedirects(
  options: RedirectIndexOptions,
): AsyncGenerator<{ from: string; to: string }, void, undefined> {
  yield* (await RedirectIndex.build(options)).entries();
}

/**
 * The entities edited on Wikidata, as they are edited (Wikimedia's
 * EventStreams `recentchange`), filtered natively by `ids`. Every change
 * carries its `eventId`: pass the last one as `lastEventId` to resume
 * right after it. Dropped connections resume the same way; nothing is
 * missed or repeated. Runs until closed (`break`, `close()`, a signal).
 */
export function liveChanges(options: LiveOptions = {}): EntityStream<LiveChange, LiveStats> {
  return new EntityStream<LiveChange, LiveStats>(
    (cb) => {
      const o: Parameters<typeof native.live>[0] = { open: openOptions(options) };
      if (options.ids !== undefined) {
        if (Array.isArray(options.ids)) o.ids = [...options.ids];
        else o.items = itemNumbers(options.ids);
      }
      if (options.lastEventId !== undefined) o.lastEventId = options.lastEventId;
      if (options.since !== undefined) o.since = options.since instanceof Date ? options.since.toISOString() : options.since;
      if (options.namespaces !== undefined) o.namespaces = [...options.namespaces];
      if (options.wiki !== undefined) o.wiki = options.wiki;
      if (options.url !== undefined) o.url = options.url;
      return native.live(o, cb);
    },
    ((b: NativeJsonBatch) => JSON.parse(b.json) as LiveChange[]) as (b: never) => LiveChange[],
    undefined,
    options.signal,
    () => ({ events: 0, changes: 0, reconnects: 0, lastEventId: null }),
  );
}

/**
 * Download a dump: the newest complete one (or `date`), in ranges from
 * the base and every mirror at once, into one preallocated `.part` file;
 * resumable (run it again after a stop or a crash), SHA-1 verified, and
 * renamed to its final name only once verified.
 */
export async function downloadDump(options: DownloadOptions): Promise<DownloadResult> {
  return task<DownloadResult>(
    (cb) => {
      const open = openOptions(options);
      if (options.rangeSize !== undefined) open.rangeSize = options.rangeSize;
      const o: Parameters<typeof native.downloadDump>[0] = { dir: toPath(options.dir), open };
      if (options.date !== undefined && options.date !== "latest") o.date = options.date;
      if (options.base !== undefined) o.base = options.base;
      if (options.mirrors !== undefined) o.mirrors = [...options.mirrors];
      if (options.file !== undefined) o.file = options.file;
      if (options.verify !== undefined) o.verify = options.verify;
      return native.downloadDump(o, cb);
    },
    { signal: options.signal },
    { onStatus: options.onProgress as ((s: unknown) => void) | undefined, holdUntilEnd: true },
  );
}

/**
 * Download SQL tables of a database dump
 * (`wikidatawiki-<date>-redirect.sql.gz` and `…-page.sql.gz` by
 * default, what {@link RedirectIndex.build} reads), each as
 * {@link downloadDump} does a dump: the newest date whose tables are all
 * done (from its `dumpstatus.json`) or `date`, from the base and every
 * mirror at once, resumable, SHA-1 verified, renamed into place when
 * verified.
 */
export async function downloadSqlDumps(options: SqlDownloadOptions): Promise<{ date: string; files: DownloadResult[] }> {
  return task<{ date: string; files: DownloadResult[] }>(
    (cb) => {
      const open = openOptions(options);
      if (options.rangeSize !== undefined) open.rangeSize = options.rangeSize;
      const o: Parameters<typeof native.downloadDump>[0] = {
        dir: toPath(options.dir),
        open,
        tables: [...(options.tables ?? ["redirect", "page"])],
      };
      if (options.date !== undefined && options.date !== "latest") o.date = options.date;
      if (options.base !== undefined) o.base = options.base;
      if (options.mirrors !== undefined) o.mirrors = [...options.mirrors];
      if (options.verify !== undefined) o.verify = options.verify;
      return native.downloadDump(o, cb);
    },
    { signal: options.signal },
    { onStatus: options.onProgress as ((s: unknown) => void) | undefined, holdUntilEnd: true },
  );
}

/**
 * The newest complete dump: the entity dump (the newest date whose
 * `sha1sums.txt` lists `file`), or, with `tables`, the database dump whose
 * tables are all done (its `dumpstatus.json`); its date and files with
 * their URLs and SHA-1s.
 */
export async function latestDump(options: LatestDumpOptions = {}): Promise<DumpStatus> {
  const o: Parameters<typeof native.latestDump>[0] = { dir: ".", open: openOptions(options) };
  if (options.tables !== undefined) o.tables = [...options.tables];
  if (options.file !== undefined) o.file = options.file;
  if (options.base !== undefined) o.base = options.base;
  try {
    return (await native.latestDump(o)) as DumpStatus;
  } catch (e) {
    throw new WikidataError(e instanceof Error ? e.message : String(e));
  }
}

/** An open dump. Create one with {@link openDump}. */
export class DumpReader {
  readonly #native: NativeReader;

  /** @internal */
  constructor(n: NativeReader) {
    this.#native = n;
  }

  /** The path or URL. */
  get input(): string {
    return this.#native.input;
  }

  /** `bzip2`, `gzip`, `zstd` or `none` (from the first bytes). */
  get compression(): string {
    return this.#native.compression;
  }

  /** Size in bytes, when known. */
  get size(): number | null {
    return this.#native.size ?? null;
  }

  /**
   * Stream the matching entities, in dump order. Iterate record by record
   * (`for await (const e of stream)`) or batch by batch (`stream.batches()`).
   */
  entities(options: EntityOptions = {}): EntityStream<EntityRecord> {
    // Options are converted when the stream starts, so that bad ones fail it.
    return new EntityStream<EntityRecord>(
      (cb) => this.#native.entities(entityOptions(options), cb),
      decodeEntities as (b: never) => EntityRecord[],
      options.onProgress,
      options.signal,
      undefined,
      entityHooks(options),
    );
  }

  /**
   * Write the matching entities' original lines to a smaller dump that
   * {@link openDump} reads back like the original: `.jsonl.zst`
   * (recommended), `.json.bz2` (Wikimedia's own layout), `.json.gz`, …
   */
  extract(options: ExtractOptions): Promise<ExtractStats> {
    return task<ExtractStats>((cb) => {
      const opts = commonOptions(options);
      opts.output = toPath(options.output);
      if (options.level !== undefined) opts.level = options.level;
      return this.#native.extract(opts, cb);
    }, options);
  }

  /** Count what the dump holds (a full pass, or `limit` entities). */
  fileInfo(options: CommonOptions = {}): Promise<FileInfo> {
    return task<FileInfo>((cb) => this.#native.fileInfo(commonOptions(options), cb), options);
  }

  /** Build the class index (a pass that parses only P279 claims; with `buildPlaceIndex`, the place index too). */
  async classIndex(options: ClassIndexOptions = {}): Promise<ClassIndex> {
    const opts: NativeOptions = {};
    if (options.threads !== undefined) opts.threads = options.threads;
    if (options.allowTruncated !== undefined) opts.allowTruncated = options.allowTruncated;
    if (options.buildPlaceIndex !== undefined) opts.placeIndexOutput = toPath(options.buildPlaceIndex);
    const stats = await task<ScanStats & { token: number }>((cb) => this.#native.buildClassIndex(opts, cb), options);
    if (stats.stopped) throw new WikidataError("building the class index was stopped");
    return new ClassIndex(native.takeClassIndex(stats.token));
  }

  /** Build the place index (a pass that parses only P17, P131 and P159; with `buildClassIndex`, the class index too). */
  async placeIndex(options: PlaceIndexOptions = {}): Promise<PlaceIndex> {
    const opts: NativeOptions = {};
    if (options.threads !== undefined) opts.threads = options.threads;
    if (options.allowTruncated !== undefined) opts.allowTruncated = options.allowTruncated;
    if (options.buildClassIndex !== undefined) opts.classIndexOutput = toPath(options.buildClassIndex);
    const stats = await task<ScanStats & { token: number }>((cb) => this.#native.buildPlaceIndex(opts, cb), options);
    if (stats.stopped) throw new WikidataError("building the place index was stopped");
    return new PlaceIndex(native.takePlaceIndex(stats.token));
  }
}

/** Open a dump: a path, a `file:` URL, or an `http(s)://` URL (streamed, resumed on dropped connections). */
export async function openDump(input: string | URL, options: OpenOptions = {}): Promise<DumpReader> {
  try {
    return new DumpReader(await native.open(toPath(input), openOptions(options)));
  } catch (e) {
    throw new WikidataError(e instanceof Error ? e.message : String(e));
  }
}

/** {@link DumpReader.entities} on a path or URL, opening it when iteration starts. */
export function streamEntities(
  input: string | URL,
  options: EntityOptions & OpenOptions = {},
): EntityStream<EntityRecord> {
  return new EntityStream<EntityRecord>(
    async (cb) => (await native.open(toPath(input), openOptions(options))).entities(entityOptions(options), cb),
    decodeEntities as (b: never) => EntityRecord[],
    options.onProgress,
    options.signal,
    undefined,
    entityHooks(options),
  );
}

/** {@link DumpReader.extract} on a path or URL. */
export async function extract(input: string | URL, options: ExtractOptions & OpenOptions): Promise<ExtractStats> {
  return (await openDump(input, options)).extract(options);
}

/** {@link DumpReader.fileInfo} on a path or URL. */
export async function fileInfo(input: string | URL, options: CommonOptions & OpenOptions = {}): Promise<FileInfo> {
  return (await openDump(input, options)).fileInfo(options);
}

/** The identifiers collected into `externalIds`: key, Wikidata property and URL formatter (`$1`: the value). */
export const EXTERNAL_IDS: ReadonlyArray<{ key: ExternalIdKey; property: string; formatter: string }> = native
  .externalIdTable()
  .map(([key, property, formatter]) => ({ key: key as ExternalIdKey, property: property!, formatter: formatter! }));

/** The URL of an external identifier (`externalIdUrl("musicbrainzArtist", "…")`). */
export function externalIdUrl(key: ExternalIdKey, value: string): string | null {
  const row = EXTERNAL_IDS.find((r) => r.key === key);
  return row ? row.formatter.replace("$1", value.replace(/\s+/g, "")) : null;
}

/** The library version. */
export function version(): string {
  return native.version();
}

/** @internal Native worker threads still running (0 once every task ended). */
export function activeThreads(): number {
  return native.activeThreads();
}

void EXTERNAL_ID_KEYS;
