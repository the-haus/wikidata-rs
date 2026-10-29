//! Parsing dump lines into [`Entity`] values.
//!
//! The parser is a set of serde seeds driven by the [`Compiled`] options,
//! so whatever is not wanted (other languages, other properties, sites,
//! references) is skipped as it is read: keys are compared as borrowed
//! strings and skipped values are never allocated. The same seeds run on
//! serde_json or on sonic-rs (SIMD skipping; the default, see
//! docs/ARCHITECTURE.md for the measurements).

use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;

use crate::entity::{
    Coordinate, Derived, Entity, EntityType, Form, LexemeData, Map, Pid, Quantity, Rank, Sense, Sitelink, Snaks,
    Statement, Text, Value,
};
use crate::external::EXTERNAL_IDS;
use crate::options::{Compiled, Slot};
use crate::sites;
use crate::time::{entity_of_uri, make_time};

/// A line that is not a valid entity.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ParseError(pub String);

/// The JSON parser to use.
///
/// Measured on the first 11k entities of the 2026-10-12 dump (520 MB, one
/// thread, M1 Max): sonic-rs parses whole entities ~20% faster (537 vs
/// 449 MB/s with the default options), serde_json skips ~15% faster when
/// only a few languages and properties are kept (757 vs 651 MB/s). So
/// [`Backend::Default`] picks sonic-rs for unrestricted options and
/// serde_json (with exact float parsing, `float_roundtrip`) for selective
/// ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Backend {
    /// Chosen from the options (see above).
    #[default]
    Default,
    /// serde_json.
    SerdeJson,
    /// sonic-rs.
    #[cfg(feature = "sonic")]
    Sonic,
}

/// Parse one dump line (an entity's JSON, without the trailing comma).
pub fn parse_entity(line: &[u8], c: &Compiled) -> Result<Entity, ParseError> {
    parse_entity_with(line, c, Backend::Default)
}

/// Parse with a chosen JSON parser (for comparisons).
pub fn parse_entity_with(line: &[u8], c: &Compiled, backend: Backend) -> Result<Entity, ParseError> {
    let seed = EntitySeed { c };
    let backend = match backend {
        Backend::Default => c.backend,
        b => b,
    };
    match backend {
        #[cfg(feature = "sonic")]
        Backend::Sonic => {
            let mut de = sonic_rs::Deserializer::from_slice(line);
            seed.deserialize(&mut de).map_err(|e| ParseError(e.to_string()))
        }
        _ => parse_serde_json(line, seed),
    }
}

fn parse_serde_json<'a, T: DeserializeSeed<'a>>(line: &'a [u8], seed: T) -> Result<T::Value, ParseError> {
    let mut de = serde_json::Deserializer::from_slice(line);
    let v = seed.deserialize(&mut de).map_err(|e| ParseError(e.to_string()))?;
    de.end().map_err(|e| ParseError(e.to_string()))?;
    Ok(v)
}

/// Run `seed` over a line with the parser that skips fastest.
pub(crate) fn run_seed<'a, T: DeserializeSeed<'a>>(line: &'a [u8], seed: T) -> Result<T::Value, ParseError> {
    parse_serde_json(line, seed)
}

// ---- string keys without allocation --------------------------------------

/// Deserialises a string and hands it, borrowed, to a function.
pub(crate) struct With<F>(pub F);

impl<'de, T, F: FnOnce(&str) -> T> DeserializeSeed<'de> for With<F> {
    type Value = T;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<T, D::Error> {
        d.deserialize_str(StrVisitor(self.0))
    }
}

struct StrVisitor<F>(F);

impl<'de, T, F: FnOnce(&str) -> T> Visitor<'de> for StrVisitor<F> {
    type Value = T;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a string")
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<T, E> {
        Ok((self.0)(v))
    }
}

fn ignore<'de, A: MapAccess<'de>>(map: &mut A) -> Result<(), A::Error> {
    map.next_value::<IgnoredAny>().map(|_| ())
}

// ---- entity ---------------------------------------------------------------

enum EntityKey {
    Type,
    Id,
    Datatype,
    Labels,
    Descriptions,
    Aliases,
    Claims,
    Sitelinks,
    LastRevId,
    Modified,
    Lemmas,
    LexicalCategory,
    Language,
    Forms,
    Senses,
    Other,
}

