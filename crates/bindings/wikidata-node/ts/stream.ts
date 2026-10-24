// A pull-based stream over a native task. Each batch the consumer takes
// grants the producer one more, so memory stays bounded by `readAhead`
// batches however slow the consumer is.
//
// The native side keeps the event loop alive only while JS awaits a batch
// (or the end of a closed stream): a stream that is abandoned half-read
// does not keep the process running.

import { Readable } from "node:stream";

import type { NativeEvent, NativeStream } from "./native.js";
import { WikidataError, type Checkpoint, type NativeProgress, type Progress, type ScanStats } from "./types.js";

type Starter = (callback: (event: NativeEvent) => void) => NativeStream | Promise<NativeStream>;

// A stream dropped without being finished (no close(), no end) is closed
// when it is garbage-collected: the native callback only holds it weakly.
// While JS awaits a batch, though, the stream is reachable only through its
// own pending promise, so it is pinned here until the batch arrives.
const pinned = new Set<object>();
const finalizer = new FinalizationRegistry<NativeStream>((n) => {
  try {
    n.close();
    n.setRef(false);
    n.release();
  } catch {
    /* already released */
  }
});

interface Waiter<T> {
  resolve: (batch: T[] | null) => void;
  reject: (error: Error) => void;
}

/** @internal More callbacks of a stream. */
export interface StreamHooks {
  /** Checkpoints, once the records before them were consumed. */
  onCheckpoint?: ((checkpoint: Checkpoint) => void) | undefined;
  /** Progress of other shapes (downloads), as it comes. */
  onStatus?: ((status: unknown) => void) | undefined;
  /**
   * After a failure (an abort), keep the native task referenced until it
   * ended, and let {@link EntityStream._ended} wait for that (a download
   * saves its state as it stops).
   */
  holdUntilEnd?: boolean;
}

/** A checkpoint in the queue, between batches. */
class QueuedCheckpoint {
  constructor(readonly checkpoint: Checkpoint) {}
}

function toCheckpoint(e: Extract<NativeEvent, { type: "checkpoint" }>): Checkpoint {
  return {
    input: e.input,
    offset: e.offset,
    block: e.block,
    chunk: e.chunk,
    bytesOut: e.bytesOut,
    entities: e.entities,
    matched: e.matched,
    data: Buffer.from(e.data.buffer, e.data.byteOffset, e.data.byteLength).toString("base64"),
  };
}

/**
 * Records streamed out of a dump. Iterate it once, record by record
 * (`for await (const f of stream)`) or batch by batch (`stream.batches()`).
 * Nothing is read until iteration starts; breaking out of a loop (or
 * `close()`) stops the export.
 */
export class EntityStream<T, S = ScanStats, P = Progress> implements AsyncIterable<T> {
  /** Resolves with counters when the stream ends (or is closed). */
  readonly stats: Promise<S>;

  #start: Starter;
  #decode: (batch: never) => T[];
  #onProgress: ((p: P) => void) | undefined;
  #signal: AbortSignal | undefined;
  #native: NativeStream | undefined;
  /** Native batches not taken yet, decoded only when taken (decoding on
   * arrival would let queued objects outlive young-generation GCs). */
  #queue: unknown[] = [];
  #waiter: Waiter<T> | undefined;
  #ended = false;
  #closed = false;
  #error: Error | undefined;
  #claimed = false;
  #referenced = true;
  #released = false;
  #emptyStats: (stopped: boolean) => S;
  #resolveStats!: (s: S) => void;
  #rejectStats!: (e: Error) => void;
  #hooks: StreamHooks;
  #checkpoint: Checkpoint | undefined;
  #nativeEnded = false;
  #onNativeEnd: (() => void) | undefined;

  /** @internal */
  constructor(
    start: Starter,
    decode: (batch: never) => T[],
    onProgress?: (p: P) => void,
    signal?: AbortSignal,
    makeEmptyStats?: (stopped: boolean) => S,
    hooks: StreamHooks = {},
  ) {
    this.#hooks = hooks;
    this.#emptyStats = makeEmptyStats ?? (emptyStats as unknown as (stopped: boolean) => S);
    this.#start = start;
    this.#decode = decode;
    this.#onProgress = onProgress;
    this.#signal = signal;
    this.stats = new Promise<S>((resolve, reject) => {
      this.#resolveStats = resolve;
      this.#rejectStats = reject;
    });
    // Errors surface through iteration; don't also report them as unhandled.
    this.stats.catch(() => undefined);
  }

  /**
   * Keep the event loop alive, and the stream reachable, only while
   * something waits on the native side.
   */
  #updateRef(): void {
    if (!this.#native || this.#released) return;
    const wanted = this.#waiter !== undefined || (this.#closed && !this.#ended);
    if (wanted) pinned.add(this);
    else pinned.delete(this);
    if (wanted !== this.#referenced) {
      this.#referenced = wanted;
      this.#native.setRef(wanted);
    }
  }

