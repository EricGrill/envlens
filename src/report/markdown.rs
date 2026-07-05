use crate::core::model::{Analysis, Diagnostic, DiagnosticCode, Severity, VariableSummary};
use crate::report::{
    count_severity, diagnostic_code_name, diagnostic_message, masked_value, sanitize_text,
};

pub fn render(analysis: &Analysis, generated_at: impl AsRef<str>, no_values: bool) -> String {
    let mut output = String::new();
    output.push_str("# EnvLens Report\n\n");
    output.push_str(&format!("Project: {}  \n", analysis.root.to_string_lossy()));
    output.push_str(&format!("Generated: {}  \n", generated_at.as_ref()));
    output.push_str(&format!("Profile: {}\n\n", analysis.profile));

    output.push_str("## Summary\n\n");
    output.push_str(&format!("- Sources scanned: {}\n", analysis.sources.len()));
    output.push_str(&format!(
        "- Variables found: {}\n",
        analysis.variables.len()
    ));
    output.push_str(&format!(
        "- Required variables missing: {}\n",
        missing_required_count(analysis)
    ));
    output.push_str(&format!("- Conflicts: {}\n", conflict_count(analysis)));
    output.push_str(&format!(
        "- Secret-like variables: {}\n",
        analysis
            .variables
            .iter()
            .filter(|variable| variable.is_secret_like)
            .count()
    ));
    output.push_str(&format!(
        "- Diagnostics: {} errors, {} warnings, {} infos\n\n",
        count_severity(analysis, Severity::Error),
        count_severity(analysis, Severity::Warning),
        count_severity(analysis, Severity::Info)
    ));

    render_diagnostic_section(&mut output, "Errors", analysis, Severity::Error, no_values);
    render_diagnostic_section(
        &mut output,
        "Warnings",
        analysis,
        Severity::Warning,
        no_values,
    );

    output
}

fn render_diagnostic_section(
    output: &mut String,
    title: &str,
    analysis: &Analysis,
    severity: Severity,
    no_values: bool,
) {
    output.push_str(&format!("## {title}\n\n"));
    let diagnostics: Vec<_> = analysis
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == severity)
        .collect();

    if diagnostics.is_empty() {
        output.push_str("None.\n\n");
        return;
    }

    for diagnostic in diagnostics {
        let subject = diagnostic
            .key
            .as_deref()
            .or(diagnostic.source_id.as_deref())
            .unwrap_or("GLOBAL");
        output.push_str(&format!(
            "### {}: {}\n\n",
            diagnostic_code_title(diagnostic.code),
            sanitize_text(subject)
        ));
        output.push_str(&format!(
            "{}\n\n",
            diagnostic_markdown_message(diagnostic, no_values)
        ));

        if let Some(variable) = diagnostic
            .key
            .as_deref()
            .and_then(|key| find_variable(analysis, key))
        {
            render_variable_details(output, variable, no_values);
        }
    }
}

fn diagnostic_markdown_message(diagnostic: &Diagnostic, no_values: bool) -> String {
    match diagnostic.code {
        DiagnosticCode::MissingRequired => diagnostic
            .key
            .as_deref()
            .map(|key| {
                format!(
                    "`{}` is required but is not defined in active value-bearing sources.",
                    sanitize_text(key)
                )
            })
            .unwrap_or_else(|| diagnostic_message(diagnostic, no_values)),
        _ => diagnostic_message(diagnostic, no_values),
    }
}

fn render_variable_details(output: &mut String, variable: &VariableSummary, no_values: bool) {
    if !no_values && let Some((value, source_id)) = &variable.effective {
        output.push_str(&format!(
            "Effective value: `{}` from `{}`\n\n",
            markdown_escape(&masked_value(value, variable.is_secret_like)),
            markdown_escape(source_id)
        ));
    }

    if variable.occurrences.is_empty() {
        return;
    }

    if no_values {
        output.push_str("| Source | Line |\n");
        output.push_str("|---|---:|\n");
        for occurrence in &variable.occurrences {
            output.push_str(&format!(
                "| `{}` | {} |\n",
                markdown_escape(&occurrence.source_id),
                line_text(occurrence.line)
            ));
        }
    } else {
        output.push_str("| Source | Line | Value |\n");
        output.push_str("|---|---:|---|\n");
        for occurrence in &variable.occurrences {
            let is_secret = variable.is_secret_like || occurrence.secret.is_secret();
            let value = occurrence
                .parsed_value
                .as_deref()
                .map(|value| masked_value(value, is_secret))
                .unwrap_or_else(|| "(inherited)".to_string());
            output.push_str(&format!(
                "| `{}` | {} | `{}` |\n",
                markdown_escape(&occurrence.source_id),
                line_text(occurrence.line),
                markdown_escape(&value)
            ));
        }
    }
    output.push('\n');
}

fn find_variable<'a>(analysis: &'a Analysis, key: &str) -> Option<&'a VariableSummary> {
    analysis
        .variables
        .iter()
        .find(|variable| variable.key == key)
}

fn missing_required_count(analysis: &Analysis) -> usize {
    analysis
        .variables
        .iter()
        .filter(|variable| variable.is_required && variable.is_missing)
        .count()
}

fn conflict_count(analysis: &Analysis) -> usize {
    analysis
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == DiagnosticCode::ConflictingValues)
        .count()
}

fn diagnostic_code_title(code: DiagnosticCode) -> String {
    diagnostic_code_name(code).to_ascii_uppercase()
}

fn line_text(line: Option<u32>) -> String {
    line.map(|line| line.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn markdown_escape(text: &str) -> String {
    text.replace('|', "\\|").replace('`', "\\`")
}
