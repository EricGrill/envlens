use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::core::model::{
    Analysis, Diagnostic, DiagnosticCode, EnvSource, Severity, SourceKind, VariableOccurrence,
    VariableSummary,
};
use crate::core::secrets::MaskedValue;

pub fn run(
    analysis: &mut Analysis,
    required: &BTreeSet<String>,
    tracked: Option<&BTreeSet<PathBuf>>,
) {
    let mut diagnostics = reference_diagnostics(analysis);
    ensure_required_variables(analysis, required);
    reset_variable_state(analysis, required);

    let sources = analysis.sources.clone();
    let variables = analysis.variables.clone();

    for source in &sources {
        diagnostics.extend(parse_error_diagnostics(source));
    }

    for var in &variables {
        diagnostics.extend(duplicate_key_diagnostics(var));
        diagnostics.extend(conflicting_value_diagnostics(var, &sources));
        diagnostics.extend(shadowed_value_diagnostics(var, &sources));
        diagnostics.extend(required_diagnostics(var, &sources, required));
        diagnostics.extend(inherited_unresolved_diagnostics(var, &sources));
        diagnostics.extend(secret_in_tracked_file_diagnostics(var, &sources, tracked));
    }

    sort_diagnostics(&mut diagnostics);
    diagnostics.dedup();

    for var in &mut analysis.variables {
        var.diagnostics = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.key.as_deref() == Some(var.key.as_str()))
            .cloned()
            .collect();
        if required.contains(&var.key) {
            var.is_missing = var
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::MissingRequired);
        }
    }

    analysis.diagnostics = diagnostics;
}

fn reference_diagnostics(analysis: &Analysis) -> Vec<Diagnostic> {
    analysis
        .diagnostics
        .iter()
        .chain(
            analysis
                .variables
                .iter()
                .flat_map(|var| var.diagnostics.iter()),
        )
        .filter(|diagnostic| {
            matches!(
                diagnostic.code,
                DiagnosticCode::UndefinedReference | DiagnosticCode::CircularReference
            )
        })
        .cloned()
        .collect()
}

fn ensure_required_variables(analysis: &mut Analysis, required: &BTreeSet<String>) {
    for key in required {
        if !analysis.variables.iter().any(|var| &var.key == key) {
            analysis.variables.push(VariableSummary {
                key: key.clone(),
                effective: None,
                occurrences: Vec::new(),
                diagnostics: Vec::new(),
                is_required: true,
                is_missing: false,
                is_secret_like: false,
            });
        }
    }
    analysis
        .variables
        .sort_by(|left, right| left.key.cmp(&right.key));
}

fn reset_variable_state(analysis: &mut Analysis, required: &BTreeSet<String>) {
    for var in &mut analysis.variables {
        var.is_required = required.contains(&var.key);
        var.is_missing = false;
        var.diagnostics.clear();
    }
}

fn parse_error_diagnostics(source: &EnvSource) -> Vec<Diagnostic> {
    source
        .errors
        .iter()
        .map(|error| {
            let location = match error.line {
                Some(line) => format!("{}:{line}", source.id),
                None => source.id.clone(),
            };
            Diagnostic {
                severity: Severity::Warning,
                code: DiagnosticCode::InvalidDotenvLine,
                message: format!("{location}: {}", error.message),
                key: None,
                source_id: Some(source.id.clone()),
                line: error.line,
            }
        })
        .collect()
}

fn duplicate_key_diagnostics(var: &VariableSummary) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut source_ids = BTreeSet::new();
    for occurrence in &var.occurrences {
        source_ids.insert(occurrence.source_id.as_str());
    }

    for source_id in source_ids {
        let occurrences: Vec<&VariableOccurrence> = var
            .occurrences
            .iter()
            .filter(|occurrence| occurrence.source_id == source_id)
            .collect();
        if occurrences.len() < 2 {
            continue;
        }
        let lines = line_list(&occurrences);
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            code: DiagnosticCode::DuplicateKey,
            message: format!(
                "{} is defined multiple times in {source_id} at {lines}. Last value wins within that source.",
                var.key
            ),
            key: Some(var.key.clone()),
            source_id: Some(source_id.to_string()),
            line: occurrences.iter().filter_map(|occurrence| occurrence.line).min(),
        });
    }

    diagnostics
}

