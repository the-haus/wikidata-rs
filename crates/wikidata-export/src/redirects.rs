//! Building the redirect index from MediaWiki's SQL dumps.
//!
//! The `redirect` table maps a redirect's page id to its target's title
//! (`Q2`), the `page` table a page id to its title (`Q1`). The join is
//! streamed: the redirect table (a few million rows, ~100 MB compressed)
//! is read first and kept as sorted page ids, then the page table (~4 GB
//! compressed) streams by and only the item pages among those ids are
//! kept. Statements are parsed on all cores; decompression (gzip) runs on
//! the reading thread.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use wikidata_dump::sql::{parse_insert, Field, SqlLines};
use wikidata_dump::{ordered_map, resolve_threads, HttpOptions, Input, Source};
use wikidata_filter::RedirectIndex;

use crate::scan::{Flow, Progress};
use crate::ExportError;

/// Options for [`build_redirect_index`].
#[derive(Clone, Debug, Default)]
pub struct RedirectOptions {
    /// Worker threads (0: all cores).
    pub threads: usize,
    /// For URL inputs.
    pub http: HttpOptions,
}

/// What [`build_redirect_index`] read.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedirectStats {
    /// Rows of the redirect table pointing at an item.
    pub redirect_rows: u64,
    /// Rows of the page table.
    pub page_rows: u64,
    /// Item redirects found (`Q1 → Q2`).
    pub redirects: u64,
    /// Compressed bytes read (both files).
    pub bytes_in: u64,
    /// Wall time.
    pub elapsed_ms: u64,
}

/// The number of an item title (`Q42`).
fn item(title: &Field<'_>) -> Option<u32> {
    match title {
        Field::Str(s) if s.first() == Some(&b'Q') && s.len() > 1 && s[1..].iter().all(u8::is_ascii_digit) => {
            std::str::from_utf8(&s[1..]).ok()?.parse().ok()
        }
        _ => None,
    }
}

/// Parse a table's statements on all cores; `row` maps a row to a value.
fn rows<T: Send + 'static>(
    lines: SqlLines,
    columns: Vec<usize>,
    threads: usize,
    row: impl Fn(&[Field<'_>]) -> Option<T> + Send + Sync + 'static,
    mut each: impl FnMut(Vec<T>, u64) -> Flow,
) -> Result<bool, ExportError> {
    let max = columns.iter().copied().max().unwrap_or(0);
    let parsed = ordered_map(
        lines,
        threads,
        threads * 2,
        move |line| -> Result<(Vec<T>, u64), ExportError> {
            let line = line?;
            let mut out = Vec::new();
            let mut n = 0u64;
            parse_insert(&line, max, &mut |r| {
                n += 1;
                let picked: Vec<Field<'_>> = columns.iter().map(|&c| r[c]).collect();
                if let Some(v) = row(&picked) {
                    out.push(v);
                }
            })?;
            Ok((out, n))
        },
    );
    for r in parsed {
        let (vs, n) = r?;
        if each(vs, n) == Flow::Stop {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Join a `redirect` and a `page` SQL dump of Wikidata
/// (`wikidatawiki-<date>-redirect.sql.gz`, `…-page.sql.gz`; files or URLs)
/// into a [`RedirectIndex`] of item redirects.
pub fn build_redirect_index(
    redirects: &Input,
    pages: &Input,
    opts: &RedirectOptions,
    progress: &mut dyn FnMut(Progress) -> Flow,
) -> Result<(RedirectIndex, RedirectStats), ExportError> {
    let start = Instant::now();
    let threads = resolve_threads(opts.threads);
    let mut stats = RedirectStats::default();
    let stopped = || ExportError::Invalid("building the redirect index was stopped".into());

    // Page id → target item, sorted by page id.
    let source = Source::open(redirects, &opts.http)?;
    let (counter, total_r) = (source.counter(), source.len());
    let lines = SqlLines::new(source, "redirect")?;
    let cols = vec![
        lines.column("rd_from")?,
        lines.column("rd_namespace")?,
        lines.column("rd_title")?,
    ];
    let mut targets: Vec<(u32, u32)> = Vec::new();
    let mut last = Instant::now();
    let report = |stats: &RedirectStats, bytes: u64, total: Option<u64>, matched: u64| Progress {
        bytes_in: bytes,
        bytes_total: total,
        bytes_out: 0,
        entities: stats.redirect_rows + stats.page_rows,
        matched,
        elapsed_ms: start.elapsed().as_millis() as u64,
    };
    let done = rows(
        lines,
        cols,
        threads,
        |r| {
            let page = u32::try_from(r[0].int()?).ok()?;
            (r[1].int()? == 0).then_some(())?;
            Some((page, item(&r[2])?))
        },
        |vs, _| {
            stats.redirect_rows += vs.len() as u64;
            targets.extend(vs);
            if last.elapsed() >= Duration::from_millis(100) {
                last = Instant::now();
                return progress(report(&stats, counter.get(), total_r, 0));
            }
            Flow::Continue
        },
    )?;
    if !done {
        return Err(stopped());
    }
    let read_r = counter.get();
    targets.sort_unstable();
    targets.dedup_by_key(|t| t.0);
    let targets = Arc::new(targets);

    // Item pages among the redirects.
    let source = Source::open(pages, &opts.http)?;
    let (counter, total_p) = (source.counter(), source.len());
    let lines = SqlLines::new(source, "page")?;
    let mut cols = vec![
        lines.column("page_id")?,
        lines.column("page_namespace")?,
        lines.column("page_title")?,
    ];
    // Not every schema has it; it only saves lookups.
    if let Ok(c) = lines.column("page_is_redirect") {
        cols.push(c);
    }
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    let t = targets.clone();
    let done = rows(
        lines,
        cols,
        threads,
        move |r| {
            if r[1].int()? != 0 || r.get(3).is_some_and(|f| f.int() == Some(0)) {
                return None;
            }
            let page = u32::try_from(r[0].int()?).ok()?;
            let i = t.binary_search_by_key(&page, |x| x.0).ok()?;
            Some((item(&r[2])?, t[i].1))
        },
        |vs, n| {
            stats.page_rows += n;
            pairs.extend(vs);
            if last.elapsed() >= Duration::from_millis(100) {
                last = Instant::now();
                let p = report(
                    &stats,
                    read_r + counter.get(),
                    total_r.zip(total_p).map(|(a, b)| a + b),
                    pairs.len() as u64,
                );
                return progress(p);
            }
            Flow::Continue
        },
    )?;
    if !done {
        return Err(stopped());
    }
    stats.bytes_in = read_r + counter.get();
    let source = format!("{} + {}", redirects.file_name(), pages.file_name());
    let index = RedirectIndex::from_pairs(pairs, source);
    stats.redirects = index.len() as u64;
    stats.elapsed_ms = start.elapsed().as_millis() as u64;
    let _ = progress(report(&stats, stats.bytes_in, None, stats.redirects));
    Ok((index, stats))
}
