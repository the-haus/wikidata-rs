//! Where items are: each item's country, through the P131 chain.
//!
//! Many items name only a place: P131 (located in the administrative
//! territorial entity) is a village or a district, with no P17 (country).
//! A [`PlaceIndex`] is built in one pass over a dump (see
//! `wikidata_export::build_place_index`) from the truthy P17, P131 and
//! P159 (headquarters location) statements of every item, and answers
//! "which country is this item in" for all of them:
//!
//! - a **place**'s country is its P17, or else the countries of its P131
//!   parents, followed up the chain (cycle-safe, at most [`MAX_DEPTH`]
//!   steps);
//! - an **item**'s countries are its P17, then the countries of its P131
//!   places, then those of its P159 places, without repeats.
//!
//! Only the resolved countries are kept (an item number and a shared set
//! of countries per item: about 6 bytes an item), and saved as a small
//! zstd-compressed file.

use std::path::Path;

use rustc_hash::FxHashMap;

use crate::FilterError;

const MAGIC: &[u8; 8] = b"WDPLACE1";

/// How many P131 steps are followed up from a place.
pub const MAX_DEPTH: usize = 24;

/// Which statement an edge comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceProperty {
    /// P17, country.
    Country = 0,
    /// P131, located in the administrative territorial entity.
    LocatedIn = 1,
    /// P159, headquarters location.
    Headquarters = 2,
}

/// Item numbers are below 2^30; the property rides in the top bits.
const VALUE_MASK: u32 = (1 << 30) - 1;

/// An edge from a pass: item `item` has `property` = item `value`.
pub fn place_edge(item: u32, property: PlaceProperty, value: u32) -> Option<(u32, u32)> {
    (value <= VALUE_MASK).then_some((item, value | (property as u32) << 30))
}

/// Items' countries (see the module docs).
#[derive(Clone, Default)]
pub struct PlaceIndex {
    /// Sorted item numbers that have a country.
    items: Vec<u32>,
    /// The country set of each item.
    sets: Vec<u32>,
    /// `values[offsets[s]..offsets[s + 1]]` are the countries of set `s`.
    offsets: Vec<u32>,
    values: Vec<u32>,
    /// `buckets[b]..buckets[b + 1]` are the items with `item >> SHIFT == b`.
    buckets: Vec<u32>,
    source: String,
}

const SHIFT: u32 = 10;

impl std::fmt::Debug for PlaceIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PlaceIndex({} items, {} sets, {:?})",
            self.len(),
            self.sets(),
            self.source
        )
    }
}

/// Resolution state of a place.
const UNSEEN: u32 = u32::MAX;
const ON_STACK: u32 = u32::MAX - 1;

/// The edges of a pass, as CSR by item.
struct Graph {
    items: Vec<u32>,
    offsets: Vec<u32>,
    /// Values with the property in the top bits, sorted by property.
    values: Vec<u32>,
}

impl Graph {
    fn index(&self, item: u32) -> Option<usize> {
        self.items.binary_search(&item).ok()
    }

    fn of(&self, i: usize, p: PlaceProperty) -> impl Iterator<Item = u32> + '_ {
        let tag = p as u32;
        self.values[self.offsets[i] as usize..self.offsets[i + 1] as usize]
            .iter()
            .filter(move |v| *v >> 30 == tag)
            .map(|v| v & VALUE_MASK)
    }
}

/// Interned country sets.
#[derive(Default)]
struct Sets {
    ids: FxHashMap<Vec<u32>, u32>,
    offsets: Vec<u32>,
    values: Vec<u32>,
}

impl Sets {
    fn id(&mut self, set: Vec<u32>) -> u32 {
        if let Some(&id) = self.ids.get(&set) {
            return id;
        }
        let id = self.ids.len() as u32;
        if self.offsets.is_empty() {
            self.offsets.push(0);
        }
        self.values.extend_from_slice(&set);
        self.offsets.push(self.values.len() as u32);
        self.ids.insert(set, id);
        id
    }

    fn get(&self, id: u32) -> &[u32] {
        &self.values[self.offsets[id as usize] as usize..self.offsets[id as usize + 1] as usize]
    }
}

fn push_unique(out: &mut Vec<u32>, vs: impl IntoIterator<Item = u32>) {
    for v in vs {
        if !out.contains(&v) {
            out.push(v);
        }
    }
}

