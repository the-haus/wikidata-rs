#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

pub mod classes;
pub mod filter;
pub mod ids;
mod json;
pub mod places;
pub mod redirects;

pub use classes::ClassIndex;
pub use filter::{CompiledFilter, Filter, Indexes, Merged, Misses, Pre};
pub use ids::{id_from_key, id_key, IdSet, IdSetBuilder};
pub use places::{place_edge, PlaceIndex, PlaceProperty};
pub use redirects::RedirectIndex;

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
