//! The engine every task runs on: chunks of lines from the dump are
//! filtered and turned into records on all cores, and come back in file
//! order to a sink on the calling thread.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use wikidata_dump::{ordered_map, resolve_threads, Chunk, Dump, ReadOptions, Result as DumpResult};
use wikidata_filter::{id_key, ClassIndex, CompiledFilter, FilterError, IdSetBuilder};
use wikidata_model::{id_number, peek, EntityType, Peek};

use crate::classes::ClassEdges;
use crate::ExportError;

/// What a sink tells the scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Keep going.
    Continue,
    /// Stop now (the scan ends with `stopped: true`).
    Stop,
}

/// Progress of a scan.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Bytes read from the file or URL.
    pub bytes_in: u64,
    /// The input's size, when known.
    pub bytes_total: Option<u64>,
    /// Decompressed bytes.
    pub bytes_out: u64,
    /// Entities seen.
    pub entities: u64,
    /// Entities that matched.
    pub matched: u64,
    /// Time since the start.
    pub elapsed_ms: u64,
}

/// What a sink receives.
pub enum Event<T> {
    /// Records of one chunk, in file order.
    Records(Vec<T>),
    /// Progress, about every 100 ms, and once more when the scan ends
    /// (finished or stopped), with its final totals.
    Progress(Progress),
}

/// Entity counts by type.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TypeCounts {
    /// Items.
    pub items: u64,
    /// Properties.
    pub properties: u64,
    /// Lexemes.
    pub lexemes: u64,
    /// Media info.
    pub mediainfo: u64,
    /// Entity schemas.
    pub entity_schemas: u64,
}

impl TypeCounts {
    fn add(&mut self, t: EntityType) {
        match t {
            EntityType::Item => self.items += 1,
            EntityType::Property => self.properties += 1,
            EntityType::Lexeme => self.lexemes += 1,
            EntityType::MediaInfo => self.mediainfo += 1,
            EntityType::EntitySchema => self.entity_schemas += 1,
        }
    }

    fn merge(&mut self, o: &TypeCounts) {
        self.items += o.items;
        self.properties += o.properties;
        self.lexemes += o.lexemes;
        self.mediainfo += o.mediainfo;
        self.entity_schemas += o.entity_schemas;
    }
}

/// Counters of a finished scan.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStats {
    /// `bzip2`, `gzip`, `zstd` or `none`.
    pub compression: &'static str,
    /// Bytes read from the file or URL.
    pub bytes_in: u64,
    /// Decompressed bytes.
    pub bytes_out: u64,
    /// Entities seen.
    pub entities: u64,
    /// Entities seen, by type.
    pub types: TypeCounts,
    /// Entities that matched.
    pub matched: u64,
    /// Entities parsed (the ones that needed a parse to be decided, and
    /// the matches).
    pub parsed: u64,
    /// bzip2 blocks decoded.
    pub blocks: u64,
    /// bzip2 streams.
    pub streams: u64,
    /// The scan was stopped early (by the consumer or `limit`).
    pub stopped: bool,
    /// The input was truncated (only with `allow_truncated`).
    pub truncated: bool,
    /// Wall time.
    pub elapsed_ms: u64,
    /// With `ids` in the filter, after a complete scan (not stopped, not
    /// truncated): the requested ids the dump does not hold (deleted, or
    /// merged into another item), in id order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing_ids: Option<Vec<String>>,
    /// The class index saved with [`ScanOptions::class_index`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_index: Option<SavedClassIndex>,
}

/// A class index built during a scan.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SavedClassIndex {
    /// Where it was saved.
    pub path: String,
    /// Classes with at least one parent.
    pub classes: u64,
    /// Subclass-of edges.
    pub edges: u64,
}

