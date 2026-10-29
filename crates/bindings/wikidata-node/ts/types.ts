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
  /** With `sitelinkCounts`: the number of sitelinks, the pruned ones included. */
  sitelinkCount?: number;
  /** With `sitelinkCounts`: the number of Wikipedia sitelinks (see {@link EntityOptions.sitelinkCounts}), the pruned ones included. */
  wikipediaCount?: number;
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
  /**
   * With a {@link RedirectIndexLike | redirect index} (`redirects`): the
   * requested ids that were merged into this entity (it came through the
   * pass because of them); `[]` otherwise.
   */
  readonly mergedFrom: string[];
  /** With `sitelinkCounts`: the number of sitelinks, the pruned ones included; `null` otherwise. */
  readonly sitelinkCount: number | null;
  /** With `sitelinkCounts`: the number of Wikipedia sitelinks, the pruned ones included; `null` otherwise. */
  readonly wikipediaCount: number | null;
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
 * Known revisions of `ids` (`lastrevid`s, in the order of `ids`): an
 * entity no newer than its known revision did not change, and is skipped
 * before parsing (it still counts as found: it is not in `missingIds`;
 * `stats.unchanged` counts them). Revision ids pass 2^32, so prefer
 * `Float64Array` or `BigUint64Array` to `Uint32Array`.
 */
export type Revisions = number[] | Float64Array | BigUint64Array | BigInt64Array | Uint32Array;

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
 * { ids: Uint32Array.of(42), revisions: Float64Array.of(2000000001) }  // only if changed since
 * { ids: { Q42: 2000000001 } }                                   // the same, as a map
 * { inCountry: "Q55" }                                           // needs placeIndex
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
      /** Ids; or a map of ids to their known revisions (see {@link Revisions}). */
      ids?: string | string[] | ItemNumbers | Record<string, number> | Map<string, number>;
      /** Known revisions of `ids`, in the same order. */
      revisions?: Revisions;
      /**
       * In one of these countries: by the item's P17, its P131 chain, or
       * its P159 (headquarters). Needs a `placeIndex`.
       */
      inCountry?: string | string[];
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
  /** The place index saved with `buildPlaceIndex`. */
  placeIndex?: { path: string; items: number; sets: number; bytes: number };
  /**
   * Entities of `ids` whose `lastrevid` was no newer than the revision
   * given with them (skipped before parsing).
   */
  unchanged: number;
  /**
   * With a `redirects` index and `ids`, after a complete pass: the missing
   * ids merged into another item (`inDump`: that item came through the
   * pass, its record's `mergedFrom` names the id).
   */
  merged?: { from: string; to: string; inDump: boolean }[];
  /** With a `redirects` index: missing ids neither in the dump nor redirected (deleted). */
  deleted?: string[];
  /** With a `redirects` index: missing ids above the dump's highest id (created after it). */
  newer?: string[];
  /** An incremental (XML) dump: the items merged that day (their newest revision is a redirect). */
  redirects?: { from: string; to: string }[];
  /** How `referencedLabels` were collected. */
  labels?: LabelStats;
  /**
   * With `referencedLabels` (and no `output`), after a complete pass: the
   * labels of the items the matches refer to, by id
   * (`{ Q5: { en: "human", nl: "mens" } }`). Parsed on first access.
   */
  referencedLabels?: Record<string, Record<string, string>>;
}

/** How the labels of referenced items were collected. */
export interface LabelStats {
  /** Distinct items referenced. */
  referenced: number;
  /** Items with a label in the asked languages. */
  labelled: number;
  /** Labels taken during the pass (items after their first reference). */
  duringPass: number;
  /** Chunks read again after the pass (for items before their first reference). */
  chunksReread: number;
  /** Chunks of the pass. */
  chunks: number;
}

/**
 * Where a pass can resume: pass it as `resumeFrom` to continue after the
 * records consumed before it (same input, same filter). JSON-safe: store
 * it as it is. The readable fields are informative; `data` holds
 * everything (position, counters, the ids found so far).
 */