fn entity_key(k: &str) -> EntityKey {
    match k {
        "type" => EntityKey::Type,
        "id" => EntityKey::Id,
        "datatype" => EntityKey::Datatype,
        "labels" => EntityKey::Labels,
        "descriptions" => EntityKey::Descriptions,
        "aliases" => EntityKey::Aliases,
        "claims" | "statements" => EntityKey::Claims,
        "sitelinks" => EntityKey::Sitelinks,
        "lastrevid" => EntityKey::LastRevId,
        "modified" => EntityKey::Modified,
        "lemmas" => EntityKey::Lemmas,
        "lexicalCategory" => EntityKey::LexicalCategory,
        "language" => EntityKey::Language,
        "forms" => EntityKey::Forms,
        "senses" => EntityKey::Senses,
        _ => EntityKey::Other,
    }
}

pub(crate) struct EntitySeed<'a> {
    pub(crate) c: &'a Compiled,
}

impl<'de> DeserializeSeed<'de> for EntitySeed<'_> {
    type Value = Entity;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Entity, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for EntitySeed<'_> {
    type Value = Entity;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a Wikidata entity")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Entity, A::Error> {
        let c = self.c;
        let o = &c.opts;
        let mut e = Entity::default();
        let mut acc = DerivedAcc::default();
        let mut lexeme: Option<LexemeData> = None;
        let mut has_type = false;
        let mut widened_early = false;
        let mut counts: Option<(u32, u32)> = None;
        while let Some(k) = map.next_key_seed(With(entity_key))? {
            match k {
                EntityKey::Type => {
                    let t = map.next_value_seed(With(EntityType::parse))?;
                    e.kind = t.ok_or_else(|| de::Error::custom("unknown entity type"))?;
                    has_type = true;
                }
                EntityKey::Id => e.id = map.next_value()?,
                EntityKey::Datatype => e.datatype = Some(map.next_value()?),
                EntityKey::Labels if o.labels => e.labels = map.next_value_seed(TermsSeed { c, filter: true })?,
                EntityKey::Descriptions if o.descriptions => {
                    e.descriptions = map.next_value_seed(TermsSeed { c, filter: true })?
                }
                EntityKey::Aliases if o.aliases => e.aliases = map.next_value_seed(AliasesSeed { c })?,
                EntityKey::Claims => {
                    e.claims = map.next_value_seed(ClaimsSeed { c, acc: Some(&mut acc) })?;
                    acc.claims_seen = true;
                }
                EntityKey::Sitelinks => {
                    // The classes are known when the claims come first (as
                    // in dumps and the API); else the extra sites are kept
                    // until they are.
                    let widen = o.sitelinks_for.is_some() && (acc.widen_sites || !acc.claims_seen);
                    widened_early = widen && !acc.claims_seen;
                    let s = map.next_value_seed(SitelinksSeed { c, widen })?;
                    e.sitelinks = s.links;
                    counts = Some((s.all, s.wikipedias));
                }
                EntityKey::LastRevId => e.lastrevid = map.next_value()?,
                EntityKey::Modified => e.modified = map.next_value()?,
                EntityKey::Lemmas => {
                    lexeme.get_or_insert_with(Default::default).lemmas =
                        map.next_value_seed(TermsSeed { c, filter: false })?
                }
                EntityKey::LexicalCategory => {
                    lexeme.get_or_insert_with(Default::default).lexical_category = map.next_value()?
                }
                EntityKey::Language => lexeme.get_or_insert_with(Default::default).language = map.next_value()?,
                EntityKey::Forms => {
                    lexeme.get_or_insert_with(Default::default).forms = map.next_value_seed(FormsSeed { c })?
                }
                EntityKey::Senses => {
                    lexeme.get_or_insert_with(Default::default).senses = map.next_value_seed(SensesSeed { c })?
                }
                _ => ignore(&mut map)?,
            }
        }
        if e.id.is_empty() {
            return Err(de::Error::custom("an entity without an id"));
        }
        if !has_type {
            e.kind =
                e.id.bytes()
                    .next()
                    .and_then(EntityType::from_letter)
                    .ok_or_else(|| de::Error::custom("an entity without a type"))?;
        }
        if e.kind == EntityType::Lexeme {
            e.lexeme = Some(lexeme.unwrap_or_default());
        }
        if widened_early && !acc.widen_sites {
            e.sitelinks.0.retain(|(s, _)| c.keeps_site(s));
        }
        if o.sitelink_counts {
            let (all, wikipedias) = counts.unwrap_or_default();
            e.sitelink_count = Some(all);
            e.wikipedia_count = Some(wikipedias);
        }
        e.label = pick(&e.labels, &c.fallback);
        e.description = pick(&e.descriptions, &c.fallback);
        e.derived = acc.finish();
        Ok(e)
    }
}

