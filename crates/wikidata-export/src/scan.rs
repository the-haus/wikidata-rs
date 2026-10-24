//! The engine every task runs on: chunks of lines from the dump are
//! filtered and turned into records on all cores, and come back in file
//! order to a sink on the calling thread.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use wikidata_dump::ResumePoint;
use wikidata_dump::{ordered_map, resolve_threads, Chunk, Dump, ReadOptions, Result as DumpResult};
use wikidata_filter::{id_key, ClassIndex, CompiledFilter, FilterError, IdSetBuilder, Misses, PlaceIndex};
use wikidata_model::{id_number, peek, EntityType, Peek};

use crate::checkpoint::Checkpoint;
use crate::classes::{ClassEdges, PlaceEdges};
use crate::labels::{self, ChunkSummary, Collector, PassLabels};
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
    /// With [`ScanOptions::checkpoint_every`]: where the scan can resume,
    /// right after the records sent before it.
    Checkpoint(Box<Checkpoint>),
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
    /// Entities of the filter's `ids` whose `lastrevid` was no newer than
    /// the known revision given with them (skipped without parsing).
    pub unchanged: u64,
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
    /// MediaWiki XML exports (incremental dumps): the pages whose newest
    /// revision is a redirect, i.e. items merged that day.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirects: Option<Vec<Redirect>>,
    /// With `ids` in the filter and a redirect index, after a complete
    /// scan: the missing ids sorted into `merged`, `deleted` and `newer`.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub misses: Option<Misses>,
    /// The class index saved with [`ScanOptions::class_index`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_index: Option<SavedClassIndex>,
    /// The place index saved with [`ScanOptions::place_index`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place_index: Option<SavedPlaceIndex>,
    /// How the referenced labels were collected
    /// ([`crate::EntityOptions::referenced_labels`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<labels::LabelStats>,
    /// The labels of the items the matches refer to, after a complete
    /// pass with [`crate::EntityOptions::referenced_labels`].
    #[serde(skip)]
    pub referenced_labels: Option<labels::ReferencedLabels>,
}

/// A place index built during a scan.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPlaceIndex {
    /// Where it was saved.
    pub path: String,
    /// Items with a country.
    pub items: u64,
    /// Distinct sets of countries.
    pub sets: u64,
    /// Its size on disk.
    pub bytes: u64,
}

/// An item redirected to another (merged into it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Redirect {
    /// The redirected item.
    pub from: String,
    /// The item it redirects to.
    pub to: String,
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
    /// Also build the place index, as [`crate::build_place_index`] does,
    /// and save it to this file when the scan ends without being stopped.
    pub place_index: Option<PathBuf>,
    /// Send an [`Event::Checkpoint`] after every this many chunks (a
    /// chunk is a bzip2 block, ~900 kB of JSON, or 4 MB of plain JSON;
    /// gzip and zstd inputs have none).
    pub checkpoint_every: Option<u64>,
    /// Continue a scan from a checkpoint (same input, same filter): the
    /// records after it, with counters and missing ids carried over.
    pub resume: Option<Arc<Checkpoint>>,
}

/// Per-chunk counters, made on the workers.
#[derive(Default)]
pub(crate) struct ChunkCounts {
    pub entities: u64,
    pub matched: u64,
    pub parsed: u64,
    pub unchanged: u64,
    pub types: TypeCounts,
    pub max_ids: [u64; 5],
    pub largest: (usize, String),
    /// Keys of the filter's `ids` seen in the chunk.
    pub seen: Vec<i64>,
    /// Subclass-of edges, with `class_index`.
    pub edges: Vec<(u32, u32)>,
    /// Place edges, with `place_index`.
    pub place_edges: Vec<(u32, u32)>,
    /// With referenced labels: the items the records refer to, the labels
    /// taken, and the ranges of item numbers in the chunk.
    pub referenced: Vec<u32>,
    pub labels: Vec<(u32, labels::Labels)>,
    pub buckets: Vec<u32>,
    /// Where reading resumes after the chunk.
    pub resume: Option<ResumePoint>,
}

/// What a worker makes of a matching line: `None` to drop it.
pub(crate) type Work<T> = dyn Fn(&[u8], &Peek<'_>, &mut ChunkCounts) -> Result<Option<T>, String> + Send + Sync;

pub(crate) struct ScanResult {
    pub stats: ScanStats,
    pub max_ids: [u64; 5],
    pub largest: (usize, String),
    /// With a label collector.
    pub labels: Option<PassLabels>,
}

/// What a worker does besides filtering and `work`.
struct Side<'a> {
    track_largest: bool,
    /// The filter whose `ids` are tracked.
    track_ids: Option<&'a CompiledFilter>,
    classes: Option<&'a ClassEdges>,
    places: Option<&'a PlaceEdges>,
    labels: Option<&'a Collector>,
}