  #release(): void {
    if (!this.#native || this.#released) return;
    this.#released = true;
    pinned.delete(this);
    finalizer.unregister(this);
    this.#native.setRef(false);
    this.#native.release();
  }

  #onEvent = (event: NativeEvent): void => {
    switch (event.type) {
      case "batch": {
        if (this.#closed || this.#error) return;
        if (this.#waiter) {
          const records = this.#take(event.batch);
          if (!records) return;
          const w = this.#waiter;
          this.#waiter = undefined;
          this.#native?.request(1);
          w.resolve(records);
        } else {
          this.#queue.push(event.batch);
        }
        this.#updateRef();
        return;
      }
      case "checkpoint": {
        if (this.#closed || this.#error) return;
        const cp = new QueuedCheckpoint(toCheckpoint(event));
        // A waiting consumer has consumed everything before it.
        if (this.#waiter) this.#fire(cp);
        else this.#queue.push(cp);
        return;
      }
      case "status": {
        try {
          this.#hooks.onStatus?.(event.status);
        } catch (e) {
          this.#fail(e instanceof Error ? e : new WikidataError(String(e)));
        }
        return;
      }
      case "progress": {
        if (!this.#onProgress) return;
        const { type: _type, ...progress } = event;
        void _type;
        try {
          this.#onProgress(withRates(progress as unknown as NativeProgress) as unknown as P);
        } catch (e) {
          // A throwing callback fails the stream (rather than the process).
          this.#fail(e instanceof Error ? e : new WikidataError(String(e)));
        }
        return;
      }
      case "end":
        this.#nativeDone();
        this.#ended = true;
        this.#release();
        this.#resolveStats(withMissingIds(event.stats) as unknown as S);
        this.#wake();
        return;
      case "error":
        this.#nativeDone();
        this.#fail(new WikidataError(event.message));
        this.#release();
        return;
    }
  };

  /**
   * The last checkpoint reached: every record before it was consumed
   * (with `checkpointEvery`). Pass it as `resumeFrom` to continue there.
   */
  get checkpoint(): Checkpoint | undefined {
    return this.#checkpoint;
  }

  #fire(cp: QueuedCheckpoint): void {
    this.#checkpoint = cp.checkpoint;
    try {
      this.#hooks.onCheckpoint?.(cp.checkpoint);
    } catch (e) {
      this.#fail(e instanceof Error ? e : new WikidataError(String(e)));
    }
  }

  #nativeDone(): void {
    this.#nativeEnded = true;
    this.#onNativeEnd?.();
  }

  /** @internal Resolves once the native task ended (with `holdUntilEnd`). */
  _ended(): Promise<void> {
    if (this.#nativeEnded || !this.#native) return Promise.resolve();
    return new Promise((resolve) => {
      this.#onNativeEnd = resolve;
    });
  }

  /** @internal Whether the stream has ended or was closed. */
  get _finished(): boolean {
    return this.#ended || this.#closed;
  }

  /** @internal Fail the stream (e.g. when its input fails). */
  _fail(error: Error): void {
    this.#fail(error);
  }

  #fail(error: Error): void {
    if (this.#error) return;
    this.#error = error;
    this.#ended = true;
    this.#queue.length = 0;
    this.#native?.close();
    if (this.#hooks.holdUntilEnd && this.#native && !this.#nativeEnded) {
      // Stay alive until the native task has stopped.
      pinned.add(this);
      if (!this.#referenced) {
        this.#referenced = true;
        this.#native.setRef(true);
      }
    } else {
      this.#release();
    }
    this.#rejectStats(error);
    if (this.#waiter) {
      const w = this.#waiter;
      this.#waiter = undefined;
      w.reject(error);
    }
  }

  #wake(): void {
    if (this.#waiter && this.#queue.length === 0) {
      const w = this.#waiter;
      this.#waiter = undefined;
      if (this.#error) w.reject(this.#error);
      else w.resolve(null);
    }
  }

  #abort = (): void => {
    const reason = this.#signal?.reason;
    this.#fail(reason instanceof Error ? reason : new WikidataError("The stream was aborted"));
  };

  async #ensureStarted(): Promise<void> {
    if (this.#native || this.#ended) return;
    if (this.#signal?.aborted) {
      this.#abort();
      return;
    }
    const weak = new WeakRef(this);
    const callback = (event: NativeEvent): void => {
      const stream = weak.deref();
      if (stream) stream.#onEvent(event);
    };
    try {
      this.#native = await this.#start(callback);
    } catch (e) {
      this.#fail(new WikidataError(e instanceof Error ? e.message : String(e)));
      return;
    }
    finalizer.register(this, this.#native, this);
    if (this.#signal?.aborted) {
      // Aborted while starting (opening the file, reading the config).
      this.#abort();
      return;
    }
    if (this.#closed) {
      this.#native.close();
    } else {
      this.#signal?.addEventListener("abort", this.#abort, { once: true });
    }
    this.#updateRef();
  }

  /** Decode a native batch; on failure, fail the stream. */
  #take(batch: unknown): T[] | undefined {
    try {
      return this.#decode(batch as never);
    } catch (e) {
      this.#fail(e instanceof Error ? e : new WikidataError(String(e)));
      return undefined;
    }
  }

  async #next(): Promise<T[] | null> {
    await this.#ensureStarted();
    // The consumer is done with what it took: the checkpoints after that
    // are reached.
    while (this.#queue[0] instanceof QueuedCheckpoint) {
      this.#fire(this.#queue.shift() as QueuedCheckpoint);
    }
    const queued = this.#queue.shift();
    if (queued !== undefined) {
      this.#native?.request(1);
      const records = this.#take(queued);
      if (!records) throw this.#error;
      return records;
    }
    if (this.#error) throw this.#error;
    if (this.#ended || this.#closed) return null;
    const p = new Promise<T[] | null>((resolve, reject) => {
      this.#waiter = { resolve, reject };
    });
    this.#updateRef();
    return p;
  }

  #claim(): void {
    if (this.#claimed) throw new WikidataError("An EntityStream can only be iterated once");
    this.#claimed = true;
  }

  /** Iterate batches of records (arrays of up to `batchSize`). */
  async *batches(): AsyncGenerator<T[], void, undefined> {
    this.#claim();
    try {
      for (;;) {
        const batch = await this.#next();
        if (batch === null) return;
        yield batch;
      }
    } finally {
      this.close();
    }
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<T, void, undefined> {
    for await (const batch of this.batches()) {
      yield* batch;
    }
  }

  /** Stop the export. Iteration ends; {@link stats} resolves with `stopped: true`. */
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#queue.length = 0;
    this.#signal?.removeEventListener("abort", this.#abort);
    if (this.#native) {
      this.#native.close();
      // Stay alive until the end event arrives, so `stats` resolves.
      this.#updateRef();
    } else if (!this.#ended) {
      // Never started: nothing ran.
      this.#ended = true;
      this.#resolveStats(this.#emptyStats(true));
    }
    this.#wake();
  }

  /** Collect every record. Only for selections that fit in memory. */
  async toArray(): Promise<T[]> {
    const out: T[] = [];
    for await (const batch of this.batches()) {
      for (const r of batch) out.push(r);
    }
    return out;
  }

  /** Map records (lazily, keeping the stream's backpressure). */
  async *map<U>(fn: (record: T) => U | Promise<U>): AsyncGenerator<Awaited<U>, void, undefined> {
    for await (const r of this) yield await fn(r);
  }

  /** Keep records for which `fn` is true (lazily). */
  async *filter(fn: (record: T) => boolean | Promise<boolean>): AsyncGenerator<T, void, undefined> {
    for await (const r of this) if (await fn(r)) yield r;
  }

  /** An object-mode `Readable` (for `pipeline()`); backpressure is preserved. */
  toReadable(): Readable {
    return Readable.from(this, { objectMode: true });
  }
}

/**
 * `missingIds` from the numbers the native side sends (`missingItemNumbers`
 * and the few other ids), made into strings only when read; and
 * `referencedLabels` parsed when read.
 */
function withMissingIds(stats: unknown): unknown {
  const s = stats as { missingItemNumbers?: Uint32Array; missingOtherIds?: string[]; referencedLabelsJson?: string };
  const labels = s.referencedLabelsJson;
  if (typeof labels === "string") {
    delete s.referencedLabelsJson;
    let parsed: unknown;
    Object.defineProperty(s, "referencedLabels", {
      enumerable: true,
      configurable: true,
      get: () => (parsed ??= JSON.parse(labels)),
    });
  }
  const numbers = s.missingItemNumbers;
  if (!(numbers instanceof Uint32Array)) return stats;
  const others = s.missingOtherIds ?? [];
  delete s.missingOtherIds;
  let ids: string[] | undefined;
  Object.defineProperty(s, "missingIds", {
    enumerable: true,
    configurable: true,
    get: () => (ids ??= [...Array.from(numbers, (n) => `Q${n}`), ...others]),
  });
  return s;
}

/** Progress with rates. */
export function withRates(p: NativeProgress): Progress {
  const s = Math.max(p.elapsedMs, 1) / 1000;
  return {
    ...p,
    fraction: p.bytesTotal ? Math.min(1, p.bytesIn / p.bytesTotal) : null,
    mbPerSecondIn: p.bytesIn / 1e6 / s,
    mbPerSecondOut: p.bytesOut / 1e6 / s,
    entitiesPerSecond: p.entities / s,
  };
}

export function emptyStats(stopped: boolean): ScanStats {
  return {
    compression: "none",
    bytesIn: 0,
    bytesOut: 0,
    entities: 0,
    types: { items: 0, properties: 0, lexemes: 0, mediainfo: 0, entitySchemas: 0 },
    matched: 0,
    parsed: 0,
    blocks: 0,
    streams: 0,
    stopped,
    truncated: false,
    elapsedMs: 0,
    unchanged: 0,
  };
}
