use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;

#[derive(Debug, Clone, Copy)]
pub struct Icons {
    pub ok: &'static str,
    pub warning: &'static str,
    pub error: &'static str,
    pub secret: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub icons: Icons,
    pub mask: char,
    pub border: symbols::border::Set<'static>,
    pub styles: Styles,
}

#[derive(Debug, Clone, Copy)]
pub struct Styles {
    pub normal: Style,
    pub focus: Style,
    pub title: Style,
    pub status: Style,
    pub error: Style,
    pub warning: Style,
    pub info: Style,
    pub secret: Style,
    pub selected: Style,
}

impl Theme {
    pub fn new(no_color: bool, ascii: bool) -> Self {
        let icons = if ascii {
            Icons {
                ok: "+",
                warning: "!",
                error: "x",
                secret: "#",
            }
        } else {
            Icons {
                ok: "✓",
                warning: "⚠",
                error: "✗",
                secret: "🔒",
            }
        };

        let border = if ascii {
            symbols::border::PLAIN
        } else {
            symbols::border::ROUNDED
        };

        // v0.1 deliberately keeps low-color degradation explicit: NO_COLOR or
        // --no-color disables color; --ascii alone controls glyph choices.
        let styles = if no_color {
            Styles {
                normal: Style::default(),
                focus: Style::default().add_modifier(Modifier::BOLD),
                title: Style::default().add_modifier(Modifier::BOLD),
                status: Style::default(),
                error: Style::default().add_modifier(Modifier::BOLD),
                warning: Style::default().add_modifier(Modifier::BOLD),
                info: Style::default(),
                secret: Style::default().add_modifier(Modifier::BOLD),
                selected: Style::default().add_modifier(Modifier::BOLD),
            }
        } else {
            Styles {
                normal: Style::default().fg(Color::Gray),
                focus: Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                title: Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
                status: Style::default().fg(Color::DarkGray),
                error: Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                warning: Style::default().fg(Color::Yellow),
                info: Style::default().fg(Color::Blue),
                secret: Style::default().fg(Color::Magenta),
                selected: Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            }
        };

        Self {
            icons,
            mask: if ascii { '*' } else { '•' },
            border,
            styles,
        }
    }
}
