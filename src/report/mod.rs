use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use std::sync::OnceLock;

use crate::core::model::{Analysis, Diagnostic, DiagnosticCode, SecretClass, Severity, SourceKind};
use crate::core::secrets::{MaskedValue, classify_value};

pub mod json;
pub mod markdown;

pub fn generated_at(source_date_epoch: Option<u64>) -> String {
    let seconds = match source_date_epoch {
        Some(seconds) => seconds,
        None => current_epoch_seconds(),
    };
    from_epoch(seconds)
}

pub fn from_epoch(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

pub fn render_check_human(analysis: &Analysis, color: bool, no_values: bool) -> String {
    let source_ids = analysis_source_ids(analysis);
    let mut output = String::new();
    for diagnostic in &analysis.diagnostics {
        let severity = severity_name(diagnostic.severity);
        let label = if color {
            colorize_severity(diagnostic.severity, severity)
        } else {
            severity.to_string()
        };
        let subject = diagnostic
            .key
            .as_deref()
            .or(diagnostic.source_id.as_deref())
            .unwrap_or("-");
        output.push_str(&format!(
            "{label} {subject} {}\n",
            diagnostic_message(diagnostic, no_values, &source_ids)
        ));
    }

    if !analysis.diagnostics.is_empty() {
        output.push('\n');
    }

    output.push_str(&format!(
        "summary: {} errors, {} warnings, {} infos, {} variables, {} sources\n",
        count_severity(analysis, Severity::Error),
        count_severity(analysis, Severity::Warning),
        count_severity(analysis, Severity::Info),
        analysis.variables.len(),
        analysis.sources.len()
    ));
    output
}

/// Redact secret-looking tokens in free text, with no knowledge of the
/// analysis. Prefer [`sanitize_text_with_sources`] anywhere the analysis is in
/// hand, so source identifiers are not mistaken for secrets.
pub fn sanitize_text(text: &str) -> String {
    sanitize_text_with_sources(text, &[])
}

/// Redact secret-looking tokens in free text while leaving known source
/// identifiers intact.
///
/// The token regex is deliberately broad, so paths like `apps/web/.env` and
/// compose tags like `docker-compose.yml[api]` clear the generic entropy
/// threshold and used to get masked — destroying the one piece of information
/// a conflict/shadow diagnostic exists to convey. Source ids are known data,
/// not a guess, so any token that resolves to one is exempted.
pub fn sanitize_text_with_sources(text: &str, source_ids: &[String]) -> String {
    secret_token_regex()
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let token = captures
                .get(0)
                .map(|matched| matched.as_str())
                .unwrap_or("");
            if !is_source_reference(token, source_ids) && classify_value(token) {
                MaskedValue::new(token, true, false).to_string()
            } else {
                token.to_string()
            }
        })
        .into_owned()
}

/// True when `token` refers to one of `source_ids`.
///
/// Two shapes have to be handled, both produced by the token regex swallowing
/// characters adjacent to the id:
/// - a trailing `:<line>` suffix is captured with the id (`apps/web/.env:1`),
///   so it is stripped before comparing;
/// - a `[service]` suffix is *not* captured (`[` is outside the character
///   class), leaving a prefix of the id (`docker-compose.yml` for the source
///   `docker-compose.yml[api]`).
fn is_source_reference(token: &str, source_ids: &[String]) -> bool {
    let core = token
        .rsplit_once(':')
        .filter(|(head, line)| {
            !head.is_empty() && !line.is_empty() && line.chars().all(|c| c.is_ascii_digit())
        })
        .map(|(head, _)| head)
        .unwrap_or(token);

    source_ids
        .iter()
        .any(|id| id == core || id.starts_with(core))
}

/// The source identifiers an analysis can legitimately mention in diagnostics.
pub(crate) fn analysis_source_ids(analysis: &Analysis) -> Vec<String> {
    analysis
        .sources
        .iter()
        .map(|source| source.id.clone())
        .collect()
}

pub(crate) fn masked_value(value: &str, is_secret: bool) -> String {
    MaskedValue::new(value.to_string(), is_secret, false).to_string()
}

pub(crate) fn source_kind_name(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Dotenv => "dotenv",
        SourceKind::DotenvExample => "dotenv_example",
        SourceKind::Direnv => "direnv",
        SourceKind::Dockerfile => "dockerfile",
        SourceKind::Compose => "compose",
        SourceKind::PackageScript => "package_script",
        SourceKind::Manifest => "manifest",
        SourceKind::Process => "process",
        SourceKind::Ci => "ci",
    }
}

pub(crate) fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

