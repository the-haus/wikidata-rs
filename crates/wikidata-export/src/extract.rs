//! Writing the matching entities to a smaller dump.
//!
//! The original lines are copied unchanged, so an extract can be read
//! back with any options (and by any tool that reads Wikidata dumps). Two
//! layouts:
//!
//! - **JSON array**, Wikimedia's own (`[`, one entity per line with a
//!   trailing comma but the last, `]`): for `.json`, `.json.bz2`,
//!   `.json.gz`, `.json.zst`;
//! - **JSON lines** (one entity per line): for `.jsonl` / `.ndjson`
//!   with any compression.
//!
//! The recommended form for a subset kept on disk is `.jsonl.zst`:
//! Zstandard compresses on every core here and decompresses at ~1 GB/s
//! on one, so reading the subset back is limited by parsing, not by
//! decompression (bzip2 reads back at ~70 MB/s a core).

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use wikidata_dump::{Compression, Dump, DumpWriter, WriteOptions};

use crate::scan::{scan, Event, Flow, Progress, ScanOptions, ScanStats, Work};
use crate::ExportError;

/// The layout of an extract (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputFormat {
    /// `[`, lines with commas, `]`.
    JsonArray,
    /// One entity per line.
    JsonLines,
}

impl OutputFormat {
    /// The layout a file name asks for.
    pub fn from_name(name: &str) -> OutputFormat {
        let n = name.to_ascii_lowercase();
        if n.contains(".jsonl") || n.contains(".ndjson") {
            OutputFormat::JsonLines
        } else {
            OutputFormat::JsonArray
        }
    }
}

/// Options for [`extract`].
#[derive(Clone, Debug, Default)]
pub struct ExtractOptions {
    /// Filter, threads, limit.
    pub scan: ScanOptions,
    /// Compression (default: from the file name) and level.
    pub write: WriteOptions,
    /// Layout (default: from the file name).
    pub format: Option<OutputFormat>,
}

/// What [`extract`] did.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractStats {
    /// The scan.
    #[serde(flatten)]
    pub scan: ScanStats,
    /// The output file.
    pub output: String,
    /// Its layout.
    pub format: OutputFormat,
    /// Its compression.
    pub output_compression: &'static str,
    /// Bytes written to it.
    pub bytes_written: u64,
}

/// Stream the original lines of the matching entities (without the
/// trailing comma) to `sink`, in file order.
pub fn raw_lines(
    dump: &Dump,
    opts: &ScanOptions,
    sink: &mut dyn FnMut(Event<Vec<u8>>) -> Flow,
) -> Result<ScanStats, ExportError> {
    let work: Arc<Work<Vec<u8>>> = Arc::new(|line: &[u8], _, _| Ok(Some(line.to_vec())));
    Ok(scan(dump, opts, true, work, false, sink)?.stats)
}

/// Write the entities matching `opts.scan.filter` to `output`.
pub fn extract(
    dump: &Dump,
    opts: &ExtractOptions,
    output: impl AsRef<Path>,
    progress: &mut dyn FnMut(Progress) -> Flow,
) -> Result<ExtractStats, ExportError> {
    let output = output.as_ref();
    let name = output.to_string_lossy().into_owned();
    let format = opts.format.unwrap_or_else(|| OutputFormat::from_name(&name));
    let mut wopts = opts.write.clone();
    if wopts.compression == Compression::None {
        wopts.compression = Compression::from_name(&name);
    }
    if wopts.threads == 0 {
        wopts.threads = opts.scan.threads;
    }
    let mut w = DumpWriter::create(output, &wopts)?;
    let compression = w.compression();
    let mut first = true;
    let mut io_error: Option<std::io::Error> = None;
    if format == OutputFormat::JsonArray {
        w.write_all(b"[\n")?;
    }
    let mut sink = |e: Event<Vec<u8>>| -> Flow {
        match e {
            Event::Progress(p) => progress(p),
            Event::Checkpoint(_) => Flow::Continue,
            Event::Records(lines) => {
                for l in lines {
                    let r = match format {
                        OutputFormat::JsonLines => w.write_all(&l).and_then(|_| w.write_all(b"\n")),
                        OutputFormat::JsonArray => {
                            let sep: &[u8] = if first { b"" } else { b",\n" };
                            w.write_all(sep).and_then(|_| w.write_all(&l))
                        }
                    };
                    first = false;
                    if let Err(e) = r {
                        io_error = Some(e);
                        return Flow::Stop;
                    }
                }
                Flow::Continue
            }
        }
    };
    let stats = raw_lines(dump, &opts.scan, &mut sink)?;
    if let Some(e) = io_error {
        return Err(e.into());
    }
    if format == OutputFormat::JsonArray {
        w.write_all(if first { b"]\n" } else { b"\n]\n" })?;
    }
    let bytes_written = w.finish()?;
    Ok(ExtractStats {
        scan: stats,
        output: name,
        format,
        output_compression: compression.as_str(),
        bytes_written,
    })
}
