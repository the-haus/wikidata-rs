//! Columnar entity batches.
//!
//! Entities are nested and varied, so they cross into JS as their JSON
//! text: all of a batch's JSON in one string, with offsets, parsed lazily
//! in JS only for the entities whose full data is read. The fields every
//! consumer reads (id, type, label, description, instance of, country,
//! coordinates, years, image, website, external ids) cross as typed
//! arrays and one string of short strings, so they are usable without
//! `JSON.parse`. A batch costs a handful of napi calls, not one per
//! value.
//!
//! Offsets into the strings are in UTF-16 code units (what JS strings
//! index by), computed while appending.

use std::sync::Arc;

use napi::{Env, JsObject, JsTypedArray, Result, TypedArrayType};
use wikidata_export::Record;
use wikidata_model::{id_number, EntityType, EXTERNAL_IDS};

/// A typed array in V8's own memory (a copy), so its garbage collector
/// counts it: external buffers made from Rust vectors are invisible to
/// it, and batches it has not collected yet would pile up natively.
pub(crate) fn typed<T: Copy>(env: &Env, v: &[T], kind: TypedArrayType) -> Result<JsTypedArray> {
    let bytes = std::mem::size_of_val(v);
    let mut buf = env.create_arraybuffer(bytes)?;
    // SAFETY: plain numbers, `bytes` long.
    let src = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, bytes) };
    buf.as_mut().copy_from_slice(src);
    buf.into_raw().into_typedarray(kind, v.len(), 0)
}

/// UTF-16 length of UTF-8 text.
#[inline]
fn utf16_len(s: &[u8]) -> u32 {
    let mut n = 0u32;
    for &b in s {
        // Every byte but continuation bytes starts a code point; 4-byte
        // sequences are surrogate pairs.
        n += u32::from((b & 0xC0) != 0x80) + u32::from(b >= 0xF0);
    }
    n
}

/// Present-field bits.
const HAS_LABEL: u8 = 1;
const HAS_DESCRIPTION: u8 = 2;
const HAS_IMAGE: u8 = 4;
const HAS_WEBSITE: u8 = 8;

#[derive(Default)]
pub(crate) struct EntityBatch {
    types: Vec<u8>,
    present: Vec<u8>,
    /// All JSON, and its offsets (n + 1, UTF-16).
    json: String,
    json_offsets: Vec<u32>,
    json_utf16: u32,
    /// Short strings: id, label, label language, description, its
    /// language, image, website; then external id values. Offsets n·7+…
    strings: String,
    string_offsets: Vec<u32>,
    strings_utf16: u32,
    instance_of: Vec<u32>,
    instance_of_offsets: Vec<u32>,
    subclass_of: Vec<u32>,
    subclass_of_offsets: Vec<u32>,
    country: Vec<u32>,
    country_offsets: Vec<u32>,
    coordinates: Vec<f64>,
    inception: Vec<f64>,
    dissolved: Vec<f64>,
    /// External ids: table index and string slot, CSR by entity.
    external_keys: Vec<u8>,
    external_slots: Vec<u32>,
    external_offsets: Vec<u32>,
    /// Named filters that matched (indices into `names`), CSR by entity.
    matched_by: Vec<u16>,
    matched_by_offsets: Vec<u32>,
    pub(crate) names: Arc<[String]>,
}

fn q_numbers(ids: &[String], out: &mut Vec<u32>, offsets: &mut Vec<u32>) {
    if offsets.is_empty() {
        offsets.push(0);
    }
    for id in ids {
        if let Some(n) = id.strip_prefix('Q').and_then(|_| id_number(id)) {
            out.push(n as u32);
        }
    }
    offsets.push(out.len() as u32);
}

impl EntityBatch {
    pub(crate) fn len(&self) -> usize {
        self.types.len()
    }

    /// Bytes held (to flush large batches early).
    pub(crate) fn bytes(&self) -> usize {
        self.json.len() + self.strings.len()
    }

    fn string(&mut self, s: &str) -> u32 {
        if self.string_offsets.is_empty() {
            self.string_offsets.push(0);
        }
        self.strings.push_str(s);
        self.strings_utf16 += utf16_len(s.as_bytes());
        self.string_offsets.push(self.strings_utf16);
        (self.string_offsets.len() - 2) as u32
    }

