//! Merged items: which item each redirect points at.
//!
//! When two items are merged, one becomes a redirect to the other. The
//! JSON entity dump leaves redirects out (a merged item is simply not in
//! it), but MediaWiki's SQL dumps have them: the `redirect` table (page
//! id → target title) joined with the `page` table (page id → title). A
//! [`RedirectIndex`] holds the joined pairs of items (`Q1 → Q2`; built by
//! `wikidata_export::build_redirect_index`), as sorted arrays (8 bytes a
//! redirect), saved as a small zstd-compressed file.
//!
//! With the dump's highest item id, it sorts the ids a pass did not find
//! into **merged** (redirected to another item), **deleted** (neither in
//! the dump nor redirected) and **newer than the dump** (above its highest
//! id).

use std::path::Path;

use crate::FilterError;

const MAGIC: &[u8; 8] = b"WDREDIR1";

/// How many redirects in a row are followed (double redirects are rare,
/// and fixed by bots).
pub const MAX_HOPS: usize = 8;

/// Item redirects (see the module docs).
#[derive(Clone, Default)]
pub struct RedirectIndex {
    /// Sorted redirected item numbers.
    from: Vec<u32>,
    /// Their targets.
    to: Vec<u32>,
    source: String,
}

impl std::fmt::Debug for RedirectIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RedirectIndex({} redirects, {:?})", self.len(), self.source)
    }
}

/// The number of a `Q` id.
fn qnum(id: &str) -> Option<u32> {
    if !id.starts_with('Q') {
        return None;
    }
    wikidata_model::id_number(id).and_then(|n| u32::try_from(n).ok())
}

impl RedirectIndex {
    /// From `(from, to)` item numbers (`Q1 → Q2` is `(1, 2)`); a
    /// redirect given twice keeps its last target.
    pub fn from_pairs(mut pairs: Vec<(u32, u32)>, source: impl Into<String>) -> RedirectIndex {
        pairs.sort_by_key(|&(f, _)| f);
        let mut from: Vec<u32> = Vec::with_capacity(pairs.len());
        let mut to: Vec<u32> = Vec::with_capacity(pairs.len());
        for (f, t) in pairs {
            if f == t {
                continue;
            }
            if from.last() == Some(&f) {
                *to.last_mut().unwrap() = t;
            } else {
                from.push(f);
                to.push(t);
            }
        }
        RedirectIndex {
            from,
            to,
            source: source.into(),
        }
    }

    /// What the index was built from (the SQL dumps' file names).
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Redirects.
    pub fn len(&self) -> usize {
        self.from.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.from.is_empty()
    }

    /// Where item number `q` redirects to, one step.
    #[inline]
    pub fn target(&self, q: u32) -> Option<u32> {
        self.from.binary_search(&q).ok().map(|i| self.to[i])
    }

    /// Where item number `q` ends up, following redirects (at most
    /// [`MAX_HOPS`]); `None` when it is not a redirect (or the redirects
    /// go round in a circle).
    pub fn resolve(&self, q: u32) -> Option<u32> {
        let mut at = self.target(q)?;
        for _ in 1..MAX_HOPS {
            match self.target(at) {
                Some(next) if next == q => return None,
                Some(next) => at = next,
                None => return Some(at),
            }
        }
        Some(at)
    }

    /// [`RedirectIndex::resolve`] by id (`"Q1"` → `"Q2"`).
    pub fn resolve_id(&self, id: &str) -> Option<String> {
        self.resolve(qnum(id)?).map(|q| format!("Q{q}"))
    }

    /// The redirects, as `(from, to)` item numbers in `from` order.
    pub fn pairs(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.from.iter().copied().zip(self.to.iter().copied())
    }

