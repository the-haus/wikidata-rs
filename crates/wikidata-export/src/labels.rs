//! Labels of the items that matching entities refer to.
//!
//! A festival's genre, a venue's operator, the village in its P131: values
//! that are other items, whose labels a consumer wants without a request
//! per item and without keeping every label of the dump. With
//! [`EntityOptions::referenced_labels`](crate::EntityOptions), a pass
//! collects them in two stages:
//!
//! 1. **During the pass.** The workers note the items each matching
//!    entity's kept claims (and their qualifiers) refer to in a shared
//!    bitmap, and take the labels of any item already noted when its line
//!    streams by. References to items later in the dump are answered here.
//! 2. **After it**, for the items that came before their first reference
//!    (most of them: the dump is roughly in creation order, and new items
//!    refer to old ones). The pass also kept, for each chunk, where it
//!    starts and which ranges of 256 item numbers it holds (the dump's ids
//!    are nearly sorted, so a chunk holds a few ranges), so only the
//!    chunks holding a missing item are read again: from a bzip2 block, or
//!    a byte of plain JSON. gzip and zstd inputs cannot be read from the
//!    middle and are read again whole, with only the missing ids parsed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use wikidata_dump::{ordered_map, Dump, ReadOptions, ResumePoint};
use wikidata_model::{id_number, parse_entity, Compiled, Entity, EntityType, ModelOptions, Peek, Value};

use crate::ExportError;

/// Options for referenced labels.
#[derive(Clone, Debug, Default)]
pub struct LabelOptions {
    /// The languages to collect (`["en", "nl"]`).
    pub languages: Vec<String>,
}

/// Labels by language, in the order asked.
pub type Labels = Vec<(String, String)>;

/// The labels of the referenced items, sorted by item number.
#[derive(Clone, Debug, Default)]
pub struct ReferencedLabels {
    /// Item number and its labels (items without a label in any asked
    /// language are left out).
    pub labels: Vec<(u32, Labels)>,
    /// How they were collected.
    pub stats: LabelStats,
}

impl ReferencedLabels {
    /// The labels of item number `q`.
    pub fn get(&self, q: u32) -> Option<&Labels> {
        self.labels
            .binary_search_by_key(&q, |(i, _)| *i)
            .ok()
            .map(|i| &self.labels[i].1)
    }
}

/// `{"Q5": {"en": "human"}, …}`.
impl Serialize for ReferencedLabels {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        struct One<'a>(&'a Labels);
        impl Serialize for One<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let mut m = s.serialize_map(Some(self.0.len()))?;
                for (l, t) in self.0 {
                    m.serialize_entry(l, t)?;
                }
                m.end()
            }
        }
        let mut m = s.serialize_map(Some(self.labels.len()))?;
        for (q, ls) in &self.labels {
            m.serialize_entry(&format!("Q{q}"), &One(ls))?;
        }
        m.end()
    }
}

/// How the referenced labels were collected.
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelStats {
    /// Distinct items referenced.
    pub referenced: u64,
    /// Items with a label.
    pub labelled: u64,
    /// Labels taken during the pass.
    pub during_pass: u64,
    /// Chunks read again after the pass.
    pub chunks_reread: u64,
    /// Chunks of the pass.
    pub chunks: u64,
}

/// Bits by item number, set from many threads, allocated 1 Mi items at a
/// time.
struct Bits {
    pages: Box<[OnceLock<Box<[AtomicU64]>>]>,
}

const PAGE_BITS: u32 = 20;

impl Bits {
    fn new() -> Bits {
        // Item numbers below 2^32.
        Bits {
            pages: (0..1usize << (32 - PAGE_BITS)).map(|_| OnceLock::new()).collect(),
        }
    }

    fn set(&self, q: u32) {
        let page = self.pages[(q >> PAGE_BITS) as usize]
            .get_or_init(|| (0..1usize << (PAGE_BITS - 6)).map(|_| AtomicU64::new(0)).collect());
        let i = (q & ((1 << PAGE_BITS) - 1)) as usize;
        page[i >> 6].fetch_or(1 << (i & 63), Ordering::Relaxed);
    }

    #[inline]
    fn get(&self, q: u32) -> bool {
        match self.pages[(q >> PAGE_BITS) as usize].get() {
            Some(page) => {
                let i = (q & ((1 << PAGE_BITS) - 1)) as usize;
                page[i >> 6].load(Ordering::Relaxed) & (1 << (i & 63)) != 0
            }
            None => false,
        }
    }
}

/// Shared by the workers of a pass (see the module docs).
pub(crate) struct Collector {
    referenced: Bits,
    model: Compiled,
}

impl Collector {
    pub(crate) fn new(opts: &LabelOptions) -> Collector {
        Collector {
            referenced: Bits::new(),
            model: Compiled::new(ModelOptions {
                languages: Some(opts.languages.clone()),
                sitelinks: Some(Vec::new()),
                properties: Some(Vec::new()),
                descriptions: false,
                aliases: false,
                qualifiers: false,
                derived: false,
                ..Default::default()
            }),
        }
    }

    /// Note the items `e`'s claims refer to (into `out` as well).
    pub(crate) fn note(&self, e: &Entity, out: &mut Vec<u32>) {
        let mut add = |v: &Value| {
            if let Some(q) = v.entity_id().filter(|id| id.starts_with('Q')).and_then(qnum) {
                self.referenced.set(q);
                out.push(q);
            }
        };
        for (_, sts) in e.claims.iter() {
            for s in sts {
                add(&s.value);
                for (_, vs) in s.qualifiers.iter() {
                    vs.iter().for_each(&mut add);
                }
            }
        }
    }

