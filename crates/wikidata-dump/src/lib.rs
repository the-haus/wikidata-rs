#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

pub mod bz2;
pub mod bzdecode;
pub mod download;
pub mod lines;
pub mod multi;
pub mod parallel;
pub mod source;
pub mod sql;
pub mod sse;
pub mod write;
pub mod xml;

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

pub use lines::{trim_line, Block, BlockStart, Chunk, Lines, ResumePoint, Splitter};
pub use parallel::{default_threads, live_threads, ordered_map, resolve_threads, OrderedMap};
pub use source::{Counter, HttpOptions, Input, Source, USER_AGENT};
pub use write::{DumpWriter, WriteOptions};

/// Errors reading or writing dumps.
#[derive(Debug, thiserror::Error)]
pub enum DumpError {
    /// File system or stream error.
    #[error("{0}")]
    Io(String),
    /// HTTP error.
    #[error("HTTP: {0}")]
    Http(String),
    /// Not a dump, or corrupt.
    #[error("{0}")]
    Format(String),
    /// The input ended early.
    #[error("truncated input: {0}")]
    Truncated(String),
}

impl From<std::io::Error> for DumpError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            DumpError::Truncated(e.to_string())
        } else {
            DumpError::Io(e.to_string())
        }
    }
}

/// Result alias.
pub type Result<T> = std::result::Result<T, DumpError>;

/// How a dump is compressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    /// bzip2 (`.bz2`), decoded in parallel.
    Bzip2,
    /// gzip (`.gz`).
    Gzip,
    /// Zstandard (`.zst`).
    Zstd,
    /// Plain JSON.
    None,
}

impl Compression {
    /// The compression of a file that starts with `head`.
    pub fn sniff(head: &[u8]) -> Compression {
        match head {
            [b'B', b'Z', b'h', _, ..] => Compression::Bzip2,
            [0x1f, 0x8b, ..] => Compression::Gzip,
            [0x28, 0xb5, 0x2f, 0xfd, ..] => Compression::Zstd,
            _ => Compression::None,
        }
    }

    /// `"bzip2"`, `"gzip"`, `"zstd"` or `"none"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Compression::Bzip2 => "bzip2",
            Compression::Gzip => "gzip",
            Compression::Zstd => "zstd",
            Compression::None => "none",
        }
    }
}

/// Counters of a running read, updated as it goes (shared with progress
/// reporting).
#[derive(Debug, Default)]
pub struct ReadStats {
    /// Bytes read from the file or URL.
    pub bytes_in: Counter,
    /// Decompressed bytes.
    pub bytes_out: AtomicU64,
    /// bzip2 streams.
    pub streams: AtomicU64,
    /// bzip2 blocks.
    pub blocks: AtomicU64,
    /// False bzip2 magic matches that were undone.
    pub false_magics: AtomicU64,
    /// The input was truncated (only with `allow_truncated`).
    pub truncated: AtomicBool,
    /// The input is a MediaWiki XML export (an incremental dump).
    pub xml: AtomicBool,
    /// XML exports: the pages whose newest revision is a redirect (a
    /// merge), as `(from, to)` ids.
    pub redirects: std::sync::Mutex<Vec<(String, String)>>,
}

/// Options for [`Dump::chunks`].
#[derive(Clone, Debug, Default)]
pub struct ReadOptions {
    /// Decoder threads for bzip2 (0: all cores).
    pub threads: usize,
    /// Treat a truncated input as ending at its last whole line (setting
    /// [`ReadStats::truncated`]) instead of failing; for partial
    /// downloads.
    pub allow_truncated: bool,
    /// Start where an earlier read left off ([`Chunk::resume`]) instead of
    /// at the beginning (bzip2 and plain JSON only).
    pub resume: Option<ResumePoint>,
}

/// A dump to read (see the crate docs).
#[derive(Clone, Debug)]
pub struct Dump {
    input: Input,
    http: HttpOptions,
    compression: Compression,
    len: Option<u64>,
}

impl Dump {
    /// Open a path or URL (see [`Input::parse`]).
    pub fn open(input: impl Into<Input>) -> Result<Dump> {
        Self::open_with(input, HttpOptions::default())
    }

