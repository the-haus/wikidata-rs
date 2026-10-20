//! The class hierarchy: which classes are subclasses (P279) of which.
//!
//! "Instance of a subclass of music festival" needs the transitive closure
//! of P279, which a dump only gives one edge at a time. A [`ClassIndex`] is
//! built in one pass over a dump (see `wikidata_export::build_class_index`)
//! from the truthy P279 statements of every item (the edges SPARQL's
//! `wdt:P279*` follows), kept as compact arrays (CSR: about 12 bytes an
//! edge), and saved as a small zstd-compressed file so later passes load
//! it in a fraction of a second.

use std::path::Path;
use std::sync::OnceLock;

use rustc_hash::FxHashSet;

use crate::FilterError;

const MAGIC: &[u8; 8] = b"WDCLASS1";

/// Child → parents edges, both ways (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct ClassIndex {
    /// Sorted classes that have parents.
    children: Vec<u32>,
    /// `parents[offsets[i]..offsets[i + 1]]` are the parents of `children[i]`.
    offsets: Vec<u32>,
    parents: Vec<u32>,
    /// What it was built from.
    source: String,
    reverse: OnceLock<Reverse>,
}

#[derive(Clone, Debug, Default)]
struct Reverse {
    parents: Vec<u32>,
    offsets: Vec<u32>,
    children: Vec<u32>,
}

fn csr(mut edges: Vec<(u32, u32)>) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
    edges.sort_unstable();
    edges.dedup();
    let mut keys = Vec::new();
    let mut offsets = Vec::new();
    let mut values = Vec::with_capacity(edges.len());
    for (k, v) in edges {
        if keys.last() != Some(&k) {
            keys.push(k);
            offsets.push(values.len() as u32);
        }
        values.push(v);
    }
    offsets.push(values.len() as u32);
    (keys, offsets, values)
}

fn lookup<'a>(keys: &[u32], offsets: &[u32], values: &'a [u32], k: u32) -> &'a [u32] {
    match keys.binary_search(&k) {
        Ok(i) => &values[offsets[i] as usize..offsets[i + 1] as usize],
        Err(_) => &[],
    }
}

/// The number of a `Q` id.
fn qnum(id: &str) -> Option<u32> {
    if !id.starts_with('Q') {
        return None;
    }
    wikidata_model::id_number(id).and_then(|n| u32::try_from(n).ok())
}

impl ClassIndex {
    /// From `(child, parent)` item numbers (`Q7` is 7).
    pub fn from_edges(edges: Vec<(u32, u32)>, source: impl Into<String>) -> ClassIndex {
        let (children, offsets, parents) = csr(edges);
        ClassIndex {
            children,
            offsets,
            parents,
            source: source.into(),
            reverse: OnceLock::new(),
        }
    }

    /// What the index was built from (the dump's file name).
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Classes with at least one parent.
    pub fn len(&self) -> usize {
        self.children.len()
    }

    /// Whether the index has no edges.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// Number of subclass-of edges.
    pub fn edges(&self) -> usize {
        self.parents.len()
    }

    /// The direct parents of item number `q`.
    pub fn parents(&self, q: u32) -> &[u32] {
        lookup(&self.children, &self.offsets, &self.parents, q)
    }

    fn reverse(&self) -> &Reverse {
        self.reverse.get_or_init(|| {
            let mut edges = Vec::with_capacity(self.parents.len());
            for (i, &c) in self.children.iter().enumerate() {
                for &p in &self.parents[self.offsets[i] as usize..self.offsets[i + 1] as usize] {
                    edges.push((p, c));
                }
            }
            let (parents, offsets, children) = csr(edges);
            Reverse {
                parents,
                offsets,
                children,
            }
        })
    }

    /// The direct subclasses of item number `q`.
    pub fn children_of(&self, q: u32) -> &[u32] {
        let r = self.reverse();
        lookup(&r.parents, &r.offsets, &r.children, q)
    }

    /// `roots` and all their subclasses, transitively (cycles are fine).
    pub fn subclasses(&self, roots: &[u32]) -> FxHashSet<u32> {
        let mut seen: FxHashSet<u32> = roots.iter().copied().collect();
        let mut stack: Vec<u32> = roots.to_vec();
        while let Some(q) = stack.pop() {
            for &c in self.children_of(q) {
                if seen.insert(c) {
                    stack.push(c);
                }
            }
        }
        seen
    }

    /// [`ClassIndex::subclasses`] by id (`["Q868557"]`); ids that are not
    /// items are ignored.
    pub fn subclasses_of_ids<S: AsRef<str>>(&self, roots: &[S]) -> FxHashSet<u32> {
        let roots: Vec<u32> = roots.iter().filter_map(|r| qnum(r.as_ref())).collect();
        self.subclasses(&roots)
    }

    /// All ancestors of `q` (not including `q`).
    pub fn ancestors(&self, q: u32) -> FxHashSet<u32> {
        let mut seen = FxHashSet::default();
        let mut stack = vec![q];
        while let Some(x) = stack.pop() {
            for &p in self.parents(x) {
                if seen.insert(p) {
                    stack.push(p);
                }
            }
        }
        seen
    }

    /// Whether `q` is `ancestor` or one of its subclasses.
    pub fn is_subclass_of(&self, q: u32, ancestor: u32) -> bool {
        q == ancestor || self.ancestors(q).contains(&ancestor)
    }

