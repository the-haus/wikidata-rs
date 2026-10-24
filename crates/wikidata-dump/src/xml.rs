//! MediaWiki XML exports as entity lines: the daily incremental dumps
//! (`wikidatawiki-<date>-pages-meta-hist-incr.xml.bz2`).
//!
//! An incremental dump holds the day's new revisions, page by page (in
//! page id order), each revision's text being the entity's JSON as
//! stored:
//!
//! ```text
//! <page>
//!   <title>Q8</title> <ns>0</ns> <id>134</id>
//!   <revision>
//!     <id>2559609318</id> … <timestamp>2026-10-21T23:51:08Z</timestamp> …
//!     <model>wikibase-item</model> <format>application/json</format>
//!     <text bytes="145195" …>{&quot;type&quot;:&quot;item&quot;,…}</text>
//!   </revision>
//!   <revision>…</revision>
//! </page>
//! ```
//!
//! [`XmlEntities`] turns it into the lines of a JSON-lines dump, so the
//! rest of the pipeline (filters, parsing) reads it like any dump: one
//! line per entity page, its newest revision only, with `lastrevid` and
//! `modified` added (the stored JSON has neither). Pages of other content
//! models (wikitext discussion pages) are skipped; a revision that is a
//! redirect (`{"entity":"Q1","redirect":"Q2"}`, a merge) is reported in
//! [`ReadStats::redirects`] instead. Only one revision is held at a time,
//! so memory is bounded by the largest revision, not the largest page.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use memchr::memmem;

use crate::{Block, DumpError, ReadStats, Result};

/// Lines are handed on in blocks of about this size.
const OUT_BLOCK: usize = 4 << 20;

/// Whether decompressed data starting with `head` is a MediaWiki XML
/// export.
pub fn is_xml(head: &[u8]) -> bool {
    let head = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    let start = head.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(head.len());
    let h = &head[start..head.len().min(start + 512)];
    h.starts_with(b"<mediawiki") || (h.starts_with(b"<?xml") && memmem::find(h, b"<mediawiki").is_some())
}

/// Undo XML escapes (`&quot;`, `&amp;`, `&lt;`, `&gt;`, `&apos;`,
/// `&#…;`), appending to `out`.
pub fn unescape_into(s: &[u8], out: &mut Vec<u8>) {
    let mut rest = s;
    while let Some(i) = memchr::memchr(b'&', rest) {
        out.extend_from_slice(&rest[..i]);
        rest = &rest[i..];
        // The entity ends at `;`, before any other `&`.
        let Some(end) = rest
            .iter()
            .take(12)
            .enumerate()
            .skip(1)
            .take_while(|(_, &b)| b != b'&')
            .find_map(|(i, &b)| (b == b';').then_some(i))
        else {
            out.push(b'&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        match name {
            b"quot" => out.push(b'"'),
            b"amp" => out.push(b'&'),
            b"lt" => out.push(b'<'),
            b"gt" => out.push(b'>'),
            b"apos" => out.push(b'\''),
            [b'#', b'x' | b'X', hex @ ..] => push_char(
                out,
                std::str::from_utf8(hex)
                    .ok()
                    .and_then(|h| u32::from_str_radix(h, 16).ok()),
            ),
            [b'#', dec @ ..] => push_char(out, std::str::from_utf8(dec).ok().and_then(|d| d.parse().ok())),
            _ => {
                // Not an entity: the `&` as it is, and read on after it.
                out.push(b'&');
                rest = &rest[1..];
                continue;
            }
        }
        rest = &rest[end + 1..];
    }
    out.extend_from_slice(rest);
}

fn push_char(out: &mut Vec<u8>, c: Option<u32>) {
    let c = c.and_then(char::from_u32).unwrap_or('\u{fffd}');
    let mut b = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
}

/// The text between `<tag>` and `</tag>` in `s`.
fn element<'a>(s: &'a [u8], open: &[u8], close: &[u8]) -> Option<&'a [u8]> {
    let a = memmem::find(s, open)? + open.len();
    let b = memmem::find(&s[a..], close)?;
    Some(&s[a..a + b])
}

/// The newest revision of the page being read.
#[derive(Default)]
struct Newest {
    id: u64,
    timestamp: String,
    entity: bool,
    /// The unescaped text.
    text: Vec<u8>,
}