    /// The labels of `line`'s item when it is referenced already.
    pub(crate) fn capture(&self, line: &[u8], p: &Peek<'_>) -> Result<Option<(u32, Labels)>, String> {
        if p.kind != EntityType::Item {
            return Ok(None);
        }
        let Some(q) = qnum(p.id).filter(|&q| self.referenced.get(q)) else {
            return Ok(None);
        };
        Ok(Some((q, self.labels(line)?)))
    }

    fn labels(&self, line: &[u8]) -> Result<Labels, String> {
        Ok(parse_entity(line, &self.model).map_err(|e| e.to_string())?.labels.0)
    }
}

fn qnum(id: &str) -> Option<u32> {
    id_number(id).and_then(|n| u32::try_from(n).ok())
}

/// Ranges of 256 item numbers.
#[inline]
pub(crate) fn bucket(q: u32) -> u32 {
    q >> 8
}

/// Where a chunk of the pass starts, and the ranges of item numbers it
/// holds.
pub(crate) struct ChunkSummary {
    /// `None`: the start of the input.
    pub start: Option<ResumePoint>,
    pub buckets: Vec<u32>,
}

/// What the pass collected.
#[derive(Default)]
pub(crate) struct PassLabels {
    pub captured: Vec<(u32, Labels)>,
    pub referenced: Vec<u32>,
    pub summaries: Vec<ChunkSummary>,
    /// Every chunk has a start (bzip2, plain JSON).
    pub seekable: bool,
}

/// The second stage (see the module docs): the labels of `missing`
/// (sorted item numbers).
fn second_stage(
    dump: &Dump,
    threads: usize,
    collector: &Arc<Collector>,
    missing: &[u32],
    pass: &PassLabels,
    stats: &mut LabelStats,
) -> Result<Vec<(u32, Labels)>, ExportError> {
    let mut wanted: Vec<u32> = missing.iter().map(|&q| bucket(q)).collect();
    wanted.dedup();
    // Runs of chunks to read: (start, count).
    let mut runs: Vec<(Option<ResumePoint>, usize)> = Vec::new();
    if pass.seekable {
        let mut i = 0;
        while i < pass.summaries.len() {
            let hit = |s: &ChunkSummary| s.buckets.iter().any(|b| wanted.binary_search(b).is_ok());
            if !hit(&pass.summaries[i]) {
                i += 1;
                continue;
            }
            let from = i;
            while i < pass.summaries.len() && hit(&pass.summaries[i]) {
                i += 1;
            }
            runs.push((pass.summaries[from].start, i - from));
        }
    } else {
        runs.push((None, usize::MAX));
    }
    let missing = Arc::new(missing.to_vec());
    let mut out = Vec::new();
    for (start, count) in runs {
        let chunks = dump.chunks(&ReadOptions {
            threads,
            resume: start,
            ..Default::default()
        })?;
        let (c, m) = (collector.clone(), missing.clone());
        let found = ordered_map(chunks.take(count), threads, threads * 2, move |chunk| {
            let chunk = chunk?;
            let mut out = Vec::new();
            for line in chunk.lines() {
                let Some(p) = wikidata_model::peek(line) else { continue };
                if p.kind != EntityType::Item {
                    continue;
                }
                if let Some(q) = qnum(p.id).filter(|q| m.binary_search(q).is_ok()) {
                    out.push((
                        q,
                        c.labels(line)
                            .map_err(|e| ExportError::Invalid(format!("{}: {e}", p.id)))?,
                    ));
                }
            }
            Ok::<_, ExportError>(out)
        });
        for r in found {
            stats.chunks_reread += 1;
            out.extend(r?);
        }
    }
    if !pass.seekable {
        stats.chunks_reread = stats.chunks_reread.min(stats.chunks);
    }
    Ok(out)
}

/// Both stages' labels, for a complete pass.
pub(crate) fn finish(
    dump: &Dump,
    threads: usize,
    collector: &Arc<Collector>,
    mut pass: PassLabels,
) -> Result<ReferencedLabels, ExportError> {
    let mut stats = LabelStats {
        chunks: pass.summaries.len() as u64,
        ..Default::default()
    };
    pass.referenced.sort_unstable();
    pass.referenced.dedup();
    stats.referenced = pass.referenced.len() as u64;
    let mut captured = std::mem::take(&mut pass.captured);
    captured.sort_unstable_by_key(|(q, _)| *q);
    captured.dedup_by_key(|(q, _)| *q);
    stats.during_pass = captured.len() as u64;
    let missing: Vec<u32> = pass
        .referenced
        .iter()
        .copied()
        .filter(|q| captured.binary_search_by_key(q, |(i, _)| *i).is_err())
        .collect();
    if !missing.is_empty() {
        let more = second_stage(dump, threads, collector, &missing, &pass, &mut stats)?;
        captured.extend(more);
        captured.sort_unstable_by_key(|(q, _)| *q);
        captured.dedup_by_key(|(q, _)| *q);
    }
    // Only referenced items (a capture may have raced a reference that was
    // noted, then dropped by a limit), and only those with a label.
    captured.retain(|(q, ls)| !ls.is_empty() && pass.referenced.binary_search(q).is_ok());
    stats.labelled = captured.len() as u64;
    Ok(ReferencedLabels {
        labels: captured,
        stats,
    })
}
