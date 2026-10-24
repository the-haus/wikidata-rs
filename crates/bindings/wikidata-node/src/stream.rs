//! A running task: a producer thread, a threadsafe function that hands
//! events to JS, and the credits that bound how far it runs ahead.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use napi::threadsafe_function::{ErrorStrategy, ThreadSafeCallContext, ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::{Env, JsFunction, JsObject, JsUnknown, Result, TypedArrayType};
use napi_derive::napi;
use wikidata_export::{Checkpoint, Event, Flow, Progress, Record};

use crate::batch::{typed, EntityBatch};
use crate::credits::Credits;

/// What the producer sends to JS.
pub(crate) enum StreamEvent {
    Batch(Box<EntityBatch>),
    /// A batch of JSON values (live changes): `{kind: "json", json}`.
    Json(String),
    Progress(Progress),
    /// Progress of another shape (downloads), passed on as it is.
    Status(serde_json::Value),
    Checkpoint(Box<Checkpoint>),
    End(serde_json::Value),
    Error(String),
}

impl StreamEvent {
    fn into_js(self, env: &Env) -> Result<JsObject> {
        let mut o = env.create_object()?;
        match self {
            StreamEvent::Batch(b) => {
                o.set("type", "batch")?;
                o.set("batch", b.into_js(env)?)?;
            }
            StreamEvent::Json(json) => {
                let mut b = env.create_object()?;
                b.set("kind", "json")?;
                b.set("json", env.create_string_from_std(json)?)?;
                o.set("type", "batch")?;
                o.set("batch", b)?;
            }
            StreamEvent::Status(v) => {
                o.set("type", "status")?;
                o.set("status", env.to_js_value(&as_doubles(v))?)?;
            }
            StreamEvent::Checkpoint(cp) => {
                o.set("type", "checkpoint")?;
                let p = &cp.position;
                o.set("input", cp.input.as_str())?;
                o.set("offset", (p.bit / 8) as f64)?;
                o.set("block", p.blocks as f64)?;
                o.set("chunk", p.chunks as f64)?;
                o.set("bytesOut", p.offset as f64)?;
                o.set("entities", cp.entities as f64)?;
                o.set("matched", cp.matched as f64)?;
                o.set("data", typed(env, &cp.to_bytes(), TypedArrayType::Uint8)?)?;
            }
            StreamEvent::Progress(p) => {
                o.set("type", "progress")?;
                o.set("bytesIn", p.bytes_in as f64)?;
                o.set("bytesTotal", p.bytes_total.map(|t| t as f64))?;
                o.set("bytesOut", p.bytes_out as f64)?;
                o.set("entities", p.entities as f64)?;
                o.set("matched", p.matched as f64)?;
                o.set("elapsedMs", p.elapsed_ms as f64)?;
            }
            StreamEvent::End(mut stats) => {
                o.set("type", "end")?;
                let missing = stats.as_object_mut().and_then(|s| s.remove("missingIds"));
                let mut js = env.to_js_value(&as_doubles(stats))?.coerce_to_object()?;
                if let Some(serde_json::Value::Array(ids)) = missing {
                    // Missing items cross as numbers (there can be a
                    // million); JS makes the `missingIds` strings on demand.
                    let mut items = Vec::new();
                    let mut others = Vec::new();
                    for id in ids.iter().filter_map(|v| v.as_str()) {
                        match id.strip_prefix('Q').and_then(|n| n.parse::<u32>().ok()) {
                            Some(n) => items.push(n),
                            None => others.push(id.to_string()),
                        }
                    }
                    js.set("missingItemNumbers", typed(env, &items, TypedArrayType::Uint32)?)?;
                    js.set("missingOtherIds", others)?;
                }
                o.set("stats", js)?;
            }
            StreamEvent::Error(message) => {
                o.set("type", "error")?;
                o.set("message", message)?;
            }
        }
        Ok(o)
    }
}

/// Integers as doubles: napi turns those beyond 32 bits into BigInts,
/// while JS wants plain numbers (counts and byte sizes fit in 53 bits).
fn as_doubles(v: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Number(n) if !n.is_f64() => n
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Number(n), Value::Number),
        Value::Array(a) => Value::Array(a.into_iter().map(as_doubles).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, as_doubles(v))).collect()),
        other => other,
    }
}

pub(crate) type Callback = ThreadsafeFunction<StreamEvent, ErrorStrategy::Fatal>;

pub(crate) fn callback(f: &JsFunction) -> Result<Callback> {
    f.create_threadsafe_function(0, |ctx: ThreadSafeCallContext<StreamEvent>| {
        let v: JsUnknown = ctx.value.into_js(&ctx.env)?.into_unknown();
        Ok(vec![v])
    })
}

/// The handle JS holds for a running task.
///
/// It keeps a handle on the threadsafe function so JS can decide when the
/// stream keeps the event loop alive: only while it awaits a batch (or the
/// end of a closed stream). An abandoned, half-read stream then does not
/// keep the process running.
#[napi]
pub struct NativeStream {
    pub(crate) credits: Arc<Credits>,
    pub(crate) callback: std::sync::Mutex<Option<Callback>>,
}

