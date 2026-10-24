#![deny(clippy::all)]

//! Node.js bindings: stream filtered, normalised entities out of Wikidata
//! dumps.
//!
//! Tasks run on their own threads (decoding and parsing on all cores);
//! entities cross into JS as columnar batches under credit-based flow
//! control (see [`credits`]), so a slow consumer bounds memory instead of
//! filling it.

mod batch;
mod class_index;
mod credits;
mod indexes;
mod options;
mod stream;

use std::sync::atomic::{AtomicU32, Ordering};

use napi::bindgen_prelude::AsyncTask;
use napi::bindgen_prelude::Uint32Array;
use napi::{Env, JsFunction, Result, Task};
use napi_derive::napi;
use wikidata_dump::download::{
    download, download_sql, latest_entity_dump, latest_sql_dump, DownloadOptions, DownloadStats, DumpStatus,
    SqlDownloadOptions, ENTITY_DUMPS, WIKI_DUMPS,
};
use wikidata_export::{
    build_redirect_index, live_changes, Dump, EntityOptions, ExtractOptions, Flow, HttpOptions, Input, LiveOptions,
    RedirectOptions, ScanOptions, WriteOptions,
};

pub use class_index::{load_class_index, take_class_index, LoadTask, NativeClassIndex};
pub use indexes::{
    load_place_index, load_redirect_index, take_place_index, take_redirect_index, LoadPlaces, LoadRedirects,
    NativePlaceIndex, NativeRedirectIndex,
};
use options::invalid;
pub use options::NativeOptions;
pub use stream::NativeStream;

// Not on macOS: mimalloc keeps each thread's heap in fixed TLS slots of the
// process (108 and 109), so two addons that each bundle it (this one and
// @pestle-rs/node, or two copies of this one) take each other's heaps and
// crash. Linux and Windows give each copy its own thread-local storage.
#[cfg(all(feature = "mimalloc", not(target_vendor = "apple")))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Options for `open`.
#[napi(object)]
#[derive(Default)]
pub struct NativeOpenOptions {
    /// HTTP `User-Agent`.
    pub user_agent: Option<String>,
    /// HTTP reconnect attempts.
    pub retries: Option<u32>,
    /// Other URLs of the same file, read in ranges at once.
    pub mirrors: Option<Vec<String>>,
    /// Connections per server when reading in ranges.
    pub connections: Option<u32>,
    /// Range size in bytes.
    pub range_size: Option<f64>,
}

fn http_options(o: NativeOpenOptions) -> HttpOptions {
    let mut http = HttpOptions::default();
    if let Some(ua) = o.user_agent {
        http.user_agent = ua;
    }
    if let Some(r) = o.retries {
        http.retries = r;
    }
    if let Some(m) = o.mirrors {
        http.mirrors = m;
    }
    if let Some(c) = o.connections {
        http.connections = Some(c.max(1) as usize);
    }
    if let Some(r) = o.range_size.filter(|r| r.is_finite() && *r >= 1.0) {
        http.range_size = r as u64;
    }
    http
}

/// An open dump.
#[napi]
pub struct NativeReader {
    dump: Dump,
}

fn to_value<T: serde::Serialize>(v: &T) -> std::result::Result<serde_json::Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

#[napi]
impl NativeReader {
    /// The path or URL.
    #[napi(getter)]
    pub fn input(&self) -> String {
        self.dump.input().display()
    }

    /// `bzip2`, `gzip`, `zstd` or `none`.
    #[napi(getter)]
    pub fn compression(&self) -> String {
        self.dump.compression().as_str().to_string()
    }

    /// Size in bytes, when known.
    #[napi(getter)]
    pub fn size(&self) -> Option<f64> {
        self.dump.len().map(|l| l as f64)
    }