fn process<T>(
    chunk: DumpResult<Chunk>,
    filter: Option<&CompiledFilter>,
    work: &Work<T>,
    side: &Side<'_>,
) -> Result<(Vec<T>, ChunkCounts), ExportError> {
    let chunk = chunk?;
    let mut out = Vec::new();
    let mut c = ChunkCounts {
        resume: chunk.resume,
        ..Default::default()
    };
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
            if let Some(k) = id_key(p.id) {
                if f.requests(k) {
                    c.seen.push(k);
                    if f.unchanged(k, line) {
                        c.unchanged += 1;
                    }
                } else if f.is_target(k) {
                    c.seen.push(k);
                }
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
        if let Some(places) = side.places {
            places
                .read(line, &p, &mut c.place_edges)
                .map_err(|e| ExportError::Invalid(format!("{}: {e}", p.id)))?;
        }
        if let Some(labels) = side.labels {
            if p.kind == EntityType::Item {
                if let Some(q) = id_number(p.id).and_then(|n| u32::try_from(n).ok()) {
                    let b = labels::bucket(q);
                    if c.buckets.last() != Some(&b) {
                        c.buckets.push(b);
                    }
                }
            }
            if let Some(l) = labels
                .capture(line, &p)
                .map_err(|e| ExportError::Invalid(format!("{}: {e}", p.id)))?
            {
                c.labels.push(l);
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
    scan_with(dump, opts, prefilter, work, track_largest, None, sink)
}

/// [`scan`], with a label collector (see [`crate::labels`]).
pub(crate) fn scan_with<T: Send + 'static>(
    dump: &Dump,
    opts: &ScanOptions,
    prefilter: bool,
    work: Arc<Work<T>>,
    track_largest: bool,
    collector: Option<Arc<Collector>>,
    sink: &mut dyn FnMut(Event<T>) -> Flow,
) -> Result<ScanResult, ExportError> {
    let start = Instant::now();
    let threads = resolve_threads(opts.threads);
    let resume = opts.resume.as_deref();
    if let Some(cp) = resume {
        check_resume(dump, opts, cp)?;
    }
    let chunks = dump.chunks(&ReadOptions {
        threads,
        allow_truncated: opts.allow_truncated,
        resume: resume.map(|cp| cp.position),
    })?;
    let read = chunks.stats();
    let base_in = resume.map_or(0, |cp| cp.position.bit / 8);
    let base_out = resume.map_or(0, |cp| cp.position.offset);
    let total = chunks.input_len().or(dump.len());
    let filter = opts.filter.clone();
    let track_ids = filter.clone().filter(|f| f.has_ids());
    let classes = opts.class_index.as_ref().map(|_| Arc::new(ClassEdges::new()));
    let worker_classes = classes.clone();
    let places = opts.place_index.as_ref().map(|_| Arc::new(PlaceEdges::new()));
    let worker_labels = collector.clone();
    let worker_track = track_ids.clone();
    let results = ordered_map(chunks, threads, threads * 2, move |c| {
        let side = Side {
            track_largest,
            track_ids: worker_track.as_deref(),
            classes: worker_classes.as_deref(),
            places: places.as_deref(),
            labels: worker_labels.as_deref(),
        };
        process(c, filter.as_deref().filter(|_| prefilter), &*work, &side)
    });
    // Keys of the filter's `ids` seen (each once, unless a dump repeats
    // an entity).
    let mut seen: Vec<i64> = resume.map(|cp| cp.seen.clone()).unwrap_or_default();
    let mut edges = Vec::new();
    let mut place_edges = Vec::new();
    let mut pass_labels = collector.as_ref().map(|_| PassLabels {
        seekable: true,
        ..Default::default()
    });
    // Where the next chunk starts.
    let mut chunk_start: Option<ResumePoint> = resume.map(|cp| cp.position);

    let mut stats = ScanStats {
        compression: dump.compression().as_str(),
        ..Default::default()
    };
    let mut max_ids = [0u64; 5];
    if let Some(cp) = resume {
        stats.entities = cp.entities;
        stats.matched = cp.matched;
        stats.parsed = cp.parsed;
        stats.unchanged = cp.unchanged;
        stats.types = cp.types;
        max_ids = cp.max_ids;
    }
    let mut largest = (0usize, String::new());
    let mut last = Instant::now();
    let limit = opts.limit.unwrap_or(u64::MAX);
    let every = opts.checkpoint_every.filter(|&n| n > 0);
    let mut since_checkpoint = 0u64;
    let progress = |stats: &ScanStats| Progress {
        bytes_in: base_in + read.bytes_in.get(),
        bytes_total: total,
        bytes_out: base_out + read.bytes_out.load(Ordering::Relaxed),
        entities: stats.entities,
        matched: stats.matched,
        elapsed_ms: start.elapsed().as_millis() as u64,
    };
    for r in results {
        let (mut records, c) = r?;
        seen.extend_from_slice(&c.seen);
        edges.extend(c.edges);
        place_edges.extend(c.place_edges);
        if let Some(l) = &mut pass_labels {
            l.captured.extend(c.labels);
            l.referenced.extend(c.referenced);
            l.seekable &= chunk_start.is_some() || l.summaries.is_empty();
            l.summaries.push(ChunkSummary {
                start: chunk_start,
                buckets: c.buckets,
            });
        }
        chunk_start = c.resume;
        stats.entities += c.entities;
        stats.parsed += c.parsed;
        stats.unchanged += c.unchanged;
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
        if let (Some(every), Some(position)) = (every, c.resume) {
            since_checkpoint += 1;
            if since_checkpoint >= every {
                since_checkpoint = 0;
                seen.sort_unstable();
                seen.dedup();
                let cp = Checkpoint {
                    position,
                    input: dump.input().file_name(),
                    size: dump.len(),
                    filter: opts.filter.as_ref().map_or(0, |f| f.fingerprint()),
                    entities: stats.entities,
                    matched: stats.matched,
                    parsed: stats.parsed,
                    unchanged: stats.unchanged,
                    types: stats.types,
                    max_ids,
                    seen: seen.clone(),
                };
                if sink(Event::Checkpoint(Box::new(cp))) == Flow::Stop {
                    stats.stopped = true;
                    break;
                }
            }
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
    stats.bytes_in = base_in + read.bytes_in.get();
    stats.bytes_out = base_out + read.bytes_out.load(Ordering::Relaxed);
    stats.blocks = read.blocks.load(Ordering::Relaxed);
    stats.streams = read.streams.load(Ordering::Relaxed);
    stats.truncated = read.truncated.load(Ordering::Relaxed);
    if read.xml.load(Ordering::Relaxed) {
        let pairs = std::mem::take(&mut *read.redirects.lock().unwrap());
        stats.redirects = Some(pairs.into_iter().map(|(from, to)| Redirect { from, to }).collect());
    }
    if let Some(f) = track_ids.filter(|_| !stats.stopped && !stats.truncated) {
        let b = IdSetBuilder::new();
        seen.sort_unstable();
        b.insert_all(&seen);
        let seen = b.freeze();
        stats.missing_ids = Some(f.missing_ids(&seen));
        stats.misses = f.misses(&seen, &max_ids);
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
    if let (Some(path), false) = (&opts.place_index, stats.stopped) {
        let index = PlaceIndex::from_edges(place_edges, dump.input().file_name());
        let bytes = index.to_bytes();
        write_atomically(path, &bytes)?;
        stats.place_index = Some(SavedPlaceIndex {
            path: path.display().to_string(),
            items: index.len() as u64,
            sets: index.sets() as u64,
            bytes: bytes.len() as u64,
        });
    }
    stats.elapsed_ms = start.elapsed().as_millis() as u64;
    Ok(ScanResult {
        stats,
        max_ids,
        largest,
        labels: pass_labels,
    })
}

/// Whether `cp` can resume a scan of `dump` with `opts`.
fn check_resume(dump: &Dump, opts: &ScanOptions, cp: &Checkpoint) -> Result<(), ExportError> {
    let bad = |m: String| Err(ExportError::Invalid(format!("cannot resume from this checkpoint: {m}")));
    let name = dump.input().file_name();
    if cp.input != name {
        return bad(format!("it was made on {:?}, not {name:?}", cp.input));
    }
    if let (Some(a), Some(b)) = (cp.size, dump.len()) {
        if a != b {
            return bad(format!("the input's size changed ({a} → {b} bytes)"));
        }
    }
    if cp.filter != opts.filter.as_ref().map_or(0, |f| f.fingerprint()) {
        return bad("it was made with another filter".into());
    }
    if opts.class_index.is_some() || opts.place_index.is_some() {
        return bad("an index cannot be built during a resumed pass".into());
    }
    Ok(())
}

/// Save through a temporary file beside `path`, so that `path` either does
/// not exist or holds a whole index.
fn save_atomically(index: &ClassIndex, path: &std::path::Path) -> Result<(), ExportError> {
    write_atomically(path, &index.to_bytes())
}

/// Write `bytes` through a temporary file beside `path`.
pub(crate) fn write_atomically(path: &std::path::Path, bytes: &[u8]) -> Result<(), ExportError> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(|e| FilterError::Io(format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        FilterError::Io(format!("{}: {e}", path.display()))
    })?;
    Ok(())
}
