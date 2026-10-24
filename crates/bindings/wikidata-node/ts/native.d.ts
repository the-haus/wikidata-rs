// Types of the N-API module (see src/lib.rs). The public, documented API
// is in index.ts; these are internal.

export interface NativeOptions {
  filterJson?: string;
  idArrays?: Uint32Array[];
  revisionArrays?: Float64Array[];
  placeIndex?: number;
  redirectIndex?: number;
  placeIndexOutput?: string;
  checkpointEvery?: number;
  resume?: Buffer;
  referencedLabelsJson?: string;
  qualifierProperties?: string[];
  languagesByJson?: string;
  sitelinksByJson?: string;
  classIndexOutput?: string;
  classIndex?: number;
  threads?: number;
  limit?: number;
  allowTruncated?: boolean;
  batchSize?: number;
  readAhead?: number;
  languages?: string[];
  sitelinks?: string[];
  properties?: string[];
  labels?: boolean;
  descriptions?: boolean;
  aliases?: boolean;
  qualifiers?: boolean;
  references?: boolean;
  deprecated?: boolean;
  bestRank?: boolean;
  json?: boolean;
  output?: string;
  level?: number;
}

export interface NativeEntityBatch {
  kind: "entities";
  count: number;
  types: Uint8Array;
  present: Uint8Array;
  json: string;
  jsonOffsets: Uint32Array;
  strings: string;
  stringOffsets: Uint32Array;
  instanceOf: Uint32Array;
  instanceOfOffsets: Uint32Array;
  subclassOf: Uint32Array;
  subclassOfOffsets: Uint32Array;
  country: Uint32Array;
  countryOffsets: Uint32Array;
  coordinates: Float64Array;
  inception: Float64Array;
  dissolved: Float64Array;
  externalKeys: Uint8Array;
  externalSlots: Uint32Array;
  externalOffsets: Uint32Array;
  /** With named filters only. */
  names?: string[];
  matchedBy?: Uint16Array;
  matchedByOffsets?: Uint32Array;
  /** With merged items only. */
  mergedFrom?: Uint32Array;
  mergedFromOffsets?: Uint32Array;
}

export interface NativeJsonBatch {
  kind: "json";
  json: string;
}

export type NativeEvent =
  | { type: "batch"; batch: NativeEntityBatch | NativeJsonBatch }
  | { type: "status"; status: unknown }
  | {
      type: "checkpoint";
      input: string;
      offset: number;
      block: number;
      chunk: number;
      bytesOut: number;
      entities: number;
      matched: number;
      data: Uint8Array;
    }
  | {
      type: "progress";
      bytesIn: number;
      bytesTotal: number | null;
      bytesOut: number;
      entities: number;
      matched: number;
      elapsedMs: number;
    }
  | { type: "end"; stats: unknown }
  | { type: "error"; message: string };

export interface NativeStream {
  setRef(referenced: boolean): void;
  release(): void;
  request(n: number): void;
  close(): void;
}

type Callback = (event: NativeEvent) => void;

export interface NativeReader {
  readonly input: string;
  readonly compression: string;
  readonly size: number | null;
  entities(options: NativeOptions, callback: Callback): NativeStream;
  extract(options: NativeOptions, callback: Callback): NativeStream;
  fileInfo(options: NativeOptions, callback: Callback): NativeStream;
  buildClassIndex(options: NativeOptions, callback: Callback): NativeStream;
  buildPlaceIndex(options: NativeOptions, callback: Callback): NativeStream;
}

export interface NativePlaceIndex {
  readonly id: number;
  readonly size: number;
  readonly sets: number;
  readonly heapBytes: number;
  readonly source: string;
  countryOf(id: string): string[];
  countriesOf(items: Uint32Array): { offsets: Uint32Array; countries: Uint32Array };
  save(path: string): void;
}

export interface NativeRedirectIndex {
  readonly id: number;
  readonly size: number;
  readonly source: string;
  resolve(id: string): string | null;
  pairs(): { from: Uint32Array; to: Uint32Array };
  save(path: string): void;
}

export interface NativeOpenOptions {
  userAgent?: string;
  retries?: number;
  mirrors?: string[];
  connections?: number;
  rangeSize?: number;
}

export interface NativeClassIndex {
  readonly id: number;
  readonly size: number;
  readonly edges: number;
  readonly source: string;
  parents(id: string): string[];
  subclassesOf(ids: string[]): string[];
  isSubclassOf(id: string, ancestor: string): boolean;
  save(path: string): void;
}

export function open(input: string, options?: NativeOpenOptions): Promise<NativeReader>;
export function loadClassIndex(path: string): Promise<NativeClassIndex>;
export function takeClassIndex(token: number): NativeClassIndex;
export function loadPlaceIndex(path: string): Promise<NativePlaceIndex>;
export function takePlaceIndex(token: number): NativePlaceIndex;
export function loadRedirectIndex(path: string): Promise<NativeRedirectIndex>;
export function takeRedirectIndex(token: number): NativeRedirectIndex;
export function buildRedirects(
  options: { redirects: string; pages: string; threads?: number; open?: NativeOpenOptions },
  callback: Callback,
): NativeStream;
export function live(
  options: {
    items?: Uint32Array;
    ids?: string[];
    lastEventId?: string;
    since?: string;
    namespaces?: number[];
    wiki?: string;
    url?: string;
    readAhead?: number;
    open?: NativeOpenOptions;
  },
  callback: Callback,
): NativeStream;
export function downloadDump(
  options: {
    base?: string;
    mirrors?: string[];
    date?: string;
    file?: string;
    dir: string;
    verify?: boolean;
    open?: NativeOpenOptions;
  },
  callback: Callback,
): NativeStream;
export function version(): string;
export function activeThreads(): number;
export function externalIdTable(): string[][];
