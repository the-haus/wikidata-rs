//! Matching entities, parsed and normalised on the workers.

use std::collections::HashMap;
use std::sync::Arc;

use wikidata_dump::Dump;
use wikidata_filter::{id_from_key, id_key, PlaceIndex, Pre};
use wikidata_model::{parse_entity, truthy, Compiled, Entity, Map, ModelOptions, Pid, Text};

use crate::labels::{self, Collector, LabelOptions};
use crate::scan::{scan_with, ChunkCounts, Event, Flow, ScanOptions, ScanStats, Work};
use crate::ExportError;

/// Options for [`entities`].
#[derive(Clone, Debug, Default)]
pub struct EntityOptions {
    /// Filter, threads, limit.
    pub scan: ScanOptions,
    /// What to keep of each entity.
    pub model: ModelOptions,
    /// Serialise each entity to JSON on the workers ([`Record::json`]).
    pub json: bool,
    /// Keep only the id, type, label, description and derived fields in
    /// [`Record::entity`] (when the JSON carries the rest).
    pub summary_only: bool,
    /// Choose each entity's languages by its claims (replaces
    /// `model.languages`).
    pub languages_by: Option<ByClaim>,
    /// Choose each entity's sitelinks by its claims (replaces
    /// `model.sitelinks`).
    pub sitelinks_by: Option<ByClaim>,
    /// Also collect the labels of the items the matching entities' kept
    /// claims refer to ([`ScanStats::referenced_labels`]; see
    /// [`crate::labels`]).
    pub referenced_labels: Option<LabelOptions>,
}

/// Languages (or sites) chosen per entity by the values of its claims:
/// with `properties` `[P17]` and `map` `{Q55: [nl], Q183: [de]}`, a Dutch
/// item keeps `nl`, a German one `de`, and every entity `always` (`en`,
/// `mul`) after them. The values are read from the truthy statements;
/// for languages, the order is the fallback chain of `label` and
/// `description`. With `places`, an entity whose claims choose nothing
/// is looked up in the place index (its countries through P131 and P159).
#[derive(Clone, Debug, Default)]
pub struct ByClaim {
    /// The properties whose values choose (`P17`).
    pub properties: Vec<Pid>,
    /// Item id → languages or sites.
    pub map: HashMap<String, Vec<String>>,
    /// Kept for every entity, after the chosen ones.
    pub always: Vec<String>,
    /// Countries for the entities whose claims choose nothing.
    pub places: Option<Arc<PlaceIndex>>,
}

impl ByClaim {
    /// Every language or site the rule can keep (what a parse must keep).
    fn union(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut keys: Vec<&String> = self.map.keys().collect();
        keys.sort();
        for l in keys.into_iter().flat_map(|k| &self.map[k]).chain(&self.always) {
            if !out.contains(l) {
                out.push(l.clone());
            }
        }
        out
    }

    /// The languages or sites of `e`, in order.
    fn select(&self, e: &Entity) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let chosen = self
            .properties
            .iter()
            .flat_map(|&p| truthy(e.statements(p)))
            .filter_map(|s| s.value.entity_id())
            .filter_map(|id| self.map.get(id))
            .flatten();
        for l in chosen {
            if !out.contains(l) {
                out.push(l.clone());
            }
        }
        if let (true, Some(places)) = (out.is_empty(), &self.places) {
            for c in places.country_of(&e.id) {
                for l in self.map.get(&c).into_iter().flatten() {
                    if !out.contains(l) {
                        out.push(l.clone());
                    }
                }
            }
        }
        for l in &self.always {
            if !out.contains(l) {
                out.push(l.clone());
            }
        }
        out
    }
}