/// The first language of `chain` present in `terms`.
fn pick(terms: &Map<String, String>, chain: &[String]) -> Option<Text> {
    chain.iter().find_map(|l| {
        terms.get(l.as_str()).map(|t| Text {
            text: t.clone(),
            language: l.clone(),
        })
    })
}

// ---- terms ---------------------------------------------------------------

#[derive(Deserialize)]
struct TermValue {
    value: String,
}

struct TermsSeed<'a> {
    c: &'a Compiled,
    filter: bool,
}

impl<'de> DeserializeSeed<'de> for TermsSeed<'_> {
    type Value = Map<String, String>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        // Empty maps are written as `[]`.
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for TermsSeed<'_> {
    type Value = Map<String, String>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("terms by language")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        // Empty term lists are written as `[]` by the PHP serialiser.
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Map::default())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out: Vec<(usize, (String, String))> = Vec::new();
        let filter = self.filter;
        while let Some(k) = map.next_key_seed(With(|k: &str| {
            let idx = if filter { self.c.language(k) } else { Some(0) };
            idx.map(|i| (i, k.to_string()))
        }))? {
            match k {
                Some((i, lang)) => {
                    let t: TermValue = map.next_value()?;
                    out.push((i, (lang, t.value)));
                }
                None => ignore(&mut map)?,
            }
        }
        out.sort_by_key(|(i, _)| *i);
        Ok(Map(out.into_iter().map(|(_, kv)| kv).collect()))
    }
}

struct AliasesSeed<'a> {
    c: &'a Compiled,
}

impl<'de> DeserializeSeed<'de> for AliasesSeed<'_> {
    type Value = Map<String, Vec<String>>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        // Empty maps are written as `[]`.
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for AliasesSeed<'_> {
    type Value = Map<String, Vec<String>>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("aliases by language")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Map::default())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out: Vec<(usize, (String, Vec<String>))> = Vec::new();
        while let Some(k) = map.next_key_seed(With(|k: &str| self.c.language(k).map(|i| (i, k.to_string()))))? {
            match k {
                Some((i, lang)) => {
                    let ts: Vec<TermValue> = map.next_value()?;
                    out.push((i, (lang, ts.into_iter().map(|t| t.value).collect())));
                }
                None => ignore(&mut map)?,
            }
        }
        out.sort_by_key(|(i, _)| *i);
        Ok(Map(out.into_iter().map(|(_, kv)| kv).collect()))
    }
}

// ---- sitelinks -------------------------------------------------------------

#[derive(Deserialize)]
struct RawSitelink {
    title: String,
    #[serde(default)]
    badges: Vec<String>,
}

struct SitelinksSeed<'a> {
    c: &'a Compiled,
    /// Also keep the sites of `sitelinks_for`.
    widen: bool,
}

/// The kept sitelinks and the counts of all of them.
struct Sitelinks {
    links: Map<String, Sitelink>,
    all: u32,
    wikipedias: u32,
}

impl<'de> DeserializeSeed<'de> for SitelinksSeed<'_> {
    type Value = Sitelinks;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        // Empty maps are written as `[]`.
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for SitelinksSeed<'_> {
    type Value = Sitelinks;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("sitelinks")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Sitelinks {
            links: Map::default(),
            all: 0,
            wikipedias: 0,
        })
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        let (mut all, mut wikipedias) = (0u32, 0u32);
        let count = self.c.opts.sitelink_counts;
        let extra = self.c.opts.sitelinks_for.as_ref().filter(|_| self.widen);
        while let Some(site) = map.next_key_seed(With(|k: &str| {
            if count {
                all += 1;
                wikipedias += u32::from(sites::is_wikipedia(k));
            }
            (self.c.keeps_site(k) || extra.is_some_and(|x| x.keeps_site(k))).then(|| k.to_string())
        }))? {
            match site {
                Some(site) => {
                    let s: RawSitelink = map.next_value()?;
                    let url = sites::sitelink_url(&site, &s.title);
                    out.push((
                        site,
                        Sitelink {
                            title: s.title,
                            url,
                            badges: s.badges,
                        },
                    ));
                }
                None => ignore(&mut map)?,
            }
        }
        Ok(Sitelinks {
            links: Map(out),
            all,
            wikipedias,
        })
    }
}