impl PlaceIndex {
    /// Build from the `(item, value)` edges of [`place_edge`].
    pub fn from_edges(mut edges: Vec<(u32, u32)>, source: impl Into<String>) -> PlaceIndex {
        // Sorted by item, then property, then value (dump order is lost:
        // a place's countries come out in id order).
        edges.sort_unstable_by_key(|&(i, v)| (i, v >> 30, v & VALUE_MASK));
        edges.dedup();
        let mut g = Graph {
            items: Vec::new(),
            offsets: Vec::new(),
            values: Vec::with_capacity(edges.len()),
        };
        for (i, v) in edges {
            if g.items.last() != Some(&i) {
                g.items.push(i);
                g.offsets.push(g.values.len() as u32);
            }
            g.values.push(v);
        }
        g.offsets.push(g.values.len() as u32);

        let mut sets = Sets::default();
        sets.id(Vec::new()); // set 0: no country
                             // The country set of each place (by graph index), resolved once.
        let mut place = vec![UNSEEN; g.items.len()];
        let mut items = Vec::new();
        let mut item_sets = Vec::new();
        for i in 0..g.items.len() {
            let mut countries: Vec<u32> = g.of(i, PlaceProperty::Country).collect();
            for p in [PlaceProperty::LocatedIn, PlaceProperty::Headquarters] {
                for parent in g.of(i, p).collect::<Vec<_>>() {
                    if let Some(j) = g.index(parent) {
                        let s = resolve(&g, &mut place, &mut sets, j);
                        push_unique(&mut countries, sets.get(s).to_vec());
                    }
                }
            }
            if !countries.is_empty() {
                items.push(g.items[i]);
                item_sets.push(sets.id(countries));
            }
        }
        let mut index = PlaceIndex {
            items,
            sets: item_sets,
            offsets: sets.offsets,
            values: sets.values,
            buckets: Vec::new(),
            source: source.into(),
        };
        index.make_buckets();
        index
    }

    fn make_buckets(&mut self) {
        let n = self.items.last().map_or(0, |&m| (m >> SHIFT) as usize + 1);
        let mut buckets = Vec::with_capacity(n + 1);
        let mut i = 0usize;
        for b in 0..=n as u32 {
            while i < self.items.len() && self.items[i] >> SHIFT < b {
                i += 1;
            }
            buckets.push(i as u32);
        }
        self.buckets = buckets;
    }

    /// What the index was built from (the dump's file name).
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Items with a country.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether no item has a country.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Distinct sets of countries.
    pub fn sets(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// Approximate heap bytes used.
    pub fn heap_bytes(&self) -> usize {
        4 * (self.items.len() + self.sets.len() + self.offsets.len() + self.values.len() + self.buckets.len())
    }

    /// The country set of item number `q` (an index for
    /// [`PlaceIndex::set`]), when it has a country.
    #[inline]
    pub fn set_of(&self, q: u32) -> Option<u32> {
        let b = (q >> SHIFT) as usize;
        if b + 1 >= self.buckets.len() {
            return None;
        }
        let (lo, hi) = (self.buckets[b] as usize, self.buckets[b + 1] as usize);
        let i = self.items[lo..hi].binary_search(&q).ok()?;
        Some(self.sets[lo + i])
    }

    /// The countries of a set.
    pub fn set(&self, s: u32) -> &[u32] {
        match (self.offsets.get(s as usize), self.offsets.get(s as usize + 1)) {
            (Some(&a), Some(&b)) => &self.values[a as usize..b as usize],
            _ => &[],
        }
    }

    /// The countries of item number `q` (P17 first, then through P131,
    /// then through P159); empty when unknown.
    pub fn countries(&self, q: u32) -> &[u32] {
        self.set_of(q).map_or(&[], |s| self.set(s))
    }

    /// [`PlaceIndex::countries`] by id (`"Q1072"` → `["Q55"]`).
    pub fn country_of(&self, id: &str) -> Vec<String> {
        qnum(id).map_or_else(Vec::new, |q| {
            self.countries(q).iter().map(|c| format!("Q{c}")).collect()
        })
    }

    /// For each set, whether it holds one of `countries` (for filters).
    pub fn sets_with(&self, countries: &[u32]) -> Vec<bool> {
        (0..self.sets() as u32)
            .map(|s| self.set(s).iter().any(|c| countries.contains(c)))
            .collect()
    }

    /// Serialise (see [`PlaceIndex::save`]).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity(16 + (2 * self.items.len() + self.offsets.len() + self.values.len()) * 4);
        let src = self.source.as_bytes();
        raw.extend((src.len() as u32).to_le_bytes());
        raw.extend(src);
        raw.extend((self.items.len() as u32).to_le_bytes());
        raw.extend((self.sets() as u32).to_le_bytes());
        raw.extend((self.values.len() as u32).to_le_bytes());
        let mut prev = 0;
        for &i in &self.items {
            raw.extend((i - prev).to_le_bytes());
            prev = i;
        }
        for &s in &self.sets {
            raw.extend(s.to_le_bytes());
        }
        for w in self.offsets.windows(2) {
            raw.extend((w[1] - w[0]).to_le_bytes());
        }
        for &v in &self.values {
            raw.extend(v.to_le_bytes());
        }
        let mut out = MAGIC.to_vec();
        out.extend(zstd::bulk::compress(&raw, 19).expect("compressing in memory"));
        out
    }

