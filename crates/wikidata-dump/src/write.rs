//! Writing dumps: plain, or compressed on every core.
//!
//! [`DumpWriter`] cuts its input into chunks and compresses each one on a
//! worker as an independent bzip2 stream, gzip member or zstd frame, then
//! writes them in order. Concatenated streams, members and frames are
//! valid files for every common tool (`bzip2 -d`, `gzip -d`, `zstd -d`,
//! Python's `bz2`, …) and for [`crate::Dump`], which reads multi-stream
//! bzip2 in parallel as well.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::thread::JoinHandle;

use crossbeam_channel::{bounded, Sender};

use crate::parallel::{ordered_map, resolve_threads};
use crate::{Compression, DumpError, Result};

/// Options for [`DumpWriter`].
#[derive(Clone, Debug)]
pub struct WriteOptions {
    /// The compression (default: from the file name).
    pub compression: Compression,
    /// Compression level (default: bzip2 9, gzip 6, zstd 9).
    pub level: Option<u32>,
    /// Worker threads (0: all cores).
    pub threads: usize,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            compression: Compression::None,
            level: None,
            threads: 0,
        }
    }
}

impl Compression {
    /// The compression a file name asks for: `.bz2`, `.gz`, `.zst`, else none.
    pub fn from_name(name: &str) -> Compression {
        let n = name.to_ascii_lowercase();
        if n.ends_with(".bz2") {
            Compression::Bzip2
        } else if n.ends_with(".gz") {
            Compression::Gzip
        } else if n.ends_with(".zst") || n.ends_with(".zstd") {
            Compression::Zstd
        } else {
            Compression::None
        }
    }

    fn chunk_size(self) -> usize {
        match self {
            // One 900 kB block per stream (level 9).
            Compression::Bzip2 => 899_000,
            Compression::Gzip => 4 << 20,
            Compression::Zstd => 8 << 20,
            Compression::None => 1 << 20,
        }
    }
}

fn compress(c: Compression, level: Option<u32>, data: &[u8]) -> io::Result<Vec<u8>> {
    match c {
        Compression::Bzip2 => {
            let mut e = bzip2::write::BzEncoder::new(
                Vec::with_capacity(data.len() / 6),
                bzip2::Compression::new(level.unwrap_or(9).clamp(1, 9)),
            );
            e.write_all(data)?;
            e.finish()
        }
        Compression::Gzip => {
            let mut e = flate2::write::GzEncoder::new(
                Vec::with_capacity(data.len() / 5),
                flate2::Compression::new(level.unwrap_or(6).min(9)),
            );
            e.write_all(data)?;
            e.finish()
        }
        Compression::Zstd => zstd::bulk::compress(data, level.unwrap_or(9).clamp(1, 22) as i32),
        Compression::None => Ok(data.to_vec()),
    }
}

/// A file writer that compresses in parallel (see the module docs).
pub struct DumpWriter {
    buf: Vec<u8>,
    chunk: usize,
    compression: Compression,
    tx: Option<Sender<Vec<u8>>>,
    writer: Option<JoinHandle<io::Result<u64>>>,
    plain: Option<BufWriter<File>>,
    written_in: u64,
}

impl DumpWriter {
    /// Create `path`; the compression comes from `opts` (or, when that
    /// is `None`, from the file name).
    pub fn create(path: impl AsRef<Path>, opts: &WriteOptions) -> Result<DumpWriter> {
        let path = path.as_ref();
        let compression = match opts.compression {
            Compression::None => Compression::from_name(&path.to_string_lossy()),
            c => c,
        };
        let file = File::create(path).map_err(|e| DumpError::Io(format!("{}: {e}", path.display())))?;
        Self::new(file, compression, opts)
    }

    /// Write to `file` with `compression`.
    pub fn new(file: File, compression: Compression, opts: &WriteOptions) -> Result<DumpWriter> {
        if compression == Compression::None {
            return Ok(DumpWriter {
                buf: Vec::new(),
                chunk: 0,
                compression,
                tx: None,
                writer: None,
                plain: Some(BufWriter::with_capacity(1 << 20, file)),
                written_in: 0,
            });
        }
        let threads = resolve_threads(opts.threads);
        let (tx, rx) = bounded::<Vec<u8>>(threads);
        let level = opts.level;
        let results = ordered_map(rx, threads, threads * 2, move |data: Vec<u8>| {
            compress(compression, level, &data)
        });
        let writer = std::thread::Builder::new()
            .name("wikidata-writer".into())
            .spawn(move || -> io::Result<u64> {
                let mut out = BufWriter::with_capacity(1 << 20, file);
                let mut n = 0u64;
                for r in results {
                    let bytes = r?;
                    out.write_all(&bytes)?;
                    n += bytes.len() as u64;
                }
                out.flush()?;
                Ok(n)
            })
            .map_err(|e| DumpError::Io(e.to_string()))?;
        Ok(DumpWriter {
            buf: Vec::with_capacity(compression.chunk_size()),
            chunk: compression.chunk_size(),
            compression,
            tx: Some(tx),
            writer: Some(writer),
            plain: None,
            written_in: 0,
        })
    }

    /// The compression used.
    pub fn compression(&self) -> Compression {
        self.compression
    }

    fn send(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let data = std::mem::replace(&mut self.buf, Vec::with_capacity(self.chunk));
        match &self.tx {
            Some(tx) => tx
                .send(data)
                .map_err(|_| io::Error::other("the compression workers stopped")),
            None => Ok(()),
        }
    }

    /// Flush everything and close the file; returns the bytes written to it.
    pub fn finish(mut self) -> Result<u64> {
        if let Some(mut p) = self.plain.take() {
            p.flush()?;
            return Ok(self.written_in);
        }
        self.send()?;
        self.tx.take();
        match self.writer.take() {
            Some(h) => Ok(h.join().map_err(|_| DumpError::Io("writer thread panicked".into()))??),
            None => Ok(0),
        }
    }
}

impl Write for DumpWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.written_in += data.len() as u64;
        if let Some(p) = &mut self.plain {
            p.write_all(data)?;
            return Ok(data.len());
        }
        let mut rest = data;
        while !rest.is_empty() {
            let room = self.chunk - self.buf.len();
            let n = room.min(rest.len());
            self.buf.extend_from_slice(&rest[..n]);
            rest = &rest[n..];
            if self.buf.len() >= self.chunk {
                self.send()?;
            }
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Chunks are compressed whole; data reaches the file on `finish`.
        Ok(())
    }
}

impl Drop for DumpWriter {
    fn drop(&mut self) {
        // Dropped without `finish` (an error elsewhere): stop the threads.
        self.tx.take();
        if let Some(h) = self.writer.take() {
            let _ = h.join();
        }
    }
}
