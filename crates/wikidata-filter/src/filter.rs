//! Filter expressions, and their evaluation in two steps.
//!
//! 1. **Before parsing**, on the raw line: the type and id from the line's
//!    first bytes ([`wikidata_model::peek`]) answer `ids` and `type`
//!    exactly; the other filters look for byte strings that must occur
//!    when they match (`"P17":[` for a claim on P17, `"id":"Q55"` for its
//!    value, `"site":"nlwiki"` for a sitelink) with memchr's SIMD
//!    substring search or, for many values, an Aho–Corasick automaton.
//!    Most lines are rejected here and never parsed.
//! 2. **After a probe parse** of only what the filter looks at (the
//!    filter's properties and sites; everything else is skipped), the
//!    filter is evaluated exactly.
//!
//! Claim filters look at the statements that are output by default: all
//! but the deprecated ones.

use std::sync::Arc;

use aho_corasick::AhoCorasick;
use memchr::memmem::Finder;
use rustc_hash::{FxHashMap, FxHashSet};
use wikidata_model::{parse_entity, Compiled, Entity, EntityType, ModelOptions, Peek, Pid, Rank};

use crate::classes::ClassIndex;
use crate::ids::{id_key, IdSet, IdSetBuilder};
use crate::json::Json;
use crate::places::PlaceIndex;
use crate::redirects::RedirectIndex;
use crate::FilterError;

/// A filter (see the crate docs for the JSON form).
#[derive(Clone, Debug, PartialEq)]
pub enum Filter {
    /// All must match.
    All(Vec<Filter>),
    /// Any must match.
    Any(Vec<Filter>),
    /// Must not match.
    Not(Box<Filter>),
    /// One of these ids (`Q42`, `P31`, `L7`).
    Ids(Vec<String>),
    /// One of these types.
    Types(Vec<EntityType>),
    /// Has a statement for any of these properties.
    HasProperty(Vec<Pid>),
    /// Has a statement for `property` with one of `values` (entity ids,
    /// or the exact string of a string-like value); no values: any value.
    Claim {
        /// The property.
        property: Pid,
        /// Accepted values.
        values: Vec<String>,
    },
    /// An instance (P31) of one of `classes` or, with `subclasses`, of one
    /// of their subclasses (needs a [`ClassIndex`]).
    InstanceOf {
        /// Item ids.
        classes: Vec<String>,
        /// Include subclasses.
        subclasses: bool,
    },
    /// Has a sitelink to one of these sites (`nlwiki`).
    Sitelink(Vec<String>),
    /// One of these items, by number (`42` is `Q42`).
    ItemNumbers(Vec<u32>),
    /// One of these ids, unless its `lastrevid` is at most the known
    /// revision (`revisions[i]` for `ids[i]`): the entity did not change.
    /// Unchanged entities are rejected before parsing, but count as seen
    /// for [`CompiledFilter::missing_ids`].
    Revisions {
        /// The ids.
        ids: Vec<String>,
        /// Their known revisions.
        revisions: Vec<u64>,
    },
    /// [`Filter::Revisions`] with item numbers (`42` is `Q42`).
    ItemRevisions {
        /// The item numbers.
        items: Vec<u32>,
        /// Their known revisions.
        revisions: Vec<u64>,
    },
    /// An item in one of these countries (item ids): by its P17, its P131
    /// chain or its P159 (needs a [`PlaceIndex`]).
    InCountry(Vec<String>),
    /// A filter with a name, reported in [`CompiledFilter::matched_names`]
    /// when it matches an entity the whole filter matches.
    Named {
        /// The name.
        name: String,
        /// The filter.
        filter: Box<Filter>,
    },
}

fn strings(v: &Json, what: &str) -> Result<Vec<String>, FilterError> {
    match v {
        Json::String(s) => Ok(s
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()),
        Json::Array(a) => a
            .iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| FilterError::Invalid(format!("{what}: expected strings")))
            })
            .collect(),
        _ => Err(FilterError::Invalid(format!(
            "{what}: expected a string or an array of strings"
        ))),
    }
}

fn pid(s: &str) -> Result<Pid, FilterError> {
    Pid::parse(s).ok_or_else(|| FilterError::Invalid(format!("not a property id: {s:?}")))
}

impl Filter {
    /// All of `filters` (one filter stays itself).
    pub fn all(mut filters: Vec<Filter>) -> Filter {
        if filters.len() == 1 {
            filters.pop().unwrap()
        } else {
            Filter::All(filters)
        }
    }

    /// Any of `filters` (one filter stays itself).
    pub fn any(mut filters: Vec<Filter>) -> Filter {
        if filters.len() == 1 {
            filters.pop().unwrap()
        } else {
            Filter::Any(filters)
        }
    }

    /// Parse the JSON form (see the crate docs). A `serde_json::Value`
    /// has its object keys sorted (without serde_json's `preserve_order`),
    /// and so are named filters; [`Filter::parse_json`] keeps the order of
    /// the text.
    pub fn from_json(v: &serde_json::Value) -> Result<Filter, FilterError> {
        Filter::from_ordered(&Json::from(v), &Arrays::default())
    }

    /// [`Filter::from_json`], where `"ids": {"$items": k}` stands for the
    /// item numbers `items[k]` (for callers that hold ids as numbers and
    /// would rather not write a million of them out as JSON).
    pub fn from_json_with(v: &serde_json::Value, items: &[&[u32]]) -> Result<Filter, FilterError> {
        Filter::from_ordered(&Json::from(v), &Arrays { items, revisions: &[] })
    }

    /// Parse JSON text, with `"ids": {"$items": k}` as in
    /// [`Filter::from_json_with`]; objects are read in their order.
    pub fn parse_json_with(text: &str, items: &[&[u32]]) -> Result<Filter, FilterError> {
        Filter::parse_json_with_revisions(text, items, &[])
    }

    /// [`Filter::parse_json_with`], where `"revisions": {"$revisions": k}`
    /// beside `ids` stands for `revisions[k]` (see [`Filter::Revisions`]).
    pub fn parse_json_with_revisions(
        text: &str,
        items: &[&[u32]],
        revisions: &[&[u64]],
    ) -> Result<Filter, FilterError> {
        let v: Json = serde_json::from_str(text).map_err(|e| FilterError::Invalid(format!("filter JSON: {e}")))?;
        Filter::from_ordered(&v, &Arrays { items, revisions })
    }