    /// Deserialise (see [`PlaceIndex::save`]).
    pub fn from_bytes(bytes: &[u8]) -> Result<PlaceIndex, FilterError> {
        let bad = |m: &str| FilterError::Invalid(format!("not a place index: {m}"));
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
        let nsets = u32_at(&raw, &mut pos).ok_or_else(|| bad("too short"))? as usize;
        let nvalues = u32_at(&raw, &mut pos).ok_or_else(|| bad("too short"))? as usize;
        if raw.len() != pos + (2 * n + nsets + nvalues) * 4 {
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
        let mut items = read(n);
        let mut acc = 0u32;
        for i in &mut items {
            acc = acc.wrapping_add(*i);
            *i = acc;
        }
        let sets = read(n);
        let sizes = read(nsets);
        let values = read(nvalues);
        let mut offsets = Vec::with_capacity(nsets + 1);
        let mut o = 0u32;
        offsets.push(0);
        for s in sizes {
            o = o.checked_add(s).ok_or_else(|| bad("bad offsets"))?;
            offsets.push(o);
        }
        if o as usize != nvalues || items.windows(2).any(|w| w[0] >= w[1]) || sets.iter().any(|&s| s as usize >= nsets)
        {
            return Err(bad("inconsistent"));
        }
        let mut index = PlaceIndex {
            items,
            sets,
            offsets,
            values,
            buckets: Vec::new(),
            source,
        };
        index.make_buckets();
        Ok(index)
    }

    /// Write to a file: `WDPLACE1`, then zstd-compressed little-endian
    /// arrays (source name; the items as deltas, their sets; the sets'
    /// sizes and countries).
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), FilterError> {
        let path = path.as_ref();
        std::fs::write(path, self.to_bytes()).map_err(|e| FilterError::Io(format!("{}: {e}", path.display())))
    }

    /// Read a file written by [`PlaceIndex::save`].
    pub fn load(path: impl AsRef<Path>) -> Result<PlaceIndex, FilterError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| FilterError::Io(format!("{}: {e}", path.display())))?;
        Self::from_bytes(&bytes).map_err(|e| FilterError::Invalid(format!("{}: {e}", path.display())))
    }
}

/// The country set of place `start` (a graph index): its P17, or else its
/// P131 parents' countries, up the chain. Iterative, so a long chain
/// cannot overflow the stack. A place met again on the current path (a
/// cycle) or beyond [`MAX_DEPTH`] adds nothing; a result missing such a
/// branch is not remembered (the place may be reached again on a shorter
/// path), and each call visits at most [`MAX_VISITS`] places.
fn resolve(g: &Graph, place: &mut [u32], sets: &mut Sets, start: usize) -> u32 {
    match place[start] {
        UNSEEN => {}
        ON_STACK => return 0,
        s => return s,
    }
    struct Frame {
        at: usize,
        parents: Vec<usize>,
        next: usize,
        countries: Vec<u32>,
        /// A branch was cut (cycle, depth, budget).
        partial: bool,
    }
    let frame = |g: &Graph, at: usize| -> Frame {
        let countries: Vec<u32> = g.of(at, PlaceProperty::Country).collect();
        let parents = if countries.is_empty() {
            g.of(at, PlaceProperty::LocatedIn).filter_map(|p| g.index(p)).collect()
        } else {
            Vec::new()
        };
        Frame {
            at,
            parents,
            next: 0,
            countries,
            partial: false,
        }
    };
    let mut visits = 1usize;
    place[start] = ON_STACK;
    let mut stack = vec![frame(g, start)];
    loop {
        let depth = stack.len();
        let top = stack.last_mut().unwrap();
        if top.next < top.parents.len() {
            let p = top.parents[top.next];
            top.next += 1;
            match place[p] {
                UNSEEN if depth < MAX_DEPTH && visits < MAX_VISITS => {
                    visits += 1;
                    place[p] = ON_STACK;
                    stack.push(frame(g, p));
                }
                UNSEEN | ON_STACK => top.partial = true,
                s => {
                    let vs = sets.get(s).to_vec();
                    push_unique(&mut top.countries, vs);
                }
            }
            continue;
        }
        let done = stack.pop().unwrap();
        let s = sets.id(done.countries);
        place[done.at] = if done.partial { UNSEEN } else { s };
        match stack.last_mut() {
            Some(parent) => {
                parent.partial |= done.partial;
                let vs = sets.get(s).to_vec();
                push_unique(&mut parent.countries, vs);
            }
            None => return s,
        }
    }
}