// ---- claims ----------------------------------------------------------------

/// Collects the derived fields from truthy statements.
#[derive(Default)]
pub(crate) struct DerivedAcc {
    d: Derived,
    external: Vec<(u16, String)>,
    /// The claims were read.
    claims_seen: bool,
    /// A truthy P31 is one of `sitelinks_for`'s classes.
    widen_sites: bool,
}

/// The best-ranked statements: preferred ones if any, else normal ones.
pub fn truthy(sts: &[Statement]) -> impl Iterator<Item = &Statement> {
    let preferred = sts.iter().any(|s| s.rank == Rank::Preferred);
    sts.iter().filter(move |s| {
        if preferred {
            s.rank == Rank::Preferred
        } else {
            s.rank == Rank::Normal
        }
    })
}

impl DerivedAcc {
    fn feed(&mut self, slot: Slot, sts: &[Statement]) {
        let d = &mut self.d;
        let ids = |v: &mut Vec<String>| {
            v.extend(truthy(sts).filter_map(|s| s.value.entity_id().map(str::to_string)));
        };
        match slot {
            Slot::InstanceOf => ids(&mut d.instance_of),
            Slot::SubclassOf => ids(&mut d.subclass_of),
            Slot::Country => ids(&mut d.country),
            Slot::Coordinates => {
                d.coordinates = truthy(sts).find_map(|s| match &s.value {
                    Value::Coordinate(c) => Some((c.lat, c.lon)),
                    _ => None,
                })
            }
            Slot::Inception | Slot::Dissolved => {
                let y = truthy(sts).find_map(|s| match &s.value {
                    Value::Time(t) => Some(t.year),
                    _ => None,
                });
                if slot == Slot::Inception {
                    d.inception = y;
                } else {
                    d.dissolved = y;
                }
            }
            Slot::Image => {
                d.image = truthy(sts).find_map(|s| match &s.value {
                    Value::Media { file, .. } => Some(file.clone()),
                    _ => None,
                })
            }
            Slot::Website => {
                d.website = truthy(sts).find_map(|s| match &s.value {
                    Value::Url { value } => Some(value.clone()),
                    _ => None,
                })
            }
            Slot::External(i) => {
                if let Some(v) = truthy(sts).find_map(|s| s.value.as_str()) {
                    self.external.push((i, v.to_string()));
                }
            }
        }
    }

    fn finish(mut self) -> Derived {
        self.external.sort_by_key(|(i, _)| *i);
        self.d.external_ids = self
            .external
            .into_iter()
            .map(|(i, v)| (EXTERNAL_IDS[i as usize].key, v))
            .collect();
        self.d
    }
}

pub(crate) struct ClaimsSeed<'a, 'b> {
    pub(crate) c: &'a Compiled,
    pub(crate) acc: Option<&'b mut DerivedAcc>,
}

impl<'de> DeserializeSeed<'de> for ClaimsSeed<'_, '_> {
    type Value = Map<Pid, Vec<Statement>>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        // Empty maps are written as `[]`.
        d.deserialize_any(self)
    }
}

/// Drop deprecated statements (unless kept) and, with `best_rank`, all
/// but the best-ranked ones.
fn filter_ranks(sts: &mut Vec<Statement>, c: &Compiled) {
    if c.opts.best_rank {
        let preferred = sts.iter().any(|s| s.rank == Rank::Preferred);
        let want = if preferred { Rank::Preferred } else { Rank::Normal };
        sts.retain(|s| s.rank == want);
    } else if !c.opts.deprecated {
        sts.retain(|s| s.rank != Rank::Deprecated);
    }
}