impl Drop for NativeStream {
    // Collected without being finished (JS dropped the stream): stop the
    // producer so it does not wait for credits forever.
    fn drop(&mut self) {
        self.credits.close();
    }
}

#[napi]
impl NativeStream {
    /// Whether the stream keeps the event loop alive.
    #[napi]
    pub fn set_ref(&self, env: Env, referenced: bool) -> napi::Result<()> {
        if let Some(cb) = self.callback.lock().unwrap().as_mut() {
            if referenced {
                cb.refer(&env)?;
            } else {
                cb.unref(&env)?;
            }
        }
        Ok(())
    }

    /// Drop this handle's reference to the callback (after the end).
    #[napi]
    pub fn release(&self) {
        self.callback.lock().unwrap().take();
    }

    /// Allow `n` more batches to be sent.
    #[napi]
    pub fn request(&self, n: u32) {
        self.credits.grant(n);
    }

    /// Stop the task; no more batches are sent, then `end` arrives.
    #[napi]
    pub fn close(&self) {
        self.credits.close();
    }
}

/// Batches also flush at this many bytes of text, so a few huge entities
/// do not wait for thousands of others.
const MAX_BATCH_BYTES: usize = 8 << 20;

/// Accumulates records into batches and sends them under flow control.
struct Sender {
    cb: Callback,
    credits: Arc<Credits>,
    batch: EntityBatch,
    batch_size: usize,
    progress: Throttle,
    /// The filter's names, sent with each batch.
    names: Arc<[String]>,
}

impl Sender {
    fn flush(&mut self) -> Flow {
        if self.batch.len() == 0 {
            return Flow::Continue;
        }
        if !self.credits.acquire() {
            return Flow::Stop;
        }
        let mut b = std::mem::take(&mut self.batch);
        b.names = self.names.clone();
        self.cb
            .call(StreamEvent::Batch(Box::new(b)), ThreadsafeFunctionCallMode::Blocking);
        Flow::Continue
    }

    fn on_event(&mut self, e: Event<Record>) -> Flow {
        if self.credits.is_closed() {
            return Flow::Stop;
        }
        match e {
            Event::Checkpoint(cp) => {
                // After the records before it, in their own batch.
                if self.flush() == Flow::Stop {
                    return Flow::Stop;
                }
                self.cb
                    .call(StreamEvent::Checkpoint(cp), ThreadsafeFunctionCallMode::Blocking);
                Flow::Continue
            }
            Event::Records(rs) => {
                for r in rs {
                    self.batch.push(r);
                    if (self.batch.len() >= self.batch_size || self.batch.bytes() >= MAX_BATCH_BYTES)
                        && self.flush() == Flow::Stop
                    {
                        return Flow::Stop;
                    }
                }
                Flow::Continue
            }
            Event::Progress(p) => {
                self.progress.offer(&self.cb, p);
                Flow::Continue
            }
        }
    }
}

/// Sends progress at most every 200 ms, keeping the latest event it held
/// back so that [`Throttle::finish`] can send it: the last progress a
/// consumer sees always carries the task's final totals.
#[derive(Default)]
struct Throttle {
    last: Option<Instant>,
    held: Option<Progress>,
}

impl Throttle {
    fn offer(&mut self, cb: &Callback, p: Progress) {
        let now = Instant::now();
        if self.last.is_none_or(|t| now - t >= Duration::from_millis(200)) {
            self.last = Some(now);
            self.held = None;
            cb.call(StreamEvent::Progress(p), ThreadsafeFunctionCallMode::NonBlocking);
        } else {
            self.held = Some(p);
        }
    }

    /// Send the held-back event, if any (before the task's end event).
    fn finish(&mut self, cb: &Callback) {
        if let Some(p) = self.held.take() {
            cb.call(StreamEvent::Progress(p), ThreadsafeFunctionCallMode::Blocking);
        }
    }
}

fn handle(credits: Arc<Credits>, cb: &Callback) -> NativeStream {
    NativeStream {
        credits,
        callback: std::sync::Mutex::new(Some(cb.clone())),
    }
}

/// Run `job` on its own thread, streaming entity batches to `cb`.
pub(crate) fn spawn_entities<F>(
    cb: Callback,
    batch_size: usize,
    read_ahead: u32,
    names: Vec<String>,
    job: F,
) -> NativeStream
where
    F: FnOnce(&mut dyn FnMut(Event<Record>) -> Flow) -> std::result::Result<serde_json::Value, String> + Send + 'static,
{
    let credits = Arc::new(Credits::default());
    credits.grant(read_ahead.max(1));
    let h = handle(credits.clone(), &cb);
    thread::Builder::new()
        .name("wikidata-entities".into())
        .spawn(move || {
            let mut sender = Sender {
                cb: cb.clone(),
                credits,
                batch: EntityBatch::default(),
                batch_size: batch_size.max(1),
                progress: Throttle::default(),
                names: names.into(),
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut |e| sender.on_event(e))))
                .unwrap_or_else(|p| Err(format!("internal error: {}", panic_message(&p))));
            let result = result.map(|mut stats| {
                if sender.flush() == Flow::Stop {
                    // Closed before the last batch: the pass was cut short
                    // for the consumer, so it reports no missing ids.
                    stats["stopped"] = true.into();
                    if let Some(o) = stats.as_object_mut() {
                        o.remove("missingIds");
                    }
                }
                stats
            });
            if !sender.credits.is_closed() {
                sender.progress.finish(&cb);
            }
            let event = match result {
                Ok(stats) => StreamEvent::End(stats),
                Err(e) => StreamEvent::Error(e),
            };
            cb.call(event, ThreadsafeFunctionCallMode::Blocking);
        })
        .expect("spawn entities thread");
    h
}

