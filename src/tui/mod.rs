pub mod app;
pub mod theme;
pub mod views;

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::cursor;
use crossterm::event::{self, Event as CrosstermEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::config::Config;
use crate::core::model::Analysis;
use crate::report;
use crate::tui::app::{App, Event, clamp_selections, update};
use crate::tui::theme::Theme;

type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

pub struct RunOptions {
    pub analysis: Analysis,
    pub root: PathBuf,
    pub config: Config,
    pub profile: Option<String>,
    pub tracked: Option<BTreeSet<PathBuf>>,
    pub theme: Theme,
    pub has_editor: bool,
}

pub fn run<F>(options: RunOptions, mut refresh: F) -> Result<()>
where
    F: FnMut() -> Result<(Analysis, Option<BTreeSet<PathBuf>>)>,
{
    let _guard = TerminalGuard::enter().context("could not enter terminal mode")?;
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("could not create terminal backend")?;
    let mut app = App::new(
        options.analysis,
        options.root,
        options.config,
        options.profile,
        options.tracked,
        options.has_editor,
    );

    loop {
        terminal
            .draw(|frame| views::draw(frame, frame.area(), &app, &options.theme))
            .context("could not draw TUI")?;

        let event = read_event()?;
        update(&mut app, event);
        if app.should_quit {
            break;
        }
        handle_side_effects(&mut app, &mut terminal, &mut refresh)?;
    }

    Ok(())
}

fn read_event() -> Result<Event> {
    if !event::poll(Duration::from_millis(250)).context("could not poll terminal events")? {
        return Ok(Event::Tick);
    }

    match event::read().context("could not read terminal event")? {
        CrosstermEvent::Key(key) => Ok(Event::Key(key)),
        CrosstermEvent::Resize(width, height) => Ok(Event::Resize(width, height)),
        _ => Ok(Event::Tick),
    }
}

fn handle_side_effects<F>(app: &mut App, terminal: &mut TuiTerminal, refresh: &mut F) -> Result<()>
where
    F: FnMut() -> Result<(Analysis, Option<BTreeSet<PathBuf>>)>,
{
    if app.want_refresh {
        app.want_refresh = false;
        match refresh() {
            Ok((analysis, tracked)) => {
                app.analysis = analysis;
                app.tracked = tracked;
                clamp_selections(app);
                app.status = Some("refreshed".to_string());
            }
            Err(err) => app.status = Some(format!("refresh failed: {err}")),
        }
    }

    if let Some((path, line)) = app.want_editor.take() {
        app.status = match open_editor(terminal, path, line) {
            Ok(()) => Some("returned from editor".to_string()),
            Err(err) => Some(format!("editor failed: {err}")),
        };
    }

    if let Some(path) = app.want_export.take() {
        app.status = match export_report(app, path) {
            Ok(path) => Some(format!("exported to {}", path.display())),
            Err(err) => Some(format!("export failed: {err}")),
        };
    }

    Ok(())
}

fn open_editor(terminal: &mut TuiTerminal, path: PathBuf, line: u32) -> Result<()> {
    let editor = std::env::var_os("EDITOR").context("$EDITOR is not set")?;
    suspend_terminal(terminal)?;
    let status = Command::new(editor)
        .arg(format!("+{line}"))
        .arg(&path)
        .status();
    let resume_result = resume_terminal(terminal);

    match (status, resume_result) {
        (Ok(status), Ok(())) if status.success() => Ok(()),
        (Ok(status), Ok(())) => Err(anyhow::anyhow!("editor exited with {status}")),
        (Err(err), Ok(())) => Err(err).context("could not launch editor"),
        (_, Err(err)) => Err(err),
    }
}

fn suspend_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    disable_raw_mode().context("could not disable raw mode for editor")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show)
        .context("could not leave alternate screen for editor")?;
    terminal.show_cursor().context("could not show cursor")?;
    Ok(())
}

fn resume_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    execute!(terminal.backend_mut(), EnterAlternateScreen, cursor::Hide)
        .context("could not re-enter alternate screen after editor")?;
    enable_raw_mode().context("could not re-enable raw mode after editor")?;
    Ok(())
}

fn export_report(app: &App, path: PathBuf) -> Result<PathBuf> {
    let rendered = report::markdown::render(&app.analysis, report::generated_at(None), false);
    fs::write(&path, rendered).with_context(|| format!("could not write {}", path.display()))?;
    Ok(path)
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(err) = execute!(stdout, EnterAlternateScreen, cursor::Hide) {
            let _ = disable_raw_mode();
            return Err(err);
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
    }
}