impl<'de> Visitor<'de> for ClaimsSeed<'_, '_> {
    type Value = Map<Pid, Vec<Statement>>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("claims")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Map::default())
    }
    fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> Result<Self::Value, A::Error> {
        let c = self.c;
        let mut out = Vec::new();
        while let Some(p) = map.next_key_seed(With(|k: &str| Pid::parse(k)))? {
            let Some(p) = p else {
                ignore(&mut map)?;
                continue;
            };
            let keep = c.keeps_property(p.0);
            let slot = match self.acc {
                Some(_) => c.slots.get(&p.0).copied(),
                None => None,
            };
            let classes = match (&self.acc, &c.opts.sitelinks_for) {
                (Some(_), Some(f)) if p.0 == 31 => Some(f),
                _ => None,
            };
            if !keep && slot.is_none() && classes.is_none() {
                ignore(&mut map)?;
                continue;
            }
            let mut sts: Vec<Statement> = map.next_value_seed(StatementsSeed { c, full: keep })?;
            if let Some(acc) = self.acc.as_deref_mut() {
                if let Some(slot) = slot {
                    acc.feed(slot, &sts);
                }
                if let Some(f) = classes {
                    acc.widen_sites |= f.matches(truthy(&sts).filter_map(|s| s.value.entity_id()));
                }
            }
            if keep {
                filter_ranks(&mut sts, c);
                if !sts.is_empty() {
                    out.push((p, sts));
                }
            }
        }
        Ok(Map(out))
    }
}

struct StatementsSeed<'a> {
    c: &'a Compiled,
    full: bool,
}

impl<'de> DeserializeSeed<'de> for StatementsSeed<'_> {
    type Value = Vec<Statement>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for StatementsSeed<'_> {
    type Value = Vec<Statement>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("statements")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(s) = seq.next_element_seed(StatementSeed {
            c: self.c,
            full: self.full,
        })? {
            if let Some(s) = s {
                out.push(s);
            }
        }
        Ok(out)
    }
}

enum StatementKey {
    Mainsnak,
    Rank,
    Qualifiers,
    References,
    Other,
}

struct StatementSeed<'a> {
    c: &'a Compiled,
    full: bool,
}

impl<'de> DeserializeSeed<'de> for StatementSeed<'_> {
    type Value = Option<Statement>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}

fn parse_rank(s: &str) -> Rank {
    match s {
        "preferred" => Rank::Preferred,
        "deprecated" => Rank::Deprecated,
        _ => Rank::Normal,
    }
}

impl<'de> Visitor<'de> for StatementSeed<'_> {
    type Value = Option<Statement>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a statement")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let o = &self.c.opts;
        let mut value = None;
        let mut rank = Rank::Normal;
        let mut qualifiers = Map::default();
        let mut references = Vec::new();
        while let Some(k) = map.next_key_seed(With(|k: &str| match k {
            "mainsnak" => StatementKey::Mainsnak,
            "rank" => StatementKey::Rank,
            "qualifiers" => StatementKey::Qualifiers,
            "references" => StatementKey::References,
            _ => StatementKey::Other,
        }))? {
            match k {
                StatementKey::Mainsnak => value = map.next_value_seed(SnakSeed)?,
                StatementKey::Rank => rank = map.next_value_seed(With(parse_rank))?,
                StatementKey::Qualifiers if self.full && o.qualifiers => {
                    qualifiers = map.next_value_seed(SnaksSeed {
                        keep: self.c.qualifier_properties.as_ref(),
                    })?
                }
                StatementKey::References if self.full && o.references => {
                    references = map.next_value_seed(ReferencesSeed)?
                }
                _ => ignore(&mut map)?,
            }
        }
        Ok(value.map(|value| Statement {
            value,
            rank,
            qualifiers,
            references,
        }))
    }
}

/// Property → snaks (qualifiers, reference snaks), only `keep`'s
/// properties when given.
struct SnaksSeed<'a> {
    keep: Option<&'a rustc_hash::FxHashSet<u32>>,
}

impl<'de> DeserializeSeed<'de> for SnaksSeed<'_> {
    type Value = Snaks;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        // Empty maps are written as `[]`.
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for SnaksSeed<'_> {
    type Value = Snaks;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("snaks by property")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Map::default())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        let keep = self.keep;
        while let Some(p) = map.next_key_seed(With(|k: &str| {
            Pid::parse(k).filter(|p| keep.is_none_or(|s| s.contains(&p.0)))
        }))? {
            match p {
                Some(p) => {
                    let vs: Vec<Value> = map.next_value_seed(SnakListSeed)?;
                    if !vs.is_empty() {
                        out.push((p, vs));
                    }
                }
                None => ignore(&mut map)?,
            }
        }
        Ok(Map(out))
    }
}