    fn from_ordered(v: &Json, arrays: &Arrays<'_>) -> Result<Filter, FilterError> {
        let items = arrays.items;
        let parse = |v: &Json| Filter::from_ordered(v, arrays);
        // `[f, g]` or `{"venue": f, "festival": g}` (named filters).
        let list = |v: &Json, what: &str| -> Result<Vec<Filter>, FilterError> {
            match v {
                Json::Array(a) => a.iter().map(parse).collect(),
                Json::Object(o) => o
                    .iter()
                    .map(|(name, f)| {
                        Ok(Filter::Named {
                            name: name.clone(),
                            filter: Box::new(parse(f)?),
                        })
                    })
                    .collect(),
                _ => Err(FilterError::Invalid(format!(
                    "{what}: expected an array or an object of named filters"
                ))),
            }
        };
        if let Json::Array(a) = v {
            return Ok(Filter::all(a.iter().map(parse).collect::<Result<_, _>>()?));
        }
        let o = v
            .as_object()
            .ok_or_else(|| FilterError::Invalid("a filter is a JSON object".into()))?;
        let mut parts = Vec::new();
        let mut name = None;
        let known = match o.get("revisions") {
            Some(r) => Some(revisions(r, arrays)?),
            None => None,
        };
        if known.is_some() && !o.contains_key("ids") {
            return Err(FilterError::Invalid("revisions: only beside ids".into()));
        }
        for (k, v) in o {
            parts.push(match k.as_str() {
                "revisions" => continue,
                "ids" if known.is_some() || matches!(v, Json::Object(m) if !m.contains_key("$items")) => {
                    with_revisions(v, known.clone(), items)?
                }
                "name" => {
                    name = Some(
                        v.as_str()
                            .ok_or_else(|| FilterError::Invalid("name: expected a string".into()))?
                            .to_string(),
                    );
                    continue;
                }
                "all" | "and" => Filter::All(list(v, "all")?),
                "any" | "or" => Filter::Any(list(v, "any")?),
                "not" => Filter::Not(Box::new(parse(v)?)),
                "ids" => match v.get("$items").and_then(Json::as_u64) {
                    Some(k) => Filter::ItemNumbers(
                        items
                            .get(k as usize)
                            .ok_or_else(|| FilterError::Invalid(format!("ids: no item numbers #{k}")))?
                            .to_vec(),
                    ),
                    None => Filter::Ids(strings(v, "ids")?),
                },
                "type" | "types" => Filter::Types(
                    strings(v, "type")?
                        .iter()
                        .map(|t| {
                            EntityType::parse(t).ok_or_else(|| FilterError::Invalid(format!("unknown type {t:?}")))
                        })
                        .collect::<Result<_, _>>()?,
                ),
                "hasProperty" => Filter::HasProperty(
                    strings(v, "hasProperty")?
                        .iter()
                        .map(|p| pid(p))
                        .collect::<Result<_, _>>()?,
                ),
                "claim" => match v {
                    Json::String(s) => Filter::parse_claim(s)?,
                    Json::Object(c) if c.contains_key("property") => Filter::Claim {
                        property: pid(c["property"]
                            .as_str()
                            .ok_or_else(|| FilterError::Invalid("claim.property: expected a string".into()))?)?,
                        values: match c.get("values").or_else(|| c.get("value")) {
                            Some(v) => strings(v, "claim.values")?,
                            None => Vec::new(),
                        },
                    },
                    // {"P17": ["Q55"]}: one claim filter per property, all of them.
                    Json::Object(c) => Filter::all(
                        c.iter()
                            .map(|(p, vs)| {
                                Ok(Filter::Claim {
                                    property: pid(p)?,
                                    values: strings(vs, "claim values")?,
                                })
                            })
                            .collect::<Result<_, FilterError>>()?,
                    ),
                    _ => return Err(FilterError::Invalid("claim: expected an object or \"P17=Q55\"".into())),
                },
                "instanceOf" => match v {
                    Json::Object(c) => Filter::InstanceOf {
                        classes: strings(
                            c.get("classes")
                                .ok_or_else(|| FilterError::Invalid("instanceOf.classes is required".into()))?,
                            "instanceOf.classes",
                        )?,
                        subclasses: c.get("subclasses").and_then(Json::as_bool).unwrap_or(false),
                    },
                    other => Filter::InstanceOf {
                        classes: strings(other, "instanceOf")?,
                        subclasses: false,
                    },
                },
                "sitelink" | "sitelinks" => Filter::Sitelink(strings(v, "sitelink")?),
                "inCountry" => Filter::InCountry(strings(v, "inCountry")?),
                other => return Err(FilterError::Invalid(format!("unknown filter {other:?}"))),
            });
        }
        if parts.is_empty() {
            return Err(FilterError::Invalid("an empty filter".into()));
        }
        let f = Filter::all(parts);
        Ok(match name {
            Some(name) => Filter::Named {
                name,
                filter: Box::new(f),
            },
            None => f,
        })
    }

    /// Parse JSON text (objects are read in their order).
    pub fn parse_json(text: &str) -> Result<Filter, FilterError> {
        Filter::parse_json_with(text, &[])
    }

    /// Parse `P17=Q55,Q31` (or `P17` for any value).
    pub fn parse_claim(s: &str) -> Result<Filter, FilterError> {
        let (p, vs) = s.split_once('=').unwrap_or((s, ""));
        Ok(Filter::Claim {
            property: pid(p.trim())?,
            values: vs
                .split(',')
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect(),
        })
    }

    /// A stable hash of the filter (FNV-1a over its parts, in order): a
    /// checkpoint records it, so a pass resumes only with the same filter.
    pub fn fingerprint(&self) -> u64 {
        let mut h = Fnv::default();
        self.hash_into(&mut h);
        h.0
    }

    fn hash_into(&self, h: &mut Fnv) {
        let strs = |h: &mut Fnv, v: &[String]| {
            h.u64(v.len() as u64);
            v.iter().for_each(|s| h.str(s));
        };
        match self {
            Filter::All(fs) | Filter::Any(fs) => {
                h.u64(if matches!(self, Filter::All(_)) { 1 } else { 2 });
                h.u64(fs.len() as u64);
                fs.iter().for_each(|f| f.hash_into(h));
            }
            Filter::Not(f) => {
                h.u64(3);
                f.hash_into(h);
            }
            Filter::Ids(ids) => {
                h.u64(4);
                strs(h, ids);
            }
            Filter::Types(ts) => {
                h.u64(5);
                ts.iter().for_each(|t| h.u64(t.code().into()));
            }
            Filter::HasProperty(ps) => {
                h.u64(6);
                ps.iter().for_each(|p| h.u64(p.0.into()));
            }
            Filter::Claim { property, values } => {
                h.u64(7);
                h.u64(property.0.into());
                strs(h, values);
            }
            Filter::InstanceOf { classes, subclasses } => {
                h.u64(8 + u64::from(*subclasses));
                strs(h, classes);
            }
            Filter::Sitelink(sites) => {
                h.u64(10);
                strs(h, sites);
            }
            Filter::ItemNumbers(ns) => {
                h.u64(11);
                h.u64(ns.len() as u64);
                ns.iter().for_each(|&n| h.u64(n.into()));
            }
            Filter::Named { name, filter } => {
                h.u64(12);
                h.str(name);
                filter.hash_into(h);
            }
            Filter::Revisions { ids, revisions } => {
                h.u64(13);
                strs(h, ids);
                revisions.iter().for_each(|&r| h.u64(r));
            }
            Filter::InCountry(countries) => {
                h.u64(15);
                strs(h, countries);
            }
            Filter::ItemRevisions { items, revisions } => {
                h.u64(14);
                h.u64(items.len() as u64);
                items.iter().for_each(|&n| h.u64(n.into()));
                revisions.iter().for_each(|&r| h.u64(r));
            }
        }
    }