/// Run `job` (which sends no records) on its own thread, reporting its
/// progress (throttled) and then its end or error to `cb`. The job's
/// progress callback says `Stop` once JS closes the stream.
pub(crate) fn spawn_task<F>(cb: Callback, job: F) -> NativeStream
where
    F: FnOnce(&mut dyn FnMut(Progress) -> Flow) -> std::result::Result<serde_json::Value, String> + Send + 'static,
{
    let credits = Arc::new(Credits::default());
    let h = handle(credits.clone(), &cb);
    thread::Builder::new()
        .name("wikidata-task".into())
        .spawn(move || {
            let mut throttle = Throttle::default();
            let mut on_progress = |p: Progress| {
                if credits.is_closed() {
                    return Flow::Stop;
                }
                throttle.offer(&cb, p);
                Flow::Continue
            };
            let event = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut on_progress)))
                .unwrap_or_else(|p| Err(format!("internal error: {}", panic_message(&p))))
                .map_or_else(StreamEvent::Error, StreamEvent::End);
            if !credits.is_closed() {
                throttle.finish(&cb);
            }
            cb.call(event, ThreadsafeFunctionCallMode::Blocking);
        })
        .expect("spawn task thread");
    h
}

/// The message of a caught panic.
pub(crate) fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a worker thread panicked".into())
}

/// Run `job` on its own thread, sending the JSON batches it makes under
/// flow control (as entity batches are). The job asks `stop` whether JS
/// closed the stream.
pub(crate) fn spawn_json<F>(cb: Callback, read_ahead: u32, job: F) -> NativeStream
where
    F: FnOnce(
            &mut dyn FnMut(String) -> Flow,
            &(dyn Fn() -> bool + Sync),
        ) -> std::result::Result<serde_json::Value, String>
        + Send
        + 'static,
{
    let credits = Arc::new(Credits::default());
    credits.grant(read_ahead.max(1));
    let h = handle(credits.clone(), &cb);
    thread::Builder::new()
        .name("wikidata-json".into())
        .spawn(move || {
            let send_cb = cb.clone();
            let send_credits = credits.clone();
            let mut send = move |json: String| {
                if !send_credits.acquire() {
                    return Flow::Stop;
                }
                send_cb.call(StreamEvent::Json(json), ThreadsafeFunctionCallMode::Blocking);
                Flow::Continue
            };
            let stop_credits = credits.clone();
            let stop = move || stop_credits.is_closed();
            let event = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut send, &stop)))
                .unwrap_or_else(|p| Err(format!("internal error: {}", panic_message(&p))))
                .map_or_else(StreamEvent::Error, StreamEvent::End);
            cb.call(event, ThreadsafeFunctionCallMode::Blocking);
        })
        .expect("spawn json thread");
    h
}

/// Run `job` (a download) on its own thread, passing its status on at most
/// every 200 ms (and the last one before the end); the status callback says
/// `Stop` once JS closes the stream.
pub(crate) fn spawn_status<F>(cb: Callback, job: F) -> NativeStream
where
    F: FnOnce(&mut dyn FnMut(serde_json::Value) -> Flow) -> std::result::Result<serde_json::Value, String>
        + Send
        + 'static,
{
    let credits = Arc::new(Credits::default());
    let h = handle(credits.clone(), &cb);
    thread::Builder::new()
        .name("wikidata-task".into())
        .spawn(move || {
            let mut last: Option<Instant> = None;
            let mut held = None;
            let mut on_status = |v: serde_json::Value| {
                if credits.is_closed() {
                    return Flow::Stop;
                }
                let now = Instant::now();
                if last.is_none_or(|t| now - t >= Duration::from_millis(200)) {
                    last = Some(now);
                    held = None;
                    cb.call(StreamEvent::Status(v), ThreadsafeFunctionCallMode::NonBlocking);
                } else {
                    held = Some(v);
                }
                Flow::Continue
            };
            let event = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut on_status)))
                .unwrap_or_else(|p| Err(format!("internal error: {}", panic_message(&p))))
                .map_or_else(StreamEvent::Error, StreamEvent::End);
            if let (Some(v), false) = (held, credits.is_closed()) {
                cb.call(StreamEvent::Status(v), ThreadsafeFunctionCallMode::Blocking);
            }
            cb.call(event, ThreadsafeFunctionCallMode::Blocking);
        })
        .expect("spawn task thread");
    h
}