    /// Stream matching entities to `callback(event)`.
    #[napi(ts_args_type = "options: NativeOptions, callback: (event: unknown) => void")]
    pub fn entities(&self, options: NativeOptions, callback: JsFunction) -> Result<NativeStream> {
        let places = options::place_index(&options)?;
        let labels = options::referenced_labels(&options)?;
        let labels_output = labels.as_ref().and_then(|(_, o)| o.clone());
        let opts = EntityOptions {
            scan: options::scan(&options)?,
            model: options::model(&options)?,
            json: options.json.unwrap_or(true),
            summary_only: true,
            languages_by: options::by_claim(&options.languages_by_json, "languages", places.clone())?,
            sitelinks_by: options::by_claim(&options.sitelinks_by_json, "sitelinks", places)?,
            referenced_labels: labels.map(|(l, _)| l),
        };
        let names = opts
            .scan
            .filter
            .as_ref()
            .map(|f| f.names().to_vec())
            .unwrap_or_default();
        let batch_size = options.batch_size.unwrap_or(1024) as usize;
        if batch_size == 0 {
            return Err(invalid("batchSize must be a positive integer"));
        }
        let read_ahead = options.read_ahead.unwrap_or(2);
        if read_ahead == 0 {
            return Err(invalid("readAhead must be a positive integer"));
        }
        let cb = stream::callback(&callback)?;
        let dump = self.dump.clone();
        Ok(stream::spawn_entities(cb, batch_size, read_ahead, names, move |sink| {
            let stats = wikidata_export::entities(&dump, &opts, sink).map_err(|e| e.to_string())?;
            let mut v = to_value(&stats)?;
            if let Some(labels) = &stats.referenced_labels {
                let json = serde_json::to_string(labels).map_err(|e| e.to_string())?;
                match &labels_output {
                    Some(path) => std::fs::write(path, json).map_err(|e| format!("{path}: {e}"))?,
                    // Parsed in JS when read.
                    None => v["referencedLabelsJson"] = json.into(),
                }
            }
            Ok(v)
        }))
    }

    /// Write matching entities to `options.output`.
    #[napi(ts_args_type = "options: NativeOptions, callback: (event: unknown) => void")]
    pub fn extract(&self, options: NativeOptions, callback: JsFunction) -> Result<NativeStream> {
        let output = options.output.clone().ok_or_else(|| invalid("output is required"))?;
        let opts = ExtractOptions {
            scan: options::scan(&options)?,
            write: WriteOptions {
                level: options.level,
                threads: options.threads.unwrap_or(0) as usize,
                ..Default::default()
            },
            format: None,
        };
        let cb = stream::callback(&callback)?;
        let dump = self.dump.clone();
        Ok(stream::spawn_task(cb, move |progress| {
            let stats = wikidata_export::extract(&dump, &opts, &output, progress).map_err(|e| e.to_string())?;
            to_value(&stats)
        }))
    }

    /// Count what the dump holds; ends with the info as its stats.
    #[napi(ts_args_type = "options: NativeOptions, callback: (event: unknown) => void")]
    pub fn file_info(&self, options: NativeOptions, callback: JsFunction) -> Result<NativeStream> {
        let opts = options::scan(&options)?;
        let cb = stream::callback(&callback)?;
        let dump = self.dump.clone();
        Ok(stream::spawn_task(cb, move |progress| {
            let info = wikidata_export::file_info(&dump, &opts, progress).map_err(|e| e.to_string())?;
            to_value(&info)
        }))
    }

    /// Build the place index; the end event's stats carry a `token` for
    /// `takePlaceIndex` (with `classIndexOutput`, the class index is built
    /// and saved in the same pass).
    #[napi(ts_args_type = "options: NativeOptions, callback: (event: unknown) => void")]
    pub fn build_place_index(&self, options: NativeOptions, callback: JsFunction) -> Result<NativeStream> {
        let opts = ScanOptions {
            filter: None,
            ..options::scan(&NativeOptions {
                threads: options.threads,
                allow_truncated: options.allow_truncated,
                class_index_output: options.class_index_output.clone(),
                ..Default::default()
            })?
        };
        let cb = stream::callback(&callback)?;
        let dump = self.dump.clone();
        Ok(stream::spawn_task(cb, move |progress| {
            let (index, stats) =
                wikidata_export::build_place_index(&dump, &opts, progress).map_err(|e| e.to_string())?;
            let mut v = to_value(&stats)?;
            v["items"] = index.len().into();
            v["sets"] = index.sets().into();
            v["token"] = indexes::places().leave(index).into();
            Ok(v)
        }))
    }

