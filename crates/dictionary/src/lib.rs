//! Read-only, versioned JMdict lookup and deterministic result ranking.

pub mod metadata;
pub mod query;
pub mod schema;

pub use metadata::{EnrichedDictionary, LexicalMetadata};
pub use query::SqliteDictionary;
