//! Parsers that turn raw source-file contents into structured entries
//! (spec §6). Each format gets its own submodule; mapping parsed entries
//! into [`crate::core::model::VariableOccurrence`]s is added alongside the
//! consumers that need it.

pub mod dotenv;
