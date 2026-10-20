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
mod options;
mod stream;

use std::sync::atomic::{AtomicU32, Ordering};

use napi::bindgen_prelude::AsyncTask;
use napi::{Env, JsFunction, Result, Task};
use napi_derive::napi;
use wikidata_export::{Dump, EntityOptions, ExtractOptions, HttpOptions, ScanOptions, WriteOptions};

pub use class_index::{load_class_index, take_class_index, LoadTask, NativeClassIndex};
use options::invalid;
pub use options::NativeOptions;
pub use stream::NativeStream;

#[cfg(feature = "mimalloc")]
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
        let opts = EntityOptions {
            scan: options::scan(&options)?,
            model: options::model(&options)?,
            json: options.json.unwrap_or(true),
            summary_only: true,
            languages_by: options::by_claim(&options.languages_by_json, "languages")?,
            sitelinks_by: options::by_claim(&options.sitelinks_by_json, "sitelinks")?,
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
            to_value(&stats)
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
    let o = options.unwrap_or_default();
    let mut http = HttpOptions::default();
    if let Some(ua) = o.user_agent {
        http.user_agent = ua;
    }
    if let Some(r) = o.retries {
        http.retries = r;
    }
    AsyncTask::new(OpenTask { input, http })
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
