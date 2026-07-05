//! Core data model shared across the scanner, parsers, resolver, diagnostics,
//! reports, and TUI layers of envlens. Field names and semantics here are the
//! authoritative vocabulary for the rest of the pipeline (spec §4).

use std::path::PathBuf;

/// Stable identifier for an [`EnvSource`], e.g. `.env`, `apps/web/.env`,
/// `docker-compose.yml[api]`, `package.json[dev]`, `process`.
pub type SourceId = String;

/// The kind of thing an [`EnvSource`] was discovered from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum SourceKind {
    Dotenv,
    DotenvExample,
    Compose,
    PackageScript,
    /// `pnpm-workspace.yaml` / `turbo.json` / `nx.json`: discovered and listed
    /// as sources, zero variables in v0.1, details pane shows a
    /// "no env contribution" note.
    Manifest,
    Process,
    Ci,
}

/// A parse-time error encountered while reading a source.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ParseError {
    pub line: Option<u32>,
    pub message: String,
}

/// A single discovered source of environment variables.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EnvSource {
    /// e.g. ".env", "apps/web/.env", "docker-compose.yml[api]",
    /// "package.json[dev]", "process".
    pub id: SourceId,
    pub kind: SourceKind,
    /// Project-relative; `None` for the process source.
    pub path: Option<PathBuf>,
    /// Service or script name, when applicable.
    pub context: Option<String>,
    /// Final resolved rank; higher wins.
    pub precedence: u32,
    pub enabled: bool,
    pub errors: Vec<ParseError>,
}

/// Classification of whether a key and/or value look like a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SecretClass {
    None,
    KeyLike,
    ValueLike,
    Both,
}

impl SecretClass {
    pub fn is_secret(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// A single occurrence of a variable in one source.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VariableOccurrence {
    pub key: String,
    /// `None` only for bare compose list keys.
    pub raw_value: Option<String>,
    pub parsed_value: Option<String>,
    pub source_id: SourceId,
    pub line: Option<u32>,
    pub is_empty: bool,
    pub is_inherited: bool,
    /// `true` for single-quoted dotenv values (no `${VAR}` expansion).
    pub no_expand: bool,
    pub secret: SecretClass,
}

/// Severity of a [`Diagnostic`]. Ordering (derived): `Info < Warning < Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// Stable code identifying the kind of a [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum DiagnosticCode {
    DuplicateKey,
    ConflictingValues,
    MissingRequired,
    EmptyRequired,
    UndefinedReference,
    CircularReference,
    InvalidDotenvLine,
    SecretInTrackedFile,
    InheritedUnresolved,
    ShadowedValue,
}

/// A single diagnostic finding produced by analysis.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
    pub key: Option<String>,
    pub source_id: Option<SourceId>,
    pub line: Option<u32>,
}

/// Aggregated view of a single variable key across all sources.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VariableSummary {
    pub key: String,
    /// `(effective value, winning source id)` — in that order. Both halves are
    /// `String`-typed, so construction sites must not swap them.
    pub effective: Option<(String, SourceId)>,
    /// Precedence ascending.
    pub occurrences: Vec<VariableOccurrence>,
    pub diagnostics: Vec<Diagnostic>,
    pub is_required: bool,
    pub is_missing: bool,
    pub is_secret_like: bool,
}

/// The full result of analyzing a project.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Analysis {
    pub root: PathBuf,
    /// "default" when none selected.
    pub profile: String,
    /// Precedence ascending; disabled sources retained.
    pub sources: Vec<EnvSource>,
    /// Sorted by key.
    pub variables: Vec<VariableSummary>,
    /// All diagnostics, ordered: severity desc, then key, then source_id.
    pub diagnostics: Vec<Diagnostic>,
}

#[cfg(test)]
mod tests {
    use super::Severity;

    #[test]
    fn severity_ordering() {
        assert!(Severity::Error > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
    }
}
