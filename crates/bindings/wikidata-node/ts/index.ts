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
  type OpenOptions,
  type Progress,
  type ScanStats,
} from "./types.js";

export * from "./types.js";
export { EntityStream } from "./stream.js";

const toPath = (p: string | URL): string => (p instanceof URL && p.protocol === "file:" ? fileURLToPath(p) : String(p));

function commonOptions(o: CommonOptions): NativeOptions {
  const out: NativeOptions = {};
  if (o.filter !== undefined) out.filterJson = JSON.stringify(o.filter);
  if (o.classIndex !== undefined) out.classIndex = o.classIndex._id;
  if (o.threads !== undefined) out.threads = o.threads;
  if (o.limit !== undefined) out.limit = o.limit;
  if (o.allowTruncated !== undefined) out.allowTruncated = o.allowTruncated;
  return out;
}

function entityOptions(o: EntityOptions): NativeOptions {
  const out = commonOptions(o);
  if (o.languages !== undefined) out.languages = [...o.languages];
  if (o.sitelinks !== undefined) out.sitelinks = [...o.sitelinks];
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
    const opts = entityOptions(options);
    return new EntityStream<EntityRecord>(
      (cb) => this.#native.entities(opts, cb),
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
    const opts = commonOptions(options);
    opts.output = toPath(options.output);
    if (options.level !== undefined) opts.level = options.level;
    return task<ExtractStats>((cb) => this.#native.extract(opts, cb), options);
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
    const o: { userAgent?: string; retries?: number } = {};
    if (options.userAgent !== undefined) o.userAgent = options.userAgent;
    if (options.retries !== undefined) o.retries = options.retries;
    return new DumpReader(await native.open(toPath(input), o));
  } catch (e) {
    throw new WikidataError(e instanceof Error ? e.message : String(e));
  }
}

/** {@link DumpReader.entities} on a path or URL, opening it when iteration starts. */
export function streamEntities(input: string | URL, options: EntityOptions = {}): EntityStream<EntityRecord> {
  const opts = entityOptions(options);
  return new EntityStream<EntityRecord>(
    async (cb) => (await native.open(toPath(input))).entities(opts, cb),
    decodeEntities as (b: never) => EntityRecord[],
    options.onProgress,
    options.signal,
  );
}

/** {@link DumpReader.extract} on a path or URL. */
export async function extract(input: string | URL, options: ExtractOptions): Promise<ExtractStats> {
  return (await openDump(input)).extract(options);
}

/** {@link DumpReader.fileInfo} on a path or URL. */
export async function fileInfo(input: string | URL, options: CommonOptions = {}): Promise<FileInfo> {
  return (await openDump(input)).fileInfo(options);
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