    /// Open with HTTP options.
    pub fn open_with(input: impl Into<Input>, http: HttpOptions) -> Result<Dump> {
        let input = input.into();
        let (compression, len) = match &input {
            Input::Path(p) => {
                let mut f = std::fs::File::open(p).map_err(|e| DumpError::Io(format!("{}: {e}", p.display())))?;
                let len = f.metadata().ok().map(|m| m.len());
                let mut head = [0u8; 4];
                let n = source::read_full(&mut f, &mut head)?;
                (Compression::sniff(&head[..n]), len)
            }
            Input::Url(_) => {
                // Opening reads the first bytes (and the size) from the
                // server, on one connection.
                let one = HttpOptions {
                    mirrors: Vec::new(),
                    connections: None,
                    ..http.clone()
                };
                let mut s = Source::open(&input, &one)?;
                let len = s.len();
                let mut head = [0u8; 4];
                let n = source::read_full(&mut s, &mut head)?;
                (Compression::sniff(&head[..n]), len)
            }
        };
        Ok(Dump {
            input,
            http,
            compression,
            len,
        })
    }

    /// Where the dump is.
    pub fn input(&self) -> &Input {
        &self.input
    }

    /// Its compression (from its first bytes).
    pub fn compression(&self) -> Compression {
        self.compression
    }

    /// Its size in bytes, when known.
    pub fn len(&self) -> Option<u64> {
        self.len
    }

    /// Whether the dump is known to be empty.
    pub fn is_empty(&self) -> bool {
        self.len == Some(0)
    }

    /// Read the dump: its lines, in file order, in chunks of whole lines.
    pub fn chunks(&self, opts: &ReadOptions) -> Result<Chunks> {
        let from = match &opts.resume {
            None => 0,
            Some(r) => {
                if !matches!(self.compression, Compression::Bzip2 | Compression::None) {
                    return Err(DumpError::Format(format!(
                        "a {} dump cannot be resumed (only bzip2 and plain JSON can)",
                        self.compression.as_str()
                    )));
                }
                r.bit / 8
            }
        };
        let source = Source::open_at(&self.input, &self.http, from)?;
        Ok(Chunks::new(source, self.compression, opts))
    }
}

/// Reads fixed-size chunks from a reader.
struct ReadChunks<R> {
    reader: R,
    size: usize,
    done: bool,
    /// The input offset of the next chunk, when chunks can be resumed from
    /// (plain JSON: the reader is the input itself).
    offset: Option<u64>,
}

impl<R: Read> Iterator for ReadChunks<R> {
    type Item = Result<Block>;

    fn next(&mut self) -> Option<Result<Block>> {
        if self.done {
            return None;
        }
        // Read into fresh capacity: no zero-filling first.
        let mut buf = Vec::with_capacity(self.size);
        match (&mut self.reader).take(self.size as u64).read_to_end(&mut buf) {
            Ok(0) => {
                self.done = true;
                None
            }
            Ok(n) => {
                let start = self.offset.map(|o| BlockStart {
                    bit: o * 8,
                    plain: true,
                    ..Default::default()
                });
                if let Some(o) = &mut self.offset {
                    *o += n as u64;
                }
                Some(Ok(Block { data: buf, start }))
            }
            Err(e) => {
                self.done = true;
                Some(Err(e.into()))
            }
        }
    }
}

/// The chunks of a dump, in file order (see [`Dump::chunks`]).
pub struct Chunks {
    inner: Box<dyn Iterator<Item = Result<Chunk>> + Send>,
    stats: Arc<ReadStats>,
    len: Option<u64>,
}