/// Turns the decompressed blocks of an XML export into blocks of entity
/// lines (see the module docs).
pub struct XmlEntities<I> {
    inner: I,
    buf: Vec<u8>,
    /// Bytes of `buf` already read.
    pos: usize,
    in_page: bool,
    /// The page's title, once read.
    title: Option<String>,
    newest: Option<Newest>,
    out: Vec<u8>,
    /// The input ended.
    ended: bool,
    closed: bool,
    done: bool,
    stats: Arc<ReadStats>,
}

impl<I: Iterator<Item = Result<Block>>> XmlEntities<I> {
    /// Read the XML in `inner`'s blocks; redirects go to `stats`.
    pub fn new(inner: I, stats: Arc<ReadStats>) -> Self {
        stats.xml.store(true, Ordering::Relaxed);
        Self {
            inner,
            buf: Vec::new(),
            pos: 0,
            in_page: false,
            title: None,
            newest: None,
            out: Vec::new(),
            ended: false,
            closed: false,
            done: false,
            stats,
        }
    }

    /// Drop the bytes read (between steps, where no position is held).
    fn compact(&mut self) {
        if self.pos > 0 && self.pos * 2 >= self.buf.len() {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
    }

    /// Read another block into `buf`; `false` at the end.
    fn fill(&mut self) -> Result<bool> {
        match self.inner.next() {
            Some(Ok(b)) => {
                self.buf.extend_from_slice(&b.data);
                Ok(true)
            }
            Some(Err(e)) => Err(e),
            None => Ok(false),
        }
    }

    /// Find `needle` at or after `pos`, reading more as needed.
    fn find(&mut self, needle: &[u8]) -> Result<Option<usize>> {
        let mut from = self.pos;
        loop {
            if let Some(i) = memmem::find(&self.buf[from..], needle) {
                return Ok(Some(from + i));
            }
            from = self.buf.len().saturating_sub(needle.len()).max(self.pos);
            if !self.fill()? {
                return Ok(None);
            }
        }
    }

    /// The next of `<revision>` and `</page>` in the page, and which.
    fn next_in_page(&mut self) -> Result<Option<(usize, bool)>> {
        let mut from = self.pos;
        loop {
            let hay = &self.buf[from..];
            let rev = memmem::find(hay, b"<revision>");
            let end = memmem::find(hay, b"</page>");
            match (rev, end) {
                (Some(r), Some(e)) => return Ok(Some(if r < e { (from + r, true) } else { (from + e, false) })),
                (Some(r), None) => return Ok(Some((from + r, true))),
                (None, Some(e)) => return Ok(Some((from + e, false))),
                (None, None) => {}
            }
            from = self.buf.len().saturating_sub(10).max(self.pos);
            if !self.fill()? {
                return Ok(None);
            }
        }
    }

    fn read_header(&mut self, upto: usize) {
        if self.title.is_none() {
            let head = &self.buf[self.pos..upto];
            self.title = element(head, b"<title>", b"</title>").map(|t| {
                let mut out = Vec::new();
                unescape_into(t, &mut out);
                String::from_utf8_lossy(&out).into_owned()
            });
        }
    }

    fn revision(&mut self, start: usize, end: usize) {
        let rev = &self.buf[start..end];
        let id = element(rev, b"<id>", b"</id>")
            .and_then(|v| std::str::from_utf8(v).ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if self.newest.as_ref().is_some_and(|n| n.id >= id) {
            return;
        }
        let model = element(rev, b"<model>", b"</model>").unwrap_or_default();
        let entity = model.starts_with(b"wikibase-");
        let mut text = Vec::new();
        if entity {
            if let Some(t) = memmem::find(rev, b"<text") {
                let tag_end = memchr::memchr(b'>', &rev[t..]).map_or(rev.len(), |i| t + i);
                if rev[tag_end - 1] != b'/' {
                    if let Some(body) = element(&rev[tag_end..], b">", b"</text>") {
                        text.reserve(body.len());
                        unescape_into(body, &mut text);
                    }
                }
            }
        }
        let timestamp = element(rev, b"<timestamp>", b"</timestamp>")
            .map(|t| String::from_utf8_lossy(t).into_owned())
            .unwrap_or_default();
        self.newest = Some(Newest {
            id,
            timestamp,
            entity,
            text,
        });
    }

    /// Hand on the page's newest revision.
    fn end_page(&mut self) {
        let title = self.title.take();
        let Some(n) = self.newest.take() else { return };
        let text = trim_json(&n.text);
        if !n.entity || text.len() < 2 || text[0] != b'{' {
            return;
        }
        // `{"entity":"Q1","redirect":"Q2"}`: a merge.
        if text.starts_with(b"{\"entity\":") {
            let from = string_field(text, b"\"entity\":\"").or(title);
            if let (Some(from), Some(to)) = (from, string_field(text, b"\"redirect\":\"")) {
                self.stats.redirects.lock().unwrap().push((from, to));
            }
            return;
        }
        self.out.extend_from_slice(&text[..text.len() - 1]);
        let tail = format!(",\"lastrevid\":{},\"modified\":\"{}\"}}\n", n.id, n.timestamp);
        if text.len() > 2 {
            self.out.extend_from_slice(tail.as_bytes());
        } else {
            self.out.extend_from_slice(&tail.as_bytes()[1..]);
        }
    }

    fn step(&mut self) -> Result<Option<Block>> {
        loop {
            if self.ended {
                return if self.closed {
                    Ok(None)
                } else {
                    Err(DumpError::Truncated(
                        "the XML export is not closed with </mediawiki>: the dump is truncated".into(),
                    ))
                };
            }
            if self.out.len() >= OUT_BLOCK {
                return Ok(Some(self.take_out()));
            }
            self.compact();
            let more = if !self.in_page {
                match self.find(b"<page>")? {
                    Some(i) => {
                        self.pos = i + 6;
                        self.in_page = true;
                        true
                    }
                    None => false,
                }
            } else {
                match self.next_in_page()? {
                    Some((at, true)) => {
                        self.read_header(at);
                        match self.find(b"</revision>")? {
                            Some(end) => {
                                self.revision(at, end);
                                self.pos = end + 11;
                                true
                            }
                            None => false,
                        }
                    }
                    Some((at, false)) => {
                        self.read_header(at);
                        self.end_page();
                        self.pos = at + 7;
                        self.in_page = false;
                        true
                    }
                    None => false,
                }
            };
            if !more {
                // The end of input: closed, unless inside a page.
                self.ended = true;
                self.closed = !self.in_page && memmem::find(&self.buf[self.pos..], b"</mediawiki>").is_some();
                if !self.out.is_empty() {
                    // The whole pages first; a truncation error comes next.
                    return Ok(Some(self.take_out()));
                }
            }
        }
    }

    fn take_out(&mut self) -> Block {
        Block {
            data: std::mem::take(&mut self.out),
            start: None,
        }
    }
}

/// The string after `key` (`"redirect":"`), when it has no escapes.
fn string_field(text: &[u8], key: &[u8]) -> Option<String> {
    let at = memmem::find(text, key)? + key.len();
    let len = memchr::memchr2(b'"', b'\\', &text[at..])?;
    (text[at + len] == b'"').then(|| String::from_utf8_lossy(&text[at..at + len]).into_owned())
}

fn trim_json(t: &[u8]) -> &[u8] {
    let s = t.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(t.len());
    let e = t.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(s, |i| i + 1);
    &t[s..e]
}

impl<I: Iterator<Item = Result<Block>>> Iterator for XmlEntities<I> {
    type Item = Result<Block>;

    fn next(&mut self) -> Option<Result<Block>> {
        if self.done {
            return None;
        }
        match self.step() {
            Ok(Some(b)) => Some(Ok(b)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes() {
        let mut out = Vec::new();
        unescape_into(
            b"{&quot;a&quot;:&quot;x &amp; y &lt;3&#33;&#x263A;&bogus; &&quot;}",
            &mut out,
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\"a\":\"x & y <3!\u{263A}&bogus; &\"}"
        );
        assert!(is_xml(b"  <mediawiki xmlns=\"x\">"));
        assert!(is_xml(b"<?xml version=\"1.0\"?>\n<mediawiki>"));
        assert!(!is_xml(b"<html>"));
        assert!(!is_xml(b"[\n{"));
    }
}
