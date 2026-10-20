//! Building the class index: one pass reading the P279 claims of items.

use std::sync::Arc;

use wikidata_dump::Dump;
use wikidata_filter::ClassIndex;
use wikidata_model::{id_number, parse_entity, truthy, Compiled, EntityType, ModelOptions, Pid};

use crate::scan::{scan, Event, Flow, Progress, ScanOptions, ScanStats, Work};
use crate::ExportError;

/// Read every item's truthy P279 (subclass of) statements into a
/// [`ClassIndex`]. Only lines that contain `"P279":[` are parsed, and only
/// their P279 claims. `opts.filter` is ignored.
pub fn build_class_index(
    dump: &Dump,
    opts: &ScanOptions,
    progress: &mut dyn FnMut(Progress) -> Flow,
) -> Result<(ClassIndex, ScanStats), ExportError> {
    let model = Arc::new(Compiled::new(ModelOptions {
        languages: Some(Vec::new()),
        sitelinks: Some(Vec::new()),
        properties: Some(vec![Pid(279)]),
        labels: false,
        descriptions: false,
        aliases: false,
        qualifiers: false,
        references: false,
        derived: false,
        ..Default::default()
    }));
    let needle = memchr::memmem::Finder::new(b"\"P279\":[").into_owned();
    let work: Arc<Work<Vec<(u32, u32)>>> = Arc::new(move |line: &[u8], p, c| {
        if p.kind != EntityType::Item || needle.find(line).is_none() {
            return Ok(None);
        }
        let Some(child) = id_number(p.id).and_then(|n| u32::try_from(n).ok()) else {
            return Ok(None);
        };
        c.parsed += 1;
        let e = parse_entity(line, &model).map_err(|e| e.to_string())?;
        let edges: Vec<(u32, u32)> = truthy(e.statements(Pid(279)))
            .filter_map(|s| s.value.entity_id())
            .filter(|id| id.starts_with('Q'))
            .filter_map(|id| id_number(id).and_then(|n| u32::try_from(n).ok()))
            .map(|parent| (child, parent))
            .collect();
        Ok((!edges.is_empty()).then_some(edges))
    });
    let mut scan_opts = opts.clone();
    scan_opts.filter = None;
    let mut edges = Vec::new();
    let mut sink = |e: Event<Vec<(u32, u32)>>| {
        match e {
            Event::Records(rs) => {
                for r in rs {
                    edges.extend(r);
                }
            }
            Event::Progress(p) => return progress(p),
        }
        Flow::Continue
    };
    let r = scan(dump, &scan_opts, work, false, &mut sink)?;
    Ok((ClassIndex::from_edges(edges, dump.input().file_name()), r.stats))
}