    /// Build the class index; the end event's stats carry a `token` for
    /// `takeClassIndex`.
    #[napi(ts_args_type = "options: NativeOptions, callback: (event: unknown) => void")]
    pub fn build_class_index(&self, options: NativeOptions, callback: JsFunction) -> Result<NativeStream> {
        static TOKENS: AtomicU32 = AtomicU32::new(0);
        let opts = ScanOptions {
            filter: None,
            ..options::scan(&NativeOptions {
                threads: options.threads,
                allow_truncated: options.allow_truncated,
                place_index_output: options.place_index_output.clone(),
                ..Default::default()
            })?
        };
        let cb = stream::callback(&callback)?;
        let dump = self.dump.clone();
        Ok(stream::spawn_task(cb, move |progress| {
            let (index, stats) =
                wikidata_export::build_class_index(&dump, &opts, progress).map_err(|e| e.to_string())?;
            let token = TOKENS.fetch_add(1, Ordering::Relaxed) + 1;
            let mut v = to_value(&stats)?;
            v["token"] = token.into();
            v["classes"] = index.len().into();
            v["edges"] = index.edges().into();
            class_index::built().lock().unwrap().insert(token, index);
            Ok(v)
        }))
    }
}

/// Opens a dump on the libuv pool (reads its first bytes).
pub struct OpenTask {
    input: String,
    http: HttpOptions,
}

impl Task for OpenTask {
    type Output = Dump;
    type JsValue = NativeReader;

    fn compute(&mut self) -> Result<Dump> {
        Dump::open_with(self.input.as_str(), self.http.clone()).map_err(|e| invalid(format!("{}: {e}", self.input)))
    }

    fn resolve(&mut self, _env: Env, dump: Dump) -> Result<NativeReader> {
        Ok(NativeReader { dump })
    }
}

/// Open a dump: a path or an http(s) URL.
#[napi(ts_return_type = "Promise<NativeReader>")]
pub fn open(input: String, options: Option<NativeOpenOptions>) -> AsyncTask<OpenTask> {
    let http = http_options(options.unwrap_or_default());
    AsyncTask::new(OpenTask { input, http })
}

/// Options for `buildRedirectIndex`.
#[napi(object)]
#[derive(Default)]
pub struct NativeRedirectOptions {
    /// The `redirect` table's SQL dump (path or URL).
    pub redirects: String,
    /// The `page` table's SQL dump (path or URL).
    pub pages: String,
    /// Worker threads.
    pub threads: Option<u32>,
    /// HTTP options.
    pub open: Option<NativeOpenOptions>,
}

/// Build a redirect index from the SQL dumps; the end event's stats carry
/// a `token` for `takeRedirectIndex`.
#[napi(ts_args_type = "options: NativeRedirectOptions, callback: (event: unknown) => void")]
pub fn build_redirects(options: NativeRedirectOptions, callback: JsFunction) -> Result<NativeStream> {
    let cb = stream::callback(&callback)?;
    let opts = RedirectOptions {
        threads: options.threads.unwrap_or(0) as usize,
        http: http_options(options.open.unwrap_or_default()),
    };
    let (r, p) = (Input::parse(&options.redirects), Input::parse(&options.pages));
    Ok(stream::spawn_task(cb, move |progress| {
        let (index, stats) = build_redirect_index(&r, &p, &opts, progress).map_err(|e| e.to_string())?;
        let mut v = to_value(&stats)?;
        v["token"] = indexes::redirects().leave(index).into();
        Ok(v)
    }))
}

