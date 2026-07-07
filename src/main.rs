use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use envlens::cli::{Cli, Command, ReportFormat};
use envlens::config::{Config, FailOn};
use envlens::core::model::{Analysis, Severity};
use envlens::core::{AnalyzeError, External};
use envlens::report::{generated_at, render_check_human, sanitize_text};
use envlens::tui::RunOptions;
use envlens::tui::theme::Theme;
use std::fs;

struct AnalysisContext {
    analysis: Analysis,
    config: Config,
    tracked: Option<BTreeSet<PathBuf>>,
}

type CliResult<T> = Result<T, (u8, String)>;
type AnalysisWithTracked = (Analysis, Option<BTreeSet<PathBuf>>);

fn main() -> ExitCode {
    install_panic_hook();
    #[cfg(debug_assertions)]
    if let Some(message) = std::env::var_os("ENVLENS_TEST_PANIC") {
        panic!("forced envlens test panic {}", message.to_string_lossy());
    }
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err((code, message)) => {
            eprintln!("{message}");
            ExitCode::from(code)
        }
    }
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let mut stderr = std::io::stderr();
        let _ = crossterm::execute!(
            stderr,
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let message = sanitize_text(&panic_info.to_string());
        let truncated: String = message.chars().take(512).collect();
        eprintln!("internal error: {truncated}");
        std::process::exit(4);
    }));
}

fn run(mut cli: Cli) -> CliResult<u8> {
    let command = cli.command.take();
    match command {
        None => {
            let root = cli.path.clone().unwrap_or_else(|| ".".into());
            let context = analyze_context_for_cli(&root, &cli)?;
            let theme = Theme::new(!should_color_output(&cli), cli.ascii);
            let has_editor = std::env::var_os("EDITOR").is_some();
            let refresh_root = root.clone();
            let refresh_config = context.config.clone();
            let refresh_profile = cli.profile.clone();
            let refresh_sources = cli.source.clone();
            envlens::tui::run(
                RunOptions {
                    analysis: context.analysis,
                    root,
                    config: context.config,
                    profile: cli.profile.clone(),
                    tracked: context.tracked,
                    theme,
                    has_editor,
                    source_date_epoch: source_date_epoch(),
                },
                move || {
                    analyze_with_external(
                        &refresh_root,
                        &refresh_config,
                        refresh_profile.as_deref(),
                        &refresh_sources,
                    )
                    .map_err(|(_, message)| anyhow::anyhow!(message))
                },
            )
            .map_err(|err| {
                (
                    4,
                    format!("could not start TUI: {}", sanitize_text(&err.to_string())),
                )
            })?;
            Ok(0)
        }
        Some(Command::Check {
            path,
            json,
            strict,
            no_values,
        }) => {
            let root = path
                .or_else(|| cli.path.clone())
                .unwrap_or_else(|| ".".into());
            let context = analyze_context_for_cli(&root, &cli)?;
            let analysis = context.analysis;
            if json {
                let generated_at = generated_at(source_date_epoch());
                println!(
                    "{}",
                    envlens::report::json::render(&analysis, generated_at, no_values)
                        .map_err(|err| (4, format!("could not serialize analysis: {err}")))?
                );
            } else {
                print!(
                    "{}",
                    render_check_human(&analysis, should_color_output(&cli), no_values)
                );
            }
            Ok(check_exit_code(&analysis, strict, context.config.fail_on))
        }
        Some(Command::Report {
            path,
            format,
            out,
            no_values,
        }) => {
            let root = path
                .or_else(|| cli.path.clone())
                .unwrap_or_else(|| ".".into());
            let analysis = analyze_for_cli(&root, &cli)?;
            let generated_at = generated_at(source_date_epoch());
            let rendered = render_report(&analysis, format, generated_at, no_values)?;
            if let Some(out) = out {
                fs::write(&out, rendered).map_err(|err| {
                    (
                        4,
                        format!("could not write report {}: {err}", out.display()),
                    )
                })?;
            } else {
                print!("{rendered}");
            }
            Ok(0)
        }
        Some(Command::Diff {
            left,
            right,
            path,
            left_profile,
            right_profile,
            json,
            all,
            no_values,
            exit_code,
        }) => {
            let root = path
                .or_else(|| cli.path.clone())
                .unwrap_or_else(|| ".".into());
            let config = load_config_for_cli(&root, &cli);

            let ((left_analysis, left_label), (right_analysis, right_label)) =
                if left_profile.is_some() || right_profile.is_some() {
                    let (Some(lp), Some(rp)) = (left_profile, right_profile) else {
                        return Err((
                            2,
                            "diff by profile requires both --left-profile and --right-profile"
                                .to_string(),
                        ));
                    };
                    let left = analyze_with_external(&root, &config, Some(&lp), &[])?.0;
                    let right = analyze_with_external(&root, &config, Some(&rp), &[])?.0;
                    ((left, lp), (right, rp))
                } else {
                    let (Some(l), Some(r)) = (left, right) else {
                        return Err((
                            2,
                            "diff requires two source tokens (LEFT RIGHT) or \
                             --left-profile/--right-profile"
                                .to_string(),
                        ));
                    };
                    let profile = cli.profile.as_deref();
                    let left =
                        analyze_with_external(&root, &config, profile, std::slice::from_ref(&l))?.0;
                    let right =
                        analyze_with_external(&root, &config, profile, std::slice::from_ref(&r))?.0;
                    ((left, l), (right, r))
                };

            let result =
                envlens::diff::compute(&left_analysis, &right_analysis, left_label, right_label);
            if json {
                println!(
                    "{}",
                    envlens::diff::render_json(&result, no_values)
                        .map_err(|err| (4, format!("could not serialize diff: {err}")))?
                );
            } else {
                print!(
                    "{}",
                    envlens::diff::render_human(&result, all, no_values, should_color_output(&cli))
                );
            }
            Ok(if exit_code && result.has_changes() {
                1
            } else {
                0
            })
        }
        Some(Command::Sync { path, dry_run }) => {
            let root = path
                .or_else(|| cli.path.clone())
                .unwrap_or_else(|| ".".into());
            let config = load_config_for_cli(&root, &cli);
            let analysis =
                analyze_with_external(&root, &config, cli.profile.as_deref(), &cli.source)?.0;
            let plan = envlens::sync::plan(&analysis);
            if !dry_run {
                envlens::sync::apply(&root, &plan)
                    .map_err(|err| (4, format!("could not write example file: {err}")))?;
            }
            print!("{}", envlens::sync::render_plan(&plan, dry_run));
            Ok(0)
        }
        Some(Command::Completions { shell }) => {
            let mut command = Cli::command();
            clap_complete::generate(shell, &mut command, "envlens", &mut std::io::stdout());
            Ok(0)
        }
    }
}

