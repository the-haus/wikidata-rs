// Types of the N-API module (see src/lib.rs). The public, documented API
// is in index.ts; these are internal.

export interface NativeOptions {
  filterJson?: string;
  idArrays?: Uint32Array[];
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
}

export type NativeEvent =
  | { type: "batch"; batch: NativeEntityBatch }
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

export function open(input: string, options?: { userAgent?: string; retries?: number }): Promise<NativeReader>;
export function loadClassIndex(path: string): Promise<NativeClassIndex>;
export function takeClassIndex(token: number): NativeClassIndex;
export function version(): string;
export function activeThreads(): number;
export function externalIdTable(): string[][];
