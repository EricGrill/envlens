use std::process::ExitCode;

use clap::Parser;
use envlens::cli::{Cli, Command};
use envlens::config::Config;
use envlens::core::model::{Analysis, Severity};
use envlens::core::{AnalyzeError, External};
use envlens::report::{generated_at, render_check_human, sanitize_text};

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

fn run(mut cli: Cli) -> Result<u8, (u8, String)> {
    let command = cli.command.take();
    match command {
        None => {
            let root = cli.path.clone().unwrap_or_else(|| ".".into());
            let _analysis = analyze_for_cli(&root, &cli)?;
            eprintln!("TUI not yet implemented");
            Ok(4)
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
            let analysis = analyze_for_cli(&root, &cli)?;
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
            Ok(check_exit_code(&analysis, strict))
        }
        Some(Command::Report { path, .. }) => {
            let root = path
                .or_else(|| cli.path.clone())
                .unwrap_or_else(|| ".".into());
            let _analysis = analyze_for_cli(&root, &cli)?;
            eprintln!("report not yet implemented");
            Ok(4)
        }
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

fn analyze_for_cli(root: &std::path::Path, cli: &Cli) -> Result<Analysis, (u8, String)> {
    let config = Config {
        ignore: cli.ignore.clone(),
        ..Config::default()
    };
    let external = External {
        process_env: envlens::core::parsers::process::capture(),
        tracked_files: envlens::git::tracked_files(root),
    };
    let source_filter = (!cli.source.is_empty()).then_some(cli.source.as_slice());

    envlens::core::analyze(
        root,
        &config,
        cli.profile.as_deref(),
        source_filter,
        external,
    )
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

fn check_exit_code(analysis: &Analysis, strict: bool) -> u8 {
    let threshold = if strict {
        Severity::Warning
    } else {
        Severity::Error
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
