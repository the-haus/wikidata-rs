//! JS options → task options.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use napi::bindgen_prelude::Uint32Array;
use napi::{Error, Result, Status};
use napi_derive::napi;
use serde::Deserialize;
use wikidata_export::{ByClaim, CompiledFilter, Filter, ModelOptions, ScanOptions};
use wikidata_model::Pid;

use crate::class_index::lookup;

/// Options shared by every task. See the TypeScript docs.
#[napi(object)]
#[derive(Default)]
pub struct NativeOptions {
    /// A filter, as JSON; `"ids": {"$items": k}` stands for `idArrays[k]`.
    pub filter_json: Option<String>,
    /// Item numbers of `ids` filters given as typed arrays.
    pub id_arrays: Option<Vec<Uint32Array>>,
    /// A class index (its registry id) for `instanceOf` with subclasses.
    pub class_index: Option<u32>,
    /// Worker threads (default: all cores).
    pub threads: Option<u32>,
    /// Stop after this many matches.
    pub limit: Option<f64>,
    /// Read a truncated input up to its last whole entity.
    pub allow_truncated: Option<bool>,
    /// Entities per batch (default 1024).
    pub batch_size: Option<u32>,
    /// Batches buffered ahead of the consumer (default 2).
    pub read_ahead: Option<u32>,
    /// Languages, in fallback order (default: all).
    pub languages: Option<Vec<String>>,
    /// Languages chosen by claims, as JSON (`{by, map, always}`).
    pub languages_by_json: Option<String>,
    /// Sites to keep sitelinks for (default: all).
    pub sitelinks: Option<Vec<String>>,
    /// Sites chosen by claims, as JSON.
    pub sitelinks_by_json: Option<String>,
    /// Build the class index during the task and save it here.
    pub class_index_output: Option<String>,
    /// Properties to keep claims for (default: all).
    pub properties: Option<Vec<String>>,
    /// Keep labels (default true).
    pub labels: Option<bool>,
    /// Keep descriptions (default true).
    pub descriptions: Option<bool>,
    /// Keep aliases (default true).
    pub aliases: Option<bool>,
    /// Keep qualifiers (default true).
    pub qualifiers: Option<bool>,
    /// Keep references (default false).
    pub references: Option<bool>,
    /// Keep deprecated statements (default false).
    pub deprecated: Option<bool>,
    /// Only best-ranked statements (default false).
    pub best_rank: Option<bool>,
    /// Send each entity's full JSON (default true).
    pub json: Option<bool>,
    /// `extract`: the output path.
    pub output: Option<String>,
    /// `extract`: compression level.
    pub level: Option<u32>,
}

pub(crate) fn invalid(msg: impl Into<String>) -> Error {
    Error::new(Status::InvalidArg, msg.into())
}

pub(crate) fn filter(o: &NativeOptions) -> Result<Option<Arc<CompiledFilter>>> {
    let Some(json) = &o.filter_json else { return Ok(None) };
    let items: Vec<&[u32]> = o.id_arrays.iter().flatten().map(|a| &a[..]).collect();
    let f = Filter::parse_json_with(json, &items).map_err(|e| invalid(e.to_string()))?;
    let index = match o.class_index {
        Some(id) => Some(lookup(id).ok_or_else(|| invalid("the class index was released"))?),
        None => None,
    };
    let c = f.compile(index.as_deref()).map_err(|e| invalid(e.to_string()))?;
    Ok(Some(Arc::new(c)))
}

pub(crate) fn scan(o: &NativeOptions) -> Result<ScanOptions> {
    let limit = match o.limit {
        Some(l) if l.is_finite() && l >= 0.0 => Some(l as u64),
        Some(_) => return Err(invalid("limit must be a non-negative number")),
        None => None,
    };
    Ok(ScanOptions {
        filter: filter(o)?,
        threads: o.threads.unwrap_or(0) as usize,
        limit,
        allow_truncated: o.allow_truncated.unwrap_or(false),
        class_index: o.class_index_output.as_ref().map(PathBuf::from),
    })
}

/// `{by: "P17" | ["P17", …], map: {Q55: ["nl"]}, always?: ["en"]}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ByClaimJson {
    by: OneOrMany,
    map: HashMap<String, Vec<String>>,
    #[serde(default)]
    always: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

pub(crate) fn by_claim(json: &Option<String>, what: &str) -> Result<Option<ByClaim>> {
    let Some(json) = json else { return Ok(None) };
    let r: ByClaimJson =
        serde_json::from_str(json).map_err(|e| invalid(format!("{what}: expected {{ by, map, always }}: {e}")))?;
    let by = match r.by {
        OneOrMany::One(p) => vec![p],
        OneOrMany::Many(ps) => ps,
    };
    let properties = by
        .iter()
        .map(|p| Pid::parse(p).ok_or_else(|| invalid(format!("{what}.by: not a property id: {p:?}"))))
        .collect::<Result<_>>()?;
    Ok(Some(ByClaim {
        properties,
        map: r.map,
        always: r.always,
    }))
}

pub(crate) fn model(o: &NativeOptions) -> Result<ModelOptions> {
    let properties = match &o.properties {
        None => None,
        Some(ps) => Some(
            ps.iter()
                .map(|p| Pid::parse(p).ok_or_else(|| invalid(format!("not a property id: {p:?}"))))
                .collect::<Result<_>>()?,
        ),
    };
    Ok(ModelOptions {
        languages: o.languages.clone(),
        sitelinks: o.sitelinks.clone(),
        properties,
        labels: o.labels.unwrap_or(true),
        descriptions: o.descriptions.unwrap_or(true),
        aliases: o.aliases.unwrap_or(true),
        qualifiers: o.qualifiers.unwrap_or(true),
        references: o.references.unwrap_or(false),
        deprecated: o.deprecated.unwrap_or(false),
        best_rank: o.best_rank.unwrap_or(false),
        derived: true,
    })
}