    /// Whether this filter (or a part) needs a class index.
    pub fn needs_class_index(&self) -> bool {
        self.any_part(&|f| matches!(f, Filter::InstanceOf { subclasses: true, .. }))
    }

    /// Whether this filter (or a part) needs a place index.
    pub fn needs_place_index(&self) -> bool {
        self.any_part(&|f| matches!(f, Filter::InCountry(_)))
    }

    fn any_part(&self, pred: &dyn Fn(&Filter) -> bool) -> bool {
        pred(self)
            || match self {
                Filter::All(f) | Filter::Any(f) => f.iter().any(|f| f.any_part(pred)),
                Filter::Not(f) | Filter::Named { filter: f, .. } => f.any_part(pred),
                _ => false,
            }
    }

    /// Prepare for matching; `classes` is required for
    /// `instanceOf` with subclasses.
    pub fn compile(&self, classes: Option<&ClassIndex>) -> Result<CompiledFilter, FilterError> {
        self.compile_with(&Indexes {
            classes,
            ..Default::default()
        })
    }

    /// Prepare for matching, with the indexes some filters need.
    pub fn compile_with(&self, indexes: &Indexes<'_>) -> Result<CompiledFilter, FilterError> {
        let classes = indexes.classes;
        let mut needs = Needs::default();
        let root = compile(self, indexes, &mut needs)?;
        let probe = (!needs.properties.is_empty() || !needs.sites.is_empty()).then(|| {
            Compiled::new(ModelOptions {
                languages: Some(Vec::new()),
                sitelinks: Some(needs.sites.iter().cloned().collect()),
                properties: Some(needs.properties.iter().map(|&p| Pid(p)).collect()),
                labels: false,
                descriptions: false,
                aliases: false,
                qualifiers: false,
                qualifier_properties: None,
                references: false,
                deprecated: false,
                best_rank: false,
                derived: false,
            })
        });
        let mut named = Vec::new();
        let mut id_sets = Vec::new();
        let mut known = Vec::new();
        walk(&root, &mut named, &mut id_sets, &mut known);
        // Requested items that redirect, by target.
        let mut merged: FxHashMap<i64, Vec<i64>> = FxHashMap::default();
        if let Some(r) = &indexes.redirects {
            let mut keys: Vec<i64> = id_sets.iter().flat_map(|s| s.iter()).collect();
            keys.sort_unstable();
            keys.dedup();
            for k in keys {
                if let Some(t) = u32::try_from(k).ok().and_then(|q| r.resolve(q)) {
                    merged.entry(i64::from(t)).or_default().push(k);
                }
            }
        }
        let mut fingerprint = self.fingerprint();
        if let Some(r) = &indexes.redirects {
            let mut h = Fnv(fingerprint);
            h.str(r.source());
            h.u64(r.len() as u64);
            fingerprint = h.0;
        }
        if let (true, Some(c)) = (self.needs_class_index(), classes) {
            // Another class index can expand the same classes differently.
            let mut h = Fnv(fingerprint);
            h.str(c.source());
            h.u64(c.edges() as u64);
            fingerprint = h.0;
        }
        if let (true, Some(p)) = (self.needs_place_index(), &indexes.places) {
            let mut h = Fnv(fingerprint);
            h.str(p.source());
            h.u64(p.len() as u64);
            fingerprint = h.0;
        }
        Ok(CompiledFilter {
            fingerprint,
            redirects: indexes.redirects.clone(),
            merged,
            root,
            probe,
            names: needs.names,
            named,
            id_sets,
            known,
        })
    }
}

/// The indexes filters may need ([`Filter::compile_with`]).
#[derive(Clone, Default)]
pub struct Indexes<'a> {
    /// For `instanceOf` with subclasses.
    pub classes: Option<&'a ClassIndex>,
    /// For `inCountry`.
    pub places: Option<Arc<PlaceIndex>>,
    /// Follow merges: `ids` also match the items their ids redirect to
    /// ([`CompiledFilter::merged_from`]), and the ids not found are sorted
    /// into merged, deleted and newer ([`CompiledFilter::misses`]).
    pub redirects: Option<Arc<RedirectIndex>>,
}

/// FNV-1a, 64 bits.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.bytes(s.as_bytes());
    }
}

/// Typed arrays a filter's JSON refers to (`{"$items": k}`,
/// `{"$revisions": k}`).
#[derive(Default)]
struct Arrays<'a> {
    items: &'a [&'a [u32]],
    revisions: &'a [&'a [u64]],
}

/// `"revisions"`: `{"$revisions": k}` or an array of numbers.
fn revisions(v: &Json, arrays: &Arrays<'_>) -> Result<Vec<u64>, FilterError> {
    if let Some(k) = v.get("$revisions").and_then(Json::as_u64) {
        return arrays
            .revisions
            .get(k as usize)
            .map(|r| r.to_vec())
            .ok_or_else(|| FilterError::Invalid(format!("revisions: no revisions #{k}")));
    }
    match v {
        Json::Array(a) => a
            .iter()
            .map(|r| {
                r.as_u64()
                    .ok_or_else(|| FilterError::Invalid("revisions: expected revision ids".into()))
            })
            .collect(),
        _ => Err(FilterError::Invalid(
            "revisions: expected an array of revision ids".into(),
        )),
    }
}

/// `ids` with known revisions: `ids` beside `revisions`, or a map
/// `{"Q42": 2000000001}`.
fn with_revisions(ids: &Json, known: Option<Vec<u64>>, items: &[&[u32]]) -> Result<Filter, FilterError> {
    let mismatch = |n: usize, m: usize| FilterError::Invalid(format!("ids and revisions: {n} ids, {m} revisions"));
    match (ids, known) {
        (Json::Object(m), None) => {
            let mut ids = Vec::new();
            let mut revisions = Vec::new();
            for (id, r) in m {
                ids.push(id.clone());
                revisions.push(
                    r.as_u64()
                        .ok_or_else(|| FilterError::Invalid(format!("ids: {id}: expected a revision id")))?,
                );
            }
            Ok(Filter::Revisions { ids, revisions })
        }
        (v, Some(revisions)) => match v.get("$items").and_then(Json::as_u64) {
            Some(k) => {
                let items = items
                    .get(k as usize)
                    .ok_or_else(|| FilterError::Invalid(format!("ids: no item numbers #{k}")))?
                    .to_vec();
                if items.len() != revisions.len() {
                    return Err(mismatch(items.len(), revisions.len()));
                }
                Ok(Filter::ItemRevisions { items, revisions })
            }
            None => {
                let ids = strings(v, "ids")?;
                if ids.len() != revisions.len() {
                    return Err(mismatch(ids.len(), revisions.len()));
                }
                Ok(Filter::Revisions { ids, revisions })
            }
        },
        (_, None) => Err(FilterError::Invalid("ids: expected ids".into())),
    }
}