/// The most places one resolution visits.
const MAX_VISITS: usize = 10_000;

/// The number of a `Q` id.
fn qnum(id: &str) -> Option<u32> {
    if !id.starts_with('Q') {
        return None;
    }
    wikidata_model::id_number(id).and_then(|n| u32::try_from(n).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use PlaceProperty::*;

    fn e(item: u32, p: PlaceProperty, value: u32) -> (u32, u32) {
        place_edge(item, p, value).unwrap()
    }

    fn sample() -> PlaceIndex {
        PlaceIndex::from_edges(
            vec![
                // A venue in a village, in a municipality, in the Netherlands.
                e(500, LocatedIn, 400),
                e(400, LocatedIn, 300),
                e(300, Country, 55),
                e(55, Country, 55),
                // A company with its own country and a headquarters abroad.
                e(600, Country, 55),
                e(600, Headquarters, 700),
                e(700, Country, 145),
                // A cycle: 800 ⊂ 801 ⊂ 800, and 802 in it.
                e(800, LocatedIn, 801),
                e(801, LocatedIn, 800),
                e(802, LocatedIn, 800),
                // A place in two countries.
                e(900, Country, 31),
                e(900, Country, 55),
                e(901, LocatedIn, 900),
            ],
            "test",
        )
    }

    #[test]
    fn chains() {
        let p = sample();
        assert_eq!(p.countries(500), [55]);
        assert_eq!(p.countries(400), [55]);
        assert_eq!(p.countries(55), [55]);
        assert_eq!(p.countries(600), [55, 145]);
        assert_eq!(p.countries(901), [31, 55]);
        assert_eq!(p.country_of("Q500"), ["Q55"]);
        assert!(p.countries(800).is_empty() && p.countries(802).is_empty());
        assert!(p.countries(12345).is_empty() && p.countries(u32::MAX).is_empty());
        assert!(p.country_of("P17").is_empty());
        let ok = p.sets_with(&[145]);
        assert!(ok[p.set_of(600).unwrap() as usize]);
        assert!(!ok[p.set_of(500).unwrap() as usize]);
    }

    #[test]
    fn cycles_do_not_hide_countries() {
        // 800 ⊂ 801 ⊂ 800, 800 ⊂ 802 in country 9: both are in 9,
        // whichever is resolved first; 803 is in 801.
        for order in [[800, 801], [801, 800]] {
            let mut edges = vec![
                e(order[0], LocatedIn, order[1]),
                e(order[1], LocatedIn, order[0]),
                e(800, LocatedIn, 802),
                e(802, Country, 9),
                e(803, LocatedIn, 801),
                e(799, LocatedIn, 801),
            ];
            edges.reverse();
            let p = PlaceIndex::from_edges(edges, "cycle");
            for i in [799, 800, 801, 803] {
                assert_eq!(p.countries(i), [9], "{i}");
            }
        }
    }

    #[test]
    fn deep_chains_stop() {
        // 0 ⊂ 1 ⊂ … ⊂ 99, and 99 is in country 7: too far for 0.
        let mut edges: Vec<(u32, u32)> = (0..99).map(|i| e(i + 10, LocatedIn, i + 11)).collect();
        edges.push(e(109, Country, 7));
        let p = PlaceIndex::from_edges(edges, "deep");
        assert_eq!(p.countries(109), [7]);
        assert_eq!(p.countries(100), [7]);
        assert!(p.countries(10).is_empty());
        // 24 steps up from 85 (the 24th is 109): found, though 85 was on
        // the path of 10's resolution, which stopped early.
        assert_eq!(p.countries(85), [7]);
        assert!(p.countries(84).is_empty());
    }

    #[test]
    fn round_trip() {
        let p = sample();
        let q = PlaceIndex::from_bytes(&p.to_bytes()).unwrap();
        assert_eq!(q.source(), "test");
        assert_eq!(q.len(), p.len());
        for i in [55, 300, 400, 500, 600, 700, 802, 900, 901] {
            assert_eq!(q.countries(i), p.countries(i), "{i}");
        }
        assert!(PlaceIndex::from_bytes(b"WDPLACE1junk").is_err());
        let empty = PlaceIndex::from_bytes(&PlaceIndex::default().to_bytes()).unwrap();
        assert!(empty.is_empty() && empty.countries(1).is_empty());
    }
}