pub(crate) fn diagnostic_code_name(code: DiagnosticCode) -> &'static str {
    match code {
        DiagnosticCode::DuplicateKey => "duplicate_key",
        DiagnosticCode::ConflictingValues => "conflicting_values",
        DiagnosticCode::MissingRequired => "missing_required",
        DiagnosticCode::EmptyRequired => "empty_required",
        DiagnosticCode::UndefinedReference => "undefined_reference",
        DiagnosticCode::CircularReference => "circular_reference",
        DiagnosticCode::InvalidDotenvLine => "invalid_dotenv_line",
        DiagnosticCode::SecretInTrackedFile => "secret_in_tracked_file",
        DiagnosticCode::InheritedUnresolved => "inherited_unresolved",
        DiagnosticCode::ShadowedValue => "shadowed_value",
    }
}

pub(crate) fn diagnostic_message(
    diagnostic: &Diagnostic,
    no_values: bool,
    source_ids: &[String],
) -> String {
    if !no_values {
        return sanitize_text_with_sources(&diagnostic.message, source_ids);
    }

    let subject = diagnostic.key.as_deref().unwrap_or("variable");
    match diagnostic.code {
        DiagnosticCode::ConflictingValues => {
            format!("{subject} differs across sources.")
        }
        DiagnosticCode::ShadowedValue => {
            format!("{subject} is shadowed by a higher-precedence source.")
        }
        DiagnosticCode::InvalidDotenvLine => "invalid dotenv line".to_string(),
        _ => sanitize_text_with_sources(&diagnostic.message, source_ids),
    }
}

pub(crate) fn secret_class_name(secret: SecretClass) -> &'static str {
    match secret {
        SecretClass::None => "none",
        SecretClass::KeyLike => "key_like",
        SecretClass::ValueLike => "value_like",
        SecretClass::Both => "both",
    }
}

pub(crate) fn count_severity(analysis: &Analysis, severity: Severity) -> usize {
    analysis
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == severity)
        .count()
}

fn current_epoch_seconds() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => 0,
    }
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }

    (year, month as u32, day as u32)
}

fn colorize_severity(severity: Severity, label: &str) -> String {
    let code = match severity {
        Severity::Error => "31",
        Severity::Warning => "33",
        Severity::Info => "34",
    };
    format!("\x1b[{code}m{label}\x1b[0m")
}

fn secret_token_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| match Regex::new(r"[A-Za-z0-9_./:@+=-]{8,}") {
        Ok(regex) => regex,
        Err(err) => panic!("secret redaction regex constant is invalid: {err}"),
    })
}

#[cfg(test)]
mod tests {
    use super::{from_epoch, sanitize_text, sanitize_text_with_sources};

    #[test]
    fn formats_epoch_as_rfc3339_utc() {
        assert_eq!(from_epoch(0), "1970-01-01T00:00:00Z");
        assert_eq!(from_epoch(86_399), "1970-01-01T23:59:59Z");
        assert_eq!(from_epoch(1_751_673_600), "2025-07-05T00:00:00Z");
    }

    fn source_ids() -> Vec<String> {
        [
            ".env",
            "apps/web/.env",
            "apps/api/.env",
            "docker-compose.yml[api]",
            "docker-compose.yml[worker]",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn keeps_compose_source_ids_readable() {
        let message = "PORT differs across sources: .env:1 (3000), \
             docker-compose.yml[web]:4 (9000), docker-compose.yml[worker]:7 (7000).";
        assert_eq!(sanitize_text_with_sources(message, &source_ids()), message);
    }

    #[test]
    fn keeps_nested_dotenv_paths_readable() {
        let message = "PORT from apps/api/.env:1 (7001) is shadowed by apps/web/.env:1 (5001).";
        assert_eq!(sanitize_text_with_sources(message, &source_ids()), message);
    }

    #[test]
    fn still_masks_secret_values_alongside_source_ids() {
        let message = "JWT_SECRET differs across sources: apps/web/.env:1 \
             (ghp_abcdefghijklmnopqrstuvwxyz0123456789).";
        let sanitized = sanitize_text_with_sources(message, &source_ids());
        assert!(
            sanitized.contains("apps/web/.env:1"),
            "source id was masked: {sanitized}"
        );
        assert!(
            !sanitized.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
            "secret leaked: {sanitized}"
        );
    }

    #[test]
    fn does_not_exempt_paths_that_are_not_known_sources() {
        // Nothing in the source list resolves this, so the generic secret
        // heuristic still applies.
        let message = "value abcd1234EFGH5678ijkl9012 seen";
        let sanitized = sanitize_text_with_sources(message, &source_ids());
        assert!(
            !sanitized.contains("abcd1234EFGH5678ijkl9012"),
            "secret leaked: {sanitized}"
        );
    }

    #[test]
    fn sanitize_text_without_sources_is_unchanged_for_secrets() {
        let sanitized = sanitize_text("token abcd1234EFGH5678ijkl9012");
        assert!(!sanitized.contains("abcd1234EFGH5678ijkl9012"));
    }
}