/// Options for `liveChanges`.
#[napi(object)]
#[derive(Default)]
pub struct NativeLiveOptions {
    /// Only these items (numbers).
    pub items: Option<Uint32Array>,
    /// Only these entities (any kind).
    pub ids: Option<Vec<String>>,
    /// Resume after this event id.
    pub last_event_id: Option<String>,
    /// Start at this time.
    pub since: Option<String>,
    /// Namespaces (default `[0]`).
    pub namespaces: Option<Vec<i64>>,
    /// The wiki (default `wikidatawiki`).
    pub wiki: Option<String>,
    /// The stream's URL.
    pub url: Option<String>,
    /// Batches buffered ahead of the consumer.
    pub read_ahead: Option<u32>,
    /// HTTP options.
    pub open: Option<NativeOpenOptions>,
}

/// Stream live changes; batches are JSON arrays of changes.
#[napi(ts_args_type = "options: NativeLiveOptions, callback: (event: unknown) => void")]
pub fn live(options: NativeLiveOptions, callback: JsFunction) -> Result<NativeStream> {
    let cb = stream::callback(&callback)?;
    let mut opts = LiveOptions {
        last_event_id: options.last_event_id,
        since: options.since,
        http: http_options(options.open.unwrap_or_default()),
        ..Default::default()
    };
    if let Some(ns) = options.namespaces {
        opts.namespaces = ns;
    }
    if let Some(w) = options.wiki {
        opts.wiki = w;
    }
    if let Some(u) = options.url {
        opts.url = u;
    }
    if options.items.is_some() || options.ids.is_some() {
        let b = wikidata_filter::IdSetBuilder::new();
        if let Some(items) = &options.items {
            let mut keys: Vec<i64> = items.iter().map(|&n| i64::from(n)).collect();
            keys.sort_unstable();
            b.insert_all(&keys);
        }
        for id in options.ids.iter().flatten() {
            b.insert(wikidata_filter::id_key(id).ok_or_else(|| invalid(format!("ids: not an entity id: {id:?}")))?);
        }
        opts.ids = Some(std::sync::Arc::new(b.freeze()));
    }
    Ok(stream::spawn_json(
        cb,
        options.read_ahead.unwrap_or(4),
        move |send, stop| {
            let stats = live_changes(&opts, stop, &mut |changes| match serde_json::to_string(&changes) {
                Ok(json) => send(json),
                Err(_) => Flow::Stop,
            })
            .map_err(|e| e.to_string())?;
            to_value(&stats)
        },
    ))
}

/// Options for `downloadDump`.
#[napi(object)]
#[derive(Default)]
pub struct NativeDownloadOptions {
    /// The dumps' directory.
    pub base: Option<String>,
    /// Mirrors of it.
    pub mirrors: Option<Vec<String>>,
    /// The date (default: the newest complete dump).
    pub date: Option<String>,
    /// The file of the dump (default `all.json.bz2`).
    pub file: Option<String>,
    /// SQL tables of the database dump instead (`["redirect", "page"]`).
    pub tables: Option<Vec<String>>,
    /// Where to put it.
    pub dir: String,
    /// Check the SHA-1 (default true).
    pub verify: Option<bool>,
    /// HTTP options (connections per server, range size, …).
    pub open: Option<NativeOpenOptions>,
}

fn download_json(s: &DownloadStats) -> serde_json::Value {
    serde_json::json!({
        "path": s.path.display().to_string(),
        "date": s.date,
        "bytes": s.bytes,
        "bytesFetched": s.bytes_fetched,
        "bytesResumed": s.bytes_resumed,
        "perServer": s.per_server,
        "sha1": s.sha1,
        "alreadyDone": s.already_done,
        "elapsedMs": s.elapsed_ms,
    })
}

