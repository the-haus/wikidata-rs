//! Class indexes held by JS.
//!
//! JS holds a [`NativeClassIndex`]; tasks refer to it by a registry id in
//! their options (napi objects cannot carry references to native
//! classes), and look it up when they start.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use napi::bindgen_prelude::AsyncTask;
use napi::{Env, Result, Task};
use napi_derive::napi;
use wikidata_filter::ClassIndex;

use crate::options::invalid;

/// The last id handed out, and the live indexes by id.
type Registry = Mutex<(u32, HashMap<u32, Weak<ClassIndex>>)>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn register(index: &Arc<ClassIndex>) -> u32 {
    let mut r = registry().lock().unwrap();
    r.0 += 1;
    let id = r.0;
    r.1.insert(id, Arc::downgrade(index));
    id
}

/// The index registered as `id`, while JS holds it.
pub(crate) fn lookup(id: u32) -> Option<Arc<ClassIndex>> {
    registry().lock().unwrap().1.get(&id).and_then(Weak::upgrade)
}

/// A class index (see `wikidata_filter::ClassIndex`).
#[napi]
pub struct NativeClassIndex {
    index: Arc<ClassIndex>,
    id: u32,
}

impl Drop for NativeClassIndex {
    fn drop(&mut self) {
        registry().lock().unwrap().1.remove(&self.id);
    }
}

impl NativeClassIndex {
    pub(crate) fn new(index: ClassIndex) -> Self {
        let index = Arc::new(index);
        let id = register(&index);
        Self { index, id }
    }
}

fn qnum(id: &str) -> Result<u32> {
    id.strip_prefix('Q')
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| invalid(format!("not an item id: {id:?}")))
}

#[napi]
impl NativeClassIndex {
    /// The registry id, for task options.
    #[napi(getter)]
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Classes with at least one parent.
    #[napi(getter)]
    pub fn size(&self) -> u32 {
        self.index.len() as u32
    }

    /// Subclass-of edges.
    #[napi(getter)]
    pub fn edges(&self) -> u32 {
        self.index.edges() as u32
    }

    /// The dump it was built from.
    #[napi(getter)]
    pub fn source(&self) -> String {
        self.index.source().to_string()
    }

    /// Direct parents.
    #[napi]
    pub fn parents(&self, id: String) -> Result<Vec<String>> {
        Ok(self.index.parents(qnum(&id)?).iter().map(|q| format!("Q{q}")).collect())
    }

    /// The classes and all their subclasses, sorted by number.
    #[napi]
    pub fn subclasses_of(&self, ids: Vec<String>) -> Result<Vec<String>> {
        let roots = ids.iter().map(|i| qnum(i)).collect::<Result<Vec<_>>>()?;
        let mut all: Vec<u32> = self.index.subclasses(&roots).into_iter().collect();
        all.sort_unstable();
        Ok(all.into_iter().map(|q| format!("Q{q}")).collect())
    }

    /// Whether `id` is `ancestor` or a subclass of it.
    #[napi]
    pub fn is_subclass_of(&self, id: String, ancestor: String) -> Result<bool> {
        Ok(self.index.is_subclass_of(qnum(&id)?, qnum(&ancestor)?))
    }

    /// Write to a file.
    #[napi]
    pub fn save(&self, path: String) -> Result<()> {
        self.index.save(&path).map_err(|e| invalid(e.to_string()))
    }
}

/// Loads an index on the libuv pool.
pub struct LoadTask {
    path: String,
}

impl Task for LoadTask {
    type Output = ClassIndex;
    type JsValue = NativeClassIndex;

    fn compute(&mut self) -> Result<ClassIndex> {
        ClassIndex::load(&self.path).map_err(|e| invalid(e.to_string()))
    }

    fn resolve(&mut self, _env: Env, index: ClassIndex) -> Result<NativeClassIndex> {
        Ok(NativeClassIndex::new(index))
    }
}

/// Load a class index file.
#[napi(ts_return_type = "Promise<NativeClassIndex>")]
pub fn load_class_index(path: String) -> AsyncTask<LoadTask> {
    AsyncTask::new(LoadTask { path })
}

/// Where a running build leaves its index (taken by `takeClassIndex`).
pub(crate) fn built() -> &'static Mutex<HashMap<u32, ClassIndex>> {
    static B: OnceLock<Mutex<HashMap<u32, ClassIndex>>> = OnceLock::new();
    B.get_or_init(Default::default)
}

/// Take the index a finished `buildClassIndex` task left under `token`.
#[napi]
pub fn take_class_index(token: u32) -> Result<NativeClassIndex> {
    let index = built()
        .lock()
        .unwrap()
        .remove(&token)
        .ok_or_else(|| invalid("no class index was built under this token"))?;
    Ok(NativeClassIndex::new(index))
}