/// Options shared by every scan.
#[derive(Clone, Debug, Default)]
pub struct ScanOptions {
    /// Which entities match (default: all).
    pub filter: Option<Arc<CompiledFilter>>,
    /// Worker threads (0: all cores).
    pub threads: usize,
    /// Stop after this many matches.
    pub limit: Option<u64>,
    /// End at the last whole entity of a truncated input instead of
    /// failing (for partial downloads; `truncated` is set in the stats).
    pub allow_truncated: bool,
    /// Also build the class index, as [`crate::build_class_index`] does,
    /// and save it to this file when the scan ends without being stopped
    /// (the filter cannot use it in the same scan).
    pub class_index: Option<PathBuf>,
}

/// Per-chunk counters, made on the workers.
#[derive(Default)]
pub(crate) struct ChunkCounts {
    pub entities: u64,
    pub matched: u64,
    pub parsed: u64,
    pub types: TypeCounts,
    pub max_ids: [u64; 5],
    pub largest: (usize, String),
    /// Keys of the filter's `ids` seen in the chunk.
    pub seen: Vec<i64>,
    /// Subclass-of edges, with `class_index`.
    pub edges: Vec<(u32, u32)>,
}

/// What a worker makes of a matching line: `None` to drop it.
pub(crate) type Work<T> = dyn Fn(&[u8], &Peek<'_>, &mut ChunkCounts) -> Result<Option<T>, String> + Send + Sync;

pub(crate) struct ScanResult {
    pub stats: ScanStats,
    pub max_ids: [u64; 5],
    pub largest: (usize, String),
}

/// What a worker does besides filtering and `work`.
struct Side<'a> {
    track_largest: bool,
    /// The filter whose `ids` are tracked.
    track_ids: Option<&'a CompiledFilter>,
    classes: Option<&'a ClassEdges>,
}

fn process<T>(
    chunk: DumpResult<Chunk>,
    filter: Option<&CompiledFilter>,
    work: &Work<T>,
    side: &Side<'_>,
) -> Result<(Vec<T>, ChunkCounts), ExportError> {
    let chunk = chunk?;
    let mut out = Vec::new();
    let mut c = ChunkCounts::default();
    for line in chunk.lines() {
        c.entities += 1;
        let Some(p) = peek(line) else {
            return Err(ExportError::Invalid(format!(
                "not an entity near byte {} of the decompressed dump: {}",
                chunk.offset,
                String::from_utf8_lossy(&line[..line.len().min(120)])
            )));
        };
        c.types.add(p.kind);
        if let Some(n) = id_number(p.id) {
            let slot = &mut c.max_ids[p.kind.code() as usize];
            *slot = (*slot).max(n);
        }
        if side.track_largest && line.len() > c.largest.0 {
            c.largest = (line.len(), p.id.to_string());
        }
        if let Some(f) = side.track_ids {
            if let Some(k) = id_key(p.id).filter(|&k| f.requests(k)) {
                c.seen.push(k);
            }
        }
        if let Some(classes) = side.classes {
            if let Some(edges) = classes
                .read(line, &p)
                .map_err(|e| ExportError::Invalid(format!("{}: {e}", p.id)))?
            {
                c.edges.extend(edges);
            }
        }
        if let Some(f) = filter {
            match f.pre(line, Some(&p)) {
                wikidata_filter::Pre::No => continue,
                wikidata_filter::Pre::Yes => {}
                wikidata_filter::Pre::Maybe => {
                    c.parsed += 1;
                    let ok = f
                        .matches_line(line, Some(&p))
                        .map_err(|e| ExportError::Invalid(format!("{}: {e}", p.id)))?;
                    if !ok {
                        continue;
                    }
                }
            }
        }
        if let Some(t) = work(line, &p, &mut c).map_err(|e| ExportError::Invalid(format!("{}: {e}", p.id)))? {
            c.matched += 1;
            out.push(t);
        }
    }
    Ok((out, c))
}