fn conflicting_value_diagnostics(var: &VariableSummary, sources: &[EnvSource]) -> Vec<Diagnostic> {
    let candidates = source_candidates(var, sources);
    let distinct_values: BTreeSet<&str> = candidates
        .iter()
        .filter_map(|occurrence| occurrence.parsed_value.as_deref())
        .collect();
    if distinct_values.len() < 2 {
        return Vec::new();
    }

    let Some((effective_value, effective_source)) = &var.effective else {
        return Vec::new();
    };

    let severity = if same_file_subsource_candidates(&candidates, sources) {
        Severity::Info
    } else {
        Severity::Warning
    };
    let parts: Vec<String> = candidates
        .iter()
        .map(|occurrence| format_occurrence_value(var, occurrence))
        .collect();
    let effective = MaskedValue::new(effective_value, var.is_secret_like, false);

    vec![Diagnostic {
        severity,
        code: DiagnosticCode::ConflictingValues,
        message: format!(
            "{} differs across sources: {}. Effective value is {effective} from {effective_source}.",
            var.key,
            parts.join(", ")
        ),
        key: Some(var.key.clone()),
        source_id: Some(effective_source.clone()),
        line: winning_occurrence(var).and_then(|occurrence| occurrence.line),
    }]
}

fn shadowed_value_diagnostics(var: &VariableSummary, sources: &[EnvSource]) -> Vec<Diagnostic> {
    let Some((effective_value, effective_source)) = &var.effective else {
        return Vec::new();
    };
    if !source_is_enabled_value_bearing(sources, effective_source) {
        return Vec::new();
    }
    let Some(winning_index) = var
        .occurrences
        .iter()
        .rposition(|occurrence| occurrence.source_id == *effective_source)
    else {
        return Vec::new();
    };
    let Some(winner) = var.occurrences.get(winning_index) else {
        return Vec::new();
    };
    let winner_location = format_location(winner);
    let effective = MaskedValue::new(effective_value, var.is_secret_like, false);

    var.occurrences
        .iter()
        .take(winning_index)
        .filter(|occurrence| occurrence.source_id != *effective_source)
        .filter(|occurrence| occurrence.parsed_value.is_some())
        .filter(|occurrence| source_is_enabled_value_bearing(sources, &occurrence.source_id))
        .map(|occurrence| {
            let value = masked_occurrence_value(var, occurrence);
            Diagnostic {
                severity: Severity::Info,
                code: DiagnosticCode::ShadowedValue,
                message: format!(
                    "{} from {} ({value}) is shadowed by {winner_location} ({effective}).",
                    var.key,
                    format_location(occurrence)
                ),
                key: Some(var.key.clone()),
                source_id: Some(occurrence.source_id.clone()),
                line: occurrence.line,
            }
        })
        .collect()
}

fn required_diagnostics(
    var: &VariableSummary,
    sources: &[EnvSource],
    required: &BTreeSet<String>,
) -> Vec<Diagnostic> {
    if !required.contains(&var.key) {
        return Vec::new();
    }

    let defined: Vec<&VariableOccurrence> = var
        .occurrences
        .iter()
        .filter(|occurrence| occurrence.parsed_value.is_some())
        .filter(|occurrence| source_is_enabled_value_bearing(sources, &occurrence.source_id))
        .collect();

    if defined.is_empty() {
        return vec![Diagnostic {
            severity: Severity::Error,
            code: DiagnosticCode::MissingRequired,
            message: format!(
                "{} is required but is not defined in any enabled value-bearing source.",
                var.key
            ),
            key: Some(var.key.clone()),
            source_id: None,
            line: None,
        }];
    }

    if defined.iter().all(|occurrence| occurrence.is_empty) {
        return vec![Diagnostic {
            severity: Severity::Warning,
            code: DiagnosticCode::EmptyRequired,
            message: format!(
                "{} is required but every enabled definition is empty.",
                var.key
            ),
            key: Some(var.key.clone()),
            source_id: None,
            line: None,
        }];
    }

    Vec::new()
}

