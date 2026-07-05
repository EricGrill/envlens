use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem};

use crate::core::model::SourceKind;
use crate::tui::app::{App, Pane};
use crate::tui::theme::Theme;
use crate::tui::views::{block, clip};

pub fn draw(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let items: Vec<ListItem<'_>> = app
        .analysis
        .sources
        .iter()
        .enumerate()
        .map(|(idx, source)| {
            let selected = idx == app.selected_source;
            let marker = if selected { ">" } else { " " };
            let enabled = if source.enabled { "[x]" } else { "[ ]" };
            let kind = kind_label(source.kind);
            let title = format!("{marker} {enabled} {kind} {}", clip(&source.id, 16));
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

fn kind_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Dotenv => "env",
        SourceKind::DotenvExample => "req",
        SourceKind::Compose => "cmp",
        SourceKind::PackageScript => "pkg",
        SourceKind::Manifest => "man",
        SourceKind::Process => "proc",
        SourceKind::Ci => "ci",
    }
}
