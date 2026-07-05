use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem};

use crate::core::model::{Severity, VariableSummary};
use crate::tui::app::{App, Pane, visible_variables};
use crate::tui::theme::Theme;
use crate::tui::views::{block, clip};

pub fn draw(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let visible = visible_variables(app);
    let items: Vec<ListItem<'_>> = visible
        .iter()
        .enumerate()
        .map(|(idx, variable)| {
            let icon = variable_icon(variable, theme);
            let value = value_text(variable, app, theme);
            let source = variable
                .effective
                .as_ref()
                .map(|(_, source_id)| source_id.as_str())
                .unwrap_or("-");
            let marker = if idx == app.selected_var { ">" } else { " " };
            let line = format!(
                "{marker} {icon} {:<18} {:<24} {}",
                clip(&variable.key, 18),
                clip(value, 24),
                clip(source, 20)
            );
            ListItem::new(Line::from(Span::raw(line))).style(if idx == app.selected_var {
                theme.styles.selected
            } else {
                theme.styles.normal
            })
        })
        .collect();
    let title = format!("Variables ({})", visible.len());
    frame.render_widget(
        List::new(items).block(block(&title, app.pane == Pane::Variables, theme)),
        area,
    );
}

pub(crate) fn variable_icon(variable: &VariableSummary, theme: &Theme) -> &'static str {
    if variable
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        theme.icons.error
    } else if variable
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Warning)
    {
        theme.icons.warning
    } else if variable.is_secret_like {
        theme.icons.secret
    } else {
        theme.icons.ok
    }
}

pub(crate) fn value_text(variable: &VariableSummary, app: &App, theme: &Theme) -> String {
    let Some((value, _)) = variable.effective.as_ref() else {
        return "-".to_string();
    };
    if variable.is_secret_like && !app.reveal_all && !app.revealed.contains(&variable.key) {
        mask_value(value, theme)
    } else {
        value.clone()
    }
}

pub(crate) fn mask_value(value: &str, theme: &Theme) -> String {
    let bullets: String = std::iter::repeat_n(theme.mask, 8).collect();
    if value.starts_with("sk_") {
        format!("sk_{bullets}")
    } else {
        bullets
    }
}