/// Run a scan (see the module docs). `prefilter`: whether lines go to
/// `work` only once `opts.filter` matched them (else `work` filters).
pub(crate) fn scan<T: Send + 'static>(
    dump: &Dump,
    opts: &ScanOptions,
    prefilter: bool,
    work: Arc<Work<T>>,
    track_largest: bool,
    sink: &mut dyn FnMut(Event<T>) -> Flow,
) -> Result<ScanResult, ExportError> {
    let start = Instant::now();
    let threads = resolve_threads(opts.threads);
    let chunks = dump.chunks(&ReadOptions {
        threads,
        allow_truncated: opts.allow_truncated,
    })?;
    let read = chunks.stats();
    let total = chunks.input_len().or(dump.len());
    let filter = opts.filter.clone();
    let track_ids = filter.clone().filter(|f| f.has_ids());
    let classes = opts.class_index.as_ref().map(|_| Arc::new(ClassEdges::new()));
    let worker_classes = classes.clone();
    let worker_track = track_ids.clone();
    let results = ordered_map(chunks, threads, threads * 2, move |c| {
        let side = Side {
            track_largest,
            track_ids: worker_track.as_deref(),
            classes: worker_classes.as_deref(),
        };
        process(c, filter.as_deref().filter(|_| prefilter), &*work, &side)
    });
    let seen = IdSetBuilder::new();
    let mut edges = Vec::new();

    let mut stats = ScanStats {
        compression: dump.compression().as_str(),
        ..Default::default()
    };
    let mut max_ids = [0u64; 5];
    let mut largest = (0usize, String::new());
    let mut last = Instant::now();
    let limit = opts.limit.unwrap_or(u64::MAX);
    let progress = |stats: &ScanStats| Progress {
        bytes_in: read.bytes_in.get(),
        bytes_total: total,
        bytes_out: read.bytes_out.load(Ordering::Relaxed),
        entities: stats.entities,
        matched: stats.matched,
        elapsed_ms: start.elapsed().as_millis() as u64,
    };
    for r in results {
        let (mut records, c) = r?;
        seen.insert_all(&c.seen);
        edges.extend(c.edges);
        stats.entities += c.entities;
        stats.parsed += c.parsed;
        stats.types.merge(&c.types);
        for (m, x) in max_ids.iter_mut().zip(c.max_ids) {
            *m = (*m).max(x);
        }
        if c.largest.0 > largest.0 {
            largest = c.largest;
        }
        let room = limit - stats.matched;
        if records.len() as u64 > room {
            records.truncate(room as usize);
        }
        stats.matched += records.len() as u64;
        if !records.is_empty() && sink(Event::Records(records)) == Flow::Stop {
            stats.stopped = true;
            break;
        }
        if stats.matched >= limit {
            stats.stopped = true;
            break;
        }
        if last.elapsed() >= Duration::from_millis(100) {
            last = Instant::now();
            if sink(Event::Progress(progress(&stats))) == Flow::Stop {
                stats.stopped = true;
                break;
            }
        }
    }
    // The last progress event always carries the final totals: at the end,
    // and when the scan stops early (a limit, or a sink saying `Stop`).
    let _ = sink(Event::Progress(progress(&stats)));
    stats.bytes_in = read.bytes_in.get();
    stats.bytes_out = read.bytes_out.load(Ordering::Relaxed);
    stats.blocks = read.blocks.load(Ordering::Relaxed);
    stats.streams = read.streams.load(Ordering::Relaxed);
    stats.truncated = read.truncated.load(Ordering::Relaxed);
    if let Some(f) = track_ids.filter(|_| !stats.stopped && !stats.truncated) {
        stats.missing_ids = Some(f.missing_ids(&seen.freeze()));
    }
    if let (Some(path), false) = (&opts.class_index, stats.stopped) {
        let index = ClassIndex::from_edges(edges, dump.input().file_name());
        save_atomically(&index, path)?;
        stats.class_index = Some(SavedClassIndex {
            path: path.display().to_string(),
            classes: index.len() as u64,
            edges: index.edges() as u64,
        });
    }
    stats.elapsed_ms = start.elapsed().as_millis() as u64;
    Ok(ScanResult {
        stats,
        max_ids,
        largest,
    })
}

/// Save through a temporary file beside `path`, so that `path` either does
/// not exist or holds a whole index.
fn save_atomically(index: &ClassIndex, path: &std::path::Path) -> Result<(), ExportError> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    index.save(&tmp)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        FilterError::Io(format!("{}: {e}", path.display()))
    })?;
    Ok(())
}
