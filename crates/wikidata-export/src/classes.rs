//! Building the class index: one pass reading the P279 claims of items.

use std::sync::Arc;

use aho_corasick::AhoCorasick;
use wikidata_dump::Dump;
use wikidata_filter::{place_edge, ClassIndex, PlaceIndex, PlaceProperty};
use wikidata_model::{id_number, parse_entity, truthy, Compiled, EntityType, ModelOptions, Peek, Pid};

use crate::scan::{scan, ChunkCounts, Event, Flow, Progress, ScanOptions, ScanStats, Work};
use crate::ExportError;

/// Reads the subclass-of edges of a line.
pub(crate) struct ClassEdges {
    model: Compiled,
    needle: memchr::memmem::Finder<'static>,
}

impl ClassEdges {
    pub(crate) fn new() -> ClassEdges {
        ClassEdges {
            model: Compiled::new(ModelOptions {
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
            }),
            needle: memchr::memmem::Finder::new(b"\"P279\":[").into_owned(),
        }
    }

    /// The `(child, parent)` edges of an item's truthy P279 statements;
    /// only lines that contain `"P279":[` are parsed (`None`: not parsed).
    pub(crate) fn read(&self, line: &[u8], p: &Peek<'_>) -> Result<Option<Vec<(u32, u32)>>, String> {
        if p.kind != EntityType::Item || self.needle.find(line).is_none() {
            return Ok(None);
        }
        let Some(child) = id_number(p.id).and_then(|n| u32::try_from(n).ok()) else {
            return Ok(None);
        };
        let e = parse_entity(line, &self.model).map_err(|e| e.to_string())?;
        Ok(Some(
            truthy(e.statements(Pid(279)))
                .filter_map(|s| s.value.entity_id())
                .filter(|id| id.starts_with('Q'))
                .filter_map(|id| id_number(id).and_then(|n| u32::try_from(n).ok()))
                .map(|parent| (child, parent))
                .collect(),
        ))
    }
}

/// Reads the P17, P131 and P159 edges of a line, for the place index.
pub(crate) struct PlaceEdges {
    model: Compiled,
    needles: AhoCorasick,
}

impl PlaceEdges {
    pub(crate) fn new() -> PlaceEdges {
        PlaceEdges {
            model: Compiled::new(ModelOptions {
                languages: Some(Vec::new()),
                sitelinks: Some(Vec::new()),
                properties: Some(vec![Pid(17), Pid(131), Pid(159)]),
                labels: false,
                descriptions: false,
                aliases: false,
                qualifiers: false,
                references: false,
                derived: false,
                ..Default::default()
            }),
            needles: AhoCorasick::new([&b"\"P17\":["[..], b"\"P131\":[", b"\"P159\":["]).expect("valid patterns"),
        }
    }

    /// The edges ([`place_edge`]) of an item's truthy P17, P131 and P159
    /// statements; only lines that mention one of them are parsed.
    pub(crate) fn read(&self, line: &[u8], p: &Peek<'_>, out: &mut Vec<(u32, u32)>) -> Result<bool, String> {
        if p.kind != EntityType::Item || !self.needles.is_match(line) {
            return Ok(false);
        }
        let Some(item) = id_number(p.id).and_then(|n| u32::try_from(n).ok()) else {
            return Ok(false);
        };
        let e = parse_entity(line, &self.model).map_err(|e| e.to_string())?;
        for (pid, kind) in [
            (17, PlaceProperty::Country),
            (131, PlaceProperty::LocatedIn),
            (159, PlaceProperty::Headquarters),
        ] {
            out.extend(
                truthy(e.statements(Pid(pid)))
                    .filter_map(|s| s.value.entity_id())
                    .filter(|id| id.starts_with('Q'))
                    .filter_map(|id| id_number(id).and_then(|n| u32::try_from(n).ok()))
                    .filter_map(|v| place_edge(item, kind, v)),
            );
        }
        Ok(true)
    }
}

/// Read every item's truthy P17 (country), P131 (located in) and P159
/// (headquarters) statements into a [`PlaceIndex`]. Only lines that
/// mention one of them are parsed, and only those claims. `opts.filter`
/// and `opts.place_index` are ignored; with `opts.class_index`, the class
/// index is built in the same pass.
pub fn build_place_index(
    dump: &Dump,
    opts: &ScanOptions,
    progress: &mut dyn FnMut(Progress) -> Flow,
) -> Result<(PlaceIndex, ScanStats), ExportError> {
    let places = PlaceEdges::new();
    let work: Arc<Work<Vec<(u32, u32)>>> = Arc::new(move |line: &[u8], p, c: &mut ChunkCounts| {
        let mut edges = Vec::new();
        if !places.read(line, p, &mut edges)? {
            return Ok(None);
        }
        c.parsed += 1;
        Ok((!edges.is_empty()).then_some(edges))
    });
    let scan_opts = ScanOptions {
        filter: None,
        place_index: None,
        ..opts.clone()
    };
    let mut edges = Vec::new();
    let mut sink = |e: Event<Vec<(u32, u32)>>| {
        match e {
            Event::Records(rs) => {
                for r in rs {
                    edges.extend(r);
                }
            }
            Event::Progress(p) => return progress(p),
            Event::Checkpoint(_) => {}
        }
        Flow::Continue
    };
    let r = scan(dump, &scan_opts, true, work, false, &mut sink)?;
    Ok((PlaceIndex::from_edges(edges, dump.input().file_name()), r.stats))
}

/// Read every item's truthy P279 (subclass of) statements into a
/// [`ClassIndex`]. Only lines that contain `"P279":[` are parsed, and only
/// their P279 claims. `opts.filter` and `opts.class_index` are ignored;
/// with `opts.place_index`, the place index is built in the same pass.
///
/// Any other task builds the same index as it goes with
/// [`ScanOptions::class_index`].
pub fn build_class_index(
    dump: &Dump,
    opts: &ScanOptions,
    progress: &mut dyn FnMut(Progress) -> Flow,
) -> Result<(ClassIndex, ScanStats), ExportError> {
    let classes = ClassEdges::new();
    let work: Arc<Work<Vec<(u32, u32)>>> = Arc::new(move |line: &[u8], p, c: &mut ChunkCounts| {
        let Some(edges) = classes.read(line, p)? else {
            return Ok(None);
        };
        c.parsed += 1;
        Ok((!edges.is_empty()).then_some(edges))
    });
    let scan_opts = ScanOptions {
        filter: None,
        class_index: None,
        ..opts.clone()
    };
    let mut edges = Vec::new();
    let mut sink = |e: Event<Vec<(u32, u32)>>| {
        match e {
            Event::Records(rs) => {
                for r in rs {
                    edges.extend(r);
                }
            }
            Event::Progress(p) => return progress(p),
            Event::Checkpoint(_) => {}
        }
        Flow::Continue
    };
    let r = scan(dump, &scan_opts, true, work, false, &mut sink)?;
    Ok((ClassIndex::from_edges(edges, dump.input().file_name()), r.stats))
}