fn render_report(
    analysis: &Analysis,
    format: ReportFormat,
    generated_at: String,
    no_values: bool,
) -> CliResult<String> {
    match format {
        ReportFormat::Markdown => Ok(envlens::report::markdown::render(
            analysis,
            generated_at,
            no_values,
        )),
        ReportFormat::Json => envlens::report::json::render(analysis, generated_at, no_values)
            .map(|json| format!("{json}\n"))
            .map_err(|err| (4, format!("could not serialize analysis: {err}"))),
    }
}

fn source_date_epoch() -> Option<u64> {
    match std::env::var("SOURCE_DATE_EPOCH") {
        Ok(value) => value.parse::<u64>().ok(),
        Err(_) => None,
    }
}

fn should_color_output(cli: &Cli) -> bool {
    !cli.no_color && std::env::var_os("NO_COLOR").is_none()
}

fn analyze_for_cli(root: &Path, cli: &Cli) -> CliResult<Analysis> {
    analyze_context_for_cli(root, cli).map(|context| context.analysis)
}

fn analyze_context_for_cli(root: &Path, cli: &Cli) -> CliResult<AnalysisContext> {
    let config = load_config_for_cli(root, cli);
    let (analysis, tracked) =
        analyze_with_external(root, &config, cli.profile.as_deref(), &cli.source)?;
    Ok(AnalysisContext {
        analysis,
        config,
        tracked,
    })
}

/// Discover and merge configuration for a CLI run, emitting any config
/// warnings and folding in `--ignore` directories. Shared by every subcommand
/// so they resolve config identically.
fn load_config_for_cli(root: &Path, cli: &Cli) -> Config {
    let mut loaded = if let Some(path) = &cli.config {
        envlens::config::load_file(path)
    } else {
        envlens::config::discover(
            root,
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        )
    };
    for warning in loaded.warnings {
        eprintln!("warning: {}", sanitize_text(&warning));
    }
    loaded.config.ignore.extend(cli.ignore.clone());
    loaded.config
}

fn analyze_with_external(
    root: &Path,
    config: &Config,
    profile: Option<&str>,
    source_filter: &[String],
) -> CliResult<AnalysisWithTracked> {
    let tracked_files = envlens::git::tracked_files(root);
    let external = External {
        process_env: envlens::core::parsers::process::capture(),
        tracked_files: tracked_files.clone(),
    };
    let source_filter = (!source_filter.is_empty()).then_some(source_filter);

    envlens::core::analyze(root, config, profile, source_filter, external)
        .map(|analysis| (analysis, tracked_files))
        .map_err(|err| match err {
            AnalyzeError::RootUnreadable(path) => {
                (3, format!("root is unreadable: {}", path.display()))
            }
            AnalyzeError::UnknownProfile(name) => {
                (2, format!("unknown profile '{}'", sanitize_text(&name)))
            }
            AnalyzeError::UnknownSource(name) => {
                (2, format!("unknown source '{}'", sanitize_text(&name)))
            }
        })
}

fn check_exit_code(analysis: &Analysis, strict: bool, fail_on: FailOn) -> u8 {
    let threshold = match (strict, fail_on) {
        (true, _) | (false, FailOn::Warning) => Severity::Warning,
        (false, FailOn::Error) => Severity::Error,
    };
    if analysis
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity >= threshold)
    {
        1
    } else {
        0
    }
}