#[derive(Default)]
struct Needs {
    properties: FxHashSet<u32>,
    sites: FxHashSet<String>,
    names: Vec<String>,
}

/// Byte strings, one of which must occur. The single-needle `Finder` is
/// boxed: it holds its searcher inline (hundreds of bytes), which would make
/// every `Needles`, and so every filter node, that large.
enum Needles {
    One(Box<Finder<'static>>),
    Many(AhoCorasick),
}

impl Needles {
    fn new(patterns: Vec<Vec<u8>>) -> Option<Needles> {
        match patterns.len() {
            0 => None,
            1 => Some(Needles::One(Box::new(Finder::new(&patterns[0]).into_owned()))),
            _ => Some(Needles::Many(
                AhoCorasick::builder()
                    .match_kind(aho_corasick::MatchKind::LeftmostFirst)
                    .build(&patterns)
                    .expect("valid patterns"),
            )),
        }
    }

    #[inline]
    fn found(&self, line: &[u8]) -> bool {
        match self {
            Needles::One(f) => f.find(line).is_some(),
            Needles::Many(a) => a.is_match(line),
        }
    }
}

impl std::fmt::Debug for Needles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Needles::One(n) => write!(f, "Needles({:?})", String::from_utf8_lossy(n.needle())),
            Needles::Many(a) => write!(f, "Needles({} patterns)", a.patterns_len()),
        }
    }
}

#[derive(Debug)]
enum Node {
    All(Vec<Node>),
    Any(Vec<Node>),
    Not(Box<Node>),
    /// The ids, and their known revisions when given.
    Ids {
        /// The ids, and the items they redirect to.
        set: Arc<IdSet>,
        /// The ids.
        requested: Arc<IdSet>,
        known: Option<Arc<Known>>,
    },
    Types(u8),
    HasProperty {
        properties: Vec<Pid>,
        needles: Option<Needles>,
    },
    Claim {
        property: Pid,
        values: FxHashSet<String>,
        property_needle: Option<Needles>,
        value_needles: Option<Needles>,
    },
    InstanceOf {
        classes: FxHashSet<u32>,
        property_needle: Option<Needles>,
        value_needles: Option<Needles>,
    },
    Sitelink {
        sites: Vec<String>,
        needles: Option<Needles>,
    },
    InCountry {
        countries: Vec<u32>,
        index: Arc<PlaceIndex>,
        /// Per country set of the index: whether it holds one of `countries`.
        sets: Vec<bool>,
        needles: Option<Needles>,
    },
    /// The index of its name in [`CompiledFilter::names`] (shared with
    /// [`CompiledFilter::named`]).
    Named(u16, Arc<Node>),
}

/// Known revisions by [`id_key`], sorted.
#[derive(Debug)]
pub(crate) struct Known {
    keys: Vec<i64>,
    revisions: Vec<u64>,
}

impl Known {
    /// From `(key, revision)` pairs; an id given twice keeps its highest
    /// revision.
    fn new(mut pairs: Vec<(i64, u64)>) -> Known {
        pairs.sort_unstable();
        let mut keys: Vec<i64> = Vec::with_capacity(pairs.len());
        let mut revisions: Vec<u64> = Vec::with_capacity(pairs.len());
        for (k, r) in pairs {
            if keys.last() == Some(&k) {
                *revisions.last_mut().unwrap() = r;
            } else {
                keys.push(k);
                revisions.push(r);
            }
        }
        Known { keys, revisions }
    }

    /// Drop the known revisions of `keys`.
    fn forget(&mut self, keys: &[i64]) {
        if keys.is_empty() {
            return;
        }
        let keys: FxHashSet<i64> = keys.iter().copied().collect();
        let (mut ks, mut rs) = (Vec::with_capacity(self.keys.len()), Vec::with_capacity(self.keys.len()));
        for (&k, &r) in self.keys.iter().zip(&self.revisions) {
            if !keys.contains(&k) {
                ks.push(k);
                rs.push(r);
            }
        }
        self.keys = ks;
        self.revisions = rs;
    }

    #[inline]
    fn get(&self, key: i64) -> Option<u64> {
        self.keys.binary_search(&key).ok().map(|i| self.revisions[i])
    }

    /// Whether the entity `key` at revision `current` is no newer than known.
    #[inline]
    fn unchanged(&self, key: i64, current: Option<u64>) -> bool {
        matches!((self.get(key), current), (Some(k), Some(c)) if c <= k)
    }
}

fn id_set(keys: impl Iterator<Item = i64>) -> Arc<IdSet> {
    let b = IdSetBuilder::new();
    let mut keys: Vec<i64> = keys.collect();
    keys.sort_unstable();
    b.insert_all(&keys);
    Arc::new(b.freeze())
}

/// The needle for a claim on `p`: its key in a map of statements or snaks.
fn property_needle(p: Pid) -> Vec<u8> {
    format!("\"{p}\":[").into_bytes()
}

