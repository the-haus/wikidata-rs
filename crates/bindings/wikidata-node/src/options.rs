//! JS options → task options.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use napi::bindgen_prelude::{Buffer, Float64Array, Uint32Array};
use napi::{Error, Result, Status};
use napi_derive::napi;
use serde::Deserialize;
use wikidata_export::{
    ByClaim, Checkpoint, CompiledFilter, Filter, Indexes, LabelOptions, ModelOptions, PlaceIndex, ScanOptions,
};
use wikidata_model::Pid;

use crate::class_index::lookup;
use crate::indexes::{places, redirects};

/// Options shared by every task. See the TypeScript docs.
#[napi(object)]
#[derive(Default)]
pub struct NativeOptions {
    /// A filter, as JSON; `"ids": {"$items": k}` stands for `idArrays[k]`.
    pub filter_json: Option<String>,
    /// Item numbers of `ids` filters given as typed arrays.
    pub id_arrays: Option<Vec<Uint32Array>>,
    /// Known revisions of `ids` filters; `"revisions": {"$revisions": k}`
    /// stands for `revisionArrays[k]`.
    pub revision_arrays: Option<Vec<Float64Array>>,
    /// A place index (its registry id) for `inCountry` and the language
    /// rules.
    pub place_index: Option<u32>,
    /// A redirect index (its registry id): `ids` follow merges.
    pub redirect_index: Option<u32>,
    /// Build the place index during the task and save it here.
    pub place_index_output: Option<String>,
    /// Send a checkpoint after every this many chunks.
    pub checkpoint_every: Option<u32>,
    /// Continue from a checkpoint's data.
    pub resume: Option<Buffer>,
    /// Labels of referenced items, as JSON (`{languages, output?}`).
    pub referenced_labels_json: Option<String>,
    /// Keep only these qualifiers.
    pub qualifier_properties: Option<Vec<String>>,
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

/// The place index the options name.
pub(crate) fn place_index(o: &NativeOptions) -> Result<Option<Arc<PlaceIndex>>> {
    match o.place_index {
        Some(id) => Ok(Some(
            places()
                .lookup(id)
                .ok_or_else(|| invalid("the place index was released"))?,
        )),
        None => Ok(None),
    }
}

pub(crate) fn filter(o: &NativeOptions) -> Result<Option<Arc<CompiledFilter>>> {
    let Some(json) = &o.filter_json else { return Ok(None) };
    let items: Vec<&[u32]> = o.id_arrays.iter().flatten().map(|a| &a[..]).collect();
    let revisions: Vec<Vec<u64>> = o
        .revision_arrays
        .iter()
        .flatten()
        .map(|a| {
            a.iter()
                .map(|&r| {
                    (r.is_finite() && r >= 0.0 && r.fract() == 0.0)
                        .then_some(r as u64)
                        .ok_or_else(|| invalid(format!("revisions: not a revision id: {r}")))
                })
                .collect::<Result<Vec<u64>>>()
        })
        .collect::<Result<_>>()?;
    let revisions: Vec<&[u64]> = revisions.iter().map(Vec::as_slice).collect();
    let f = Filter::parse_json_with_revisions(json, &items, &revisions).map_err(|e| invalid(e.to_string()))?;
    let index = match o.class_index {
        Some(id) => Some(lookup(id).ok_or_else(|| invalid("the class index was released"))?),
        None => None,
    };
    let redirects = match o.redirect_index {
        Some(id) => Some(
            redirects()
                .lookup(id)
                .ok_or_else(|| invalid("the redirect index was released"))?,
        ),
        None => None,
    };
    let indexes = Indexes {
        classes: index.as_deref(),
        places: place_index(o)?,
        redirects,
    };
    let c = f.compile_with(&indexes).map_err(|e| invalid(e.to_string()))?;
    Ok(Some(Arc::new(c)))
}

/// `{languages: [...], output?: "…"}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LabelsJson {
    pub languages: Vec<String>,
    #[serde(default)]
    pub output: Option<String>,
}

pub(crate) fn referenced_labels(o: &NativeOptions) -> Result<Option<(LabelOptions, Option<String>)>> {
    let Some(json) = &o.referenced_labels_json else {
        return Ok(None);
    };
    let r: LabelsJson = serde_json::from_str(json)
        .map_err(|e| invalid(format!("referencedLabels: expected {{ languages, output? }}: {e}")))?;
    if r.languages.is_empty() {
        return Err(invalid("referencedLabels.languages: name at least one language"));
    }
    Ok(Some((LabelOptions { languages: r.languages }, r.output)))
}

pub(crate) fn scan(o: &NativeOptions) -> Result<ScanOptions> {
    let limit = match o.limit {
        Some(l) if l.is_finite() && l >= 0.0 => Some(l as u64),
        Some(_) => return Err(invalid("limit must be a non-negative number")),
        None => None,
    };
    let resume = match &o.resume {
        Some(b) => Some(Arc::new(
            Checkpoint::from_bytes(b).map_err(|e| invalid(format!("resumeFrom: {e}")))?,
        )),
        None => None,
    };
    Ok(ScanOptions {
        filter: filter(o)?,
        threads: o.threads.unwrap_or(0) as usize,
        limit,
        allow_truncated: o.allow_truncated.unwrap_or(false),
        class_index: o.class_index_output.as_ref().map(PathBuf::from),
        place_index: o.place_index_output.as_ref().map(PathBuf::from),
        checkpoint_every: o.checkpoint_every.filter(|&n| n > 0).map(u64::from),
        resume,
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

pub(crate) fn by_claim(json: &Option<String>, what: &str, places: Option<Arc<PlaceIndex>>) -> Result<Option<ByClaim>> {
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
        places,
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
    let qualifier_properties = match &o.qualifier_properties {
        None => None,
        Some(ps) => Some(
            ps.iter()
                .map(|p| Pid::parse(p).ok_or_else(|| invalid(format!("qualifiers: not a property id: {p:?}"))))
                .collect::<Result<_>>()?,
        ),
    };
    Ok(ModelOptions {
        languages: o.languages.clone(),
        qualifier_properties,
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
