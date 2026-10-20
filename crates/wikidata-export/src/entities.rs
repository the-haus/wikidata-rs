//! Matching entities, parsed and normalised on the workers.

use std::sync::Arc;

use wikidata_dump::Dump;
use wikidata_filter::Pre;
use wikidata_model::{parse_entity, Compiled, Entity, Map, ModelOptions};

use crate::scan::{scan, ChunkCounts, Event, Flow, ScanOptions, ScanStats, Work};
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
}

/// A matching entity.
#[derive(Clone, Debug)]
pub struct Record {
    /// The entity (or its summary, with `summary_only`).
    pub entity: Entity,
    /// Its JSON (with `json`), one line without the newline.
    pub json: Vec<u8>,
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
    let full = Arc::new(Compiled::new(opts.model.clone()));
    let filter = opts.scan.filter.clone();
    let single = filter
        .as_ref()
        .is_none_or(|f| f.probe_options().is_none_or(|p| covers(&full, p)));
    let json = opts.json;
    let summary = opts.summary_only;
    let work: Arc<Work<Record>> = Arc::new(move |line: &[u8], p, c: &mut ChunkCounts| {
        let mut probed = false;
        if let Some(f) = &filter {
            match f.pre(line, Some(p)) {
                Pre::No => return Ok(None),
                Pre::Yes => {}
                Pre::Maybe if single => {}
                Pre::Maybe => {
                    c.parsed += 1;
                    if !f.matches_line(line, Some(p)).map_err(|e| e.to_string())? {
                        return Ok(None);
                    }
                    probed = true;
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
        Ok(Some(Record { entity: e, json: out }))
    });
    let mut scan_opts = opts.scan.clone();
    // The filter runs inside the work, sharing the parse when it can.
    scan_opts.filter = None;
    let r = scan(dump, &scan_opts, work, false, sink)?;
    Ok(r.stats)
}
