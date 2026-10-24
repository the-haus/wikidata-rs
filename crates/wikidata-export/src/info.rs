//! What a dump holds.

use std::sync::Arc;

use serde::Serialize;
use wikidata_dump::Dump;

use crate::scan::{scan, Flow, Progress, ScanOptions, ScanStats, Work};
use crate::ExportError;

/// The largest entity of a dump.
#[derive(Clone, Debug, Default, Serialize)]
pub struct LargestEntity {
    /// Its id.
    pub id: String,
    /// Its JSON's size in bytes.
    pub bytes: u64,
}

/// What [`file_info`] found.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    /// The path or URL.
    pub input: String,
    /// Its size in bytes, when known.
    pub size: Option<u64>,
    /// Counters (entity counts by type, bytes, time).
    #[serde(flatten)]
    pub scan: ScanStats,
    /// The highest id number of each type (`Q`, `P`, `L`, `M`, `E`).
    pub max_ids: MaxIds,
    /// The largest entity.
    pub largest: LargestEntity,
}

/// Highest id numbers by type.
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaxIds {
    /// Items (`Q`).
    pub item: u64,
    /// Properties (`P`).
    pub property: u64,
    /// Lexemes (`L`).
    pub lexeme: u64,
    /// Media info (`M`).
    pub mediainfo: u64,
    /// Entity schemas (`E`).
    pub entity_schema: u64,
}

/// Read the whole dump (or `opts.limit` entities, which `opts.filter`
/// selects) and count what it holds. Only the first bytes of each line
/// are looked at.
pub fn file_info(
    dump: &Dump,
    opts: &ScanOptions,
    progress: &mut dyn FnMut(Progress) -> Flow,
) -> Result<FileInfo, ExportError> {
    let work: Arc<Work<()>> = Arc::new(|_, _, _| Ok(Some(())));
    let mut sink = |e: crate::Event<()>| match e {
        crate::Event::Progress(p) => progress(p),
        crate::Event::Records(_) | crate::Event::Checkpoint(_) => Flow::Continue,
    };
    let r = scan(dump, opts, true, work, true, &mut sink)?;
    let m = r.max_ids;
    Ok(FileInfo {
        input: dump.input().display(),
        size: dump.len(),
        scan: r.stats,
        max_ids: MaxIds {
            item: m[0],
            property: m[1],
            lexeme: m[2],
            mediainfo: m[3],
            entity_schema: m[4],
        },
        largest: LargestEntity {
            id: r.largest.1,
            bytes: r.largest.0 as u64,
        },
    })
}
