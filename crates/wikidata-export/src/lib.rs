#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

mod classes;
mod entities;
mod extract;
mod info;
pub mod scan;

pub use classes::build_class_index;
pub use entities::{entities, ByClaim, EntityOptions, Record};
pub use extract::{extract, raw_lines, ExtractOptions, ExtractStats, OutputFormat};
pub use info::{file_info, FileInfo, LargestEntity};
pub use scan::{Event, Flow, Progress, SavedClassIndex, ScanOptions, ScanStats, TypeCounts};

pub use wikidata_dump::{Compression, Dump, DumpError, HttpOptions, Input, WriteOptions};
pub use wikidata_filter::{ClassIndex, CompiledFilter, Filter, FilterError};
pub use wikidata_model::{Compiled, Entity, ModelOptions};

/// Errors of a task.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// Reading or writing the dump.
    #[error(transparent)]
    Dump(#[from] DumpError),
    /// A bad filter or class index.
    #[error(transparent)]
    Filter(#[from] FilterError),
    /// A line that is not a valid entity.
    #[error("{0}")]
    Invalid(String),
}

impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        ExportError::Dump(e.into())
    }
}