/// The needle for a value, when its JSON form is certain: entity ids as
/// `"id":"Q55"`, plain ASCII strings quoted. Strings the dump escapes
/// (non-ASCII, `/`, quotes, backslashes) get none.
fn value_needle(v: &str) -> Option<Vec<u8>> {
    let is_id = v.len() > 1
        && matches!(v.as_bytes()[0], b'Q' | b'P' | b'L' | b'E' | b'M')
        && v[1..]
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'-' || b == b'F' || b == b'S');
    if is_id {
        return Some(format!("\"id\":\"{v}\"").into_bytes());
    }
    if v.bytes()
        .all(|b| (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\' && b != b'/')
    {
        return Some(format!("\"{v}\"").into_bytes());
    }
    None
}

/// An `ids` node: with a redirect index, it also matches the items the
/// requested ones redirect to.
fn ids_node(keys: Vec<i64>, mut known: Option<Known>, indexes: &Indexes<'_>) -> Node {
    let requested = id_set(keys.iter().copied());
    let set = match &indexes.redirects {
        Some(r) => {
            let targets: Vec<i64> = keys
                .iter()
                .filter_map(|&k| u32::try_from(k).ok())
                .filter_map(|q| r.resolve(q))
                .map(i64::from)
                .collect();
            // An item another requested id was merged into must come
            // through even when its own revision is known and unchanged:
            // the merge is news for the id merged into it.
            if let Some(k) = &mut known {
                k.forget(&targets);
            }
            id_set(keys.iter().copied().chain(targets))
        }
        None => requested.clone(),
    };
    Node::Ids {
        set,
        requested,
        known: known.map(Arc::new),
    }
}

fn keys(ids: &[String]) -> Result<Vec<i64>, FilterError> {
    ids.iter()
        .map(|id| id_key(id).ok_or_else(|| FilterError::Invalid(format!("not an entity id: {id:?}"))))
        .collect()
}

/// Value needles are only worth it for a limited number of values.
const MAX_VALUE_NEEDLES: usize = 5000;

fn compile(f: &Filter, indexes: &Indexes<'_>, needs: &mut Needs) -> Result<Node, FilterError> {
    let classes = indexes.classes;
    Ok(match f {
        Filter::All(fs) => Node::All(
            fs.iter()
                .map(|f| compile(f, indexes, needs))
                .collect::<Result<_, _>>()?,
        ),
        Filter::Any(fs) => Node::Any(
            fs.iter()
                .map(|f| compile(f, indexes, needs))
                .collect::<Result<_, _>>()?,
        ),
        Filter::Not(f) => Node::Not(Box::new(compile(f, indexes, needs)?)),
        Filter::Ids(ids) => ids_node(keys(ids)?, None, indexes),
        // Item keys are their numbers (see `id_key`).
        Filter::ItemNumbers(ns) => ids_node(ns.iter().map(|&n| n as i64).collect(), None, indexes),
        Filter::Revisions { ids, revisions } => {
            let keys = keys(ids)?;
            let known = Known::new(keys.iter().copied().zip(revisions.iter().copied()).collect());
            ids_node(keys, Some(known), indexes)
        }
        Filter::ItemRevisions { items, revisions } => {
            let keys: Vec<i64> = items.iter().map(|&n| n as i64).collect();
            let known = Known::new(keys.iter().copied().zip(revisions.iter().copied()).collect());
            ids_node(keys, Some(known), indexes)
        }
        Filter::Named { name, filter } => {
            let i = match needs.names.iter().position(|n| n == name) {
                Some(i) => i,
                None => {
                    needs.names.push(name.clone());
                    needs.names.len() - 1
                }
            };
            let i = u16::try_from(i).map_err(|_| FilterError::Invalid("too many named filters".into()))?;
            Node::Named(i, Arc::new(compile(filter, indexes, needs)?))
        }
        Filter::Types(ts) => Node::Types(ts.iter().fold(0, |m, t| m | 1 << t.code())),
        Filter::HasProperty(ps) => {
            needs.properties.extend(ps.iter().map(|p| p.0));
            Node::HasProperty {
                properties: ps.clone(),
                needles: Needles::new(ps.iter().map(|&p| property_needle(p)).collect()),
            }
        }
        Filter::Claim { property, values } => {
            needs.properties.insert(property.0);
            let vn: Option<Vec<Vec<u8>>> = values.iter().map(|v| value_needle(v)).collect();
            Node::Claim {
                property: *property,
                values: values.iter().cloned().collect(),
                property_needle: Needles::new(vec![property_needle(*property)]),
                value_needles: vn.and_then(|v| {
                    if v.len() <= MAX_VALUE_NEEDLES {
                        Needles::new(v)
                    } else {
                        None
                    }
                }),
            }
        }
        Filter::InstanceOf {
            classes: roots,
            subclasses,
        } => {
            needs.properties.insert(31);
            let set: FxHashSet<u32> = if *subclasses {
                let index = classes.ok_or_else(|| {
                    FilterError::Invalid(
                        "instanceOf with subclasses needs a class index (build one with `wikidata classes`)".into(),
                    )
                })?;
                index.subclasses_of_ids(roots)
            } else {
                roots
                    .iter()
                    .map(|r| {
                        r.strip_prefix('Q')
                            .and_then(|n| n.parse().ok())
                            .ok_or_else(|| FilterError::Invalid(format!("not an item id: {r:?}")))
                    })
                    .collect::<Result<_, _>>()?
            };
            let value_needles = if set.len() <= MAX_VALUE_NEEDLES {
                Needles::new(set.iter().map(|q| format!("\"id\":\"Q{q}\"").into_bytes()).collect())
            } else {
                None
            };
            Node::InstanceOf {
                classes: set,
                property_needle: Needles::new(vec![property_needle(Pid(31))]),
                value_needles,
            }
        }
        Filter::InCountry(countries) => {
            let index = indexes.places.clone().ok_or_else(|| {
                FilterError::Invalid("inCountry needs a place index (build one with `wikidata places`)".into())
            })?;
            let countries: Vec<u32> = countries
                .iter()
                .map(|c| {
                    c.strip_prefix('Q')
                        .and_then(|n| n.parse().ok())
                        .ok_or_else(|| FilterError::Invalid(format!("inCountry: not an item id: {c:?}")))
                })
                .collect::<Result<_, _>>()?;
            needs.properties.extend([17, 131, 159]);
            Node::InCountry {
                sets: index.sets_with(&countries),
                countries,
                index,
                needles: Needles::new([17, 131, 159].map(|p| property_needle(Pid(p))).to_vec()),
            }
        }
        Filter::Sitelink(sites) => {
            needs.sites.extend(sites.iter().cloned());
            Node::Sitelink {
                sites: sites.clone(),
                needles: Needles::new(sites.iter().map(|s| format!("\"site\":\"{s}\"").into_bytes()).collect()),
            }
        }
    })
}

/// The requested ids a pass did not find, sorted ([`CompiledFilter::misses`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Misses {
    /// Merged into another item.
    pub merged: Vec<Merged>,
    /// Neither in the dump nor redirected: deleted.
    pub deleted: Vec<String>,
    /// Above the dump's highest id of their type: created after the dump.
    pub newer: Vec<String>,
}

/// A requested id merged into another item.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Merged {
    /// The requested id.
    pub from: String,
    /// The item it redirects to.
    pub to: String,
    /// Whether the dump holds that item (it then came through the pass).
    pub in_dump: bool,
}

/// Three-valued result of the check before parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pre {
    /// Rejected.
    No,
    /// Accepted.
    Yes,
    /// Needs a parse to tell.
    Maybe,
}

fn not(p: Pre) -> Pre {
    match p {
        Pre::No => Pre::Yes,
        Pre::Yes => Pre::No,
        Pre::Maybe => Pre::Maybe,
    }
}

fn found(n: &Option<Needles>, line: &[u8]) -> bool {
    n.as_ref().is_none_or(|n| n.found(line))
}

fn pre(n: &Node, line: &[u8], peek: Option<&Peek>) -> Pre {
    match n {
        Node::All(ns) => {
            let mut r = Pre::Yes;
            for n in ns {
                match pre(n, line, peek) {
                    Pre::No => return Pre::No,
                    Pre::Maybe => r = Pre::Maybe,
                    Pre::Yes => {}
                }
            }
            r
        }
        Node::Any(ns) => {
            let mut r = Pre::No;
            for n in ns {
                match pre(n, line, peek) {
                    Pre::Yes => return Pre::Yes,
                    Pre::Maybe => r = Pre::Maybe,
                    Pre::No => {}
                }
            }
            r
        }
        Node::Not(n) => not(pre(n, line, peek)),
        Node::Named(_, n) => pre(n, line, peek),
        Node::Ids { set, known, .. } => match peek {
            Some(p) => match id_key(p.id) {
                Some(k) if set.contains(k) => match known {
                    Some(known) if known.unchanged(k, wikidata_model::lastrevid(line)) => Pre::No,
                    _ => Pre::Yes,
                },
                _ => Pre::No,
            },
            None => Pre::Maybe,
        },
        Node::Types(mask) => match peek {
            Some(p) if mask & (1 << p.kind.code()) != 0 => Pre::Yes,
            Some(_) => Pre::No,
            None => Pre::Maybe,
        },
        Node::HasProperty { needles, .. } | Node::Sitelink { needles, .. } => {
            if found(needles, line) {
                Pre::Maybe
            } else {
                Pre::No
            }
        }
        Node::InCountry {
            index, sets, needles, ..
        } => match peek {
            // The index answers for the items it knows; the others are
            // decided from their own claims (when they have any).
            Some(p) => match qnum(p.id).and_then(|q| index.set_of(q)) {
                Some(s) if sets[s as usize] => Pre::Yes,
                Some(_) => Pre::No,
                None if p.kind == EntityType::Item && found(needles, line) => Pre::Maybe,
                None => Pre::No,
            },
            None => Pre::Maybe,
        },
        Node::Claim {
            property_needle,
            value_needles,
            ..
        }
        | Node::InstanceOf {
            property_needle,
            value_needles,
            ..
        } => {
            if found(property_needle, line) && found(value_needles, line) {
                Pre::Maybe
            } else {
                Pre::No
            }
        }
    }
}

