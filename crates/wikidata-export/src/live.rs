//! Live changes: the entities edited on Wikidata, as they are edited.
//!
//! Wikimedia's EventStreams `recentchange` stream carries every edit of
//! every wiki. [`live_changes`] keeps those of one wiki (`wikidatawiki`)
//! in the entity namespaces asked (items by default) and, given ids, only
//! those entities; a consumer then reads the changed entities again by id.
//! Every change carries the stream's event id: passing the last one back
//! ([`LiveOptions::last_event_id`]) resumes right after it, and dropped
//! connections resume the same way, so no change is missed or repeated.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wikidata_dump::sse::SseReader;
use wikidata_dump::HttpOptions;
use wikidata_filter::{id_key, IdSet};

use crate::scan::Flow;
use crate::ExportError;

/// Wikimedia's recent changes stream.
pub const RECENT_CHANGES: &str = "https://stream.wikimedia.org/v2/stream/recentchange";

/// Options for [`live_changes`].
#[derive(Clone, Debug)]
pub struct LiveOptions {
    /// The stream (default [`RECENT_CHANGES`]).
    pub url: String,
    /// The wiki (default `wikidatawiki`).
    pub wiki: String,
    /// Namespaces (default `[0]`, items; `120` properties, `146` lexemes).
    pub namespaces: Vec<i64>,
    /// Only these entities ([`id_key`]s), when given.
    pub ids: Option<Arc<IdSet>>,
    /// Resume after this event id (from [`Change::event_id`]).
    pub last_event_id: Option<String>,
    /// Start at this time instead (`2026-10-24T00:00:00Z`, or a Unix time
    /// in milliseconds); EventStreams keeps about a week.
    pub since: Option<String>,
    /// User agent, timeout, reconnect attempts.
    pub http: HttpOptions,
}

impl Default for LiveOptions {
    fn default() -> Self {
        LiveOptions {
            url: RECENT_CHANGES.into(),
            wiki: "wikidatawiki".into(),
            namespaces: vec![0],
            ids: None,
            last_event_id: None,
            since: None,
            http: HttpOptions::default(),
        }
    }
}

/// A change to an entity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// The entity (`Q42`, `P31`, `L7`).
    pub id: String,
    /// `edit`, `new` or `log` (deletions and other log actions).
    pub kind: String,
    /// The new revision (edits and creations).
    pub revision: Option<u64>,
    /// The revision it replaced (edits).
    pub old_revision: Option<u64>,
    /// When (Unix time, seconds).
    pub timestamp: i64,
    /// Who.
    pub user: Option<String>,
    /// Made by a bot.
    pub bot: bool,
    /// The edit summary: `/* wbmergeitems-to:0||Q2 */` for a merge,
    /// `/* wbcreateredirect:… */` for a redirect.
    pub comment: String,
    /// Log events: `delete`, …
    pub log_type: Option<String>,
    /// Log events: `delete`, `restore`, …
    pub log_action: Option<String>,
    /// The stream's event id: resume after this change with it.
    pub event_id: String,
}

#[derive(Deserialize)]
struct Revision {
    old: Option<u64>,
    new: Option<u64>,
}

/// The fields of a `recentchange` event read here.
#[derive(Deserialize)]
struct RecentChange {
    wiki: Option<String>,
    namespace: Option<i64>,
    title: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<i64>,
    user: Option<String>,
    #[serde(default)]
    bot: bool,
    #[serde(default)]
    comment: String,
    revision: Option<Revision>,
    log_type: Option<String>,
    log_action: Option<String>,
}

/// The entity id of a title (`Property:P31` → `P31`).
fn entity_id(title: &str) -> &str {
    title.rsplit(':').next().unwrap_or(title)
}

/// The stream's URL with `since`.
fn url(opts: &LiveOptions) -> String {
    match (&opts.since, &opts.last_event_id) {
        (Some(since), None) => {
            let sep = if opts.url.contains('?') { '&' } else { '?' };
            let enc: String = since
                .chars()
                .map(|c| match c {
                    ':' => "%3A".to_string(),
                    '+' => "%2B".to_string(),
                    c => c.to_string(),
                })
                .collect();
            format!("{}{sep}since={enc}", opts.url)
        }
        _ => opts.url.clone(),
    }
}

/// Read the stream, handing the matching changes to `sink` (a batch at
/// a time: what arrived together) until it says [`Flow::Stop`]. `stop` is
/// asked between events (and every half second while the stream is
/// quiet), so a consumer can end the stream at any time.
pub fn live_changes(
    opts: &LiveOptions,
    stop: &(dyn Fn() -> bool + Sync),
    sink: &mut dyn FnMut(Vec<Change>) -> Flow,
) -> Result<LiveStats, ExportError> {
    let mut reader = SseReader::open(&url(opts), opts.last_event_id.clone(), opts.http.clone())?;
    // Reading blocks; it runs on its own thread so that `stop` is heard
    // while the stream is quiet.
    let (tx, rx) = std::sync::mpsc::sync_channel(1024);
    let reading = std::thread::Builder::new()
        .name("wikidata-live".into())
        .spawn(move || loop {
            let r = reader.next_event();
            let failed = r.is_err();
            let more = reader.has_buffered();
            if tx.send((r, more, reader.connections)).is_err() || failed {
                return;
            }
        })
        .expect("spawn live thread");
    let mut stats = LiveStats::default();
    let mut batch = Vec::new();
    let result = loop {
        if stop() {
            break Ok(());
        }
        let (event, more, connections) = match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(x) => x,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break Ok(()),
        };
        stats.reconnects = connections.saturating_sub(1);
        let ev = match event {
            Ok(ev) => ev,
            Err(e) => break Err(ExportError::from(e)),
        };
        stats.events += 1;
        if let Some(c) = change(opts, &ev.data, ev.id.unwrap_or_default()) {
            stats.changes += 1;
            stats.last_event_id = Some(c.event_id.clone());
            batch.push(c);
        }
        // Hand on what arrived together.
        if !batch.is_empty() && (!more || batch.len() >= 256) && sink(std::mem::take(&mut batch)) == Flow::Stop {
            break Ok(());
        }
    };
    drop(rx);
    // The reading thread ends at its next event, or when its connection
    // fails; it is not waited for.
    drop(reading);
    result.map(|_| stats)
}

/// The change of an event, when it matches.
fn change(opts: &LiveOptions, data: &str, event_id: String) -> Option<Change> {
    let rc: RecentChange = serde_json::from_str(data).ok()?;
    if rc.wiki.as_deref() != Some(opts.wiki.as_str()) || !opts.namespaces.contains(&rc.namespace?) {
        return None;
    }
    let id = entity_id(rc.title.as_deref()?).to_string();
    if let Some(ids) = &opts.ids {
        if !id_key(&id).is_some_and(|k| ids.contains(k)) {
            return None;
        }
    }
    let rev = rc.revision.as_ref();
    Some(Change {
        id,
        kind: rc.kind.unwrap_or_default(),
        revision: rev.and_then(|r| r.new),
        old_revision: rev.and_then(|r| r.old),
        timestamp: rc.timestamp.unwrap_or_default(),
        user: rc.user,
        bot: rc.bot,
        comment: rc.comment,
        log_type: rc.log_type,
        log_action: rc.log_action,
        event_id,
    })
}

/// Counters of a live stream.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStats {
    /// Events read (every wiki).
    pub events: u64,
    /// Changes handed on.
    pub changes: u64,
    /// Reconnects after dropped connections.
    pub reconnects: u64,
    /// The last change's event id.
    pub last_event_id: Option<String>,
}
