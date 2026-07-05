use std::process::ExitCode;

use clap::Parser;
use envlens::cli::{Cli, Command};
use envlens::config::Config;
use envlens::core::model::{Analysis, Severity, SourceKind};
use envlens::core::secrets::{MaskedValue, classify_value};
use envlens::core::{AnalyzeError, External};
use regex::Regex;
use serde_json::json;
use std::sync::OnceLock;

fn main() -> ExitCode {
    install_panic_hook();
    #[cfg(debug_assertions)]
    if std::env::var_os("ENVLENS_TEST_PANIC").is_some() {
        panic!("forced envlens test panic {}", "x".repeat(1024));
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
        let message = panic_info.to_string();
        let truncated: String = message.chars().take(512).collect();
        eprintln!("internal error: {truncated}");
        std::process::exit(4);
    }));
}

fn run(mut cli: Cli) -> Result<u8, (u8, String)> {
    let command = cli.command.take();
    match command {
        None => {
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
                println!("{}", minimal_json(&analysis, no_values)?);
            } else {
                print_human_check(&analysis);
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
        AnalyzeError::UnknownProfile(name) => (2, format!("unknown profile '{name}'")),
        AnalyzeError::UnknownSource(name) => (2, format!("unknown source '{name}'")),
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

fn print_human_check(analysis: &Analysis) {
    for diagnostic in &analysis.diagnostics {
        println!(
            "{:?} {:?} {}",
            diagnostic.severity,
            diagnostic.code,
            sanitize_text(&diagnostic.message)
        );
    }
}

fn minimal_json(analysis: &Analysis, no_values: bool) -> Result<String, (u8, String)> {
    let sources: Vec<_> = analysis
        .sources
        .iter()
        .map(|source| {
            json!({
                "id": source.id,
                "kind": source_kind(source.kind),
                "path": source.path.as_ref().map(|path| path.to_string_lossy().into_owned()),
                "context": source.context,
                "precedence": source.precedence,
                "enabled": source.enabled,
                "errors": source.errors.iter().map(|error| {
                    json!({
                        "line": error.line,
                        "message": sanitize_text(&error.message),
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();

    let variables: Vec<_> = analysis
        .variables
        .iter()
        .map(|var| {
            let effective = if no_values {
                var.effective
                    .as_ref()
                    .map(|(_, source_id)| json!({ "source_id": source_id }))
            } else {
                var.effective.as_ref().map(|(value, source_id)| {
                    json!({
                        "value": MaskedValue::new(value.clone(), var.is_secret_like, false).to_string(),
                        "source_id": source_id,
                    })
                })
            };
            json!({
                "key": var.key,
                "effective": effective,
                "is_required": var.is_required,
                "is_missing": var.is_missing,
                "is_secret_like": var.is_secret_like,
                "diagnostics": var.diagnostics.iter().map(diagnostic_json).collect::<Vec<_>>(),
            })
        })
        .collect();

    serde_json::to_string_pretty(&json!({
        "root": analysis.root.to_string_lossy(),
        "profile": analysis.profile,
        "sources": sources,
        "variables": variables,
        "diagnostics": analysis.diagnostics.iter().map(diagnostic_json).collect::<Vec<_>>(),
    }))
    .map_err(|err| (4, format!("could not serialize analysis: {err}")))
}

fn diagnostic_json(diagnostic: &envlens::core::model::Diagnostic) -> serde_json::Value {
    json!({
        "severity": diagnostic.severity,
        "code": diagnostic.code,
        "message": sanitize_text(&diagnostic.message),
        "key": diagnostic.key,
        "source_id": diagnostic.source_id,
        "line": diagnostic.line,
    })
}

fn sanitize_text(text: &str) -> String {
    secret_token_regex()
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let token = captures
                .get(0)
                .map(|matched| matched.as_str())
                .unwrap_or("");
            if classify_value(token) {
                MaskedValue::new(token, true, false).to_string()
            } else {
                token.to_string()
            }
        })
        .into_owned()
}

fn secret_token_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| match Regex::new(r"[A-Za-z0-9_./:@+=-]{8,}") {
        Ok(regex) => regex,
        Err(err) => panic!("secret redaction regex constant is invalid: {err}"),
    })
}

fn source_kind(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Dotenv => "dotenv",
        SourceKind::DotenvExample => "dotenv_example",
        SourceKind::Compose => "compose",
        SourceKind::PackageScript => "package_script",
        SourceKind::Manifest => "manifest",
        SourceKind::Process => "process",
        SourceKind::Ci => "ci",
    }
}
