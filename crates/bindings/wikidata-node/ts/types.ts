// Public types. The entity shapes mirror the JSON the Rust side writes
// (wikidata-model's `Entity`): stable keys, typed values.

/** Errors from this package (bad options, unreadable or truncated dumps, aborts). */
export class WikidataError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "WikidataError";
  }
}

/** Entity kinds. */
export type EntityType = "item" | "property" | "lexeme" | "mediainfo" | "entity-schema";

/** A text in a language. */
export interface Text {
  text: string;
  language: string;
}

/** Statement rank. */
export type Rank = "preferred" | "normal" | "deprecated";

/** A point in time. */
export interface TimeValue {
  type: "time";
  /** The raw timestamp, e.g. `+1830-10-04T00:00:00Z`. */
  time: string;
  /** The date at its precision: `1830-10-04`, `1830-10` or `1830` (`-0490` before the common era). */
  iso: string;
  /** Wikidata's precision code: 11 day, 10 month, 9 year, 8 decade, 7 century, 6 millennium, … */
  precision: number;
  /** `day`, `month`, `year`, `decade`, `century`, `millennium`, `tenThousandYears`, … */
  precisionName: string;
  year: number;
  month?: number;
  day?: number;
  /** `gregorian`, `julian`, or the calendar's item id. */
  calendar: string;
}

/** An amount. */
export interface QuantityValue {
  type: "quantity";
  amount: number;
  /** The unit's item id, `null` for a pure number. */
  unit: string | null;
  lower?: number;
  upper?: number;
}

/** A point on a globe. */
export interface CoordinateValue {
  type: "coordinate";
  lat: number;
  lon: number;
  precision?: number;
  /** The globe's item id (`Q2`: the Earth). */
  globe: string;
}

/** A typed claim value. */
export type Value =
  | { type: "entity"; id: string }
  | { type: "string"; value: string }
  | { type: "externalId"; value: string }
  | { type: "url"; value: string }
  | { type: "media"; file: string; url: string }
  | { type: "monolingual"; text: string; language: string }
  | TimeValue
  | QuantityValue
  | CoordinateValue
  | { type: "math"; value: string }
  | { type: "musicalNotation"; value: string }
  | { type: "geoShape"; value: string; url: string }
  | { type: "tabularData"; value: string; url: string }
  | { type: "someValue" }
  | { type: "noValue" };

/** Property id → values (qualifiers, reference snaks). */
export type Snaks = Record<string, Value[]>;

/** A statement. */
export interface Statement {
  value: Value;
  rank: Rank;
  /** Present when the statement has qualifiers (and `qualifiers` is on). */
  qualifiers?: Snaks;
  /** Present with `references: true`. */
  references?: Snaks[];
}

/** A sitelink. */
export interface Sitelink {
  title: string;
  url: string | null;
  badges: string[];
}

/** A lexeme form. */
export interface Form {
  id: string;
  representations: Record<string, string>;
  grammaticalFeatures: string[];
  claims: Record<string, Statement[]>;
}

/** A lexeme sense. */
export interface Sense {
  id: string;
  glosses: Record<string, string>;
  claims: Record<string, Statement[]>;
}

/** A normalised entity: the full data of an {@link EntityRecord}. */
export interface Entity {
  id: string;
  type: EntityType;
  /** Properties only. */
  datatype?: string | null;
  /** The label in the first language of the fallback chain that has one. */
  label: Text | null;
  description: Text | null;
  instanceOf: string[];
  subclassOf: string[];
  country: string[];
  coordinates: { lat: number; lon: number } | null;
  inception: number | null;
  dissolved: number | null;
  image: string | null;
  website: string | null;
  externalIds: Partial<Record<ExternalIdKey, string>>;
  labels: Record<string, string>;
  descriptions: Record<string, string>;
  aliases: Record<string, string[]>;
  sitelinks: Record<string, Sitelink>;
  claims: Record<string, Statement[]>;
  /** Lexemes only. */
  lemmas?: Record<string, string>;
  lexicalCategory?: string | null;
  language?: string | null;
  forms?: Form[];
  senses?: Sense[];
  lastrevid: number | null;
  modified: string | null;
}

/** Keys of `externalIds` (see {@link EXTERNAL_IDS}). */
export type ExternalIdKey =
  | "musicbrainzArtist"
  | "musicbrainzWork"
  | "musicbrainzReleaseGroup"
  | "musicbrainzRelease"
  | "musicbrainzRecording"
  | "musicbrainzLabel"
  | "musicbrainzArea"
  | "musicbrainzPlace"
  | "musicbrainzEvent"
  | "musicbrainzSeries"
  | "discogsArtist"
  | "discogsMaster"
  | "discogsLabel"
  | "spotifyArtist"
  | "spotifyAlbum"
  | "spotifyTrack"
  | "appleMusicArtist"
  | "deezerArtist"
  | "lastfm"
  | "soundcloud"
  | "bandcamp"
  | "songkickArtist"
  | "geniusArtist"
  | "allmusicArtist"
  | "osmRelation"
  | "osmWay"
  | "osmNode"
  | "geonames"
  | "isni"
  | "viaf"
  | "gnd"
  | "loc"
  | "freebase"
  | "imdb"
  | "youtubeChannel"
  | "instagram"
  | "x"
  | "facebook";

