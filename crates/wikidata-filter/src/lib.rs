#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

pub mod classes;
pub mod filter;
pub mod ids;

pub use classes::ClassIndex;
pub use filter::{CompiledFilter, Filter, Pre};
pub use ids::{id_key, IdSet, IdSetBuilder};

/// Errors in filters and class indexes.
#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    /// A malformed filter or file.
    #[error("{0}")]
    Invalid(String),
    /// File system error.
    #[error("{0}")]
    Io(String),
}
