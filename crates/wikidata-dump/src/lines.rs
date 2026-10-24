//! Cutting decompressed bytes into chunks of whole lines.
//!
//! A dump is a JSON array with one entity per line:
//!
//! ```text
//! [
//! {"type":"item","id":"Q31",…},
//! {"type":"item","id":"Q8",…},
//! …
//! {"type":"item","id":"Q…",…}
//! ]
//! ```
//!
//! Every entity line but the last ends with a comma. JSON lines (one
//! entity per line, no array, no commas) are read too.
//!
//! [`Splitter`] takes decompressed blocks in file order and hands out
//! [`Chunk`]s that hold only whole lines: the line that straddles two
//! blocks is stitched into the next chunk's `prefix`, so a block is never
//! copied, only the bytes of one line. Chunks are then processed on any
//! thread. The splitter also checks the framing: an array dump must end
//! with `]`, or it was truncated.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::{DumpError, ReadStats, Result};

/// A decompressed block, and where it starts in the input when reading
/// can resume there.
#[derive(Debug, Default)]
pub struct Block {
    /// The bytes.
    pub data: Vec<u8>,
    /// Where the block starts (bzip2 and plain JSON).
    pub start: Option<BlockStart>,
}

impl From<Vec<u8>> for Block {
    fn from(data: Vec<u8>) -> Block {
        Block { data, start: None }
    }
}

/// Where a decompressed block starts in the input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlockStart {
    /// Its first bit (bzip2: the block magic; plain JSON: the byte × 8).
    pub bit: u64,
    /// bzip2: the combined CRC of the stream's blocks before it.
    pub crc: u32,
    /// bzip2: the blocks before it.
    pub blocks: u64,
    /// bzip2: the streams that ended before it.
    pub streams: u64,
    /// Plain JSON: the block is the input itself (its bytes are input
    /// bytes).
    pub plain: bool,
}

/// Where a read can start again ([`crate::ReadOptions::resume`]): at the
/// block where the first unread line starts, skipping the bytes of the
/// block that were read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResumePoint {
    /// The block's first bit in the input (plain JSON: byte × 8).
    pub bit: u64,
    /// bzip2: the stream's combined CRC before the block.
    pub crc: u32,
    /// bzip2: blocks before it.
    pub blocks: u64,
    /// bzip2: streams before it.
    pub streams: u64,
    /// Bytes of the decompressed block already read.
    pub skip: u64,
    /// Offset of the first unread byte in the decompressed dump.
    pub offset: u64,
    /// Chunks before it.
    pub chunks: u64,
    /// The dump is a JSON array (not JSON lines).
    pub array: bool,
    /// The array's closing `]` was read before the resume point.
    pub closed: bool,
    /// Plain JSON: `skip` counts input bytes too.
    pub plain: bool,
}

impl ResumePoint {
    /// The byte of the input reading resumes at: the bzip2 block's first
    /// byte, or, for plain JSON, the first unread byte itself.
    pub fn input_offset(&self) -> u64 {
        self.bit / 8 + if self.plain { self.skip } else { 0 }
    }
}

/// Whole lines of a dump (see the module docs).
#[derive(Debug, Default)]
pub struct Chunk {
    /// The line stitched from the end of the previous block and the start
    /// of this one (and any lines of blocks with no line end), or empty.
    pub prefix: Vec<u8>,
    /// A decompressed block.
    pub data: Vec<u8>,
    /// The whole lines of `data`: `data[start..end]`.
    pub start: usize,
    /// End of the whole lines of `data`.
    pub end: usize,
    /// Position in the chunk sequence.
    pub index: u64,
    /// Offset of the chunk's first byte in the decompressed dump.
    pub offset: u64,
    /// Where reading can resume after this chunk's lines (bzip2 and plain
    /// JSON).
    pub resume: Option<ResumePoint>,
}

impl Chunk {
    /// A chunk of whole lines (for tests and in-memory input).
    pub fn from_bytes(data: Vec<u8>) -> Chunk {
        let end = data.len();
        Chunk {
            prefix: Vec::new(),
            data,
            start: 0,
            end,
            index: 0,
            offset: 0,
            resume: None,
        }
    }

    /// The entity lines: trimmed of whitespace and the trailing comma;
    /// empty lines and the array brackets are skipped.
    pub fn lines(&self) -> Lines<'_> {
        Lines {
            parts: [&self.prefix[..], &self.data[self.start..self.end]],
            part: 0,
            pos: 0,
        }
    }

    /// Bytes of whole lines held.
    pub fn len(&self) -> usize {
        self.prefix.len() + self.end - self.start
    }

    /// Whether the chunk holds no bytes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Entity lines of a [`Chunk`].
