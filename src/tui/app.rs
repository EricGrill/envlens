use std::collections::BTreeSet;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::Config;
use crate::core::model::{Analysis, DiagnosticCode, Severity, VariableSummary};
use crate::core::re_resolve;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sources,
    Variables,
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    All,
    Warnings,
    Missing,
    Conflicts,
    Secrets,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Key,
    Severity,
    SourceCount,
    EffectiveSource,
    Secret,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    Help,
    SortMenu,
    ConfirmRevealAll,
    ExportPrompt { input: String },
}

#[derive(Debug, Clone)]
pub struct App {
    pub analysis: Analysis,
    pub root: PathBuf,
    pub pane: Pane,
    pub selected_source: usize,
    pub selected_var: usize,
    pub search: Option<String>,
    pub filter: FilterMode,
    pub sort: SortMode,
    pub revealed: BTreeSet<String>,
    pub reveal_all: bool,
    pub modal: Option<Modal>,
    pub status: Option<String>,
    pub expanded: BTreeSet<String>,
    pub has_editor: bool,
    pub config: Config,
    pub profile: Option<String>,
    pub tracked: Option<BTreeSet<PathBuf>>,
    pub should_quit: bool,
    pub want_refresh: bool,
    pub want_editor: Option<(PathBuf, u32)>,
    pub want_export: Option<PathBuf>,
}

impl App {
    pub fn new(
        analysis: Analysis,
        root: PathBuf,
        config: Config,
        profile: Option<String>,
        tracked: Option<BTreeSet<PathBuf>>,
        has_editor: bool,
    ) -> Self {
        let mut app = Self {
            analysis,
            root,
            pane: Pane::Sources,
            selected_source: 0,
            selected_var: 0,
            search: None,
            filter: FilterMode::All,
            sort: SortMode::Key,
            revealed: BTreeSet::new(),
            reveal_all: false,
            modal: None,
            status: None,
            expanded: BTreeSet::new(),
            has_editor,
            config,
            profile,
            tracked,
            should_quit: false,
            want_refresh: false,
            want_editor: None,
            want_export: None,
        };
        clamp_selections(&mut app);
        app
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Key(KeyEvent),
    Resize(u16, u16),
    Tick,
}

pub fn update(app: &mut App, ev: Event) {
    let Event::Key(key) = ev else {
        return;
    };

    if handle_escape(app, key) {
        clamp_selections(app);
        return;
    }
    if handle_modal(app, key) {
        clamp_selections(app);
        return;
    }
    if handle_search(app, key) {
        clamp_selections(app);
        return;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
        app.want_refresh = true;
        return;
    }

    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Tab => app.pane = next_pane(app.pane),
        KeyCode::Up | KeyCode::Char('k') => move_selection(app, -1),
        KeyCode::Down | KeyCode::Char('j') => move_selection(app, 1),
        KeyCode::Char('/') => app.search = Some(String::new()),
        KeyCode::Char('f') => {
            app.filter = next_filter(app.filter);
            app.selected_var = 0;
        }
        KeyCode::Char('s') => app.modal = Some(Modal::SortMenu),
        KeyCode::Char('r') => toggle_selected_reveal(app),
        KeyCode::Char('R') => app.modal = Some(Modal::ConfirmRevealAll),
        KeyCode::Char(' ') if app.pane == Pane::Sources => toggle_selected_source(app),
        KeyCode::Char('e') => {
            app.modal = Some(Modal::ExportPrompt {
                input: "envlens-report.md".to_string(),
            });
        }
        KeyCode::Char('o') => open_selected_in_editor(app),
        KeyCode::Enter if app.pane == Pane::Variables => toggle_selected_expanded(app),
        KeyCode::Char('?') => app.modal = Some(Modal::Help),
        _ => {}
    }
    clamp_selections(app);
}

pub fn visible_variables(app: &App) -> Vec<&VariableSummary> {
    let selected_source = app
        .analysis
        .sources
        .get(app.selected_source)
        .map(|source| source.id.as_str());
    let search = app.search.as_ref().map(|query| query.to_ascii_lowercase());

    let mut variables: Vec<&VariableSummary> = app
        .analysis
        .variables
        .iter()
        .filter(|variable| {
            selected_source.is_none_or(|source_id| {
                variable
                    .occurrences
                    .iter()
                    .any(|occurrence| occurrence.source_id == source_id)
            })
        })
        .filter(|variable| {
            search.as_ref().is_none_or(|query| {
                query.is_empty() || variable.key.to_ascii_lowercase().contains(query)
            })
        })
        .filter(|variable| filter_matches(variable, app.filter))
        .collect();

    sort_variables(&mut variables, app.sort);
    variables
}