export interface Checkpoint {
  /** The input's file name. */
  input: string;
  /** The byte of the input reading resumes at (a bzip2 block, or a line of plain JSON). */
  offset: number;
  /** bzip2 blocks before it. */
  block: number;
  /** Chunks before it. */
  chunk: number;
  /** Decompressed bytes before it. */
  bytesOut: number;
  /** Entities seen before it. */
  entities: number;
  /** Entities matched before it. */
  matched: number;
  /** Everything, base64. */
  data: string;
}

/** Options every task takes. */
export interface CommonOptions {
  /** Which entities (default: all). */
  filter?: Filter;
  /** For `instanceOf` with `subclasses: true`. */
  classIndex?: ClassIndexLike;
  /** For `inCountry` filters, and for `languages`/`sitelinks` rules (an item whose claims choose nothing is looked up by its country). */
  placeIndex?: PlaceIndexLike;
  /**
   * Follow merges: `ids` also match the items their ids were merged into
   * (`EntityRecord.mergedFrom`), and the missing ids are sorted into
   * `stats.merged`, `stats.deleted` and `stats.newer`.
   */
  redirects?: RedirectIndexLike;
  /**
   * Also build the place index, as {@link PlaceIndex.build} would, and
   * save it to this file when the pass ends without being stopped
   * (`stats.placeIndex`).
   */
  buildPlaceIndex?: string | URL;
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

/** @internal What tasks need of a place index. */
export interface PlaceIndexLike {
  readonly _placeId: number;
}

/** @internal What tasks need of a redirect index. */
export interface RedirectIndexLike {
  readonly _redirectId: number;
}

/**
 * Languages or sites chosen per entity by its claims. With
 * `{ by: "P17", map: { Q55: ["nl"], Q183: ["de"] }, always: ["en", "mul"] }`
 * a Dutch item keeps `nl`, `en`, `mul` and a German one `de`, `en`, `mul`.
 * The values are read from the entity's truthy statements (the property
 * need not be among `properties`). When several properties are named,
 * the sites or languages all their values choose are kept (a union, in
 * the order of `by`); with a `placeIndex`, an item whose claims choose
 * nothing is looked up by its place.
 */
export interface ByClaim {
  /** The properties whose values choose (`"P17"`, `["P17", "P495"]`); default none. */
  by?: string | string[];
  /** Item id → languages (in fallback order) or sites; default none. */
  map?: Record<string, string[]>;
  /** Kept for every entity, after the chosen ones. */
  always?: string[];
  /**
   * Sitelinks only: more sites for the items of some classes, on top of
   * what `by`, `map` and `always` choose (which still apply to every
   * item). See {@link SitesFor}.
   */
  allFor?: SitesFor;
}

/**
 * The sites of a group, kept for the items of some classes:
 * `{ instanceOf: ["Q5", "Q215380"], group: "wikipedia" }` keeps every
 * Wikipedia sitelink of a human or a musical group. Matched against the
 * item's truthy P31 values (not their subclasses), which are read before
 * its sitelinks, so the other items' extra sitelinks are skipped without
 * being built.
 */
export interface SitesFor {
  /** The classes, as P31 values (`"Q5"`). */
  instanceOf: string | string[];
  /**
   * Wikidata's site groups (default: every site): `"wikipedia"` (the
   * `<code>wiki` sites: not `commonswiki`, `specieswiki`, `wikidatawiki`,
   * `metawiki`, `mediawikiwiki`, `sourceswiki`, `wikimaniawiki`,
   * `foundationwiki`, `outreachwiki`, `incubatorwiki`,
   * `wikifunctionswiki`), `"wikivoyage"`, `"wikiquote"`, `"wikisource"`,
   * `"wikinews"`, `"wikibooks"`, `"wikiversity"`, `"wiktionary"`, or
   * `"special"` (those other sites).
   */
  group?: SiteGroup | SiteGroup[];
}

/** Wikidata's site groups. */
export type SiteGroup =
  | "wikipedia"
  | "wikivoyage"
  | "wikiquote"
  | "wikisource"
  | "wikinews"
  | "wikibooks"
  | "wikiversity"
  | "wiktionary"
  | "special";

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
  /**
   * Count each entity's sitelinks, the pruned ones included, into
   * `sitelinkCount` (every site) and `wikipediaCount` (the `<code>wiki`
   * sites of the `wikipedia` group, not Commons, Wikispecies, Wikidata,
   * Meta and the other special sites), on the record (without parsing
   * its JSON) and on the entity. The skipped sitelinks are counted from
   * their keys, without being built. Default false.
   */
  sitelinkCounts?: boolean;
  /** Properties to keep claims for (default: all). Derived fields are computed anyway. */
  properties?: string[];
  /** Keep labels (default true). */
  labels?: boolean;
  /** Keep descriptions (default true). */
  descriptions?: boolean;
  /** Keep aliases (default true). */
  aliases?: boolean;
  /** Keep qualifiers (default true), or only the qualifiers of these properties (`["P585", "P580", "P582"]`). */
  qualifiers?: boolean | string[];
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
  /**
   * Call `onCheckpoint` after every this many chunks (a chunk is a bzip2
   * block, ~900 kB of JSON, or 4 MB of plain JSON; gzip and zstd inputs
   * have none). 1000 is about every 900 MB of JSON.
   */
  checkpointEvery?: number;
  /**
   * Where the pass can resume, called once every record before it has
   * been consumed (the loop body ran for it). Store it; pass it as
   * `resumeFrom` after a stop or a crash.
   */
  onCheckpoint?: (checkpoint: Checkpoint) => void;
  /**
   * Continue a pass from a checkpoint (same input, same filter): the
   * records after it, with counters and missing ids carried over. Not with
   * `buildClassIndex`, `buildPlaceIndex` or `referencedLabels`.
   */
  resumeFrom?: Checkpoint;
  /**
   * Also collect the labels, in these languages, of the items the
   * matching entities' kept claims (and their qualifiers) refer to: a
   * festival's genre, a venue's P131. `stats.referencedLabels` has them
   * after a complete pass, or they are written as JSON to `output`. Items
   * referenced after they stream by are read again afterwards, from the
   * chunks that hold them only (bzip2 and plain JSON; gzip and zstd are
   * read again whole).
   */
  referencedLabels?: { languages: string[]; output?: string | URL };
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
  /** Build the place index in the same pass and save it here. */
  buildPlaceIndex?: string | URL;
  onProgress?: (p: Progress) => void;
  signal?: AbortSignal;
}

