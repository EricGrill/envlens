use std::collections::BTreeMap;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use envlens::config::Config;
use envlens::core::{External, analyze};
use envlens::tui::app::{App, Event, FilterMode, Pane, update};
use envlens::tui::theme::Theme;
use envlens::tui::views;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn app_for(name: &str) -> App {
    let root = fixture(name);
    let analysis = match analyze(
        &root,
        &Config::default(),
        None,
        None,
        External {
            process_env: BTreeMap::new(),
            tracked_files: None,
        },
    ) {
        Ok(analysis) => analysis,
        Err(err) => panic!("fixture analysis failed for {name}: {err}"),
    };
    App::new(analysis, root, Config::default(), None, None, true)
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn render(app: &App, theme: &Theme, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(err) => panic!("test terminal failed: {err}"),
    };
    if let Err(err) = terminal.draw(|frame| views::draw(frame, frame.area(), app, theme)) {
        panic!("draw failed: {err}");
    }
    buffer_to_string(terminal.backend().buffer())
}

fn buffer_to_string(buffer: &Buffer) -> String {
    let area = *buffer.area();
    let mut output = String::new();
    for y in area.y..area.y + area.height {
        let mut line = String::new();
        for x in area.x..area.x + area.width {
            line.push_str(buffer[(x, y)].symbol());
        }
        output.push_str(line.trim_end());
        output.push('\n');
    }
    output
}

fn select_source(app: &mut App, id: &str) {
    app.selected_source = app
        .analysis
        .sources
        .iter()
        .position(|source| source.id == id)
        .unwrap_or_else(|| panic!("missing source {id}"));
    app.selected_var = 0;
}

fn select_visible_key(app: &mut App, key: &str) {
    app.selected_var = envlens::tui::app::visible_variables(app)
        .iter()
        .position(|variable| variable.key == key)
        .unwrap_or_else(|| panic!("missing visible key {key}"));
}

fn assert_no_secret_leak(rendered: &str) {
    for raw in [
        "envlensFakeSecretFirst1234",
        "envlensFakeSecretSecond9999",
        "eyJhbGciOi.eyJzdWIiOjE.sig",
        "envlensFakeSecretValue12345678",
        "secret123",
    ] {
        assert!(!rendered.contains(raw), "render leaked {raw}:\n{rendered}");
    }
}

#[test]
fn initial_render_basic() {
    let app = app_for("basic");
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    assert_no_secret_leak(&rendered);
    insta::assert_snapshot!(rendered);
}

#[test]
fn search_active() {
    let mut app = app_for("basic");
    update(&mut app, key(KeyCode::Char('/')));
    update(&mut app, key(KeyCode::Char('p')));
    update(&mut app, key(KeyCode::Char('o')));
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    insta::assert_snapshot!(rendered);
}

#[test]
fn filter_warnings() {
    let mut app = app_for("basic");
    app.filter = FilterMode::Warnings;
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    insta::assert_snapshot!(rendered);
}

#[test]
fn filter_conflicts() {
    let mut app = app_for("basic");
    app.filter = FilterMode::Conflicts;
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    insta::assert_snapshot!(rendered);
}

#[test]
fn filter_missing() {
    let mut app = app_for("basic");
    app.filter = FilterMode::Missing;
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    insta::assert_snapshot!(rendered);
}

#[test]
fn filter_secrets() {
    let mut app = app_for("secrets");
    app.filter = FilterMode::Secrets;
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    assert_no_secret_leak(&rendered);
    insta::assert_snapshot!(rendered);
}

#[test]
fn source_parse_error_badge() {
    let app = app_for("invalid");
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    assert!(rendered.contains("[x]!"));
    insta::assert_snapshot!(rendered);
}

#[test]
fn details_unresolved_annotation() {
    let mut app = app_for("basic");
    app.pane = Pane::Variables;
    select_source(&mut app, ".env");
    select_visible_key(&mut app, "API_URL");
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    assert!(rendered.contains("(unresolved)"));
    assert!(rendered.contains("UndefinedReference"));
    insta::assert_snapshot!(rendered);
}

#[test]
fn details_secret_masked() {
    let mut app = app_for("secrets");
    app.pane = Pane::Variables;
    select_source(&mut app, ".env");
    select_visible_key(&mut app, "JWT_SECRET");
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    assert_no_secret_leak(&rendered);
    insta::assert_snapshot!(rendered);
}

#[test]
fn details_secret_revealed() {
    let mut app = app_for("secrets");
    app.pane = Pane::Variables;
    select_source(&mut app, ".env");
    select_visible_key(&mut app, "JWT_SECRET");
    update(&mut app, key(KeyCode::Char('r')));
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    assert!(rendered.contains("envlensFakeSecretFirst1234"));
    insta::assert_snapshot!(rendered);
}

#[test]
fn details_expanded() {
    let mut app = app_for("basic");
    app.pane = Pane::Variables;
    select_source(&mut app, ".env");
    select_visible_key(&mut app, "PORT");
    update(&mut app, key(KeyCode::Enter));
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    insta::assert_snapshot!(rendered);
}

#[test]
fn help_overlay() {
    let mut app = app_for("basic");
    update(&mut app, key(KeyCode::Char('?')));
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    insta::assert_snapshot!(rendered);
}

#[test]
fn ascii_no_color_mode() {
    let app = app_for("secrets");
    let rendered = render(&app, &Theme::new(true, true), 100, 30);
    assert_no_secret_leak(&rendered);
    insta::assert_snapshot!(rendered);
}

#[test]
fn narrow_terminal_80x24() {
    let app = app_for("basic");
    let rendered = render(&app, &Theme::new(false, false), 80, 24);
    insta::assert_snapshot!(rendered);
}

#[test]
fn empty_project() {
    let app = app_for("empty");
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    insta::assert_snapshot!(rendered);
}

#[test]
fn manifest_source_details() {
    let mut app = app_for("monorepo");
    select_source(&mut app, "turbo.json");
    let rendered = render(&app, &Theme::new(false, false), 100, 30);
    assert!(rendered.contains("contributes no environment variables"));
    insta::assert_snapshot!(rendered);
}