/**
 * One matching entity. The summary fields are read from columnar data
 * without parsing any JSON; {@link EntityRecord.entity} parses the full
 * entity on first access.
 */
export interface EntityRecord {
  readonly id: string;
  readonly type: EntityType;
  readonly label: Text | null;
  readonly description: Text | null;
  /** P31, truthy statements. */
  readonly instanceOf: string[];
  /** P279, truthy statements. */
  readonly subclassOf: string[];
  /** P17. */
  readonly country: string[];
  /** P625, the first truthy one. */
  readonly coordinates: { lat: number; lon: number } | null;
  /** P571, year. */
  readonly inception: number | null;
  /** P576, year. */
  readonly dissolved: number | null;
  /** P18, a Commons file name. */
  readonly image: string | null;
  /** P856. */
  readonly website: string | null;
  readonly externalIds: Partial<Record<ExternalIdKey, string>>;
  /**
   * The named filters that match it (`{ any: { venue: …, festival: … } }`
   * or `{ name: "venue", … }`), every one of them, wherever they are in
   * the filter; `[]` when the filter names none.
   */
  readonly matchedBy: string[];
  /** The full entity (parsed once, on first access; needs `json: true`, the default). */
  readonly entity: Entity;
  /** The full entity's JSON text. */
  readonly json: string;
  /** The summary fields as a plain object. */
  toJSON(): Omit<EntityRecord, "entity" | "json" | "toJSON">;
}

/**
 * Item numbers (`42` for `Q42`) in a typed array, for large `ids` filters:
 * passed to the native side as numbers, not as a million strings.
 * `Uint32Array` and `Int32Array` are passed without a copy.
 */
export type ItemNumbers =
  | Uint32Array
  | Int32Array
  | Uint16Array
  | Int16Array
  | Uint8Array
  | Int8Array
  | Float64Array
  | Float32Array
  | BigUint64Array
  | BigInt64Array;

/**
 * Which entities match. Objects with several keys, and arrays, mean all
 * of them. `any` and `all` also take an object of named filters, and any
 * filter can carry a `name`: an {@link EntityRecord}'s `matchedBy` lists
 * the named filters that match it.
 *
 * ```ts
 * { claim: "P17=Q55" }
 * { instanceOf: { classes: ["Q868557"], subclasses: true } }   // needs classIndex
 * { any: [{ hasProperty: ["P434"] }, { sitelink: "nlwiki" }] }
 * { any: { venue: { instanceOf: "Q8719053" }, artist: { hasProperty: "P434" } } }
 * { ids: Uint32Array.of(42, 64) }                                // Q42, Q64
 * ```
 */
export type Filter =
  | Filter[]
  | {
      /** Reported in `matchedBy` when this filter matches. */
      name?: string;
      all?: Filter[] | Record<string, Filter>;
      any?: Filter[] | Record<string, Filter>;
      not?: Filter;
      ids?: string | string[] | ItemNumbers;
      type?: EntityType | EntityType[];
      hasProperty?: string | string[];
      claim?: string | { property: string; values?: string[] } | Record<string, string[]>;
      instanceOf?: string | string[] | { classes: string | string[]; subclasses?: boolean };
      sitelink?: string | string[];
    };

/** Progress of a task. */
export interface NativeProgress {
  /** Bytes read from the file or URL. */
  bytesIn: number;
  /** The input's size, when known. */
  bytesTotal: number | null;
  /** Decompressed bytes. */
  bytesOut: number;
  /** Entities seen. */
  entities: number;
  /** Entities that matched. */
  matched: number;
  elapsedMs: number;
}

/** Progress, with rates. */
export interface Progress extends NativeProgress {
  /** `bytesIn / bytesTotal`, when the size is known. */
  fraction: number | null;
  mbPerSecondIn: number;
  mbPerSecondOut: number;
  entitiesPerSecond: number;
}

/** Entity counts by type. */
export interface TypeCounts {
  items: number;
  properties: number;
  lexemes: number;
  mediainfo: number;
  entitySchemas: number;
}

/** Counters of a finished task. */
export interface ScanStats {
  compression: "bzip2" | "gzip" | "zstd" | "none";
  bytesIn: number;
  bytesOut: number;
  entities: number;
  types: TypeCounts;
  matched: number;
  /** Entities that needed a parse (to decide the filter, or because they matched). */
  parsed: number;
  blocks: number;
  streams: number;
  /** Stopped early (closed, aborted, or `limit` reached). */
  stopped: boolean;
  /** The input was truncated (only with `allowTruncated`). */
  truncated: boolean;
  elapsedMs: number;
  /**
   * With `ids` in the filter (anywhere in it), after a complete pass (not
   * stopped by `limit`, `close()` or a signal, not truncated): the
   * requested ids the dump does not hold (deleted, or merged into another
   * item), in id order. Absent otherwise. Made from
   * {@link ScanStats.missingItemNumbers} on first access.
   */
  missingIds?: string[];
  /** The missing items of {@link ScanStats.missingIds} as numbers (`42` for `Q42`), cheap to hold when there are many. */
  missingItemNumbers?: Uint32Array;
  /** The class index saved with `buildClassIndex`. */
  classIndex?: { path: string; classes: number; edges: number };
}

