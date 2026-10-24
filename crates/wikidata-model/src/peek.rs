//! An entity's type and id without parsing it.
//!
//! Dump lines start with the type and the id:
//! `{"type":"item","id":"Q31","labels":…`, `{"type":"property",
//! "datatype":"…","id":"P31",…`, so both are read from the first bytes.
//! Lines written in another key order (`pageid` first, as in lexeme
//! dumps) fall back to a scan of the top level that skips every value.

use memchr::memmem;
use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, Visitor};

use crate::entity::EntityType;
use crate::parse::{run_seed, With};

/// An entity's type and id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peek<'a> {
    /// The type.
    pub kind: EntityType,
    /// The id (`Q31`).
    pub id: &'a str,
}

const HEAD: usize = 512;

/// The string value of `"key":"` at the top level of `line`'s head.
fn head_value<'a>(line: &'a [u8], needle: &[u8]) -> Option<&'a str> {
    let head = &line[..line.len().min(HEAD)];
    let at = memmem::find(head, needle)?;
    // Nothing nested may come before it (the first byte is the entity's `{`).
    if memchr::memchr2(b'{', b'[', &head[1..at]).is_some() {
        return None;
    }
    let start = at + needle.len();
    let len = memchr::memchr(b'"', &line[start..])?;
    let v = &line[start..start + len];
    if v.contains(&b'\\') {
        return None;
    }
    std::str::from_utf8(v).ok()
}

/// The numeric part of an id (`Q31` → 31).
pub fn id_number(id: &str) -> Option<u64> {
    let d = id.get(1..)?;
    if d.is_empty() || d.len() > 18 || !d.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    d.parse().ok()
}

/// An entity line's `lastrevid`, without parsing it. The dumps write it
/// near the end of the line (`…,"title":"Q31","lastrevid":2553637656,
/// "modified":"…"}`), so the tail is searched first, then the whole line.
pub fn lastrevid(line: &[u8]) -> Option<u64> {
    const KEY: &[u8] = b"\"lastrevid\":";
    let tail = line.len().saturating_sub(HEAD);
    let at = memmem::rfind(&line[tail..], KEY)
        .map(|i| i + tail)
        .or_else(|| memmem::rfind(&line[..tail.saturating_add(KEY.len()).min(line.len())], KEY))?;
    let digits = &line[at + KEY.len()..];
    let n = digits.iter().take_while(|b| b.is_ascii_digit()).count();
    if n == 0 || n > 19 {
        return None;
    }
    std::str::from_utf8(&digits[..n]).ok()?.parse().ok()
}

/// The type and id of an entity line (see the module docs).
pub fn peek(line: &[u8]) -> Option<Peek<'_>> {
    if let (Some(t), Some(id)) = (head_value(line, b"\"type\":\""), head_value(line, b"\"id\":\"")) {
        if let Some(kind) = EntityType::parse(t) {
            return Some(Peek { kind, id });
        }
    }
    peek_slow(line)
}

struct PeekSeed;

impl<'de> DeserializeSeed<'de> for PeekSeed {
    type Value = (Option<EntityType>, Option<(usize, usize)>);
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for PeekSeed {
    type Value = (Option<EntityType>, Option<(usize, usize)>);
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an entity")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut kind = None;
        let mut id: Option<String> = None;
        while let Some(k) = map.next_key_seed(With(|k: &str| match k {
            "type" => 0,
            "id" => 1,
            _ => 2,
        }))? {
            match k {
                0 => kind = map.next_value_seed(With(EntityType::parse))?,
                1 => id = Some(map.next_value()?),
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        // Report the id by length; the caller finds it in the line.
        Ok((kind, id.map(|i| (0, i.len()))))
    }
}

fn peek_slow(line: &[u8]) -> Option<Peek<'_>> {
    let (kind, id) = run_seed(line, PeekSeed).ok()?;
    let (_, len) = id?;
    // The top-level id: the occurrence of `"id":"` followed by `len` bytes
    // and a quote, outside of nested values.
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in line.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth -= 1,
            b'"' => {
                if depth == 1 && line[i..].starts_with(b"\"id\":\"") {
                    let s = i + 6;
                    let v = line.get(s..s + len)?;
                    let id = std::str::from_utf8(v).ok()?;
                    let kind = kind.or_else(|| EntityType::from_letter(*v.first()?))?;
                    return Some(Peek { kind, id });
                }
                in_str = true;
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_and_slow() {
        let p = peek(br#"{"type":"item","id":"Q31","labels":{}}"#).unwrap();
        assert_eq!((p.kind, p.id), (EntityType::Item, "Q31"));
        let p = peek(br#"{"type":"property","datatype":"wikibase-item","id":"P31"}"#).unwrap();
        assert_eq!((p.kind, p.id), (EntityType::Property, "P31"));
        // Nested ids first: the slow path.
        let p = peek(br#"{"pageid":1,"claims":{"P1":[{"id":"Q9$x"}]},"type":"lexeme","id":"L7"}"#).unwrap();
        assert_eq!((p.kind, p.id), (EntityType::Lexeme, "L7"));
        assert!(peek(b"{\"labels\":{}}").is_none());
        assert_eq!(id_number("Q31"), Some(31));
        assert_eq!(id_number("Q"), None);
    }

    #[test]
    fn revisions() {
        assert_eq!(
            lastrevid(br#"{"type":"item","id":"Q1","lastrevid":2553637656,"modified":"x"}"#),
            Some(2553637656)
        );
        // Far from the end (the whole line is searched).
        let mut long = br#"{"type":"item","lastrevid":7,"id":"Q1","labels":""#.to_vec();
        long.extend(std::iter::repeat_n(b'x', 2000));
        long.extend(br#""}"#);
        assert_eq!(lastrevid(&long), Some(7));
        assert_eq!(lastrevid(br#"{"type":"item","id":"Q1"}"#), None);
        // Inside a string, the quotes are escaped: no match.
        assert_eq!(lastrevid(br#"{"a":"\"lastrevid\":5"}"#), None);
    }
}