fn inherited_unresolved_diagnostics(
    var: &VariableSummary,
    sources: &[EnvSource],
) -> Vec<Diagnostic> {
    let process_has_key = var.occurrences.iter().any(|occurrence| {
        occurrence.parsed_value.is_some()
            && source_kind(sources, &occurrence.source_id) == Some(SourceKind::Process)
            && source_enabled(sources, &occurrence.source_id)
    });

    var.occurrences
        .iter()
        .filter(|occurrence| occurrence.is_inherited)
        .filter(|_| !process_has_key)
        .filter(|occurrence| {
            source_kind(sources, &occurrence.source_id) == Some(SourceKind::Compose)
                && source_enabled(sources, &occurrence.source_id)
        })
        .map(|occurrence| Diagnostic {
            severity: Severity::Info,
            code: DiagnosticCode::InheritedUnresolved,
            message: format!(
                "{} is inherited by {} but is not set in the process environment.",
                var.key, occurrence.source_id
            ),
            key: Some(var.key.clone()),
            source_id: Some(occurrence.source_id.clone()),
            line: occurrence.line,
        })
        .collect()
}

fn secret_in_tracked_file_diagnostics(
    var: &VariableSummary,
    sources: &[EnvSource],
    tracked: Option<&BTreeSet<PathBuf>>,
) -> Vec<Diagnostic> {
    let Some(tracked) = tracked else {
        return Vec::new();
    };

    var.occurrences
        .iter()
        .filter(|occurrence| occurrence.secret.is_secret())
        .filter(|occurrence| occurrence.parsed_value.is_some())
        .filter(|occurrence| !occurrence.is_empty)
        .filter_map(|occurrence| {
            let source = source_by_id(sources, &occurrence.source_id)?;
            let path = source.path.as_ref()?;
            if !tracked.contains(path) {
                return None;
            }
            let value = masked_occurrence_value(var, occurrence);
            Some(Diagnostic {
                severity: Severity::Warning,
                code: DiagnosticCode::SecretInTrackedFile,
                message: format!(
                    "Secret-like {} appears in tracked file {} ({value}).",
                    var.key,
                    format_location(occurrence)
                ),
                key: Some(var.key.clone()),
                source_id: Some(occurrence.source_id.clone()),
                line: occurrence.line,
            })
        })
        .collect()
}

fn source_candidates<'a>(
    var: &'a VariableSummary,
    sources: &[EnvSource],
) -> Vec<&'a VariableOccurrence> {
    let mut candidates: Vec<&VariableOccurrence> = Vec::new();
    for occurrence in &var.occurrences {
        if occurrence.parsed_value.is_none()
            || !source_is_enabled_value_bearing(sources, &occurrence.source_id)
        {
            continue;
        }
        if let Some(pos) = candidates
            .iter()
            .position(|candidate| candidate.source_id == occurrence.source_id)
        {
            candidates[pos] = occurrence;
        } else {
            candidates.push(occurrence);
        }
    }
    candidates
}

fn same_file_subsource_candidates(
    candidates: &[&VariableOccurrence],
    sources: &[EnvSource],
) -> bool {
    let Some(first) = candidates.first() else {
        return false;
    };
    let Some(first_source) = source_by_id(sources, &first.source_id) else {
        return false;
    };
    if !matches!(
        first_source.kind,
        SourceKind::Compose | SourceKind::PackageScript
    ) {
        return false;
    }
    let Some(first_path) = first_source.path.as_ref() else {
        return false;
    };

    candidates.iter().all(|candidate| {
        source_by_id(sources, &candidate.source_id).is_some_and(|source| {
            source.kind == first_source.kind && source.path.as_ref() == Some(first_path)
        })
    })
}

