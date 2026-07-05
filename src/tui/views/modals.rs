use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::tui::app::{App, Modal};
use crate::tui::theme::Theme;

pub fn draw(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let Some(modal) = &app.modal else {
        return;
    };

    let popup = centered_rect(area, 58, 11);
    match modal {
        Modal::Help => crate::tui::views::help::draw(frame, popup, theme),
        Modal::SortMenu => draw_panel(
            frame,
            popup,
            "Sort",
            vec![
                Line::from(format!("current: {:?}", app.sort)),
                Line::from("j/k or arrows cycle sort modes"),
                Line::from("Enter applies"),
                Line::from(""),
                Line::from("Key"),
                Line::from("Severity"),
                Line::from("SourceCount"),
                Line::from("EffectiveSource"),
                Line::from("Secret"),
            ],
            theme,
        ),
        Modal::ConfirmRevealAll => draw_panel(
            frame,
            popup,
            "Reveal all secrets?",
            vec![
                Line::from("This will show every secret-like value in the TUI."),
                Line::from("Press y to reveal all, n or Esc to cancel."),
            ],
            theme,
        ),
        Modal::ExportPrompt { input } => draw_panel(
            frame,
            popup,
            "Export report",
            vec![
                Line::from("Write Markdown report to:"),
                Line::from(format!("> {input}")),
                Line::from("Enter confirms, Esc cancels."),
            ],
            theme,
        ),
    }
}

fn draw_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &'static str,
    lines: Vec<Line<'static>>,
    theme: &Theme,
) {
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_set(theme.border)
                    .border_style(theme.styles.focus)
                    .title_style(theme.styles.title),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(area.height.saturating_sub(height) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(area.width.saturating_sub(width) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(vertical[1]);
    horizontal[1]
}
