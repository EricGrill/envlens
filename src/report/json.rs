use serde::Serialize;

use crate::core::model::{Analysis, Diagnostic, EnvSource, ParseError, VariableOccurrence};
use crate::report::{
    count_severity, diagnostic_code_name, diagnostic_message, masked_value, sanitize_text,
    secret_class_name, severity_name, source_kind_name,
};

#[derive(Serialize)]
struct Report {
    version: u8,
    generated_at: String,
    root: String,
    profile: String,
    summary: Summary,
    sources: Vec<Source>,
    variables: Vec<Variable>,
    diagnostics: Vec<DiagnosticEntry>,
}

#[derive(Serialize)]
struct Summary {
    sources: usize,
    variables: usize,
    errors: usize,
    warnings: usize,
    infos: usize,
    secrets: usize,
    missing_required: usize,
}

#[derive(Serialize)]
struct Source {
    id: String,
    kind: &'static str,
    path: Option<String>,
    context: Option<String>,
    precedence: u32,
    enabled: bool,
    errors: Vec<SourceError>,
}

#[derive(Serialize)]
struct SourceError {
    line: Option<u32>,
    message: String,
}

#[derive(Serialize)]
struct Variable {
    key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    effective: Option<Effective>,
    occurrences: Vec<Occurrence>,
    is_required: bool,
    is_missing: bool,
    is_secret_like: bool,
    diagnostics: Vec<DiagnosticEntry>,
}

#[derive(Serialize)]
struct Effective {
    source_id: String,
    value: String,
}

#[derive(Serialize)]
struct Occurrence {
    source_id: String,
    line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    is_empty: bool,
    is_inherited: bool,
    no_expand: bool,
    secret: &'static str,
}

#[derive(Serialize)]
struct DiagnosticEntry {
    severity: &'static str,
    code: &'static str,
    message: String,
    key: Option<String>,
    source_id: Option<String>,
    line: Option<u32>,
}

pub fn render(
    analysis: &Analysis,
    generated_at: impl Into<String>,
    no_values: bool,
) -> Result<String, serde_json::Error> {
    let report = Report {
        version: 1,
        generated_at: generated_at.into(),
        root: analysis.root.to_string_lossy().into_owned(),
        profile: analysis.profile.clone(),
        summary: Summary {
            sources: analysis.sources.len(),
            variables: analysis.variables.len(),
            errors: count_severity(analysis, crate::core::model::Severity::Error),
            warnings: count_severity(analysis, crate::core::model::Severity::Warning),
            infos: count_severity(analysis, crate::core::model::Severity::Info),
            secrets: analysis
                .variables
                .iter()
                .filter(|variable| variable.is_secret_like)
                .count(),
            missing_required: analysis
                .variables
                .iter()
                .filter(|variable| variable.is_required && variable.is_missing)
                .count(),
        },
        sources: analysis
            .sources
            .iter()
            .map(|source| source_json(source, no_values))
            .collect(),
        variables: analysis
            .variables
            .iter()
            .map(|variable| {
                let effective = if no_values {
                    None
                } else {
                    variable
                        .effective
                        .as_ref()
                        .map(|(value, source_id)| Effective {
                            source_id: source_id.clone(),
                            value: masked_value(value, variable.is_secret_like),
                        })
                };

                Variable {
                    key: variable.key.clone(),
                    effective,
                    occurrences: variable
                        .occurrences
                        .iter()
                        .map(|occurrence| {
                            occurrence_json(occurrence, variable.is_secret_like, no_values)
                        })
                        .collect(),
                    is_required: variable.is_required,
                    is_missing: variable.is_missing,
                    is_secret_like: variable.is_secret_like,
                    diagnostics: variable
                        .diagnostics
                        .iter()
                        .map(|diagnostic| diagnostic_json(diagnostic, no_values))
                        .collect(),
                }
            })
            .collect(),
        diagnostics: analysis
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic_json(diagnostic, no_values))
            .collect(),
    };

    serde_json::to_string_pretty(&report)
}

fn source_json(source: &EnvSource, no_values: bool) -> Source {
    Source {
        id: source.id.clone(),
        kind: source_kind_name(source.kind),
        path: source
            .path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        context: source.context.clone(),
        precedence: source.precedence,
        enabled: source.enabled,
        errors: source
            .errors
            .iter()
            .map(|error| source_error_json(error, no_values))
            .collect(),
    }
}

fn source_error_json(error: &ParseError, no_values: bool) -> SourceError {
    SourceError {
        line: error.line,
        message: if no_values {
            "source parse error".to_string()
        } else {
            sanitize_text(&error.message)
        },
    }
}

fn occurrence_json(
    occurrence: &VariableOccurrence,
    parent_is_secret_like: bool,
    no_values: bool,
) -> Occurrence {
    let is_secret = parent_is_secret_like || occurrence.secret.is_secret();
    Occurrence {
        source_id: occurrence.source_id.clone(),
        line: occurrence.line,
        raw: if no_values {
            None
        } else {
            occurrence
                .raw_value
                .as_ref()
                .map(|value| masked_value(value, is_secret))
        },
        value: if no_values {
            None
        } else {
            occurrence
                .parsed_value
                .as_ref()
                .map(|value| masked_value(value, is_secret))
        },
        is_empty: occurrence.is_empty,
        is_inherited: occurrence.is_inherited,
        no_expand: occurrence.no_expand,
        secret: secret_class_name(occurrence.secret),
    }
}

fn diagnostic_json(diagnostic: &Diagnostic, no_values: bool) -> DiagnosticEntry {
    DiagnosticEntry {
        severity: severity_name(diagnostic.severity),
        code: diagnostic_code_name(diagnostic.code),
        message: diagnostic_message(diagnostic, no_values),
        key: diagnostic.key.clone(),
        source_id: diagnostic.source_id.clone(),
        line: diagnostic.line,
    }
}