fn source_is_enabled_value_bearing(sources: &[EnvSource], source_id: &str) -> bool {
    source_by_id(sources, source_id).is_some_and(|source| {
        source.enabled
            && matches!(
                source.kind,
                SourceKind::Dotenv
                    | SourceKind::Compose
                    | SourceKind::PackageScript
                    | SourceKind::Process
            )
    })
}

fn source_enabled(sources: &[EnvSource], source_id: &str) -> bool {
    source_by_id(sources, source_id).is_some_and(|source| source.enabled)
}

fn source_kind(sources: &[EnvSource], source_id: &str) -> Option<SourceKind> {
    source_by_id(sources, source_id).map(|source| source.kind)
}

fn source_by_id<'a>(sources: &'a [EnvSource], source_id: &str) -> Option<&'a EnvSource> {
    sources.iter().find(|source| source.id == source_id)
}

fn winning_occurrence(var: &VariableSummary) -> Option<&VariableOccurrence> {
    let (_, source_id) = var.effective.as_ref()?;
    var.occurrences
        .iter()
        .rfind(|occurrence| occurrence.source_id == *source_id)
}

fn format_occurrence_value(var: &VariableSummary, occurrence: &VariableOccurrence) -> String {
    let value = masked_occurrence_value(var, occurrence);
    format!("{} ({value})", format_location(occurrence))
}

fn masked_occurrence_value(var: &VariableSummary, occurrence: &VariableOccurrence) -> MaskedValue {
    MaskedValue::new(
        occurrence.parsed_value.clone().unwrap_or_default(),
        var.is_secret_like || occurrence.secret.is_secret(),
        false,
    )
}

fn format_location(occurrence: &VariableOccurrence) -> String {
    match occurrence.line {
        Some(line) => format!("{}:{line}", occurrence.source_id),
        None => occurrence.source_id.clone(),
    }
}

fn line_list(occurrences: &[&VariableOccurrence]) -> String {
    let lines: Vec<String> = occurrences
        .iter()
        .filter_map(|occurrence| occurrence.line)
        .map(|line| line.to_string())
        .collect();
    if lines.is_empty() {
        "unknown lines".to_string()
    } else if lines.len() == 1 {
        format!("line {}", lines[0])
    } else {
        format!("lines {}", lines.join(", "))
    }
}

fn sort_diagnostics(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then_with(|| diagnostic_key(left).cmp(diagnostic_key(right)))
            .then_with(|| diagnostic_source(left).cmp(diagnostic_source(right)))
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| diagnostic_code_rank(left.code).cmp(&diagnostic_code_rank(right.code)))
            .then_with(|| left.message.cmp(&right.message))
    });
}

fn diagnostic_key(diagnostic: &Diagnostic) -> &str {
    diagnostic.key.as_deref().unwrap_or("")
}

fn diagnostic_source(diagnostic: &Diagnostic) -> &str {
    diagnostic.source_id.as_deref().unwrap_or("")
}

