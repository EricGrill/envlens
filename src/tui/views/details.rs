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
    let lines = if let Some(source) = app.analysis.sources.get(app.selected_source)
        && source.kind == SourceKind::Manifest
    {
        vec![
            Line::from(format!("{} selected", source.id)),
            Line::from("discovered; contributes no environment variables in v0.1"),
        ]
    } else if let Some(variable) = visible_variables(app).get(app.selected_var).copied() {
        variable_lines(variable, app, theme)
    } else {
        vec![Line::from(
            "No variables match the current source/filter/search.",
        )]
    };

    frame.render_widget(
        Paragraph::new(lines)
            .block(block("Details", app.pane == Pane::Details, theme))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn variable_lines(variable: &VariableSummary, app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(format!("{} {}", variable.key, status(variable))));
    match &variable.effective {
        Some((_, source_id)) => lines.push(Line::from(format!(
            "effective: {} from {}",
            clip(value_text(variable, app, theme), 60),
            source_id
        ))),
        None => lines.push(Line::from("effective: <missing>")),
    }
    if app.expanded.contains(&variable.key) {
        lines.push(Line::from("expanded: showing every occurrence"));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("occurrences:"));
    for occurrence in &variable.occurrences {
        lines.push(Line::from(format!(
            "  {} {}",
            location(occurrence),
            occurrence_value(variable, occurrence, app, theme)
        )));
    }
    if !variable.diagnostics.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from("diagnostics:"));
        for diagnostic in &variable.diagnostics {
            lines.push(Line::from(format!(
                "  {:?} {:?}: {}",
                diagnostic.severity, diagnostic.code, diagnostic.message
            )));
        }
    }
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
