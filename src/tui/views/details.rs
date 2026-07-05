use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use crate::core::model::{DiagnosticCode, SourceKind, VariableOccurrence, VariableSummary};
use crate::tui::app::{App, Pane, visible_variables};
use crate::tui::theme::Theme;
use crate::tui::views::variables::value_text;
use crate::tui::views::{block, clip};

pub fn draw(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let inner_height = area.height.saturating_sub(2) as usize;
    let inner_width = area.width.saturating_sub(2) as usize;
    let lines = if let Some(source) = app.analysis.sources.get(app.selected_source)
        && source.kind == SourceKind::Manifest
    {
        vec![
            clipped_line(format!("{} selected", source.id), inner_width),
            clipped_line(
                "discovered; contributes no environment variables in v0.1",
                inner_width,
            ),
        ]
    } else if let Some(variable) = visible_variables(app).get(app.selected_var).copied() {
        variable_lines(variable, app, theme, inner_height, inner_width)
    } else {
        vec![clipped_line(
            "No variables match the current source/filter/search.",
            inner_width,
        )]
    };

    frame.render_widget(
        Paragraph::new(lines)
            .block(block("Details", app.pane == Pane::Details, theme))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn variable_lines(
    variable: &VariableSummary,
    app: &App,
    theme: &Theme,
    inner_height: usize,
    inner_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(clipped_line(
        format!("{} {}", variable.key, status(variable)),
        inner_width,
    ));
    match &variable.effective {
        Some((_, source_id)) => lines.push(clipped_line(
            format!(
                "effective: {} from {}",
                clip(value_text(variable, app, theme), 60),
                source_id
            ),
            inner_width,
        )),
        None => lines.push(clipped_line("effective: <missing>", inner_width)),
    }
    if app.expanded.contains(&variable.key) {
        lines.push(clipped_line(
            "expanded: showing every occurrence",
            inner_width,
        ));
    }
    lines.push(Line::from(""));
    lines.push(clipped_line("occurrences:", inner_width));

    let diagnostics = diagnostic_lines(variable, inner_width);
    let available_occurrences = inner_height.saturating_sub(lines.len() + diagnostics.len());
    let truncated = variable.occurrences.len() > available_occurrences;
    let visible_occurrences = if truncated {
        available_occurrences.saturating_sub(1)
    } else {
        available_occurrences
    };
    for occurrence in variable.occurrences.iter().take(visible_occurrences) {
        lines.push(clipped_line(
            format!(
                "  {} {}",
                location(occurrence),
                occurrence_value(variable, occurrence, app, theme)
            ),
            inner_width,
        ));
    }
    if truncated && available_occurrences > 0 {
        let hidden = variable.occurrences.len() - visible_occurrences;
        lines.push(clipped_line(
            format!("  ... +{hidden} more occurrences"),
            inner_width,
        ));
    }
    lines.extend(diagnostics);
    lines
}

fn status(variable: &VariableSummary) -> &'static str {
    if variable.is_missing {
        "(missing)"
    } else if variable.is_secret_like {
        "(secret-like)"
    } else if variable
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == DiagnosticCode::ConflictingValues)
    {
        "(conflict)"
    } else {
        ""
    }
}

fn location(occurrence: &VariableOccurrence) -> String {
    match occurrence.line {
        Some(line) => format!("{}:{line}", occurrence.source_id),
        None => occurrence.source_id.clone(),
    }
}

fn occurrence_value(
    variable: &VariableSummary,
    occurrence: &VariableOccurrence,
    app: &App,
    theme: &Theme,
) -> String {
    let mut annotations = Vec::new();
    if occurrence.is_empty {
        annotations.push("empty");
    }
    if occurrence.is_inherited {
        annotations.push("inherited");
    }
    if occurrence.no_expand {
        annotations.push("literal");
    }
    if occurrence_is_unresolved(variable, occurrence) {
        annotations.push("unresolved");
    }
    let suffix = if annotations.is_empty() {
        String::new()
    } else {
        format!(" ({})", annotations.join(", "))
    };
    let value = occurrence
        .parsed_value
        .as_deref()
        .map(|value| {
            if (variable.is_secret_like || occurrence.secret.is_secret())
                && !app.reveal_all
                && !app.revealed.contains(&variable.key)
            {
                crate::tui::views::variables::mask_value(value, theme)
            } else {
                value.to_string()
            }
        })
        .unwrap_or_else(|| "<inherited>".to_string());
    format!("={}{}", clip(value, 80), suffix)
}

fn occurrence_is_unresolved(variable: &VariableSummary, occurrence: &VariableOccurrence) -> bool {
    variable.diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic.code,
            DiagnosticCode::UndefinedReference | DiagnosticCode::InheritedUnresolved
        ) && diagnostic.key.as_ref() == Some(&variable.key)
            && diagnostic
                .source_id
                .as_ref()
                .is_none_or(|source_id| source_id == &occurrence.source_id)
            && diagnostic
                .line
                .is_none_or(|line| Some(line) == occurrence.line)
    })
}

fn diagnostic_lines(variable: &VariableSummary, inner_width: usize) -> Vec<Line<'static>> {
    if variable.diagnostics.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(clipped_line("diagnostics:", inner_width));
    for diagnostic in &variable.diagnostics {
        lines.push(clipped_line(
            format!(
                "  {:?} {:?}: {}",
                diagnostic.severity, diagnostic.code, diagnostic.message
            ),
            inner_width,
        ));
    }
    lines
}

fn clipped_line(value: impl AsRef<str>, width: usize) -> Line<'static> {
    Line::from(clip(value, width))
}