struct SnakListSeed;

impl<'de> DeserializeSeed<'de> for SnakListSeed {
    type Value = Vec<Value>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for SnakListSeed {
    type Value = Vec<Value>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("snaks")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(v) = seq.next_element_seed(SnakSeed)? {
            if let Some(v) = v {
                out.push(v);
            }
        }
        Ok(out)
    }
}

struct ReferencesSeed;

impl<'de> DeserializeSeed<'de> for ReferencesSeed {
    type Value = Vec<Snaks>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for ReferencesSeed {
    type Value = Vec<Snaks>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("references")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(r) = seq.next_element_seed(ReferenceSeed)? {
            if !r.is_empty() {
                out.push(r);
            }
        }
        Ok(out)
    }
}

struct ReferenceSeed;

impl<'de> DeserializeSeed<'de> for ReferenceSeed {
    type Value = Snaks;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for ReferenceSeed {
    type Value = Snaks;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a reference")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut snaks = Map::default();
        while let Some(is_snaks) = map.next_key_seed(With(|k: &str| k == "snaks"))? {
            if is_snaks {
                snaks = map.next_value_seed(SnaksSeed { keep: None })?;
            } else {
                ignore(&mut map)?;
            }
        }
        Ok(snaks)
    }
}

// ---- snaks and values ------------------------------------------------------

/// Property datatypes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DataType {
    Entity,
    String,
    ExternalId,
    Url,
    CommonsMedia,
    Math,
    MusicalNotation,
    GeoShape,
    TabularData,
    Other,
}