fn diagnostic_code_rank(code: DiagnosticCode) -> u8 {
    match code {
        DiagnosticCode::DuplicateKey => 0,
        DiagnosticCode::ConflictingValues => 1,
        DiagnosticCode::MissingRequired => 2,
        DiagnosticCode::EmptyRequired => 3,
        DiagnosticCode::UndefinedReference => 4,
        DiagnosticCode::CircularReference => 5,
        DiagnosticCode::InvalidDotenvLine => 6,
        DiagnosticCode::SecretInTrackedFile => 7,
        DiagnosticCode::InheritedUnresolved => 8,
        DiagnosticCode::ShadowedValue => 9,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{
        Diagnostic, DiagnosticCode, EnvSource, ParseError, SecretClass, Severity, SourceKind,
        VariableOccurrence, VariableSummary,
    };

    fn source(id: &str, kind: SourceKind, path: Option<&str>, precedence: u32) -> EnvSource {
        EnvSource {
            id: id.to_string(),
            kind,
            path: path.map(PathBuf::from),
            context: id
                .rsplit_once('[')
                .and_then(|(_, rest)| rest.strip_suffix(']'))
                .map(ToString::to_string),
            precedence,
            enabled: true,
            errors: Vec::new(),
        }
    }

    fn disabled(mut source: EnvSource) -> EnvSource {
        source.enabled = false;
        source
    }

    fn parse_error(mut source: EnvSource, line: Option<u32>, message: &str) -> EnvSource {
        source.errors.push(ParseError {
            line,
            message: message.to_string(),
        });
        source
    }

    fn occ(key: &str, value: &str, source_id: &str, line: Option<u32>) -> VariableOccurrence {
        VariableOccurrence {
            key: key.to_string(),
            raw_value: Some(value.to_string()),
            parsed_value: Some(value.to_string()),
            source_id: source_id.to_string(),
            line,
            is_empty: value.is_empty(),
            is_inherited: false,
            no_expand: false,
            secret: SecretClass::None,
        }
    }

    fn secret_occ(
        key: &str,
        value: &str,
        source_id: &str,
        line: Option<u32>,
    ) -> VariableOccurrence {
        VariableOccurrence {
            secret: SecretClass::Both,
            ..occ(key, value, source_id, line)
        }
    }

    fn inherited_occ(key: &str, source_id: &str, line: u32) -> VariableOccurrence {
        VariableOccurrence {
            key: key.to_string(),
            raw_value: None,
            parsed_value: None,
            source_id: source_id.to_string(),
            line: Some(line),
            is_empty: false,
            is_inherited: true,
            no_expand: false,
            secret: SecretClass::None,
        }
    }

    fn var(
        key: &str,
        effective: Option<(&str, &str)>,
        occurrences: Vec<VariableOccurrence>,
    ) -> VariableSummary {
        VariableSummary {
            key: key.to_string(),
            effective: effective
                .map(|(value, source_id)| (value.to_string(), source_id.to_string())),
            is_secret_like: occurrences
                .iter()
                .any(|occurrence| occurrence.secret.is_secret()),
            occurrences,
            diagnostics: Vec::new(),
            is_required: false,
            is_missing: false,
        }
    }

    fn analysis(sources: Vec<EnvSource>, variables: Vec<VariableSummary>) -> Analysis {
        Analysis {
            root: PathBuf::from("."),
            profile: "default".to_string(),
            sources,
            variables,
            diagnostics: Vec::new(),
        }
    }

    fn required(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|key| (*key).to_string()).collect()
    }

    fn codes(analysis: &Analysis, key: &str) -> Vec<DiagnosticCode> {
        analysis
            .variables
            .iter()
            .find(|var| var.key == key)
            .map(|var| var.diagnostics.iter().map(|diag| diag.code).collect())
            .unwrap_or_default()
    }

    #[test]
    fn duplicate_key() {
        let mut analysis = analysis(
            vec![source(".env", SourceKind::Dotenv, Some(".env"), 10)],
            vec![var(
                "PORT",
                Some(("5001", ".env")),
                vec![
                    occ("PORT", "3000", ".env", Some(1)),
                    occ("PORT", "5001", ".env", Some(2)),
                ],
            )],
        );

        run(&mut analysis, &required(&[]), None);

        let diagnostic = analysis
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == DiagnosticCode::DuplicateKey)
            .unwrap();
        assert_eq!(diagnostic.severity, Severity::Warning);
        assert!(diagnostic.message.contains("lines 1, 2"));
        assert_eq!(codes(&analysis, "PORT"), vec![DiagnosticCode::DuplicateKey]);
    }

    #[test]
    fn conflicting_values_cross_source() {
        let mut analysis = analysis(
            vec![
                source(".env", SourceKind::Dotenv, Some(".env"), 10),
                source(
                    "docker-compose.yml[api]",
                    SourceKind::Compose,
                    Some("docker-compose.yml"),
                    20,
                ),
            ],
            vec![var(
                "PORT",
                Some(("5001", "docker-compose.yml[api]")),
                vec![
                    occ("PORT", "3000", ".env", Some(3)),
                    occ("PORT", "5001", "docker-compose.yml[api]", Some(12)),
                ],
            )],
        );

        run(&mut analysis, &required(&[]), None);

        let diagnostic = analysis
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == DiagnosticCode::ConflictingValues)
            .unwrap();
        assert_eq!(diagnostic.severity, Severity::Warning);
        assert_eq!(
            diagnostic.message,
            "PORT differs across sources: .env:3 (3000), docker-compose.yml[api]:12 (5001). Effective value is 5001 from docker-compose.yml[api]."
        );
    }

    #[test]
    fn conflict_message_masks_secrets() {
        let mut analysis = analysis(
            vec![
                source(".env", SourceKind::Dotenv, Some(".env"), 10),
                source(".env.local", SourceKind::Dotenv, Some(".env.local"), 20),
            ],
            vec![var(
                "JWT_SECRET",
                Some(("envlensFakeHistoricalSecret", ".env.local")),
                vec![
                    secret_occ("JWT_SECRET", "envlensFakeHistoricalSecret", ".env", Some(2)),
                    secret_occ(
                        "JWT_SECRET",
                        "envlensFakeHistoricalSecret",
                        ".env.local",
                        Some(5),
                    ),
                ],
            )],
        );

        run(&mut analysis, &required(&[]), None);

        let message = analysis
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == DiagnosticCode::ConflictingValues)
            .unwrap()
            .message
            .clone();
        assert!(message.contains("sk_"));
        assert!(message.contains('•'));
        assert!(!message.contains("firstsecret"));
        assert!(!message.contains("secondsecret"));
    }

    #[test]
    fn process_occurrence_renders_without_line() {
        let mut analysis = analysis(
            vec![
                source(".env", SourceKind::Dotenv, Some(".env"), 10),
                source("process", SourceKind::Process, None, 20),
            ],
            vec![var(
                "PORT",
                Some(("5001", "process")),
                vec![
                    occ("PORT", "3000", ".env", Some(3)),
                    occ("PORT", "5001", "process", None),
                ],
            )],
        );

        run(&mut analysis, &required(&[]), None);

        let message = analysis
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == DiagnosticCode::ConflictingValues)
            .unwrap()
            .message
            .clone();
        assert!(message.contains("process (5001)"));
    }

    #[test]
    fn same_file_subsource_conflict_is_info() {
        let mut analysis = analysis(
            vec![
                source(
                    "docker-compose.yml[api]",
                    SourceKind::Compose,
                    Some("docker-compose.yml"),
                    10,
                ),
                source(
                    "docker-compose.yml[worker]",
                    SourceKind::Compose,
                    Some("docker-compose.yml"),
                    20,
                ),
            ],
            vec![var(
                "PORT",
                Some(("5001", "docker-compose.yml[worker]")),
                vec![
                    occ("PORT", "3000", "docker-compose.yml[api]", Some(6)),
                    occ("PORT", "5001", "docker-compose.yml[worker]", Some(14)),
                ],
            )],
        );

        run(&mut analysis, &required(&[]), None);

        let diagnostic = analysis
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == DiagnosticCode::ConflictingValues)
            .unwrap();
        assert_eq!(diagnostic.severity, Severity::Info);
    }

    #[test]
    fn shadowed_value_info() {
        let mut analysis = analysis(
            vec![
                source(".env", SourceKind::Dotenv, Some(".env"), 10),
                source(".env.local", SourceKind::Dotenv, Some(".env.local"), 20),
                source(
                    ".github/workflows/ci.yml",
                    SourceKind::Ci,
                    Some(".github/workflows/ci.yml"),
                    0,
                ),
            ],
            vec![var(
                "PORT",
                Some(("5001", ".env.local")),
                vec![
                    occ("PORT", "3000", ".env", Some(1)),
                    occ("PORT", "5001", ".env.local", Some(1)),
                    occ("PORT", "7000", ".github/workflows/ci.yml", Some(4)),
                ],
            )],
        );

        run(&mut analysis, &required(&[]), None);

        let shadowed: Vec<&Diagnostic> = analysis
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == DiagnosticCode::ShadowedValue)
            .collect();
        assert_eq!(shadowed.len(), 1);
        assert_eq!(shadowed[0].severity, Severity::Info);
        assert!(shadowed[0].message.contains(".env:1"));
        assert!(shadowed[0].message.contains(".env.local:1"));
    }

    #[test]
    fn equal_value_shadowed_but_not_conflicting() {
        let mut analysis = analysis(
            vec![
                source(".env", SourceKind::Dotenv, Some(".env"), 10),
                source(".env.local", SourceKind::Dotenv, Some(".env.local"), 20),
            ],
            vec![var(
                "PORT",
                Some(("3000", ".env.local")),
                vec![
                    occ("PORT", "3000", ".env", Some(1)),
                    occ("PORT", "3000", ".env.local", Some(1)),
                ],
            )],
        );

        run(&mut analysis, &required(&[]), None);

        assert!(
            analysis
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::ShadowedValue)
        );
        assert!(
            !analysis
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::ConflictingValues)
        );
    }

    #[test]
    fn missing_required_error() {
        let mut analysis = analysis(
            vec![source(
                ".github/workflows/ci.yml",
                SourceKind::Ci,
                Some(".github/workflows/ci.yml"),
                0,
            )],
            vec![var(
                "JWT_SECRET",
                None,
                vec![occ(
                    "JWT_SECRET",
                    "ci-secret",
                    ".github/workflows/ci.yml",
                    Some(4),
                )],
            )],
        );

        run(&mut analysis, &required(&["JWT_SECRET", "REDIS_URL"]), None);

        let jwt = analysis
            .variables
            .iter()
            .find(|var| var.key == "JWT_SECRET")
            .unwrap();
        let redis = analysis
            .variables
            .iter()
            .find(|var| var.key == "REDIS_URL")
            .unwrap();
        assert!(jwt.is_required);
        assert!(jwt.is_missing);
        assert!(redis.is_required);
        assert!(redis.is_missing);
        assert_eq!(jwt.diagnostics[0].severity, Severity::Error);
        assert_eq!(redis.diagnostics[0].code, DiagnosticCode::MissingRequired);
    }

    #[test]
    fn empty_required_warning() {
        let mut analysis = analysis(
            vec![source(".env", SourceKind::Dotenv, Some(".env"), 10)],
            vec![var(
                "DATABASE_URL",
                Some(("", ".env")),
                vec![occ("DATABASE_URL", "", ".env", Some(1))],
            )],
        );

        run(&mut analysis, &required(&["DATABASE_URL"]), None);

        let var = analysis
            .variables
            .iter()
            .find(|var| var.key == "DATABASE_URL")
            .unwrap();
        assert!(var.is_required);
        assert!(!var.is_missing);
        assert_eq!(var.diagnostics[0].code, DiagnosticCode::EmptyRequired);
        assert_eq!(var.diagnostics[0].severity, Severity::Warning);
    }

    #[test]
    fn disabled_source_does_not_satisfy_required() {
        let mut analysis = analysis(
            vec![disabled(source(
                ".env",
                SourceKind::Dotenv,
                Some(".env"),
                10,
            ))],
            vec![var(
                "DATABASE_URL",
                Some(("postgres://host/db", ".env")),
                vec![occ("DATABASE_URL", "postgres://host/db", ".env", Some(1))],
            )],
        );

        run(&mut analysis, &required(&["DATABASE_URL"]), None);

        let var = analysis
            .variables
            .iter()
            .find(|var| var.key == "DATABASE_URL")
            .unwrap();
        assert!(var.is_missing);
        assert_eq!(var.diagnostics[0].code, DiagnosticCode::MissingRequired);
    }

    #[test]
    fn inherited_unresolved_info() {
        let mut analysis = analysis(
            vec![source(
                "docker-compose.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
                10,
            )],
            vec![var(
                "DATABASE_URL",
                None,
                vec![inherited_occ("DATABASE_URL", "docker-compose.yml[api]", 5)],
            )],
        );

        run(&mut analysis, &required(&[]), None);

        let diagnostic = analysis
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == DiagnosticCode::InheritedUnresolved)
            .unwrap();
        assert_eq!(diagnostic.severity, Severity::Info);
        assert_eq!(
            diagnostic.source_id.as_deref(),
            Some("docker-compose.yml[api]")
        );
    }

    #[test]
    fn invalid_dotenv_line_warning() {
        let mut analysis = analysis(
            vec![parse_error(
                source(".env", SourceKind::Dotenv, Some(".env"), 10),
                Some(7),
                "invalid key 'DATABASE URL'",
            )],
            Vec::new(),
        );

        run(&mut analysis, &required(&[]), None);

        let diagnostic = analysis
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == DiagnosticCode::InvalidDotenvLine)
            .unwrap();
        assert_eq!(diagnostic.severity, Severity::Warning);
        assert_eq!(diagnostic.source_id.as_deref(), Some(".env"));
        assert_eq!(diagnostic.line, Some(7));
        assert!(diagnostic.message.contains("invalid key"));
    }

    #[test]
    fn secret_in_tracked_file() {
        let mut tracked_analysis = analysis(
            vec![source(".env", SourceKind::Dotenv, Some(".env"), 10)],
            vec![var(
                "JWT_SECRET",
                Some(("envlensFakeHistoricalSecret", ".env")),
                vec![secret_occ(
                    "JWT_SECRET",
                    "envlensFakeHistoricalSecret",
                    ".env",
                    Some(1),
                )],
            )],
        );
        let tracked = BTreeSet::from([PathBuf::from(".env")]);

        run(&mut tracked_analysis, &required(&[]), Some(&tracked));

        assert!(
            tracked_analysis
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::SecretInTrackedFile)
        );

        let mut without_tracked = analysis(
            vec![source(".env", SourceKind::Dotenv, Some(".env"), 10)],
            vec![var(
                "JWT_SECRET",
                Some(("envlensFakeHistoricalSecret", ".env")),
                vec![secret_occ(
                    "JWT_SECRET",
                    "envlensFakeHistoricalSecret",
                    ".env",
                    Some(1),
                )],
            )],
        );
        run(&mut without_tracked, &required(&[]), None);
        assert!(
            !without_tracked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::SecretInTrackedFile)
        );
    }

    #[test]
    fn determinism() {
        let sources = vec![
            source(".env", SourceKind::Dotenv, Some(".env"), 10),
            source(".env.local", SourceKind::Dotenv, Some(".env.local"), 20),
        ];
        let variables = vec![
            var(
                "BETA",
                Some(("2", ".env.local")),
                vec![
                    occ("BETA", "1", ".env", Some(1)),
                    occ("BETA", "2", ".env.local", Some(1)),
                ],
            ),
            var("ALPHA", None, Vec::new()),
        ];
        let mut first = analysis(sources.clone(), variables.clone());
        let mut second = analysis(sources, variables);
        let required = required(&["ALPHA"]);

        run(&mut first, &required, None);
        run(&mut second, &required, None);

        assert_eq!(first, second);
        let severities: Vec<Severity> = first
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.severity)
            .collect();
        assert_eq!(
            severities,
            vec![Severity::Error, Severity::Warning, Severity::Info]
        );
    }
}
