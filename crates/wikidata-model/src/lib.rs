#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

mod entity;
mod external;
mod options;
mod parse;
mod peek;
pub mod sites;
pub mod time;

pub use entity::{
    Coordinate, Derived, Entity, EntityType, Form, LexemeData, Map, Pid, Quantity, Rank, Sense, Sitelink, Snaks,
    Statement, Text, Time, Value,
};
pub use external::{external_id, external_id_url, ExternalId, EXTERNAL_IDS};
pub use options::{Compiled, ModelOptions};
pub use parse::{parse_entity, parse_entity_with, truthy, Backend, ParseError};
pub use peek::{id_number, lastrevid, peek, Peek};

/// The properties the derived fields are computed from (besides
/// [`EXTERNAL_IDS`]): P31, P279, P17, P625, P571, P576, P18, P856.
pub fn derived_properties() -> impl Iterator<Item = Pid> {
    options::DERIVED.iter().map(|(p, _)| Pid(*p))
}