fn live(e: &Entity, p: Pid) -> impl Iterator<Item = &wikidata_model::Statement> {
    e.statements(p).iter().filter(|s| s.rank != Rank::Deprecated)
}

fn exact(n: &Node, e: &Entity) -> bool {
    match n {
        Node::All(ns) => ns.iter().all(|n| exact(n, e)),
        Node::Any(ns) => ns.iter().any(|n| exact(n, e)),
        Node::Not(n) => !exact(n, e),
        Node::Named(_, n) => exact(n, e),
        Node::Ids { set, known, .. } => id_key(&e.id)
            .is_some_and(|k| set.contains(k) && known.as_ref().is_none_or(|known| !known.unchanged(k, e.lastrevid))),
        Node::Types(mask) => mask & (1 << e.kind.code()) != 0,
        Node::HasProperty { properties, .. } => properties.iter().any(|&p| live(e, p).next().is_some()),
        Node::Claim { property, values, .. } => live(e, *property).any(|s| {
            values.is_empty()
                || s.value.entity_id().is_some_and(|id| values.contains(id))
                || s.value.as_str().is_some_and(|v| values.contains(v))
        }),
        Node::InstanceOf { classes, .. } => live(e, Pid(31)).any(|s| {
            s.value
                .entity_id()
                .and_then(|id| id.strip_prefix('Q'))
                .and_then(|n| n.parse::<u32>().ok())
                .is_some_and(|q| classes.contains(&q))
        }),
        Node::Sitelink { sites, .. } => sites.iter().any(|s| e.sitelinks.get(s.as_str()).is_some()),
        Node::InCountry {
            countries, index, sets, ..
        } => {
            if let Some(s) = qnum(&e.id).and_then(|q| index.set_of(q)) {
                return sets[s as usize];
            }
            if e.kind != EntityType::Item {
                return false;
            }
            let values = |p: u32| {
                wikidata_model::truthy(e.statements(Pid(p)))
                    .filter_map(|s| s.value.entity_id())
                    .filter_map(qnum)
                    .collect::<Vec<u32>>()
            };
            values(17).iter().any(|c| countries.contains(c))
                || [131, 159]
                    .into_iter()
                    .flat_map(values)
                    .any(|place| index.countries(place).iter().any(|c| countries.contains(c)))
        }
    }
}

/// The number of a `Q` id.
fn qnum(id: &str) -> Option<u32> {
    if !id.starts_with('Q') {
        return None;
    }
    wikidata_model::id_number(id).and_then(|n| u32::try_from(n).ok())
}

/// The named nodes of a tree, the id sets and the known revisions.
fn walk(n: &Node, named: &mut Vec<(u16, Arc<Node>)>, ids: &mut Vec<Arc<IdSet>>, known: &mut Vec<Arc<Known>>) {
    match n {
        Node::All(ns) | Node::Any(ns) => ns.iter().for_each(|n| walk(n, named, ids, known)),
        Node::Not(n) => walk(n, named, ids, known),
        Node::Named(i, inner) => {
            named.push((*i, inner.clone()));
            walk(inner, named, ids, known);
        }
        Node::Ids {
            requested, known: k, ..
        } => {
            ids.push(requested.clone());
            known.extend(k.clone());
        }
        _ => {}
    }
}

/// A filter ready for matching (see the module docs).
#[derive(Debug)]
pub struct CompiledFilter {
    fingerprint: u64,
    redirects: Option<Arc<RedirectIndex>>,
    /// Requested items that redirect, by target key.
    merged: FxHashMap<i64, Vec<i64>>,
    root: Node,
    probe: Option<Compiled>,
    names: Vec<String>,
    /// Every named node of the tree, with its name's index.
    named: Vec<(u16, Arc<Node>)>,
    /// Every `ids` set in the tree.
    id_sets: Vec<Arc<IdSet>>,
    /// Every table of known revisions in the tree.
    known: Vec<Arc<Known>>,
}

impl CompiledFilter {
    /// The check before parsing.
    #[inline]
    pub fn pre(&self, line: &[u8], peek: Option<&Peek>) -> Pre {
        pre(&self.root, line, peek)
    }

    /// The exact check on an entity parsed with at least the filter's
    /// properties and sites.
    pub fn matches_entity(&self, e: &Entity) -> bool {
        exact(&self.root, e)
    }

    /// Whether a line matches: the check before parsing, then, when it
    /// cannot tell, a probe parse of what the filter looks at.
    pub fn matches_line(&self, line: &[u8], peek: Option<&Peek>) -> Result<bool, FilterError> {
        Ok(self.check_line(line, peek)?.0)
    }

    /// [`CompiledFilter::matches_line`], and the probe-parsed entity when a
    /// parse was needed (to hand to [`CompiledFilter::matched_names`]).
    pub fn check_line(&self, line: &[u8], peek: Option<&Peek>) -> Result<(bool, Option<Entity>), FilterError> {
        match self.pre(line, peek) {
            Pre::Yes => Ok((true, None)),
            Pre::No => Ok((false, None)),
            Pre::Maybe => {
                let e = self.probe_parse(line)?;
                Ok((self.matches_entity(&e), Some(e)))
            }
        }
    }

    fn probe_parse(&self, line: &[u8]) -> Result<Entity, FilterError> {
        let probe = self.probe.clone().unwrap_or_else(|| Compiled::new(minimal()));
        parse_entity(line, &probe).map_err(|e| FilterError::Invalid(e.to_string()))
    }