    /// Serialise (see [`ClassIndex::save`]).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity(16 + (self.children.len() * 2 + self.parents.len()) * 4);
        let src = self.source.as_bytes();
        raw.extend((src.len() as u32).to_le_bytes());
        raw.extend(src);
        raw.extend((self.children.len() as u32).to_le_bytes());
        raw.extend((self.parents.len() as u32).to_le_bytes());
        // Children as deltas compress well.
        let mut prev = 0;
        for &c in &self.children {
            raw.extend((c - prev).to_le_bytes());
            prev = c;
        }
        for w in self.offsets.windows(2) {
            raw.extend((w[1] - w[0]).to_le_bytes());
        }
        for &p in &self.parents {
            raw.extend(p.to_le_bytes());
        }
        let mut out = MAGIC.to_vec();
        out.extend(zstd::bulk::compress(&raw, 19).expect("compressing in memory"));
        out
    }

    /// Deserialise (see [`ClassIndex::save`]).
    pub fn from_bytes(bytes: &[u8]) -> Result<ClassIndex, FilterError> {
        let bad = |m: &str| FilterError::Invalid(format!("not a class index: {m}"));
        let body = bytes.strip_prefix(MAGIC).ok_or_else(|| bad("wrong magic"))?;
        let raw = zstd::stream::decode_all(body).map_err(|e| bad(&e.to_string()))?;
        fn u32_at(raw: &[u8], pos: &mut usize) -> Option<u32> {
            let b = raw.get(*pos..*pos + 4)?;
            *pos += 4;
            Some(u32::from_le_bytes(b.try_into().unwrap()))
        }
        let mut pos = 0usize;
        let slen = u32_at(&raw, &mut pos).ok_or_else(|| bad("too short"))? as usize;
        let source = raw
            .get(4..4 + slen)
            .and_then(|s| std::str::from_utf8(s).ok())
            .ok_or_else(|| bad("bad source name"))?
            .to_string();
        pos = 4 + slen;
        let n = u32_at(&raw, &mut pos).ok_or_else(|| bad("too short"))? as usize;
        let m = u32_at(&raw, &mut pos).ok_or_else(|| bad("too short"))? as usize;
        if raw.len() != pos + (2 * n + m) * 4 {
            return Err(bad("wrong length"));
        }
        let mut read = |count: usize| -> Vec<u32> {
            let v = raw[pos..pos + count * 4]
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            pos += count * 4;
            v
        };
        let mut children = read(n);
        let mut acc = 0u32;
        for c in &mut children {
            acc = acc.wrapping_add(*c);
            *c = acc;
        }
        let sizes = read(n);
        let parents = read(m);
        let mut offsets = Vec::with_capacity(n + 1);
        let mut o = 0u32;
        offsets.push(0);
        for s in sizes {
            o = o.checked_add(s).ok_or_else(|| bad("bad offsets"))?;
            offsets.push(o);
        }
        if o as usize != m || children.windows(2).any(|w| w[0] >= w[1]) {
            return Err(bad("inconsistent"));
        }
        Ok(ClassIndex {
            children,
            offsets,
            parents,
            source,
            reverse: OnceLock::new(),
        })
    }

    /// Write to a file: `WDCLASS1`, then zstd-compressed little-endian
    /// arrays (source name, the classes as deltas, their parent counts,
    /// the parents).
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), FilterError> {
        let path = path.as_ref();
        std::fs::write(path, self.to_bytes()).map_err(|e| FilterError::Io(format!("{}: {e}", path.display())))
    }

    /// Read a file written by [`ClassIndex::save`].
    pub fn load(path: impl AsRef<Path>) -> Result<ClassIndex, FilterError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| FilterError::Io(format!("{}: {e}", path.display())))?;
        Self::from_bytes(&bytes).map_err(|e| FilterError::Invalid(format!("{}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ClassIndex {
        // 868557 music festival ⊂ 132241 festival ⊂ 1656682 event;
        // 1 ⊂ 2 ⊂ 1 (a cycle).
        ClassIndex::from_edges(
            vec![
                (868557, 132241),
                (132241, 1656682),
                (99, 868557),
                (1, 2),
                (2, 1),
                (99, 868557),
            ],
            "test",
        )
    }

    #[test]
    fn closure() {
        let c = sample();
        assert_eq!(c.len(), 5);
        assert_eq!(c.edges(), 5);
        assert_eq!(c.parents(868557), &[132241]);
        let s = c.subclasses(&[132241]);
        assert_eq!(s.len(), 3);
        assert!(s.contains(&99) && s.contains(&868557) && s.contains(&132241));
        assert!(c.is_subclass_of(99, 1656682));
        assert!(!c.is_subclass_of(1656682, 99));
        assert_eq!(c.subclasses(&[1]).len(), 2);
        assert_eq!(c.subclasses_of_ids(&["Q868557", "P31"]).len(), 2);
    }

    #[test]
    fn round_trip() {
        let c = sample();
        let b = c.to_bytes();
        let d = ClassIndex::from_bytes(&b).unwrap();
        assert_eq!(d.source(), "test");
        assert_eq!(d.parents(99), &[868557]);
        assert_eq!(d.subclasses(&[1656682]).len(), 4);
        assert!(ClassIndex::from_bytes(b"WDCLASS1junk").is_err());
        assert!(ClassIndex::from_bytes(b"nope").is_err());
        let empty = ClassIndex::from_bytes(&ClassIndex::default().to_bytes()).unwrap();
        assert!(empty.is_empty());
    }
}