fn handle_escape(app: &mut App, key: KeyEvent) -> bool {
    if key.code != KeyCode::Esc {
        return false;
    }
    if app.modal.is_some() {
        app.modal = None;
        return true;
    }
    if app.search.is_some() {
        app.search = None;
        return true;
    }
    if app.reveal_all || !app.revealed.is_empty() {
        app.reveal_all = false;
        app.revealed.clear();
        return true;
    }
    true
}

fn handle_modal(app: &mut App, key: KeyEvent) -> bool {
    let Some(modal) = app.modal.take() else {
        return false;
    };

    match modal {
        Modal::Help => {
            app.modal = Some(Modal::Help);
            true
        }
        Modal::SortMenu => {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => app.sort = next_sort(app.sort),
                KeyCode::Up | KeyCode::Char('k') => app.sort = previous_sort(app.sort),
                KeyCode::Enter => {
                    app.selected_var = 0;
                    return true;
                }
                _ => {}
            }
            app.modal = Some(Modal::SortMenu);
            true
        }
        Modal::ConfirmRevealAll => {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => app.reveal_all = true,
                KeyCode::Char('n') | KeyCode::Char('N') => {}
                _ => app.modal = Some(Modal::ConfirmRevealAll),
            }
            true
        }
        Modal::ExportPrompt { mut input } => {
            match key.code {
                KeyCode::Enter => {
                    let path = if input.trim().is_empty() {
                        PathBuf::from("envlens-report.md")
                    } else {
                        PathBuf::from(input.trim())
                    };
                    app.want_export = Some(path);
                }
                KeyCode::Backspace => {
                    input.pop();
                    app.modal = Some(Modal::ExportPrompt { input });
                }
                KeyCode::Char(ch) => {
                    input.push(ch);
                    app.modal = Some(Modal::ExportPrompt { input });
                }
                _ => app.modal = Some(Modal::ExportPrompt { input }),
            }
            true
        }
    }
}

fn handle_search(app: &mut App, key: KeyEvent) -> bool {
    let Some(mut query) = app.search.take() else {
        return false;
    };

    match key.code {
        KeyCode::Char(ch) => {
            query.push(ch);
            app.search = Some(query);
        }
        KeyCode::Backspace => {
            query.pop();
            app.search = Some(query);
        }
        KeyCode::Enter => app.search = Some(query),
        _ => app.search = Some(query),
    }
    true
}

fn next_pane(pane: Pane) -> Pane {
    match pane {
        Pane::Sources => Pane::Variables,
        Pane::Variables => Pane::Details,
        Pane::Details => Pane::Sources,
    }
}

fn move_selection(app: &mut App, delta: isize) {
    match app.pane {
        Pane::Sources => {
            app.selected_source =
                moved_index(app.selected_source, app.analysis.sources.len(), delta);
            app.selected_var = 0;
        }
        Pane::Variables | Pane::Details => {
            app.selected_var = moved_index(app.selected_var, visible_variables(app).len(), delta);
        }
    }
}

fn moved_index(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    current
        .saturating_add_signed(delta)
        .min(len.saturating_sub(1))
}

fn next_filter(filter: FilterMode) -> FilterMode {
    match filter {
        FilterMode::All => FilterMode::Warnings,
        FilterMode::Warnings => FilterMode::Missing,
        FilterMode::Missing => FilterMode::Conflicts,
        FilterMode::Conflicts => FilterMode::Secrets,
        FilterMode::Secrets => FilterMode::All,
    }
}

fn next_sort(sort: SortMode) -> SortMode {
    match sort {
        SortMode::Key => SortMode::Severity,
        SortMode::Severity => SortMode::SourceCount,
        SortMode::SourceCount => SortMode::EffectiveSource,
        SortMode::EffectiveSource => SortMode::Secret,
        SortMode::Secret => SortMode::Key,
    }
}