    /// The names of the named filters (`{"name": "venue", …}`, or the keys
    /// of `{"any": {"venue": …}}`), in the order they first occur.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Which named filters match `line` (as indices into
    /// [`CompiledFilter::names`], ascending), wherever they are in the
    /// filter. `parsed` is the line's entity if it was parsed with at least
    /// [`CompiledFilter::probe_options`]; without it, the line is parsed
    /// when a named filter needs it.
    pub fn matched_names(
        &self,
        line: &[u8],
        peek: Option<&Peek>,
        parsed: Option<&Entity>,
    ) -> Result<Vec<u16>, FilterError> {
        if self.names.is_empty() {
            return Ok(Vec::new());
        }
        let mut probed = None;
        let mut out = Vec::new();
        for (i, n) in &self.named {
            let (i, n) = (*i, &**n);
            if out.contains(&i) {
                continue;
            }
            let yes = match pre(n, line, peek) {
                Pre::Yes => true,
                Pre::No => false,
                Pre::Maybe => {
                    let e = match parsed {
                        Some(e) => e,
                        None => probed.insert(self.probe_parse(line)?),
                    };
                    exact(n, e)
                }
            };
            if yes {
                out.push(i);
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    /// Whether the filter has `ids` (to report the ids a dump lacks).
    pub fn has_ids(&self) -> bool {
        !self.id_sets.is_empty()
    }

    /// Whether an [`id_key`] is one of the filter's `ids` (anywhere in it).
    #[inline]
    pub fn requests(&self, key: i64) -> bool {
        self.id_sets.iter().any(|s| s.contains(key))
    }

    /// Whether `key` is an item some of the filter's `ids` redirect to
    /// (compiled with a redirect index).
    #[inline]
    pub fn is_target(&self, key: i64) -> bool {
        !self.merged.is_empty() && self.merged.contains_key(&key)
    }

    /// The filter's ids that redirect to the item of `key` (as
    /// [`id_key`]s), when compiled with a redirect index.
    pub fn merged_from(&self, key: i64) -> &[i64] {
        self.merged.get(&key).map_or(&[], Vec::as_slice)
    }

    /// Whether the filter follows merges (compiled with a redirect index
    /// and has `ids`).
    pub fn follows_redirects(&self) -> bool {
        self.redirects.is_some() && self.has_ids()
    }

    /// Sort the requested ids that are not in `seen` (the keys a complete
    /// pass found: requested ids and merge targets) into merged, deleted
    /// and newer ones, with the dump's highest id numbers by type (see
    /// `wikidata_export::FileInfo::max_ids`). `None` without a redirect
    /// index.
    pub fn misses(&self, seen: &IdSet, max_ids: &[u64; 5]) -> Option<Misses> {
        let redirects = self.redirects.as_ref()?;
        let mut out = Misses::default();
        let mut keys: Vec<i64> = self
            .id_sets
            .iter()
            .flat_map(|s| s.iter())
            .filter(|&k| !seen.contains(k))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        for k in keys {
            let Some(id) = crate::ids::id_from_key(k) else { continue };
            if let Some(t) = u32::try_from(k).ok().and_then(|q| redirects.resolve(q)) {
                out.merged.push(Merged {
                    from: id,
                    to: format!("Q{t}"),
                    in_dump: seen.contains(i64::from(t)),
                });
                continue;
            }
            let kind = (k >> 34) as usize;
            let number = (k & ((1 << 34) - 1)) as u64;
            if max_ids.get(kind).is_some_and(|&m| number > m) {
                out.newer.push(id);
            } else {
                out.deleted.push(id);
            }
        }
        Some(out)
    }

    /// The filter's [`Filter::fingerprint`] (with the class index it
    /// expands subclasses with).
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// Whether the filter has known revisions (`ids` with `revisions`).
    pub fn has_revisions(&self) -> bool {
        !self.known.is_empty()
    }

    /// Whether the entity of `key` on `line` is one of the filter's `ids`
    /// with a known revision, and no newer than it (see
    /// [`Filter::Revisions`]).
    #[inline]
    pub fn unchanged(&self, key: i64, line: &[u8]) -> bool {
        if self.known.is_empty() {
            return false;
        }
        let current = wikidata_model::lastrevid(line);
        self.known.iter().any(|k| k.unchanged(key, current))
    }

    /// The filter's `ids` (anywhere in it) not in `seen`, as ids
    /// (`Q42`), in key order.
    pub fn missing_ids(&self, seen: &IdSet) -> Vec<String> {
        let ids = &self.id_sets;
        let mut keys: Vec<i64> = ids
            .iter()
            .flat_map(|s| s.iter())
            .filter(|&k| !seen.contains(k))
            .collect();
        if ids.len() > 1 {
            keys.sort_unstable();
            keys.dedup();
        }
        keys.into_iter().filter_map(crate::ids::id_from_key).collect()
    }

    /// The options a probe parse uses (`None` when the filter decides on
    /// the type and id alone).
    pub fn probe_options(&self) -> Option<&Compiled> {
        self.probe.as_ref()
    }
}

fn minimal() -> ModelOptions {
    ModelOptions {
        languages: Some(Vec::new()),
        sitelinks: Some(Vec::new()),
        properties: Some(Vec::new()),
        labels: false,
        descriptions: false,
        aliases: false,
        qualifiers: false,
        references: false,
        derived: false,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wikidata_model::peek;

    const BELGIUM: &str = r#"{"type":"item","id":"Q31","labels":{"nl":{"language":"nl","value":"België"}},"claims":{"P31":[{"mainsnak":{"snaktype":"value","property":"P31","datavalue":{"value":{"entity-type":"item","numeric-id":6256,"id":"Q6256"},"type":"wikibase-entityid"},"datatype":"wikibase-item"},"type":"statement","rank":"normal"}],"P17":[{"mainsnak":{"snaktype":"value","property":"P17","datavalue":{"value":{"entity-type":"item","numeric-id":31,"id":"Q31"},"type":"wikibase-entityid"},"datatype":"wikibase-item"},"type":"statement","rank":"normal"},{"mainsnak":{"snaktype":"value","property":"P17","datavalue":{"value":{"entity-type":"item","numeric-id":55,"id":"Q55"},"type":"wikibase-entityid"},"datatype":"wikibase-item"},"type":"statement","rank":"deprecated"}],"P297":[{"mainsnak":{"snaktype":"value","property":"P297","datavalue":{"value":"BE","type":"string"},"datatype":"external-id"},"type":"statement","rank":"normal"}]},"sitelinks":{"nlwiki":{"site":"nlwiki","title":"België","badges":[]}}}"#;

    fn m(f: &str, classes: Option<&ClassIndex>) -> bool {
        let f = Filter::parse_json(f).unwrap().compile(classes).unwrap();
        let line = BELGIUM.as_bytes();
        f.matches_line(line, peek(line).as_ref()).unwrap()
    }

    #[test]
    fn filters() {
        assert!(m(r#"{"ids":["Q31","Q55"]}"#, None));
        assert!(!m(r#"{"ids":"Q32"}"#, None));
        assert!(m(r#"{"type":"item"}"#, None));
        assert!(!m(r#"{"type":["property","lexeme"]}"#, None));
        assert!(m(r#"{"hasProperty":"P17"}"#, None));
        assert!(!m(r#"{"hasProperty":"P18"}"#, None));
        assert!(m(r#"{"claim":"P17=Q31"}"#, None));
        // Deprecated statements do not count.
        assert!(!m(r#"{"claim":{"property":"P17","values":["Q55"]}}"#, None));
        assert!(m(r#"{"claim":{"P297":["BE","NL"]}}"#, None));
        assert!(m(r#"{"instanceOf":["Q6256"]}"#, None));
        assert!(!m(r#"{"instanceOf":["Q5"]}"#, None));
        assert!(m(r#"{"sitelink":"nlwiki"}"#, None));
        assert!(!m(r#"{"sitelink":"enwiki"}"#, None));
        assert!(m(r#"{"any":[{"ids":"Q1"},{"sitelink":"nlwiki"}]}"#, None));
        assert!(!m(r#"{"all":[{"ids":"Q31"},{"not":{"sitelink":"nlwiki"}}]}"#, None));
        assert!(m(r#"[{"type":"item"},{"claim":"P17"}]"#, None));
    }

    #[test]
    fn subclasses() {
        // Q6256 country ⊂ Q7275 state ⊂ Q43702 federation (made up).
        let c = ClassIndex::from_edges(vec![(6256, 7275), (7275, 43702)], "t");
        assert!(m(
            r#"{"instanceOf":{"classes":["Q43702"],"subclasses":true}}"#,
            Some(&c)
        ));
        assert!(!m(
            r#"{"instanceOf":{"classes":["Q43702"],"subclasses":false}}"#,
            Some(&c)
        ));
        let f = Filter::parse_json(r#"{"instanceOf":{"classes":["Q43702"],"subclasses":true}}"#).unwrap();
        assert!(f.needs_class_index());
        assert!(f.compile(None).is_err());
    }

    #[test]
    fn rejects_before_parsing() {
        let f = Filter::parse_json(r#"{"claim":"P17=Q99999"}"#)
            .unwrap()
            .compile(None)
            .unwrap();
        let line = BELGIUM.as_bytes();
        assert_eq!(f.pre(line, peek(line).as_ref()), Pre::No);
        let f = Filter::parse_json(r#"{"claim":"P17=Q55"}"#)
            .unwrap()
            .compile(None)
            .unwrap();
        assert_eq!(f.pre(line, peek(line).as_ref()), Pre::Maybe);
    }

    #[test]
    fn named() {
        let f = Filter::parse_json(
            r#"{"not":{"name":"german","claim":"P17=Q183"},
                "any":{"dutch":{"sitelink":"nlwiki"},"country":{"instanceOf":"Q6256"},"person":{"instanceOf":"Q5"}}}"#,
        )
        .unwrap()
        .compile(None)
        .unwrap();
        // In the order written.
        assert_eq!(f.names(), ["german", "dutch", "country", "person"]);
        let line = BELGIUM.as_bytes();
        let p = peek(line);
        assert!(f.matches_line(line, p.as_ref()).unwrap());
        // Every named filter that matches, not only the first.
        assert_eq!(f.matched_names(line, p.as_ref(), None).unwrap(), [1, 2]);
        let unnamed = Filter::parse_json(r#"{"any":[{"ids":"Q31"}]}"#)
            .unwrap()
            .compile(None)
            .unwrap();
        assert!(unnamed.names().is_empty());
        assert!(unnamed.matched_names(line, p.as_ref(), None).unwrap().is_empty());
        assert!(Filter::parse_json(r#"{"name":3,"ids":"Q1"}"#).is_err());
    }

    #[test]
    fn item_numbers_and_missing_ids() {
        let numbers: &[u32] = &[31, 999_999_999];
        let v: serde_json::Value =
            serde_json::from_str(r#"{"any":[{"ids":{"$items":0}},{"ids":["P31","Q31"]}]}"#).unwrap();
        let f = Filter::from_json_with(&v, &[numbers]).unwrap().compile(None).unwrap();
        let line = BELGIUM.as_bytes();
        assert!(f.matches_line(line, peek(line).as_ref()).unwrap());
        assert!(f.has_ids());
        assert!(f.requests(id_key("Q999999999").unwrap()));
        assert!(!f.requests(id_key("Q32").unwrap()));
        let seen = IdSetBuilder::new();
        seen.insert(id_key("Q31").unwrap());
        assert_eq!(f.missing_ids(&seen.freeze()), ["Q999999999", "P31"]);
        assert!(Filter::from_json_with(&v, &[]).is_err());
        assert!(!Filter::parse_json(r#"{"type":"item"}"#)
            .unwrap()
            .compile(None)
            .unwrap()
            .has_ids());
    }

    #[test]
    fn errors() {
        assert!(Filter::parse_json(r#"{"bogus":1}"#).is_err());
        assert!(Filter::parse_json(r#"{"hasProperty":"Q1"}"#).is_err());
        assert!(Filter::parse_json(r#"{}"#).is_err());
        assert!(Filter::parse_json(r#"{"ids":["x"]}"#).unwrap().compile(None).is_err());
    }
}

#[cfg(test)]
mod revision_tests {
    use super::*;
    use wikidata_model::peek;

    const LINE: &str = r#"{"type":"item","id":"Q31","labels":[],"claims":[],"lastrevid":2000000001}"#;

    fn m(f: &Filter) -> bool {
        let f = f.compile(None).unwrap();
        let line = LINE.as_bytes();
        f.matches_line(line, peek(line).as_ref()).unwrap()
    }

    #[test]
    fn skips_unchanged() {
        let at = |r: u64| Filter::Revisions {
            ids: vec!["Q31".into()],
            revisions: vec![r],
        };
        assert!(!m(&at(2000000001)), "the same revision");
        assert!(!m(&at(2000000002)), "a newer known revision");
        assert!(m(&at(2000000000)), "changed since");
        let items = Filter::ItemRevisions {
            items: vec![31],
            revisions: vec![2000000000],
        };
        assert!(m(&items));
        let f = at(2000000001).compile(None).unwrap();
        assert!(f.has_revisions() && f.requests(31));
        assert!(f.unchanged(31, LINE.as_bytes()));
        assert!(!f.unchanged(55, LINE.as_bytes()));
    }

    #[test]
    fn json_forms() {
        let revs: &[u64] = &[2000000001, 5];
        let items: &[u32] = &[31, 55];
        let f = Filter::parse_json_with_revisions(
            r#"{"ids":{"$items":0},"revisions":{"$revisions":0}}"#,
            &[items],
            &[revs],
        )
        .unwrap();
        assert_eq!(
            f,
            Filter::ItemRevisions {
                items: vec![31, 55],
                revisions: vec![2000000001, 5]
            }
        );
        assert!(!m(&f));
        let f = Filter::parse_json(r#"{"ids":["Q31"],"revisions":[2000000000]}"#).unwrap();
        assert!(m(&f));
        let f = Filter::parse_json(r#"{"ids":{"Q31":2000000001,"Q55":3}}"#).unwrap();
        assert!(!m(&f));
        assert!(Filter::parse_json(r#"{"ids":["Q31"],"revisions":[1,2]}"#).is_err());
        assert!(Filter::parse_json(r#"{"revisions":[1]}"#).is_err());
        assert!(Filter::parse_json(r#"{"ids":{"Q31":"x"}}"#).is_err());
    }
}