/** Options for `placeIndex()` and `PlaceIndex.build()`. */
export interface PlaceIndexOptions {
  threads?: number;
  allowTruncated?: boolean;
  /** Build the class index in the same pass and save it here. */
  buildClassIndex?: string | URL;
  onProgress?: (p: Progress) => void;
  signal?: AbortSignal;
}

/** Options for {@link RedirectIndex.build} and {@link readRedirects}. */
export interface RedirectIndexOptions extends OpenOptions {
  /** The `redirect` table's SQL dump: `wikidatawiki-<date>-redirect.sql.gz` (a path or URL). */
  redirects: string | URL;
  /** The `page` table's SQL dump: `wikidatawiki-<date>-page.sql.gz`. */
  pages: string | URL;
  threads?: number;
  onProgress?: (p: Progress) => void;
  signal?: AbortSignal;
}

/** A change to an entity, from {@link liveChanges}. */
export interface LiveChange {
  /** The entity (`Q42`, `P31`, `L7`). */
  id: string;
  /** `edit`, `new` or `log` (deletions, …). */
  kind: string;
  /** The new revision (edits and creations). */
  revision: number | null;
  /** The revision it replaced (edits). */
  oldRevision: number | null;
  /** When (Unix time, seconds). */
  timestamp: number;
  user: string | null;
  bot: boolean;
  /** The edit summary: `/* wbmergeitems-to:0||Q2 *\/` for a merge. */
  comment: string;
  /** Log events: `delete`, … */
  logType: string | null;
  logAction: string | null;
  /** Resume after this change: pass it as `lastEventId`. */
  eventId: string;
}

/** Options for {@link liveChanges}. */
export interface LiveOptions extends OpenOptions {
  /** Only these entities (ids, or item numbers). */
  ids?: string[] | ItemNumbers;
  /** Resume after this change (its `eventId`). */
  lastEventId?: string;
  /** Start at this time (`2026-10-24T00:00:00Z`); EventStreams keeps about a week. */
  since?: string | Date;
  /** Namespaces (default `[0]`: items; `120` properties, `146` lexemes). */
  namespaces?: number[];
  /** The wiki (default `wikidatawiki`). */
  wiki?: string;
  /** The stream (default Wikimedia's `recentchange`). */
  url?: string;
  signal?: AbortSignal;
}