pub struct Lines<'a> {
    parts: [&'a [u8]; 2],
    part: usize,
    pos: usize,
}

/// Trim whitespace and a trailing comma.
#[inline]
pub fn trim_line(mut l: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = l {
        if first.is_ascii_whitespace() {
            l = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = l {
        if last.is_ascii_whitespace() || *last == b',' {
            l = rest;
        } else {
            break;
        }
    }
    l
}

impl<'a> Iterator for Lines<'a> {
    type Item = &'a [u8];

    #[inline]
    fn next(&mut self) -> Option<&'a [u8]> {
        loop {
            let buf = *self.parts.get(self.part)?;
            if self.pos >= buf.len() {
                self.part += 1;
                self.pos = 0;
                continue;
            }
            let rest = &buf[self.pos..];
            let (line, adv) = match memchr::memchr(b'\n', rest) {
                Some(i) => (&rest[..i], i + 1),
                None => (rest, rest.len()),
            };
            self.pos += adv;
            let l = trim_line(line);
            if l.is_empty() || l == b"[" || l == b"]" {
                continue;
            }
            return Some(l);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Framing {
    Unknown,
    Array,
    Lines,
}

/// Turns decompressed blocks into [`Chunk`]s (see the module docs).
pub struct Splitter<I> {
    inner: I,
    carry: Vec<u8>,
    /// Where `carry` starts: its block, and the offset in that block.
    carry_start: Option<(BlockStart, u64)>,
    /// The last byte handed out in a chunk, but whitespace, was `]`.
    closed: bool,
    /// Bytes to drop from the first block (resuming).
    skip: u64,
    framing: Framing,
    last_byte: Option<u8>,
    offset: u64,
    index: u64,
    done: bool,
    stats: Arc<ReadStats>,
    allow_truncated: bool,
}

impl<I: Iterator<Item = Result<Block>>> Splitter<I> {
    /// Split `inner`'s blocks; counts go to `stats`. With
    /// `allow_truncated`, a truncated input ends at its last whole line
    /// (and `stats.truncated` is set) instead of failing.
    pub fn new(inner: I, stats: Arc<ReadStats>, allow_truncated: bool) -> Self {
        Self::resume(inner, stats, allow_truncated, None)
    }

    /// [`Splitter::new`] for blocks that start at `resume` (whose first
    /// `skip` bytes were read before).
    pub fn resume(inner: I, stats: Arc<ReadStats>, allow_truncated: bool, resume: Option<ResumePoint>) -> Self {
        let r = resume.unwrap_or_default();
        Self {
            inner,
            carry: Vec::new(),
            carry_start: None,
            closed: r.closed,
            skip: r.skip,
            framing: match resume {
                None => Framing::Unknown,
                Some(r) if r.array => Framing::Array,
                Some(_) => Framing::Lines,
            },
            last_byte: r.closed.then_some(b']'),
            offset: r.offset,
            index: r.chunks,
            done: false,
            stats,
            allow_truncated,
        }
    }

    /// Where reading resumes after the chunks handed out so far.
    fn resume_point(&self) -> Option<ResumePoint> {
        let (b, skip) = self.carry_start?;
        Some(ResumePoint {
            bit: b.bit,
            crc: b.crc,
            blocks: b.blocks,
            streams: b.streams,
            skip,
            offset: self.offset,
            chunks: self.index,
            array: self.framing == Framing::Array,
            closed: self.closed,
            plain: b.plain,
        })
    }

    fn observe(&mut self, block: &[u8]) -> Result<()> {
        if self.framing == Framing::Unknown {
            if let Some(&b) = block.iter().find(|b| !b.is_ascii_whitespace()) {
                self.framing = match b {
                    b'[' => Framing::Array,
                    b'{' => Framing::Lines,
                    _ => {
                        return Err(DumpError::Format(
                            "not a Wikidata JSON dump (it starts with neither '[' nor '{')".into(),
                        ))
                    }
                };
            }
        }
        if let Some(&b) = block.iter().rev().find(|b| !b.is_ascii_whitespace()) {
            self.last_byte = Some(b);
        }
        Ok(())
    }

    fn chunk(&mut self, prefix: Vec<u8>, data: Vec<u8>, start: usize, end: usize) -> Chunk {
        let mut c = Chunk {
            offset: self.offset,
            index: self.index,
            prefix,
            data,
            start,
            end,
            resume: None,
        };
        self.index += 1;
        self.offset += c.len() as u64;
        let last = [&c.data[c.start..c.end], &c.prefix[..]]
            .into_iter()
            .find_map(|part| part.iter().rev().find(|b| !b.is_ascii_whitespace()));
        if let Some(&b) = last {
            self.closed = b == b']';
        }
        c.resume = self.resume_point();
        c
    }

    fn truncated(&mut self, message: String) -> Option<Result<Chunk>> {
        self.done = true;
        if self.allow_truncated {
            self.stats.truncated.store(true, Ordering::Relaxed);
            None
        } else {
            Some(Err(DumpError::Truncated(message)))
        }
    }

    fn finish(&mut self) -> Option<Result<Chunk>> {
        self.done = true;
        match self.framing {
            Framing::Unknown => Some(Err(DumpError::Format("the dump is empty".into()))),
            Framing::Array if self.last_byte != Some(b']') => {
                self.carry.clear();
                self.truncated("the JSON array is not closed with ']': the dump is truncated".into())
            }
            _ => {
                if self.carry.iter().all(u8::is_ascii_whitespace) {
                    return None;
                }
                let prefix = std::mem::take(&mut self.carry);
                // Nothing is left to resume from after the last line.
                self.carry_start = None;
                Some(Ok(self.chunk(prefix, Vec::new(), 0, 0)))
            }
        }
    }
}

impl<I: Iterator<Item = Result<Block>>> Iterator for Splitter<I> {
    type Item = Result<Chunk>;

    fn next(&mut self) -> Option<Result<Chunk>> {
        if self.done {
            return None;
        }
        loop {
            let Block { data: mut block, start } = match self.inner.next() {
                None => return self.finish(),
                Some(Err(DumpError::Truncated(m))) => return self.truncated(m),
                Some(Err(e)) => {
                    self.done = true;
                    return Some(Err(e));
                }
                Some(Ok(b)) => b,
            };
            // The block's offset of `block[0]` (after a resume's skip).
            let mut base = 0u64;
            if self.skip > 0 {
                let n = (self.skip as usize).min(block.len());
                block.drain(..n);
                base = n as u64;
                self.skip -= n as u64;
            }
            if block.is_empty() {
                continue;
            }
            self.stats.bytes_out.fetch_add(block.len() as u64, Ordering::Relaxed);
            if let Err(e) = self.observe(&block) {
                self.done = true;
                return Some(Err(e));
            }
            if self.carry.is_empty() {
                self.carry_start = start.map(|s| (s, base));
            }
            let Some(first) = memchr::memchr(b'\n', &block) else {
                self.carry.extend_from_slice(&block);
                continue;
            };
            let last = memchr::memrchr(b'\n', &block).unwrap();
            let mut prefix = std::mem::take(&mut self.carry);
            prefix.extend_from_slice(&block[..=first]);
            self.carry.extend_from_slice(&block[last + 1..]);
            self.carry_start = start.map(|s| (s, base + last as u64 + 1));
            return Some(Ok(self.chunk(prefix, block, first + 1, last + 1)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(blocks: &[&[u8]], allow: bool) -> (Result<Vec<Vec<u8>>>, Arc<ReadStats>) {
        let stats = Arc::new(ReadStats::default());
        let it = blocks.iter().map(|b| Ok(b.to_vec().into()));
        let mut out = Vec::new();
        for c in Splitter::new(it, stats.clone(), allow) {
            match c {
                Ok(c) => out.extend(c.lines().map(<[u8]>::to_vec)),
                Err(e) => return (Err(e), stats),
            }
        }
        (Ok(out), stats)
    }

    #[test]
    fn stitches_lines_across_blocks() {
        let (lines, _) = split(&[b"[\n{\"a\":1},\n{\"b\"", b":2},\n{\"c", b"\":3}", b"\n]\n"], false);
        assert_eq!(
            lines.unwrap(),
            vec![b"{\"a\":1}".to_vec(), b"{\"b\":2}".to_vec(), b"{\"c\":3}".to_vec()]
        );
    }

    #[test]
    fn json_lines_without_final_newline() {
        let (lines, _) = split(&[b"{\"a\":1}\n{\"b\":2}"], false);
        assert_eq!(lines.unwrap().len(), 2);
    }

    #[test]
    fn unclosed_array_is_truncated() {
        let (r, _) = split(&[b"[\n{\"a\":1},\n{\"b\":"], false);
        assert!(matches!(r, Err(DumpError::Truncated(_))));
        let (r, stats) = split(&[b"[\n{\"a\":1},\n{\"b\":"], true);
        assert_eq!(r.unwrap(), vec![b"{\"a\":1}".to_vec()]);
        assert!(stats.truncated.load(Ordering::Relaxed));
    }

    #[test]
    fn empty_and_garbage() {
        assert!(split(&[b"  \n"], false).0.is_err());
        assert!(split(&[b"<html>"], false).0.is_err());
    }

    #[test]
    fn trims() {
        assert_eq!(trim_line(b"  {\"a\":1},\r"), b"{\"a\":1}");
        assert_eq!(trim_line(b",,"), b"");
    }
}
