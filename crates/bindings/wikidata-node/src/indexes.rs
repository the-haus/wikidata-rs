//! Place and redirect indexes held by JS, like class indexes (see
//! [`crate::class_index`]): JS holds the native object, tasks refer to it
//! by a registry id.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use napi::bindgen_prelude::{AsyncTask, Uint32Array};
use napi::{Env, Result, Task};
use napi_derive::napi;
use wikidata_export::{PlaceIndex, RedirectIndex};

use crate::options::invalid;

/// Live indexes of one kind by id, and finished builds by token.
pub(crate) struct Registry<T> {
    live: Mutex<(u32, HashMap<u32, Weak<T>>)>,
    built: Mutex<HashMap<u32, T>>,
}

impl<T> Registry<T> {
    fn new() -> Self {
        Registry {
            live: Mutex::new((0, HashMap::new())),
            built: Mutex::new(HashMap::new()),
        }
    }

    fn register(&self, index: &Arc<T>) -> u32 {
        let mut r = self.live.lock().unwrap();
        r.0 += 1;
        let id = r.0;
        r.1.insert(id, Arc::downgrade(index));
        id
    }

    fn release(&self, id: u32) {
        self.live.lock().unwrap().1.remove(&id);
    }

    /// The index registered as `id`, while JS holds it.
    pub(crate) fn lookup(&self, id: u32) -> Option<Arc<T>> {
        self.live.lock().unwrap().1.get(&id).and_then(Weak::upgrade)
    }

    /// Leave a finished build for JS to take; its token.
    pub(crate) fn leave(&self, index: T) -> u32 {
        static TOKENS: AtomicU32 = AtomicU32::new(0);
        let token = TOKENS.fetch_add(1, Ordering::Relaxed) + 1;
        self.built.lock().unwrap().insert(token, index);
        token
    }

    fn take(&self, token: u32) -> Option<T> {
        self.built.lock().unwrap().remove(&token)
    }
}

pub(crate) fn places() -> &'static Registry<PlaceIndex> {
    static R: OnceLock<Registry<PlaceIndex>> = OnceLock::new();
    R.get_or_init(Registry::new)
}

pub(crate) fn redirects() -> &'static Registry<RedirectIndex> {
    static R: OnceLock<Registry<RedirectIndex>> = OnceLock::new();
    R.get_or_init(Registry::new)
}

/// A place index (see `wikidata_filter::PlaceIndex`).
#[napi]
pub struct NativePlaceIndex {
    index: Arc<PlaceIndex>,
    id: u32,
}

impl Drop for NativePlaceIndex {
    fn drop(&mut self) {
        places().release(self.id);
    }
}

impl NativePlaceIndex {
    fn new(index: PlaceIndex) -> Self {
        let index = Arc::new(index);
        let id = places().register(&index);
        Self { index, id }
    }
}

#[napi]
impl NativePlaceIndex {
    /// The registry id, for task options.
    #[napi(getter)]
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Items with a country.
    #[napi(getter)]
    pub fn size(&self) -> u32 {
        self.index.len() as u32
    }

    /// Distinct sets of countries.
    #[napi(getter)]
    pub fn sets(&self) -> u32 {
        self.index.sets() as u32
    }

    /// Bytes in memory.
    #[napi(getter)]
    pub fn heap_bytes(&self) -> f64 {
        self.index.heap_bytes() as f64
    }

    /// The dump it was built from.
    #[napi(getter)]
    pub fn source(&self) -> String {
        self.index.source().to_string()
    }

    /// The countries of an item.
    #[napi]
    pub fn country_of(&self, id: String) -> Vec<String> {
        self.index.country_of(&id)
    }

    /// The countries of many items at once (item numbers): CSR arrays.
    #[napi(ts_return_type = "{ offsets: Uint32Array, countries: Uint32Array }")]
    pub fn countries_of(&self, env: Env, items: Uint32Array) -> Result<napi::JsObject> {
        let mut offsets = Vec::with_capacity(items.len() + 1);
        let mut countries = Vec::new();
        offsets.push(0u32);
        for &q in items.iter() {
            countries.extend_from_slice(self.index.countries(q));
            offsets.push(countries.len() as u32);
        }
        let mut o = env.create_object()?;
        o.set(
            "offsets",
            crate::batch::typed(&env, &offsets, napi::TypedArrayType::Uint32)?,
        )?;
        o.set(
            "countries",
            crate::batch::typed(&env, &countries, napi::TypedArrayType::Uint32)?,
        )?;
        Ok(o)
    }

