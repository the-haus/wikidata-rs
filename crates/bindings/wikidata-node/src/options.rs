//! JS options → task options.

use std::sync::Arc;

use napi::{Error, Result, Status};
use napi_derive::napi;
use wikidata_export::{CompiledFilter, Filter, ModelOptions, ScanOptions};
use wikidata_model::Pid;

use crate::class_index::lookup;

/// Options shared by every task. See the TypeScript docs.
#[napi(object)]
#[derive(Default)]
pub struct NativeOptions {
    /// A filter, as JSON.
    pub filter_json: Option<String>,
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
    /// Sites to keep sitelinks for (default: all).
    pub sitelinks: Option<Vec<String>>,
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
    let f = Filter::parse_json(json).map_err(|e| invalid(e.to_string()))?;
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
    })
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