/// Download a dump (or, with `tables`, SQL tables of a database dump);
/// status events carry its progress.
#[napi(ts_args_type = "options: NativeDownloadOptions, callback: (event: unknown) => void")]
pub fn download_dump(options: NativeDownloadOptions, callback: JsFunction) -> Result<NativeStream> {
    let cb = stream::callback(&callback)?;
    let http = http_options(options.open.unwrap_or_default());
    let verify = options.verify.unwrap_or(true);
    let dir: std::path::PathBuf = options.dir.into();
    let mirrors = options.mirrors.unwrap_or_default();
    let tables = options.tables.filter(|t| !t.is_empty());
    let entity = DownloadOptions {
        base: options.base.clone().unwrap_or_else(|| ENTITY_DUMPS.into()),
        mirrors: mirrors.clone(),
        date: options.date.clone(),
        file: options.file.unwrap_or_else(|| "all.json.bz2".into()),
        dir: dir.clone(),
        http: http.clone(),
        verify,
    };
    let sql = tables.map(|tables| SqlDownloadOptions {
        base: options.base.unwrap_or_else(|| WIKI_DUMPS.into()),
        mirrors,
        date: options.date,
        tables,
        dir,
        http,
        verify,
    });
    Ok(stream::spawn_status(cb, move |status| {
        let mut on = |p: &wikidata_dump::download::DownloadProgress| {
            let v = serde_json::json!({
                "file": p.file,
                "bytesDone": p.bytes_done,
                "bytesTotal": p.bytes_total,
                "bytesFetched": p.bytes_fetched,
                "bytesHashed": p.bytes_hashed,
                "perServer": p.per_server,
                "elapsedMs": p.elapsed_ms,
            });
            status(v) == Flow::Continue
        };
        match &sql {
            Some(sql) => {
                let files = download_sql(sql, &mut on).map_err(|e| e.to_string())?;
                let date = files.first().map(|f| f.date.clone()).unwrap_or_default();
                Ok(serde_json::json!({ "date": date, "files": files.iter().map(download_json).collect::<Vec<_>>() }))
            }
            None => Ok(download_json(&download(&entity, &mut on).map_err(|e| e.to_string())?)),
        }
    }))
}

/// Looks up the newest complete dump on the libuv pool.
pub struct LatestTask {
    options: NativeDownloadOptions,
}

impl Task for LatestTask {
    type Output = DumpStatus;
    type JsValue = napi::JsUnknown;

    fn compute(&mut self) -> Result<DumpStatus> {
        let o = &self.options;
        let http = http_options(NativeOpenOptions {
            user_agent: o.open.as_ref().and_then(|x| x.user_agent.clone()),
            retries: o.open.as_ref().and_then(|x| x.retries),
            ..Default::default()
        });
        let r = match o.tables.as_ref().filter(|t| !t.is_empty()) {
            Some(tables) => latest_sql_dump(o.base.as_deref().unwrap_or(WIKI_DUMPS), tables, &http),
            None => latest_entity_dump(
                o.base.as_deref().unwrap_or(ENTITY_DUMPS),
                o.file.as_deref().unwrap_or("all.json.bz2"),
                &http,
            ),
        };
        r.map_err(|e| invalid(e.to_string()))
    }

    fn resolve(&mut self, env: Env, s: DumpStatus) -> Result<napi::JsUnknown> {
        let files: Vec<serde_json::Value> = s
            .files
            .iter()
            .map(|f| {
                serde_json::json!({
                    "part": f.part, "name": f.name, "url": f.url,
                    "size": f.size.map(|n| n as f64), "sha1": f.sha1,
                })
            })
            .collect();
        env.to_js_value(&serde_json::json!({ "date": s.date, "files": files }))
    }
}

/// The newest complete dump: the entity dump of `file`, or, with
/// `tables`, the database dump whose tables are all done.
#[napi(ts_return_type = "Promise<unknown>")]
pub fn latest_dump(options: NativeDownloadOptions) -> AsyncTask<LatestTask> {
    AsyncTask::new(LatestTask { options })
}

/// The library version.
#[napi]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Worker threads still running (for tests: 0 once every task ended).
#[napi]
pub fn active_threads() -> u32 {
    wikidata_dump::live_threads() as u32
}

/// The external id table: `[key, property, formatter]` rows.
#[napi]
pub fn external_id_table() -> Vec<Vec<String>> {
    wikidata_model::EXTERNAL_IDS
        .iter()
        .map(|e| vec![e.key.to_string(), e.property.to_string(), e.formatter.to_string()])
        .collect()
}
