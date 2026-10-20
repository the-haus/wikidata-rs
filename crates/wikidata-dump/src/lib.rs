#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

pub mod bz2;
pub mod bzdecode;
pub mod lines;
pub mod parallel;
pub mod source;
pub mod write;

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

pub use lines::{trim_line, Chunk, Lines, Splitter};
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
                // Opening reads the first bytes (and the size) from the server.
                let mut s = Source::open(&input, &http)?;
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
        let source = Source::open(&self.input, &self.http)?;
        Ok(Chunks::new(source, self.compression, opts))
    }
}

/// Reads fixed-size chunks from a reader.
struct ReadChunks<R> {
    reader: R,
    size: usize,
    done: bool,
}

impl<R: Read> Iterator for ReadChunks<R> {
    type Item = Result<Vec<u8>>;

    fn next(&mut self) -> Option<Result<Vec<u8>>> {
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
            Ok(_) => Some(Ok(buf)),
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
        let len = source.len();
        let allow = opts.allow_truncated;
        let inner: Box<dyn Iterator<Item = Result<Chunk>> + Send> = match compression {
            Compression::Bzip2 => {
                let threads = resolve_threads(opts.threads);
                let blocks: Box<dyn Iterator<Item = Result<Vec<u8>>> + Send> = match bz2::Scanner::new(source) {
                    Ok(scanner) => {
                        let decoded = ordered_map(scanner, threads, threads * 2, |p: Result<bz2::Piece>| {
                            p.map(bz2::decode_piece)
                        });
                        Box::new(CountedBlocks {
                            blocks: bz2::Blocks::new(decoded),
                            stats: stats.clone(),
                        })
                    }
                    Err(e) => Box::new(std::iter::once(Err(e))),
                };
                Box::new(Splitter::new(blocks, stats.clone(), allow))
            }
            Compression::Gzip => {
                let r = flate2::read::MultiGzDecoder::new(std::io::BufReader::with_capacity(1 << 20, source));
                Box::new(Splitter::new(read_chunks(r), stats.clone(), allow))
            }
            Compression::Zstd => match zstd::stream::read::Decoder::new(source) {
                Ok(r) => Box::new(Splitter::new(read_chunks(r), stats.clone(), allow)),
                Err(e) => Box::new(std::iter::once(Err(e.into()))),
            },
            Compression::None => Box::new(Splitter::new(read_chunks(source), stats.clone(), allow)),
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

fn read_chunks<R: Read + Send + 'static>(r: R) -> ReadChunks<R> {
    ReadChunks {
        reader: r,
        size: 4 << 20,
        done: false,
    }
}

/// Copies bzip2 counters to the shared stats.
struct CountedBlocks<I> {
    blocks: bz2::Blocks<I>,
    stats: Arc<ReadStats>,
}

impl<I: Iterator<Item = Result<bz2::Decoded>>> Iterator for CountedBlocks<I> {
    type Item = Result<Vec<u8>>;
    fn next(&mut self) -> Option<Result<Vec<u8>>> {
        let r = self.blocks.next();
        let s = self.blocks.stats;
        self.stats.streams.store(s.streams, Ordering::Relaxed);
        self.stats.blocks.store(s.blocks, Ordering::Relaxed);
        self.stats.false_magics.store(s.false_magics, Ordering::Relaxed);
        r
    }
}

impl Iterator for Chunks {
    type Item = Result<Chunk>;
    fn next(&mut self) -> Option<Result<Chunk>> {
        self.inner.next()
    }
}
