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
import type { NativeClassIndex, NativeOptions, NativeReader } from "./native.js";
import { EntityStream } from "./stream.js";
import {
  WikidataError,
  type ClassIndexLike,
  type ClassIndexOptions,
  type CommonOptions,
  type EntityOptions,
  type EntityRecord,
  type ExtractOptions,
  type ExtractStats,
  type ExternalIdKey,
  type FileInfo,
  type ItemNumbers,
  type OpenOptions,
  type Progress,
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

/** The filter as JSON, with the typed arrays of `ids` passed aside (`{"$items": k}`). */
function filterOptions(filter: unknown, out: NativeOptions): void {
  const arrays: Uint32Array[] = [];
  out.filterJson = JSON.stringify(filter, (key, value: unknown) => {
    if (!ArrayBuffer.isView(value) || value instanceof DataView) return value;
    if (key !== "ids") throw new WikidataError(`${key}: a typed array is only accepted for ids`);
    arrays.push(itemNumbers(value as ItemNumbers));
    return { $items: arrays.length - 1 };
  });
  if (arrays.length > 0) out.idArrays = arrays;
}

function commonOptions(o: CommonOptions): NativeOptions {
  const out: NativeOptions = {};
  if (o.filter !== undefined) filterOptions(o.filter, out);
  if (o.classIndex !== undefined) out.classIndex = o.classIndex._id;
  if (o.threads !== undefined) out.threads = o.threads;
  if (o.limit !== undefined) out.limit = o.limit;
  if (o.allowTruncated !== undefined) out.allowTruncated = o.allowTruncated;
  if (o.buildClassIndex !== undefined) out.classIndexOutput = toPath(o.buildClassIndex);
  return out;
}

function openOptions(o: OpenOptions): { userAgent?: string; retries?: number } {
  const out: { userAgent?: string; retries?: number } = {};
  if (o.userAgent !== undefined) out.userAgent = o.userAgent;
  if (o.retries !== undefined) out.retries = o.retries;
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
  if (o.qualifiers !== undefined) out.qualifiers = o.qualifiers;
  if (o.references !== undefined) out.references = o.references;
  if (o.deprecated !== undefined) out.deprecated = o.deprecated;
  if (o.bestRank !== undefined) out.bestRank = o.bestRank;
  if (o.json !== undefined) out.json = o.json;
  if (o.batchSize !== undefined) out.batchSize = o.batchSize;
  if (o.readAhead !== undefined) out.readAhead = o.readAhead;
  return out;
}

/** Run a task that sends no records; resolve with its end stats. */
async function task<S>(
  start: (cb: Parameters<NativeReader["extract"]>[1]) => ReturnType<NativeReader["extract"]> | Promise<ReturnType<NativeReader["extract"]>>,
  options: { onProgress?: ((p: Progress) => void) | undefined; signal?: AbortSignal | undefined },
): Promise<S> {
  const stream = new EntityStream<never, S>(start, () => [], options.onProgress, options.signal);
  for await (const _ of stream) {
    void _;
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

  /** Build the class index of a dump (a pass that parses only P279 claims). */
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

  /** Build the class index (a pass that parses only P279 claims). */
  async classIndex(options: ClassIndexOptions = {}): Promise<ClassIndex> {
    const opts: NativeOptions = {};
    if (options.threads !== undefined) opts.threads = options.threads;
    if (options.allowTruncated !== undefined) opts.allowTruncated = options.allowTruncated;
    const stats = await task<ScanStats & { token: number }>((cb) => this.#native.buildClassIndex(opts, cb), options);
    if (stats.stopped) throw new WikidataError("building the class index was stopped");
    return new ClassIndex(native.takeClassIndex(stats.token));
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