    /// Write to a file.
    #[napi]
    pub fn save(&self, path: String) -> Result<()> {
        self.index.save(&path).map_err(|e| invalid(e.to_string()))
    }
}

/// A redirect index (see `wikidata_filter::RedirectIndex`).
#[napi]
pub struct NativeRedirectIndex {
    index: Arc<RedirectIndex>,
    id: u32,
}

impl Drop for NativeRedirectIndex {
    fn drop(&mut self) {
        redirects().release(self.id);
    }
}

impl NativeRedirectIndex {
    fn new(index: RedirectIndex) -> Self {
        let index = Arc::new(index);
        let id = redirects().register(&index);
        Self { index, id }
    }
}

#[napi]
impl NativeRedirectIndex {
    /// The registry id, for task options.
    #[napi(getter)]
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Redirects.
    #[napi(getter)]
    pub fn size(&self) -> u32 {
        self.index.len() as u32
    }

    /// The SQL dumps it was built from.
    #[napi(getter)]
    pub fn source(&self) -> String {
        self.index.source().to_string()
    }

    /// Where an item ends up, following redirects; `null` when it is not
    /// a redirect.
    #[napi]
    pub fn resolve(&self, id: String) -> Option<String> {
        self.index.resolve_id(&id)
    }

    /// Every redirect: `from` and `to` item numbers.
    #[napi(ts_return_type = "{ from: Uint32Array, to: Uint32Array }")]
    pub fn pairs(&self, env: Env) -> Result<napi::JsObject> {
        let (from, to): (Vec<u32>, Vec<u32>) = self.index.pairs().unzip();
        let mut o = env.create_object()?;
        o.set("from", crate::batch::typed(&env, &from, napi::TypedArrayType::Uint32)?)?;
        o.set("to", crate::batch::typed(&env, &to, napi::TypedArrayType::Uint32)?)?;
        Ok(o)
    }

    /// Write to a file.
    #[napi]
    pub fn save(&self, path: String) -> Result<()> {
        self.index.save(&path).map_err(|e| invalid(e.to_string()))
    }
}

/// Loads an index file on the libuv pool.
pub struct LoadPlaces {
    path: String,
}

impl Task for LoadPlaces {
    type Output = PlaceIndex;
    type JsValue = NativePlaceIndex;

    fn compute(&mut self) -> Result<PlaceIndex> {
        PlaceIndex::load(&self.path).map_err(|e| invalid(e.to_string()))
    }

    fn resolve(&mut self, _env: Env, index: PlaceIndex) -> Result<NativePlaceIndex> {
        Ok(NativePlaceIndex::new(index))
    }
}

/// Load a place index file.
#[napi(ts_return_type = "Promise<NativePlaceIndex>")]
pub fn load_place_index(path: String) -> AsyncTask<LoadPlaces> {
    AsyncTask::new(LoadPlaces { path })
}

/// Take the place index a finished build left under `token`.
#[napi]
pub fn take_place_index(token: u32) -> Result<NativePlaceIndex> {
    let index = places()
        .take(token)
        .ok_or_else(|| invalid("no place index was built under this token"))?;
    Ok(NativePlaceIndex::new(index))
}

/// Loads a redirect index file on the libuv pool.
pub struct LoadRedirects {
    path: String,
}

impl Task for LoadRedirects {
    type Output = RedirectIndex;
    type JsValue = NativeRedirectIndex;

    fn compute(&mut self) -> Result<RedirectIndex> {
        RedirectIndex::load(&self.path).map_err(|e| invalid(e.to_string()))
    }

    fn resolve(&mut self, _env: Env, index: RedirectIndex) -> Result<NativeRedirectIndex> {
        Ok(NativeRedirectIndex::new(index))
    }
}

/// Load a redirect index file.
#[napi(ts_return_type = "Promise<NativeRedirectIndex>")]
pub fn load_redirect_index(path: String) -> AsyncTask<LoadRedirects> {
    AsyncTask::new(LoadRedirects { path })
}

/// Take the redirect index a finished build left under `token`.
#[napi]
pub fn take_redirect_index(token: u32) -> Result<NativeRedirectIndex> {
    let index = redirects()
        .take(token)
        .ok_or_else(|| invalid("no redirect index was built under this token"))?;
    Ok(NativeRedirectIndex::new(index))
}