fn previous_sort(sort: SortMode) -> SortMode {
    match sort {
        SortMode::Key => SortMode::Secret,
        SortMode::Severity => SortMode::Key,
        SortMode::SourceCount => SortMode::Severity,
        SortMode::EffectiveSource => SortMode::SourceCount,
        SortMode::Secret => SortMode::EffectiveSource,
    }
}

fn filter_matches(variable: &VariableSummary, filter: FilterMode) -> bool {
    match filter {
        FilterMode::All => true,
        FilterMode::Warnings => variable
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Warning),
        FilterMode::Missing => {
            variable.is_missing
                || variable
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == DiagnosticCode::MissingRequired)
        }
        FilterMode::Conflicts => variable
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::ConflictingValues),
        FilterMode::Secrets => variable.is_secret_like,
    }
}

fn sort_variables(variables: &mut Vec<&VariableSummary>, sort: SortMode) {
    variables.sort_by(|left, right| match sort {
        SortMode::Key => left.key.cmp(&right.key),
        SortMode::Severity => max_severity(right)
            .cmp(&max_severity(left))
            .then_with(|| left.key.cmp(&right.key)),
        SortMode::SourceCount => right
            .occurrences
            .len()
            .cmp(&left.occurrences.len())
            .then_with(|| left.key.cmp(&right.key)),
        SortMode::EffectiveSource => effective_source(left)
            .cmp(effective_source(right))
            .then_with(|| left.key.cmp(&right.key)),
        SortMode::Secret => right
            .is_secret_like
            .cmp(&left.is_secret_like)
            .then_with(|| left.key.cmp(&right.key)),
    });
}

fn max_severity(variable: &VariableSummary) -> Option<Severity> {
    variable
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.severity)
        .max()
}

fn effective_source(variable: &VariableSummary) -> &str {
    variable
        .effective
        .as_ref()
        .map(|(_, source_id)| source_id.as_str())
        .unwrap_or("")
}

fn toggle_selected_reveal(app: &mut App) {
    let Some(key) = selected_variable_key(app) else {
        return;
    };
    if !app.revealed.remove(&key) {
        app.revealed.insert(key);
    }
}

fn toggle_selected_expanded(app: &mut App) {
    let Some(key) = selected_variable_key(app) else {
        return;
    };
    if !app.expanded.remove(&key) {
        app.expanded.insert(key);
    }
}

fn toggle_selected_source(app: &mut App) {
    let Some(source_id) = app
        .analysis
        .sources
        .get(app.selected_source)
        .map(|source| source.id.clone())
    else {
        return;
    };

    if let Some(source) = app
        .analysis
        .sources
        .iter_mut()
        .find(|source| source.id == source_id)
    {
        source.enabled = !source.enabled;
    }

    re_resolve(
        &mut app.analysis,
        &app.config,
        app.profile.as_deref(),
        app.tracked.as_ref(),
    );
    if let Some(idx) = app
        .analysis
        .sources
        .iter()
        .position(|source| source.id == source_id)
    {
        app.selected_source = idx;
    }
}

fn open_selected_in_editor(app: &mut App) {
    if !app.has_editor {
        app.status = Some("$EDITOR is not set".to_string());
        return;
    }

    match selected_editor_target(app) {
        Some(target) => app.want_editor = Some(target),
        None => app.status = Some("no source location for selected variable".to_string()),
    }
}

fn selected_editor_target(app: &App) -> Option<(PathBuf, u32)> {
    let variable = selected_variable(app)?;
    let (_, source_id) = variable.effective.as_ref()?;
    let source = app
        .analysis
        .sources
        .iter()
        .find(|source| &source.id == source_id)?;
    let path = source.path.as_ref()?;
    let line = variable
        .occurrences
        .iter()
        .rfind(|occurrence| &occurrence.source_id == source_id)
        .and_then(|occurrence| occurrence.line)?;
    Some((app.root.join(path), line))
}

fn selected_variable_key(app: &App) -> Option<String> {
    selected_variable(app).map(|variable| variable.key.clone())
}

fn selected_variable(app: &App) -> Option<&VariableSummary> {
    visible_variables(app).get(app.selected_var).copied()
}