fn data_type(s: &str) -> DataType {
    match s {
        "wikibase-item" | "wikibase-property" | "wikibase-lexeme" | "wikibase-form" | "wikibase-sense"
        | "entity-schema" => DataType::Entity,
        "string" => DataType::String,
        "external-id" => DataType::ExternalId,
        "url" => DataType::Url,
        "commonsMedia" => DataType::CommonsMedia,
        "math" => DataType::Math,
        "musical-notation" => DataType::MusicalNotation,
        "geo-shape" => DataType::GeoShape,
        "tabular-data" => DataType::TabularData,
        _ => DataType::Other,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SnakType {
    Value,
    Some,
    No,
}

/// A snak's value (`None` for a value this version does not understand).
struct SnakSeed;

impl<'de> DeserializeSeed<'de> for SnakSeed {
    type Value = Option<Value>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}

enum SnakKey {
    SnakType,
    DataValue,
    DataType,
    Other,
}

impl<'de> Visitor<'de> for SnakSeed {
    type Value = Option<Value>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a snak")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut snaktype = SnakType::Value;
        let mut raw = None;
        let mut dtype = DataType::Other;
        while let Some(k) = map.next_key_seed(With(|k: &str| match k {
            "snaktype" => SnakKey::SnakType,
            "datavalue" => SnakKey::DataValue,
            "datatype" => SnakKey::DataType,
            _ => SnakKey::Other,
        }))? {
            match k {
                SnakKey::SnakType => {
                    snaktype = map.next_value_seed(With(|s: &str| match s {
                        "somevalue" => SnakType::Some,
                        "novalue" => SnakType::No,
                        _ => SnakType::Value,
                    }))?
                }
                SnakKey::DataValue => raw = Some(map.next_value_seed(DataValueSeed)?),
                SnakKey::DataType => dtype = map.next_value_seed(With(data_type))?,
                SnakKey::Other => ignore(&mut map)?,
            }
        }
        Ok(match snaktype {
            SnakType::Some => Some(Value::SomeValue),
            SnakType::No => Some(Value::NoValue),
            SnakType::Value => raw.and_then(|r| make_value(r, dtype)),
        })
    }
}

/// `datavalue.value` before its type is known (it comes after it).
enum Raw {
    Str(String),
    Obj(Box<RawObj>),
    Other,
}

#[derive(Default)]
struct RawObj {
    entity_type: Option<String>,
    numeric_id: Option<u64>,
    id: Option<String>,
    time: Option<String>,
    precision: Option<f64>,
    calendar: Option<String>,
    amount: Option<String>,
    unit: Option<String>,
    upper: Option<String>,
    lower: Option<String>,
    lat: Option<f64>,
    lon: Option<f64>,
    globe: Option<String>,
    text: Option<String>,
    language: Option<String>,
}

struct DataValueSeed;

impl<'de> DeserializeSeed<'de> for DataValueSeed {
    type Value = Raw;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Raw, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for DataValueSeed {
    type Value = Raw;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a datavalue")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Raw, A::Error> {
        let mut raw = Raw::Other;
        while let Some(is_value) = map.next_key_seed(With(|k: &str| k == "value"))? {
            if is_value {
                raw = map.next_value_seed(RawSeed)?;
            } else {
                ignore(&mut map)?;
            }
        }
        Ok(raw)
    }
}

struct RawSeed;

impl<'de> DeserializeSeed<'de> for RawSeed {
    type Value = Raw;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Raw, D::Error> {
        d.deserialize_any(self)
    }
}

enum RawKey {
    EntityType,
    NumericId,
    Id,
    Time,
    Precision,
    Calendar,
    Amount,
    Unit,
    Upper,
    Lower,
    Lat,
    Lon,
    Globe,
    Text,
    Language,
    Other,
}

fn raw_key(k: &str) -> RawKey {
    match k {
        "entity-type" => RawKey::EntityType,
        "numeric-id" => RawKey::NumericId,
        "id" => RawKey::Id,
        "time" => RawKey::Time,
        "precision" => RawKey::Precision,
        "calendarmodel" => RawKey::Calendar,
        "amount" => RawKey::Amount,
        "unit" => RawKey::Unit,
        "upperBound" => RawKey::Upper,
        "lowerBound" => RawKey::Lower,
        "latitude" => RawKey::Lat,
        "longitude" => RawKey::Lon,
        "globe" => RawKey::Globe,
        "text" => RawKey::Text,
        "language" => RawKey::Language,
        _ => RawKey::Other,
    }
}

impl<'de> Visitor<'de> for RawSeed {
    type Value = Raw;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a value")
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Raw, E> {
        Ok(Raw::Str(v.to_string()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Raw, E> {
        Ok(Raw::Str(v))
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Raw, E> {
        Ok(Raw::Other)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Raw, E> {
        Ok(Raw::Other)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Raw, E> {
        Ok(Raw::Other)
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Raw, E> {
        Ok(Raw::Other)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Raw, E> {
        Ok(Raw::Other)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Raw, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Raw::Other)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Raw, A::Error> {
        let mut o = RawObj::default();
        while let Some(k) = map.next_key_seed(With(raw_key))? {
            match k {
                RawKey::EntityType => o.entity_type = map.next_value()?,
                RawKey::NumericId => o.numeric_id = map.next_value()?,
                RawKey::Id => o.id = map.next_value()?,
                RawKey::Time => o.time = map.next_value()?,
                RawKey::Precision => o.precision = map.next_value()?,
                RawKey::Calendar => o.calendar = map.next_value()?,
                RawKey::Amount => o.amount = map.next_value()?,
                RawKey::Unit => o.unit = map.next_value()?,
                RawKey::Upper => o.upper = map.next_value()?,
                RawKey::Lower => o.lower = map.next_value()?,
                RawKey::Lat => o.lat = map.next_value()?,
                RawKey::Lon => o.lon = map.next_value()?,
                RawKey::Globe => o.globe = map.next_value()?,
                RawKey::Text => o.text = map.next_value()?,
                RawKey::Language => o.language = map.next_value()?,
                RawKey::Other => ignore(&mut map)?,
            }
        }
        Ok(Raw::Obj(Box::new(o)))
    }
}

fn number(s: &Option<String>) -> Option<f64> {
    s.as_deref().and_then(|s| s.parse().ok())
}

fn make_value(raw: Raw, dtype: DataType) -> Option<Value> {
    match raw {
        Raw::Str(s) => Some(match dtype {
            DataType::ExternalId => Value::ExternalId { value: s },
            DataType::Url => Value::Url { value: s },
            DataType::CommonsMedia => Value::Media {
                url: sites::commons_file_url(&s),
                file: s,
            },
            DataType::Math => Value::Math { value: s },
            DataType::MusicalNotation => Value::MusicalNotation { value: s },
            DataType::GeoShape => Value::GeoShape {
                url: sites::commons_page_url(&s),
                value: s,
            },
            DataType::TabularData => Value::TabularData {
                url: sites::commons_page_url(&s),
                value: s,
            },
            _ => Value::String { value: s },
        }),
        Raw::Obj(o) => {
            let o = *o;
            if let Some(t) = o.time {
                return make_time(t, o.precision, o.calendar.as_deref()).map(Value::Time);
            }
            if o.amount.is_some() {
                return Some(Value::Quantity(Quantity {
                    amount: number(&o.amount)?,
                    unit: o
                        .unit
                        .as_deref()
                        .filter(|u| *u != "1")
                        .map(|u| entity_of_uri(u).to_string()),
                    lower: number(&o.lower),
                    upper: number(&o.upper),
                }));
            }
            if let (Some(lat), Some(lon)) = (o.lat, o.lon) {
                return Some(Value::Coordinate(Coordinate {
                    lat,
                    lon,
                    precision: o.precision,
                    globe: o
                        .globe
                        .as_deref()
                        .map_or_else(|| "Q2".to_string(), |g| entity_of_uri(g).to_string()),
                }));
            }
            if let (Some(text), Some(language)) = (o.text.clone(), o.language) {
                return Some(Value::Monolingual { text, language });
            }
            if let Some(id) = o.id {
                return Some(Value::Entity { id });
            }
            let letter = match o.entity_type.as_deref() {
                Some("item") => 'Q',
                Some("property") => 'P',
                Some("lexeme") => 'L',
                _ => return None,
            };
            o.numeric_id.map(|n| Value::Entity {
                id: format!("{letter}{n}"),
            })
        }
        Raw::Other => None,
    }
}

// ---- lexemes ---------------------------------------------------------------

struct FormsSeed<'a> {
    c: &'a Compiled,
}

impl<'de> DeserializeSeed<'de> for FormsSeed<'_> {
    type Value = Vec<Form>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for FormsSeed<'_> {
    type Value = Vec<Form>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("forms")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(f) = seq.next_element_seed(FormSeed { c: self.c })? {
            out.push(f);
        }
        Ok(out)
    }
}

struct FormSeed<'a> {
    c: &'a Compiled,
}

impl<'de> DeserializeSeed<'de> for FormSeed<'_> {
    type Value = Form;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for FormSeed<'_> {
    type Value = Form;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a form")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Form, A::Error> {
        let mut f = Form::default();
        while let Some(k) = map.next_key_seed(With(|k: &str| match k {
            "id" => 0,
            "representations" => 1,
            "grammaticalFeatures" => 2,
            "claims" => 3,
            _ => 4,
        }))? {
            match k {
                0 => f.id = map.next_value()?,
                1 => {
                    f.representations = map.next_value_seed(TermsSeed {
                        c: self.c,
                        filter: false,
                    })?
                }
                2 => f.grammatical_features = map.next_value()?,
                3 => f.claims = map.next_value_seed(ClaimsSeed { c: self.c, acc: None })?,
                _ => ignore(&mut map)?,
            }
        }
        Ok(f)
    }
}

struct SensesSeed<'a> {
    c: &'a Compiled,
}

impl<'de> DeserializeSeed<'de> for SensesSeed<'_> {
    type Value = Vec<Sense>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for SensesSeed<'_> {
    type Value = Vec<Sense>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("senses")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(s) = seq.next_element_seed(SenseSeed { c: self.c })? {
            out.push(s);
        }
        Ok(out)
    }
}

struct SenseSeed<'a> {
    c: &'a Compiled,
}

impl<'de> DeserializeSeed<'de> for SenseSeed<'_> {
    type Value = Sense;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for SenseSeed<'_> {
    type Value = Sense;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a sense")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Sense, A::Error> {
        let mut s = Sense::default();
        while let Some(k) = map.next_key_seed(With(|k: &str| match k {
            "id" => 0,
            "glosses" => 1,
            "claims" => 2,
            _ => 3,
        }))? {
            match k {
                0 => s.id = map.next_value()?,
                1 => {
                    s.glosses = map.next_value_seed(TermsSeed {
                        c: self.c,
                        filter: true,
                    })?
                }
                2 => s.claims = map.next_value_seed(ClaimsSeed { c: self.c, acc: None })?,
                _ => ignore(&mut map)?,
            }
        }
        Ok(s)
    }
}