/** Options every task takes. */
export interface CommonOptions {
  /** Which entities (default: all). */
  filter?: Filter;
  /** For `instanceOf` with `subclasses: true`. */
  classIndex?: ClassIndexLike;
  /** Worker threads (default: all cores). */
  threads?: number;
  /** Stop after this many matching entities. */
  limit?: number;
  /** Read a truncated dump (a partial download) up to its last whole entity instead of failing. */
  allowTruncated?: boolean;
  /**
   * Also build the class index, as {@link ClassIndex.build} would, and save
   * it to this file when the pass ends without being stopped (`stats.classIndex`
   * says so). The filter cannot use it in the same pass; the next one can.
   */
  buildClassIndex?: string | URL;
  /** Progress, about every 200 ms, and a last event with the final totals. */
  onProgress?: (p: Progress) => void;
  /** Abort the task. */
  signal?: AbortSignal;
}

/** @internal What tasks need of a class index. */
export interface ClassIndexLike {
  readonly _id: number;
}

/**
 * Languages or sites chosen per entity by its claims. With
 * `{ by: "P17", map: { Q55: ["nl"], Q183: ["de"] }, always: ["en", "mul"] }`
 * a Dutch item keeps `nl`, `en`, `mul` and a German one `de`, `en`, `mul`.
 * The values are read from the entity's truthy statements (the property
 * need not be among `properties`).
 */
export interface ByClaim {
  /** The properties whose values choose (`"P17"`, `["P17", "P495"]`). */
  by: string | string[];
  /** Item id → languages (in fallback order) or sites. */
  map: Record<string, string[]>;
  /** Kept for every entity, after the chosen ones. */
  always?: string[];
}

/** Options for `entities()`. */
export interface EntityOptions extends CommonOptions {
  /**
   * Languages to keep, in fallback order for `label` and `description`
   * (default: all, fallback `en`, `mul`), or a {@link ByClaim} rule to
   * choose them per entity.
   */
  languages?: string[] | ByClaim;
  /** Sites to keep sitelinks for (default: all), or a {@link ByClaim} rule. */
  sitelinks?: string[] | ByClaim;
  /** Properties to keep claims for (default: all). Derived fields are computed anyway. */
  properties?: string[];
  /** Keep labels (default true). */
  labels?: boolean;
  /** Keep descriptions (default true). */
  descriptions?: boolean;
  /** Keep aliases (default true). */
  aliases?: boolean;
  /** Keep qualifiers (default true). */
  qualifiers?: boolean;
  /** Keep references (default false). */
  references?: boolean;
  /** Keep deprecated statements (default false). */
  deprecated?: boolean;
  /** Keep only each property's best-ranked statements (default false). */
  bestRank?: boolean;
  /** Send the full entity's JSON (default true); `false` sends the summary fields only (faster). */
  json?: boolean;
  /** Entities per batch (default 1024). */
  batchSize?: number;
  /** Batches buffered ahead of the consumer (default 2). */
  readAhead?: number;
}

/** Options for `extract()`. */
export interface ExtractOptions extends CommonOptions {
  /** The output: `.jsonl.zst` (recommended), `.json.bz2`, `.json.gz`, `.jsonl`, `.json`, … */
  output: string | URL;
  /** Compression level (default: zstd 9, bzip2 9, gzip 6). */
  level?: number;
}

/** What `extract()` did. */
export interface ExtractStats extends ScanStats {
  output: string;
  format: "jsonArray" | "jsonLines";
  outputCompression: "bzip2" | "gzip" | "zstd" | "none";
  bytesWritten: number;
}

/** What a dump holds. */
export interface FileInfo extends ScanStats {
  input: string;
  size: number | null;
  /** Highest id number per type. */
  maxIds: { item: number; property: number; lexeme: number; mediainfo: number; entitySchema: number };
  largest: { id: string; bytes: number };
}

/** Options for `classIndex()` and `ClassIndex.build()`. */
export interface ClassIndexOptions {
  threads?: number;
  allowTruncated?: boolean;
  onProgress?: (p: Progress) => void;
  signal?: AbortSignal;
}

/**
 * Options for `openDump()`, also taken by the functions that open a dump
 * themselves (`streamEntities`, `extract`, `fileInfo`, `ClassIndex.build`).
 */
export interface OpenOptions {
  /** HTTP `User-Agent` (default `wikidata-rs/<version> (https://github.com/the-haus/wikidata-rs)`). */
  userAgent?: string;
  /** HTTP reconnect attempts per position (default 8). */
  retries?: number;
}
