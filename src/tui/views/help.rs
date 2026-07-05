use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crate::tui::theme::Theme;

pub fn draw(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    frame.render_widget(Clear, area);
    let lines = vec![
        Line::from("EnvLens keys"),
        Line::from("Tab switch panes    j/k or arrows move"),
        Line::from("/ search           f filter"),
        Line::from("s sort             Space toggle source"),
        Line::from("r reveal key       R reveal all"),
        Line::from("Enter expand       e export report"),
        Line::from("o open source      Ctrl+r refresh"),
        Line::from("Esc closes layers  q quit"),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::bordered()
                    .border_set(theme.border)
                    .title("Help")
                    .border_style(theme.styles.focus)
                    .title_style(theme.styles.title),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}