/// Keep `chain`'s languages of `terms`, in `chain` order.
fn keep<V>(terms: &mut Map<String, V>, chain: &[String]) {
    terms.0.retain(|(l, _)| chain.contains(l));
    terms.0.sort_by_key(|(l, _)| chain.iter().position(|c| c == l));
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

/// Apply a languages rule to a parsed entity.
fn keep_languages(e: &mut Entity, chain: &[String]) {
    keep(&mut e.labels, chain);
    keep(&mut e.descriptions, chain);
    keep(&mut e.aliases, chain);
    if let Some(l) = &mut e.lexeme {
        for s in &mut l.senses {
            keep(&mut s.glosses, chain);
        }
    }
    e.label = pick(&e.labels, chain);
    e.description = pick(&e.descriptions, chain);
}

/// A matching entity.
#[derive(Clone, Debug)]
pub struct Record {
    /// The entity (or its summary, with `summary_only`).
    pub entity: Entity,
    /// Its JSON (with `json`), one line without the newline.
    pub json: Vec<u8>,
    /// The named filters that match it, as indices into the filter's
    /// [`names`](wikidata_filter::CompiledFilter::names).
    pub matched_by: Vec<u16>,
    /// The filter's `ids` that were merged into it (with a redirect
    /// index; see [`wikidata_filter::Indexes::redirects`]).
    pub merged_from: Vec<String>,
}

/// Whether parsing with `full` reads everything the filter looks at, so
/// one parse can serve both.
fn covers(full: &Compiled, probe: &Compiled) -> bool {
    let f = full.options();
    let p = probe.options();
    if f.best_rank {
        return false;
    }
    let props = match (&f.properties, &p.properties) {
        (None, _) => true,
        (Some(have), Some(need)) => need.iter().all(|n| have.contains(n)),
        (Some(_), None) => false,
    };
    let sites = match (&f.sitelinks, &p.sitelinks) {
        (None, _) => true,
        (Some(have), Some(need)) => need.iter().all(|n| have.contains(n)),
        (Some(_), None) => false,
    };
    props && sites
}

/// Stream the entities that match `opts.scan.filter` to `sink`, in file
/// order. Returns when the dump ends, the sink says [`Flow::Stop`] or
/// `limit` entities matched.
pub fn entities(
    dump: &Dump,
    opts: &EntityOptions,
    sink: &mut dyn FnMut(Event<Record>) -> Flow,
) -> Result<ScanStats, ExportError> {
    let mut model = opts.model.clone();
    // Properties a rule reads but `properties` leaves out: parsed, read,
    // then dropped.
    let mut extra: Vec<Pid> = Vec::new();
    for rule in [&opts.languages_by, &opts.sitelinks_by].into_iter().flatten() {
        if let Some(ps) = &mut model.properties {
            for p in &rule.properties {
                if !ps.contains(p) {
                    ps.push(*p);
                    extra.push(*p);
                }
            }
        }
    }
    if let Some(r) = &opts.languages_by {
        model.languages = Some(r.union());
    }
    if let Some(r) = &opts.sitelinks_by {
        model.sitelinks = Some(r.union());
    }
    let languages_by = opts.languages_by.clone();
    let sitelinks_by = opts.sitelinks_by.clone();
    let full = Arc::new(Compiled::new(model));
    let filter = opts.scan.filter.clone();
    let single = filter
        .as_ref()
        .is_none_or(|f| f.probe_options().is_none_or(|p| covers(&full, p)));
    let json = opts.json;
    let summary = opts.summary_only;
    if opts.referenced_labels.is_some() && opts.scan.resume.is_some() {
        return Err(ExportError::Invalid(
            "referenced labels cannot be collected during a resumed pass".into(),
        ));
    }
    let collector = opts.referenced_labels.as_ref().map(|o| Arc::new(Collector::new(o)));
    let worker_collector = collector.clone();
    let work: Arc<Work<Record>> = Arc::new(move |line: &[u8], p, c: &mut ChunkCounts| {
        let mut probed = false;
        // The probe parse, kept for the named filters.
        let mut probe = None;
        if let Some(f) = &filter {
            match f.pre(line, Some(p)) {
                Pre::No => return Ok(None),
                Pre::Yes => {}
                Pre::Maybe if single => {}
                Pre::Maybe => {
                    c.parsed += 1;
                    let (ok, e) = f.check_line(line, Some(p)).map_err(|e| e.to_string())?;
                    if !ok {
                        return Ok(None);
                    }
                    probed = true;
                    probe = e;
                }
            }
        }
        c.parsed += 1;
        let mut e = parse_entity(line, &full).map_err(|e| e.to_string())?;
        if let (Some(f), false) = (&filter, probed) {
            if single && !f.matches_entity(&e) {
                return Ok(None);
            }
        }
        let matched_by = match &filter {
            // With `single`, the full parse has what the filter reads;
            // else the probe parse does, if there was one.
            Some(f) if !f.names().is_empty() => f
                .matched_names(line, Some(p), if single { Some(&e) } else { probe.as_ref() })
                .map_err(|e| e.to_string())?,
            _ => Vec::new(),
        };
        if let Some(r) = &languages_by {
            let chain = r.select(&e);
            keep_languages(&mut e, &chain);
        }
        if let Some(r) = &sitelinks_by {
            let sites = r.select(&e);
            e.sitelinks.0.retain(|(s, _)| sites.contains(s));
        }
        if !extra.is_empty() {
            e.claims.0.retain(|(p, _)| !extra.contains(p));
        }
        if let Some(c2) = &worker_collector {
            c2.note(&e, &mut c.referenced);
        }
        let merged_from = match (&filter, id_key(&e.id)) {
            (Some(f), Some(k)) if f.is_target(k) => f.merged_from(k).iter().filter_map(|&k| id_from_key(k)).collect(),
            _ => Vec::new(),
        };
        let mut out = Vec::new();
        if json {
            out.reserve(line.len() / 4 + 256);
            e.write_json(&mut out);
        }
        if summary {
            e.labels = Map::default();
            e.descriptions = Map::default();
            e.aliases = Map::default();
            e.sitelinks = Map::default();
            e.claims = Map::default();
            e.lexeme = None;
        }
        Ok(Some(Record {
            entity: e,
            json: out,
            matched_by,
            merged_from,
        }))
    });
    // The filter runs inside the work, sharing the parse when it can.
    let r = scan_with(dump, &opts.scan, false, work, false, collector.clone(), sink)?;
    let mut stats = r.stats;
    if let (Some(collector), Some(pass), false) = (collector, r.labels, stats.stopped || stats.truncated) {
        let threads = wikidata_dump::resolve_threads(opts.scan.threads);
        let found = labels::finish(dump, threads, &collector, pass)?;
        stats.labels = Some(found.stats);
        stats.referenced_labels = Some(found);
    }
    Ok(stats)
}