impl Chunks {
    /// Read `source`, compressed with `compression`.
    pub fn new(source: Source, compression: Compression, opts: &ReadOptions) -> Chunks {
        let stats = Arc::new(ReadStats {
            bytes_in: source.counter(),
            ..Default::default()
        });
        // A resumed source starts at the resume point's byte.
        let start = opts.resume.map_or(0, |r| r.bit / 8);
        let len = source.len().map(|l| l + start);
        let allow = opts.allow_truncated;
        let resume = opts.resume;
        let split_stats = stats.clone();
        let split = move |blocks: Box<dyn Iterator<Item = Result<Block>> + Send>| {
            Splitter::resume(
                Sniff::new(blocks, split_stats.clone()),
                split_stats.clone(),
                allow,
                resume,
            )
        };
        let inner: Box<dyn Iterator<Item = Result<Chunk>> + Send> = match compression {
            Compression::Bzip2 => {
                let threads = resolve_threads(opts.threads);
                let scanner = match resume {
                    None => bz2::Scanner::new(source),
                    Some(r) => bz2::Scanner::resume(source, r.bit),
                };
                let blocks: Box<dyn Iterator<Item = Result<Block>> + Send> = match scanner {
                    Ok(scanner) => {
                        let decoded = ordered_map(scanner, threads, threads * 2, |p: Result<bz2::Piece>| {
                            p.map(bz2::decode_piece)
                        });
                        let mut blocks = bz2::Blocks::new(decoded);
                        if let Some(r) = resume {
                            blocks = blocks.resumed(r.crc, r.blocks, r.streams);
                        }
                        Box::new(CountedBlocks {
                            blocks,
                            stats: stats.clone(),
                        })
                    }
                    Err(e) => Box::new(std::iter::once(Err(e))),
                };
                Box::new(split(blocks))
            }
            Compression::Gzip => {
                let r = flate2::read::MultiGzDecoder::new(std::io::BufReader::with_capacity(1 << 20, source));
                Box::new(split(Box::new(read_chunks(r, None))))
            }
            Compression::Zstd => match zstd::stream::read::Decoder::new(source) {
                Ok(r) => Box::new(split(Box::new(read_chunks(r, None)))),
                Err(e) => Box::new(std::iter::once(Err(e.into()))),
            },
            Compression::None => Box::new(split(Box::new(read_chunks(source, Some(start))))),
        };
        Chunks { inner, stats, len }
    }

    /// Live counters.
    pub fn stats(&self) -> Arc<ReadStats> {
        self.stats.clone()
    }

    /// The size of the input in bytes, when known.
    pub fn input_len(&self) -> Option<u64> {
        self.len
    }
}

fn read_chunks<R: Read + Send + 'static>(r: R, offset: Option<u64>) -> ReadChunks<R> {
    ReadChunks {
        reader: r,
        size: 4 << 20,
        done: false,
        offset,
    }
}

/// Blocks as they are, or, for a MediaWiki XML export, its entity lines
/// ([`xml::XmlEntities`]).
enum Sniff {
    Start(Box<dyn Iterator<Item = Result<Block>> + Send>, Arc<ReadStats>),
    Plain(Box<dyn Iterator<Item = Result<Block>> + Send>),
    Xml(Box<xml::XmlEntities<Box<dyn Iterator<Item = Result<Block>> + Send>>>),
    Done,
}

impl Sniff {
    fn new(blocks: Box<dyn Iterator<Item = Result<Block>> + Send>, stats: Arc<ReadStats>) -> Sniff {
        Sniff::Start(blocks, stats)
    }
}

impl Iterator for Sniff {
    type Item = Result<Block>;
    fn next(&mut self) -> Option<Result<Block>> {
        loop {
            match self {
                Sniff::Plain(b) => return b.next(),
                Sniff::Xml(x) => return x.next(),
                Sniff::Done => return None,
                Sniff::Start(..) => {
                    let Sniff::Start(mut blocks, stats) = std::mem::replace(self, Sniff::Done) else {
                        unreachable!()
                    };
                    let first = match blocks.next() {
                        Some(Ok(b)) => b,
                        other => return other,
                    };
                    if xml::is_xml(&first.data) {
                        let again: Box<dyn Iterator<Item = Result<Block>> + Send> =
                            Box::new(std::iter::once(Ok(first)).chain(blocks));
                        *self = Sniff::Xml(Box::new(xml::XmlEntities::new(again, stats)));
                    } else {
                        let again = std::iter::once(Ok(first)).chain(blocks);
                        *self = Sniff::Plain(Box::new(again));
                    }
                }
            }
        }
    }
}

/// Copies bzip2 counters to the shared stats.
struct CountedBlocks<I> {
    blocks: bz2::Blocks<I>,
    stats: Arc<ReadStats>,
}

impl<I: Iterator<Item = Result<bz2::Decoded>>> Iterator for CountedBlocks<I> {
    type Item = Result<Block>;
    fn next(&mut self) -> Option<Result<Block>> {
        let r = self.blocks.next();
        let s = self.blocks.stats;
        self.stats.streams.store(s.streams, Ordering::Relaxed);
        self.stats.blocks.store(s.blocks, Ordering::Relaxed);
        self.stats.false_magics.store(s.false_magics, Ordering::Relaxed);
        let start = self.blocks.last_start;
        Some(r?.map(|data| Block { data, start }))
    }
}

impl Iterator for Chunks {
    type Item = Result<Chunk>;
    fn next(&mut self) -> Option<Result<Chunk>> {
        self.inner.next()
    }
}