    /// Serialise (see [`RedirectIndex::save`]).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity(12 + self.source.len() + self.from.len() * 8);
        let src = self.source.as_bytes();
        raw.extend((src.len() as u32).to_le_bytes());
        raw.extend(src);
        raw.extend((self.from.len() as u32).to_le_bytes());
        let mut prev = 0;
        for &f in &self.from {
            raw.extend((f - prev).to_le_bytes());
            prev = f;
        }
        for &t in &self.to {
            raw.extend(t.to_le_bytes());
        }
        let mut out = MAGIC.to_vec();
        out.extend(zstd::bulk::compress(&raw, 19).expect("compressing in memory"));
        out
    }

    /// Deserialise (see [`RedirectIndex::save`]).
    pub fn from_bytes(bytes: &[u8]) -> Result<RedirectIndex, FilterError> {
        let bad = |m: &str| FilterError::Invalid(format!("not a redirect index: {m}"));
        let body = bytes.strip_prefix(MAGIC).ok_or_else(|| bad("wrong magic"))?;
        let raw = zstd::stream::decode_all(body).map_err(|e| bad(&e.to_string()))?;
        let u32_at =
            |pos: usize| -> Option<u32> { Some(u32::from_le_bytes(raw.get(pos..pos + 4)?.try_into().unwrap())) };
        let slen = u32_at(0).ok_or_else(|| bad("too short"))? as usize;
        let source = raw
            .get(4..4 + slen)
            .and_then(|s| std::str::from_utf8(s).ok())
            .ok_or_else(|| bad("bad source name"))?
            .to_string();
        let n = u32_at(4 + slen).ok_or_else(|| bad("too short"))? as usize;
        let pos = 8 + slen;
        if raw.len() != pos + n * 8 {
            return Err(bad("wrong length"));
        }
        let read = |at: usize| -> Vec<u32> {
            raw[at..at + n * 4]
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                .collect()
        };
        let mut from = read(pos);
        let mut acc = 0u32;
        for f in &mut from {
            acc = acc.wrapping_add(*f);
            *f = acc;
        }
        if from.windows(2).any(|w| w[0] >= w[1]) {
            return Err(bad("inconsistent"));
        }
        Ok(RedirectIndex {
            from,
            to: read(pos + n * 4),
            source,
        })
    }

    /// Write to a file: `WDREDIR1`, then zstd-compressed little-endian
    /// arrays (source name, the redirected items as deltas, their
    /// targets).
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), FilterError> {
        let path = path.as_ref();
        std::fs::write(path, self.to_bytes()).map_err(|e| FilterError::Io(format!("{}: {e}", path.display())))
    }

    /// Read a file written by [`RedirectIndex::save`].
    pub fn load(path: impl AsRef<Path>) -> Result<RedirectIndex, FilterError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| FilterError::Io(format!("{}: {e}", path.display())))?;
        Self::from_bytes(&bytes).map_err(|e| FilterError::Invalid(format!("{}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_chains() {
        let r = RedirectIndex::from_pairs(vec![(1, 2), (5, 6), (6, 7), (8, 9), (9, 8), (3, 3), (5, 6)], "t");
        assert_eq!(r.len(), 5);
        assert_eq!(r.resolve(1), Some(2));
        assert_eq!(r.resolve(5), Some(7), "a double redirect");
        assert_eq!(r.resolve(8), None, "a circle");
        assert_eq!(r.resolve(2), None);
        assert_eq!(r.resolve(3), None);
        assert_eq!(r.resolve_id("Q1").as_deref(), Some("Q2"));
        assert_eq!(r.resolve_id("P1"), None);
        let back = RedirectIndex::from_bytes(&r.to_bytes()).unwrap();
        assert_eq!(back.pairs().collect::<Vec<_>>(), r.pairs().collect::<Vec<_>>());
        assert_eq!(back.source(), "t");
        assert!(RedirectIndex::from_bytes(b"WDREDIR1x").is_err());
        assert!(RedirectIndex::from_bytes(&RedirectIndex::default().to_bytes())
            .unwrap()
            .is_empty());
    }
}
