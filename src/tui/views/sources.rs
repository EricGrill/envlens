use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem};

use crate::core::model::SourceKind;
use crate::tui::app::{App, Pane};
use crate::tui::theme::Theme;
use crate::tui::views::{block, clip};

pub fn draw(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let occurrence_counts = source_occurrence_counts(app);
    let items: Vec<ListItem<'_>> = app
        .analysis
        .sources
        .iter()
        .enumerate()
        .map(|(idx, source)| {
            let selected = idx == app.selected_source;
            let marker = if selected { ">" } else { " " };
            let enabled = if source.enabled { "[x]" } else { "[ ]" };
            let errors = if source.errors.is_empty() { " " } else { "!" };
            let kind = kind_label(source.kind);
            let count = occurrence_counts
                .get(source.id.as_str())
                .copied()
                .unwrap_or_default();
            let title = format!(
                "{marker} {enabled}{errors} {kind} {:>2} {}",
                count,
                clip(&source.id, 11)
            );
            ListItem::new(Line::from(Span::raw(title))).style(if selected {
                theme.styles.selected
            } else {
                theme.styles.normal
            })
        })
        .collect();
    frame.render_widget(
        List::new(items).block(block("Sources", app.pane == Pane::Sources, theme)),
        area,
    );
}

fn source_occurrence_counts(app: &App) -> BTreeMap<&str, usize> {
    let mut counts = BTreeMap::new();
    for occurrence in app
        .analysis
        .variables
        .iter()
        .flat_map(|variable| &variable.occurrences)
    {
        *counts.entry(occurrence.source_id.as_str()).or_default() += 1;
    }
    counts
}

fn kind_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Dotenv => "env",
        SourceKind::DotenvExample => "req",
        SourceKind::Direnv => "rc",
        SourceKind::Dockerfile => "dkr",
        SourceKind::Compose => "cmp",
        SourceKind::PackageScript => "pkg",
        SourceKind::Manifest => "man",
        SourceKind::Process => "proc",
        SourceKind::Ci => "ci",
    }
}