/** Counters of a live stream. */
export interface LiveStats {
  /** Events read (every wiki). */
  events: number;
  /** Changes handed on. */
  changes: number;
  /** Reconnects after dropped connections. */
  reconnects: number;
  lastEventId: string | null;
}

/** Progress of {@link downloadDump} and {@link downloadSqlDumps}. */
export interface DownloadProgress {
  /** The file being downloaded. */
  file: string;
  /** Bytes on disk (resumed ones included). */
  bytesDone: number;
  bytesTotal: number;
  /** Bytes fetched by this run. */
  bytesFetched: number;
  /** Bytes hashed (SHA-1, in file order). */
  bytesHashed: number;
  /** Bytes fetched from each server: the base, then the mirrors. */
  perServer: number[];
  elapsedMs: number;
}

/** Options for {@link downloadDump}. */
export interface DownloadOptions extends OpenOptions {
  /** Where to put the dump. */
  dir: string | URL;
  /** The dump's date (`20261012`), or `"latest"` (the default): the newest complete one. */
  date?: string;
  /** The dumps' directory (default `https://dumps.wikimedia.org/wikidatawiki/entities/`). */
  base?: string;
  /** The same directory on mirrors (`https://dumps.wikimedia.your.org/wikidatawiki/entities/`). */
  mirrors?: string[];
  /** Which file of the dump (default `all.json.bz2`). */
  file?: string;
  /** Range size in bytes (default 8 MiB). */
  rangeSize?: number;
  /** Check the SHA-1 (default true). */
  verify?: boolean;
  onProgress?: (p: DownloadProgress) => void;
  signal?: AbortSignal;
}

/** Options for {@link downloadSqlDumps}. */
export interface SqlDownloadOptions extends OpenOptions {
  /** Where to put the files. */
  dir: string | URL;
  /** The database dump's date (`20261001`), or `"latest"` (the default): the newest whose tables are all done. */
  date?: string;
  /** The tables (default `["redirect", "page"]`, what {@link RedirectIndex.build} reads). */
  tables?: string[];
  /** The database dumps' directory (default `https://dumps.wikimedia.org/wikidatawiki/`). */
  base?: string;
  /** The same directory on mirrors. */
  mirrors?: string[];
  /** Range size in bytes (default 8 MiB). */
  rangeSize?: number;
  /** Check the SHA-1s (default true). */
  verify?: boolean;
  onProgress?: (p: DownloadProgress) => void;
  signal?: AbortSignal;
}

/** Options for {@link latestDump}. */
export interface LatestDumpOptions extends OpenOptions {
  /** SQL tables of the database dumps (`["redirect", "page"]`); without them, the entity dump of `file`. */
  tables?: string[];
  /** The entity dump's file (default `all.json.bz2`). */
  file?: string;
  /** The dumps' directory (default: the entity dumps, or with `tables` the database dumps, on dumps.wikimedia.org). */
  base?: string;
}

/** The newest complete dump, from {@link latestDump}. */
export interface DumpStatus {
  /** Its date (`20261001`): the folder to download into. */
  date: string;
  /** The files asked for, in that order. */
  files: { part: string; name: string; url: string; size: number | null; sha1: string }[];
}

/** What {@link downloadDump} did. */
export interface DownloadResult {
  /** The file, under its final name (only once verified). */
  path: string;
  date: string;
  bytes: number;
  bytesFetched: number;
  /** Bytes already there (a resumed download). */
  bytesResumed: number;
  perServer: number[];
  /** The verified SHA-1. */
  sha1: string | null;
  /** The file was there already. */
  alreadyDone: boolean;
  elapsedMs: number;
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
  /**
   * Other URLs of the same file: it is then read in ranges from all of
   * them at once (mirrors cap bandwidth per client, so two are about twice
   * as fast), in bounded memory.
   */
  mirrors?: string[];
  /** HTTP connections per server when reading in ranges (default 2; more than 1 reads in ranges without mirrors too). */
  connections?: number;
}
