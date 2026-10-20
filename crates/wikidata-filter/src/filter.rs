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

use aho_corasick::AhoCorasick;
use memchr::memmem::Finder;
use rustc_hash::FxHashSet;
use serde_json::Value as Json;
use wikidata_model::{parse_entity, Compiled, Entity, EntityType, ModelOptions, Peek, Pid, Rank};

use crate::classes::ClassIndex;
use crate::ids::{id_key, IdSet, IdSetBuilder};
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

    /// Parse the JSON form (see the crate docs).
    pub fn from_json(v: &Json) -> Result<Filter, FilterError> {
        if let Json::Array(a) = v {
            return Ok(Filter::all(a.iter().map(Filter::from_json).collect::<Result<_, _>>()?));
        }
        let o = v
            .as_object()
            .ok_or_else(|| FilterError::Invalid("a filter is a JSON object".into()))?;
        let mut parts = Vec::new();
        for (k, v) in o {
            parts.push(match k.as_str() {
                "all" | "and" => Filter::All(
                    v.as_array()
                        .ok_or_else(|| FilterError::Invalid("all: expected an array".into()))?
                        .iter()
                        .map(Filter::from_json)
                        .collect::<Result<_, _>>()?,
                ),
                "any" | "or" => Filter::Any(
                    v.as_array()
                        .ok_or_else(|| FilterError::Invalid("any: expected an array".into()))?
                        .iter()
                        .map(Filter::from_json)
                        .collect::<Result<_, _>>()?,
                ),
                "not" => Filter::Not(Box::new(Filter::from_json(v)?)),
                "ids" => Filter::Ids(strings(v, "ids")?),
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
                other => return Err(FilterError::Invalid(format!("unknown filter {other:?}"))),
            });
        }
        if parts.is_empty() {
            return Err(FilterError::Invalid("an empty filter".into()));
        }
        Ok(Filter::all(parts))
    }

    /// Parse JSON text.
    pub fn parse_json(text: &str) -> Result<Filter, FilterError> {
        let v: Json = serde_json::from_str(text).map_err(|e| FilterError::Invalid(format!("filter JSON: {e}")))?;
        Filter::from_json(&v)
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

    /// Whether this filter (or a part) needs a class index.
    pub fn needs_class_index(&self) -> bool {
        match self {
            Filter::All(f) | Filter::Any(f) => f.iter().any(Filter::needs_class_index),
            Filter::Not(f) => f.needs_class_index(),
            Filter::InstanceOf { subclasses, .. } => *subclasses,
            _ => false,
        }
    }

    /// Prepare for matching; `classes` is required for
    /// `instanceOf` with subclasses.
    pub fn compile(&self, classes: Option<&ClassIndex>) -> Result<CompiledFilter, FilterError> {
        let mut needs = Needs::default();
        let root = compile(self, classes, &mut needs)?;
        let probe = (!needs.properties.is_empty() || !needs.sites.is_empty()).then(|| {
            Compiled::new(ModelOptions {
                languages: Some(Vec::new()),
                sitelinks: Some(needs.sites.iter().cloned().collect()),
                properties: Some(needs.properties.iter().map(|&p| Pid(p)).collect()),
                labels: false,
                descriptions: false,
                aliases: false,
                qualifiers: false,
                references: false,
                deprecated: false,
                best_rank: false,
                derived: false,
            })
        });
        Ok(CompiledFilter { root, probe })
    }
}

#[derive(Default)]
struct Needs {
    properties: FxHashSet<u32>,
    sites: FxHashSet<String>,
}

/// Byte strings, one of which must occur.
enum Needles {
    One(Finder<'static>),
    Many(AhoCorasick),
}

impl Needles {
    fn new(patterns: Vec<Vec<u8>>) -> Option<Needles> {
        match patterns.len() {
            0 => None,
            1 => Some(Needles::One(Finder::new(&patterns[0]).into_owned())),
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
    Ids(IdSet),
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

/// Value needles are only worth it for a limited number of values.
const MAX_VALUE_NEEDLES: usize = 5000;

fn compile(f: &Filter, classes: Option<&ClassIndex>, needs: &mut Needs) -> Result<Node, FilterError> {
    Ok(match f {
        Filter::All(fs) => Node::All(
            fs.iter()
                .map(|f| compile(f, classes, needs))
                .collect::<Result<_, _>>()?,
        ),
        Filter::Any(fs) => Node::Any(
            fs.iter()
                .map(|f| compile(f, classes, needs))
                .collect::<Result<_, _>>()?,
        ),
        Filter::Not(f) => Node::Not(Box::new(compile(f, classes, needs)?)),
        Filter::Ids(ids) => {
            let b = IdSetBuilder::new();
            for id in ids {
                b.insert(id_key(id).ok_or_else(|| FilterError::Invalid(format!("not an entity id: {id:?}")))?);
            }
            Node::Ids(b.freeze())
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
        Filter::Sitelink(sites) => {
            needs.sites.extend(sites.iter().cloned());
            Node::Sitelink {
                sites: sites.clone(),
                needles: Needles::new(sites.iter().map(|s| format!("\"site\":\"{s}\"").into_bytes()).collect()),
            }
        }
    })
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
        Node::Ids(set) => match peek {
            Some(p) => match id_key(p.id) {
                Some(k) if set.contains(k) => Pre::Yes,
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
        Node::Ids(set) => id_key(&e.id).is_some_and(|k| set.contains(k)),
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
    }
}

/// A filter ready for matching (see the module docs).
#[derive(Debug)]
pub struct CompiledFilter {
    root: Node,
    probe: Option<Compiled>,
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
        match self.pre(line, peek) {
            Pre::Yes => Ok(true),
            Pre::No => Ok(false),
            Pre::Maybe => {
                let probe = self.probe.clone().unwrap_or_else(|| Compiled::new(minimal()));
                let e = parse_entity(line, &probe).map_err(|e| FilterError::Invalid(e.to_string()))?;
                Ok(self.matches_entity(&e))
            }
        }
    }

    /// The options a probe parse uses (`None` when the filter decides on
    /// the type and id alone).
    pub fn probe_options(&self) -> Option<&Compiled> {
        self.probe.as_ref()
    }

    /// The ids of an `ids` filter at the top (or under `all`), to report
    /// requested ids that were not found.
    pub fn id_count(&self) -> Option<usize> {
        fn find(n: &Node) -> Option<usize> {
            match n {
                Node::Ids(s) => Some(s.len()),
                Node::All(ns) => ns.iter().find_map(find),
                _ => None,
            }
        }
        find(&self.root)
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
    fn errors() {
        assert!(Filter::parse_json(r#"{"bogus":1}"#).is_err());
        assert!(Filter::parse_json(r#"{"hasProperty":"Q1"}"#).is_err());
        assert!(Filter::parse_json(r#"{}"#).is_err());
        assert!(Filter::parse_json(r#"{"ids":["x"]}"#).unwrap().compile(None).is_err());
    }
}
