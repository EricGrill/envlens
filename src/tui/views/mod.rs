pub mod details;
pub mod help;
pub mod modals;
pub mod sources;
pub mod variables;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::tui::app::App;
use crate::tui::theme::Theme;

pub fn draw(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(1)])
        .split(area);
    let main = vertical[0];
    let status = vertical[1];
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(main);
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(20)])
        .split(body[0]);

    sources::draw(frame, top[0], app, theme);
    variables::draw(frame, top[1], app, theme);
    details::draw(frame, body[1], app, theme);
    draw_status(frame, status, app, theme);
    modals::draw(frame, area, app, theme);
}

pub(crate) fn block<'a>(title: &'a str, focused: bool, theme: &Theme) -> Block<'a> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(theme.border)
        .border_style(if focused {
            theme.styles.focus
        } else {
            theme.styles.normal
        })
        .title_style(theme.styles.title)
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let search = app
        .search
        .as_ref()
        .map(|query| format!(" search=/{query}"))
        .unwrap_or_default();
    let message = app
        .status
        .as_ref()
        .map(|status| format!(" | {status}"))
        .unwrap_or_default();
    let text = format!(
        "profile={} filter={:?} sort={:?} pane={:?}{search}{message}",
        app.analysis.profile, app.filter, app.sort, app.pane
    );
    frame.render_widget(Paragraph::new(text).style(theme.styles.status), area);
}

pub(crate) fn clip(value: impl AsRef<str>, max: usize) -> String {
    let value = value.as_ref();
    let mut output = String::new();
    for ch in value.chars().take(max) {
        output.push(ch);
    }
    if value.chars().count() > max && max > 1 {
        output.pop();
        output.push('…');
    }
    output
}
