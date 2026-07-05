//! Project configuration model (spec §7, §11).
//!
//! This is the minimal shape the resolver (Task 9) needs: precedence
//! overrides, custom profiles, and the fields that later tasks (config-file
//! loading, required/secret handling) will populate. Full config-file loading
//! is a later task; for now this type is constructed in-process and by tests.

use std::collections::BTreeMap;

/// What severity of finding should make the run fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailOn {
    Error,
    Warning,
}

/// A named profile: an ordered include list whose order *is* the precedence
/// (lowest first, highest last), matching the built-in profile semantics.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Profile {
    /// Ordered source tokens (filenames like `.env.local`, or the aliases
    /// `compose` / `scripts` / `process`). Order is precedence, lowest first.
    pub include: Vec<String>,
}

/// Project configuration. Constructed in-process for now; file loading lands
/// in a later task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub ignore: Vec<String>,
    pub required: Vec<String>,
    pub required_from_examples: bool,
    pub secret_patterns: Vec<String>,
    /// Source tokens to lift above their default rank, lowest first.
    pub precedence: Vec<String>,
    pub fail_on: FailOn,
    pub profiles: BTreeMap<String, Profile>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ignore: Vec::new(),
            required: Vec::new(),
            required_from_examples: true,
            secret_patterns: Vec::new(),
            precedence: Vec::new(),
            fail_on: FailOn::Error,
            profiles: BTreeMap::new(),
        }
    }
}