    pub(crate) fn push(&mut self, r: Record) {
        let e = &r.entity;
        let d = &e.derived;
        self.types.push(e.kind.code());
        let mut present = 0;
        self.string(&e.id);
        match &e.label {
            Some(t) => {
                present |= HAS_LABEL;
                self.string(&t.text);
                self.string(&t.language);
            }
            None => {
                self.string("");
                self.string("");
            }
        }
        match &e.description {
            Some(t) => {
                present |= HAS_DESCRIPTION;
                self.string(&t.text);
                self.string(&t.language);
            }
            None => {
                self.string("");
                self.string("");
            }
        }
        if let Some(i) = &d.image {
            present |= HAS_IMAGE;
            self.string(i);
        } else {
            self.string("");
        }
        if let Some(w) = &d.website {
            present |= HAS_WEBSITE;
            self.string(w);
        } else {
            self.string("");
        }
        self.present.push(present);
        q_numbers(&d.instance_of, &mut self.instance_of, &mut self.instance_of_offsets);
        q_numbers(&d.subclass_of, &mut self.subclass_of, &mut self.subclass_of_offsets);
        q_numbers(&d.country, &mut self.country, &mut self.country_offsets);
        let (lat, lon) = d.coordinates.unwrap_or((f64::NAN, f64::NAN));
        self.coordinates.push(lat);
        self.coordinates.push(lon);
        self.inception.push(d.inception.map_or(f64::NAN, |y| y as f64));
        self.dissolved.push(d.dissolved.map_or(f64::NAN, |y| y as f64));
        if self.external_offsets.is_empty() {
            self.external_offsets.push(0);
        }
        for (k, v) in &d.external_ids {
            let idx = EXTERNAL_IDS.iter().position(|x| x.key == *k).unwrap_or(0) as u8;
            let slot = self.string(v);
            self.external_keys.push(idx);
            self.external_slots.push(slot);
        }
        self.external_offsets.push(self.external_keys.len() as u32);
        if self.matched_by_offsets.is_empty() {
            self.matched_by_offsets.push(0);
        }
        self.matched_by.extend_from_slice(&r.matched_by);
        self.matched_by_offsets.push(self.matched_by.len() as u32);
        if self.json_offsets.is_empty() {
            self.json_offsets.push(0);
        }
        // serde_json writes UTF-8.
        let text = std::str::from_utf8(&r.json).unwrap_or("");
        self.json.push_str(text);
        self.json_utf16 += utf16_len(text.as_bytes());
        self.json_offsets.push(self.json_utf16);
    }

    pub(crate) fn into_js(self, env: &Env) -> Result<JsObject> {
        let mut o = env.create_object()?;
        o.set("kind", "entities")?;
        o.set("count", self.types.len() as u32)?;
        o.set("types", typed(env, &self.types, TypedArrayType::Uint8)?)?;
        o.set("present", typed(env, &self.present, TypedArrayType::Uint8)?)?;
        o.set("json", env.create_string_from_std(self.json)?)?;
        o.set("jsonOffsets", typed(env, &self.json_offsets, TypedArrayType::Uint32)?)?;
        o.set("strings", env.create_string_from_std(self.strings)?)?;
        o.set(
            "stringOffsets",
            typed(env, &self.string_offsets, TypedArrayType::Uint32)?,
        )?;
        o.set("instanceOf", typed(env, &self.instance_of, TypedArrayType::Uint32)?)?;
        o.set(
            "instanceOfOffsets",
            typed(env, &self.instance_of_offsets, TypedArrayType::Uint32)?,
        )?;
        o.set("subclassOf", typed(env, &self.subclass_of, TypedArrayType::Uint32)?)?;
        o.set(
            "subclassOfOffsets",
            typed(env, &self.subclass_of_offsets, TypedArrayType::Uint32)?,
        )?;
        o.set("country", typed(env, &self.country, TypedArrayType::Uint32)?)?;
        o.set(
            "countryOffsets",
            typed(env, &self.country_offsets, TypedArrayType::Uint32)?,
        )?;
        o.set("coordinates", typed(env, &self.coordinates, TypedArrayType::Float64)?)?;
        o.set("inception", typed(env, &self.inception, TypedArrayType::Float64)?)?;
        o.set("dissolved", typed(env, &self.dissolved, TypedArrayType::Float64)?)?;
        o.set("externalKeys", typed(env, &self.external_keys, TypedArrayType::Uint8)?)?;
        o.set(
            "externalSlots",
            typed(env, &self.external_slots, TypedArrayType::Uint32)?,
        )?;
        o.set(
            "externalOffsets",
            typed(env, &self.external_offsets, TypedArrayType::Uint32)?,
        )?;
        if !self.names.is_empty() {
            let mut names = env.create_array_with_length(self.names.len())?;
            for (i, n) in self.names.iter().enumerate() {
                names.set_element(i as u32, env.create_string(n)?)?;
            }
            o.set("names", names)?;
            o.set("matchedBy", typed(env, &self.matched_by, TypedArrayType::Uint16)?)?;
            o.set(
                "matchedByOffsets",
                typed(env, &self.matched_by_offsets, TypedArrayType::Uint32)?,
            )?;
        }
        Ok(o)
    }
}

/// The entity type of a code (for messages).
#[allow(dead_code)]
pub(crate) fn type_name(code: u8) -> &'static str {
    [
        EntityType::Item,
        EntityType::Property,
        EntityType::Lexeme,
        EntityType::MediaInfo,
        EntityType::EntitySchema,
    ]
    .get(code as usize)
    .map_or("item", |t| t.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16() {
        assert_eq!(utf16_len("abc".as_bytes()), 3);
        assert_eq!(utf16_len("België".as_bytes()), 6);
        assert_eq!(utf16_len("ベルギー".as_bytes()), 4);
        assert_eq!(utf16_len("😀".as_bytes()), 2);
        assert_eq!("😀".encode_utf16().count(), 2);
    }
}