fn clamp_selections(app: &mut App) {
    app.selected_source = app
        .selected_source
        .min(app.analysis.sources.len().saturating_sub(1));
    app.selected_var = app
        .selected_var
        .min(visible_variables(app).len().saturating_sub(1));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::config::Config;
    use crate::core::model::{
        Analysis, DiagnosticCode, EnvSource, ParseError, SecretClass, SourceKind,
        VariableOccurrence, VariableSummary,
    };
    use crate::core::re_resolve;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl_r() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
    }

    fn source(id: &str, kind: SourceKind, path: Option<&str>) -> EnvSource {
        EnvSource {
            id: id.to_string(),
            kind,
            path: path.map(PathBuf::from),
            context: None,
            precedence: 0,
            enabled: true,
            errors: Vec::<ParseError>::new(),
        }
    }

    fn occ(
        key: &str,
        value: &str,
        source_id: &str,
        line: u32,
        secret: SecretClass,
    ) -> VariableOccurrence {
        VariableOccurrence {
            key: key.to_string(),
            raw_value: Some(value.to_string()),
            parsed_value: Some(value.to_string()),
            source_id: source_id.to_string(),
            line: Some(line),
            is_empty: value.is_empty(),
            is_inherited: false,
            no_expand: false,
            secret,
        }
    }

    fn var(key: &str, occurrences: Vec<VariableOccurrence>) -> VariableSummary {
        VariableSummary {
            key: key.to_string(),
            effective: None,
            occurrences,
            diagnostics: Vec::new(),
            is_required: false,
            is_missing: false,
            is_secret_like: false,
        }
    }

    fn analysis_fixture() -> Analysis {
        Analysis {
            root: PathBuf::from("/repo"),
            profile: "default".to_string(),
            sources: vec![
                source(
                    ".env.example",
                    SourceKind::DotenvExample,
                    Some(".env.example"),
                ),
                source(".env", SourceKind::Dotenv, Some(".env")),
                source(".env.local", SourceKind::Dotenv, Some(".env.local")),
                source("process", SourceKind::Process, None),
            ],
            variables: vec![
                var(
                    "ALPHA",
                    vec![occ("ALPHA", "z", ".env.local", 2, SecretClass::None)],
                ),
                var(
                    "API_KEY",
                    vec![occ(
                        "API_KEY",
                        "envlensFakeHistoricalSecret",
                        ".env",
                        5,
                        SecretClass::Both,
                    )],
                ),
                var(
                    "API_URL",
                    vec![occ("API_URL", "${HOST}:5001", ".env", 3, SecretClass::None)],
                ),
                var(
                    "CLEAN",
                    vec![occ("CLEAN", "ok", ".env", 6, SecretClass::None)],
                ),
                var(
                    "JWT_SECRET",
                    vec![occ(
                        "JWT_SECRET",
                        "",
                        ".env.example",
                        4,
                        SecretClass::KeyLike,
                    )],
                ),
                var(
                    "PORT",
                    vec![
                        occ("PORT", "", ".env.example", 2, SecretClass::None),
                        occ("PORT", "3000", ".env", 2, SecretClass::None),
                        occ("PORT", "5001", ".env.local", 1, SecretClass::None),
                    ],
                ),
            ],
            diagnostics: Vec::new(),
        }
    }

    fn sample_app() -> App {
        let config = Config::default();
        let tracked = Some(BTreeSet::from([PathBuf::from(".env")]));
        let mut analysis = analysis_fixture();
        re_resolve(&mut analysis, &config, None, tracked.as_ref());
        App::new(
            analysis,
            PathBuf::from("/repo"),
            config,
            None,
            tracked,
            true,
        )
    }

    fn keys(app: &App) -> Vec<String> {
        visible_variables(app)
            .into_iter()
            .map(|var| var.key.clone())
            .collect()
    }

    fn source_index(app: &App, id: &str) -> usize {
        app.analysis
            .sources
            .iter()
            .position(|source| source.id == id)
            .unwrap_or_else(|| panic!("missing source {id}"))
    }

    fn select_source(app: &mut App, id: &str) {
        app.selected_source = source_index(app, id);
        app.selected_var = 0;
    }

    fn select_visible_key(app: &mut App, key: &str) {
        app.selected_var = visible_variables(app)
            .iter()
            .position(|var| var.key == key)
            .unwrap_or_else(|| panic!("missing visible variable {key}"));
    }

    fn effective<'a>(app: &'a App, key: &str) -> Option<&'a (String, String)> {
        app.analysis
            .variables
            .iter()
            .find(|var| var.key == key)
            .and_then(|var| var.effective.as_ref())
    }

    #[test]
    fn q_quits_tab_cycles_and_movement_clamps() {
        let mut app = sample_app();
        update(&mut app, key(KeyCode::Char('q')));
        assert!(app.should_quit);

        let mut app = sample_app();
        assert_eq!(app.pane, Pane::Sources);
        update(&mut app, key(KeyCode::Tab));
        assert_eq!(app.pane, Pane::Variables);
        update(&mut app, key(KeyCode::Tab));
        assert_eq!(app.pane, Pane::Details);
        update(&mut app, key(KeyCode::Tab));
        assert_eq!(app.pane, Pane::Sources);

        update(&mut app, key(KeyCode::Up));
        assert_eq!(app.selected_source, 0);
        update(&mut app, key(KeyCode::Down));
        assert_eq!(app.selected_source, 1);
        for _ in 0..10 {
            update(&mut app, key(KeyCode::Char('j')));
        }
        assert_eq!(app.selected_source, app.analysis.sources.len() - 1);

        select_source(&mut app, ".env");
        app.pane = Pane::Variables;
        app.selected_var = 0;
        update(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.selected_var, 0);
        update(&mut app, key(KeyCode::Char('j')));
        assert_eq!(app.selected_var, 1);
    }

    #[test]
    fn slash_enters_search_typing_filters() {
        let mut app = sample_app();

        update(&mut app, key(KeyCode::Char('/')));
        update(&mut app, key(KeyCode::Char('p')));
        update(&mut app, key(KeyCode::Char('o')));

        assert_eq!(app.search.as_deref(), Some("po"));
        assert_eq!(keys(&app), vec!["PORT"]);
    }

    #[test]
    fn esc_layering_closes_modal_then_search_then_remasks() {
        let mut app = sample_app();
        app.modal = Some(Modal::Help);
        app.search = Some("po".to_string());
        app.reveal_all = true;
        app.revealed.insert("API_KEY".to_string());

        update(&mut app, key(KeyCode::Esc));
        assert_eq!(app.modal, None);
        assert_eq!(app.search.as_deref(), Some("po"));
        assert!(app.reveal_all);

        update(&mut app, key(KeyCode::Esc));
        assert_eq!(app.search, None);
        assert!(app.reveal_all);

        update(&mut app, key(KeyCode::Esc));
        assert!(!app.reveal_all);
        assert!(app.revealed.is_empty());

        update(&mut app, key(KeyCode::Esc));
        assert!(!app.reveal_all);
    }

    #[test]
    fn f_cycles_filters_and_warnings_filter_hides_clean_vars() {
        let mut app = sample_app();
        assert_eq!(app.filter, FilterMode::All);
        update(&mut app, key(KeyCode::Char('f')));
        assert_eq!(app.filter, FilterMode::Warnings);
        update(&mut app, key(KeyCode::Char('f')));
        assert_eq!(app.filter, FilterMode::Missing);
        update(&mut app, key(KeyCode::Char('f')));
        assert_eq!(app.filter, FilterMode::Conflicts);
        update(&mut app, key(KeyCode::Char('f')));
        assert_eq!(app.filter, FilterMode::Secrets);
        update(&mut app, key(KeyCode::Char('f')));
        assert_eq!(app.filter, FilterMode::All);

        select_source(&mut app, ".env");
        app.filter = FilterMode::Warnings;
        let visible = keys(&app);
        assert!(visible.contains(&"API_URL".to_string()));
        assert!(visible.contains(&"PORT".to_string()));
        assert!(!visible.contains(&"CLEAN".to_string()));
    }

    #[test]
    fn sort_menu_selection_applies() {
        let mut app = sample_app();
        select_source(&mut app, ".env");
        assert_eq!(keys(&app), vec!["API_KEY", "API_URL", "CLEAN", "PORT"]);

        update(&mut app, key(KeyCode::Char('s')));
        assert_eq!(app.modal, Some(Modal::SortMenu));
        update(&mut app, key(KeyCode::Down));
        update(&mut app, key(KeyCode::Down));
        update(&mut app, key(KeyCode::Enter));

        assert_eq!(app.modal, None);
        assert_eq!(app.sort, SortMode::SourceCount);
        assert_eq!(keys(&app).first().map(String::as_str), Some("PORT"));
    }

    #[test]
    fn reveal_toggles_and_reveal_all_requires_confirm() {
        let mut app = sample_app();
        app.pane = Pane::Variables;
        select_source(&mut app, ".env");
        select_visible_key(&mut app, "API_KEY");

        update(&mut app, key(KeyCode::Char('r')));
        assert!(app.revealed.contains("API_KEY"));
        update(&mut app, key(KeyCode::Char('r')));
        assert!(!app.revealed.contains("API_KEY"));

        update(&mut app, key(KeyCode::Char('R')));
        assert_eq!(app.modal, Some(Modal::ConfirmRevealAll));
        update(&mut app, key(KeyCode::Char('n')));
        assert!(!app.reveal_all);
        assert_eq!(app.modal, None);

        update(&mut app, key(KeyCode::Char('R')));
        update(&mut app, key(KeyCode::Char('y')));
        assert!(app.reveal_all);
        assert_eq!(app.modal, None);
    }

    #[test]
    fn source_pane_space_toggles_enabled_and_reresolves() {
        let mut app = sample_app();
        assert_eq!(
            effective(&app, "PORT"),
            Some(&("5001".to_string(), ".env.local".to_string()))
        );

        app.pane = Pane::Sources;
        select_source(&mut app, ".env.local");
        update(&mut app, key(KeyCode::Char(' ')));

        assert!(!app.analysis.sources[source_index(&app, ".env.local")].enabled);
        assert_eq!(
            effective(&app, "PORT"),
            Some(&("3000".to_string(), ".env".to_string()))
        );
        assert!(app.analysis.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::SecretInTrackedFile
                && diagnostic.key.as_deref() == Some("API_KEY")
        }));
    }

    #[test]
    fn selecting_source_filters_variables() {
        let mut app = sample_app();

        select_source(&mut app, ".env.local");
        assert_eq!(keys(&app), vec!["ALPHA", "PORT"]);

        select_source(&mut app, ".env");
        let visible = keys(&app);
        assert!(visible.contains(&"API_KEY".to_string()));
        assert!(visible.contains(&"CLEAN".to_string()));
        assert!(!visible.contains(&"ALPHA".to_string()));
    }

    #[test]
    fn export_prompt_and_editor_target() {
        let mut app = sample_app();

        update(&mut app, key(KeyCode::Char('e')));
        assert_eq!(
            app.modal,
            Some(Modal::ExportPrompt {
                input: "envlens-report.md".to_string()
            })
        );
        update(&mut app, key(KeyCode::Enter));
        assert_eq!(app.want_export, Some(PathBuf::from("envlens-report.md")));
        assert_eq!(app.modal, None);

        select_source(&mut app, ".env");
        select_visible_key(&mut app, "API_KEY");
        update(&mut app, key(KeyCode::Char('o')));
        assert_eq!(app.want_editor, Some((PathBuf::from("/repo/.env"), 5)));

        let mut app = sample_app();
        app.has_editor = false;
        update(&mut app, key(KeyCode::Char('o')));
        assert!(
            app.status
                .as_deref()
                .is_some_and(|status| status.contains("EDITOR"))
        );
    }

    #[test]
    fn ctrl_r_sets_want_refresh() {
        let mut app = sample_app();
        update(&mut app, ctrl_r());
        assert!(app.want_refresh);
    }

    #[test]
    fn enter_toggles_expanded_in_variables_pane() {
        let mut app = sample_app();
        app.pane = Pane::Variables;
        select_source(&mut app, ".env");
        select_visible_key(&mut app, "PORT");

        update(&mut app, key(KeyCode::Enter));
        assert!(app.expanded.contains("PORT"));
        update(&mut app, key(KeyCode::Enter));
        assert!(!app.expanded.contains("PORT"));
    }

    #[test]
    fn question_mark_opens_help() {
        let mut app = sample_app();
        update(&mut app, key(KeyCode::Char('?')));
        assert_eq!(app.modal, Some(Modal::Help));
    }
}
