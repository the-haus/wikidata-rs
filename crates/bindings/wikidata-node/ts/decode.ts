// Columnar native batches → records. A record is two fields (its batch and
// index); every getter reads the batch's typed arrays or slices its
// strings, so fields that are never read cost nothing, and the full
// entity is parsed from JSON only when `entity` is read.

import * as native from "./native.js";
import type { NativeEntityBatch } from "./native.js";
import type { Entity, EntityRecord, EntityType, ExternalIdKey, Text } from "./types.js";

const TYPES: readonly EntityType[] = ["item", "property", "lexeme", "mediainfo", "entity-schema"];
const HAS_LABEL = 1;
const HAS_DESCRIPTION = 2;
const HAS_IMAGE = 4;
const HAS_WEBSITE = 8;
/** Short strings per entity: id, label, its language, description, its language, image, website. */
const SLOTS = 7;

/** The external id keys, in table order (as the native side numbers them). */
export const EXTERNAL_ID_KEYS: readonly ExternalIdKey[] = native.externalIdTable().map((r) => r[0] as ExternalIdKey);

/** Where each entity's short strings start: entity i's first slot. */
function slotStarts(b: NativeEntityBatch): Uint32Array {
  const starts = new Uint32Array(b.count);
  let slot = 0;
  for (let i = 0; i < b.count; i++) {
    starts[i] = slot;
    slot += SLOTS + (b.externalOffsets[i + 1]! - b.externalOffsets[i]!);
  }
  return starts;
}

class Batch {
  readonly b: NativeEntityBatch;
  readonly starts: Uint32Array;
  constructor(b: NativeEntityBatch) {
    this.b = b;
    this.starts = slotStarts(b);
  }
  str(slot: number): string {
    const o = this.b.stringOffsets;
    return this.b.strings.slice(o[slot]!, o[slot + 1]!);
  }
}

function qids(values: Uint32Array, offsets: Uint32Array, i: number): string[] {
  const out: string[] = [];
  for (let k = offsets[i]!; k < offsets[i + 1]!; k++) out.push(`Q${values[k]!}`);
  return out;
}

class LazyRecord implements EntityRecord {
  readonly #batch: Batch;
  readonly #i: number;
  #entity: Entity | undefined;

  constructor(batch: Batch, i: number) {
    this.#batch = batch;
    this.#i = i;
  }

  #slot(k: number): string {
    return this.#batch.str(this.#batch.starts[this.#i]! + k);
  }

  #has(bit: number): boolean {
    return (this.#batch.b.present[this.#i]! & bit) !== 0;
  }

  get id(): string {
    return this.#slot(0);
  }

  get type(): EntityType {
    return TYPES[this.#batch.b.types[this.#i]!] ?? "item";
  }

  get label(): Text | null {
    return this.#has(HAS_LABEL) ? { text: this.#slot(1), language: this.#slot(2) } : null;
  }

  get description(): Text | null {
    return this.#has(HAS_DESCRIPTION) ? { text: this.#slot(3), language: this.#slot(4) } : null;
  }

  get image(): string | null {
    return this.#has(HAS_IMAGE) ? this.#slot(5) : null;
  }

  get website(): string | null {
    return this.#has(HAS_WEBSITE) ? this.#slot(6) : null;
  }

  get instanceOf(): string[] {
    const b = this.#batch.b;
    return qids(b.instanceOf, b.instanceOfOffsets, this.#i);
  }

  get subclassOf(): string[] {
    const b = this.#batch.b;
    return qids(b.subclassOf, b.subclassOfOffsets, this.#i);
  }

  get country(): string[] {
    const b = this.#batch.b;
    return qids(b.country, b.countryOffsets, this.#i);
  }

  get coordinates(): { lat: number; lon: number } | null {
    const c = this.#batch.b.coordinates;
    const lat = c[2 * this.#i]!;
    return Number.isNaN(lat) ? null : { lat, lon: c[2 * this.#i + 1]! };
  }

  get inception(): number | null {
    const v = this.#batch.b.inception[this.#i]!;
    return Number.isNaN(v) ? null : v;
  }

  get dissolved(): number | null {
    const v = this.#batch.b.dissolved[this.#i]!;
    return Number.isNaN(v) ? null : v;
  }

  get externalIds(): Partial<Record<ExternalIdKey, string>> {
    const b = this.#batch.b;
    const out: Partial<Record<ExternalIdKey, string>> = {};
    for (let k = b.externalOffsets[this.#i]!; k < b.externalOffsets[this.#i + 1]!; k++) {
      const key = EXTERNAL_ID_KEYS[b.externalKeys[k]!];
      if (key) out[key] = this.#batch.str(b.externalSlots[k]!);
    }
    return out;
  }

  get matchedBy(): string[] {
    const b = this.#batch.b;
    const out: string[] = [];
    if (!b.names || !b.matchedBy || !b.matchedByOffsets) return out;
    for (let k = b.matchedByOffsets[this.#i]!; k < b.matchedByOffsets[this.#i + 1]!; k++) {
      out.push(b.names[b.matchedBy[k]!]!);
    }
    return out;
  }

  get mergedFrom(): string[] {
    const b = this.#batch.b;
    if (!b.mergedFrom || !b.mergedFromOffsets) return [];
    return qids(b.mergedFrom, b.mergedFromOffsets, this.#i);
  }

  get json(): string {
    const b = this.#batch.b;
    return b.json.slice(b.jsonOffsets[this.#i]!, b.jsonOffsets[this.#i + 1]!);
  }

  get entity(): Entity {
    if (this.#entity === undefined) {
      const text = this.json;
      if (text.length === 0) {
        throw new Error("this stream was opened with `json: false`; only the summary fields are available");
      }
      this.#entity = JSON.parse(text) as Entity;
    }
    return this.#entity;
  }

  toJSON(): Omit<EntityRecord, "entity" | "json" | "toJSON"> {
    return {
      id: this.id,
      type: this.type,
      label: this.label,
      description: this.description,
      instanceOf: this.instanceOf,
      subclassOf: this.subclassOf,
      country: this.country,
      coordinates: this.coordinates,
      inception: this.inception,
      dissolved: this.dissolved,
      image: this.image,
      website: this.website,
      externalIds: this.externalIds,
      matchedBy: this.matchedBy,
      mergedFrom: this.mergedFrom,
    };
  }

  [Symbol.for("nodejs.util.inspect.custom")](): unknown {
    return this.toJSON();
  }
}

export function decodeEntities(b: NativeEntityBatch): EntityRecord[] {
  const batch = new Batch(b);
  const out = new Array<EntityRecord>(b.count);
  for (let i = 0; i < b.count; i++) out[i] = new LazyRecord(batch, i);
  return out;
}
