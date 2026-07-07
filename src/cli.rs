use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "envlens", version, about)]
pub struct Cli {
    pub path: Option<PathBuf>,
    #[arg(long, global = true)]
    pub profile: Option<String>,
    #[arg(long, global = true)]
    pub source: Vec<String>,
    #[arg(long, global = true)]
    pub ignore: Vec<String>,
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    #[arg(long, global = true)]
    pub no_color: bool,
    #[arg(long, global = true)]
    pub ascii: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Check {
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        strict: bool,
        #[arg(long)]
        no_values: bool,
    },
    Report {
        path: Option<PathBuf>,
        #[arg(long, value_enum)]
        format: ReportFormat,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        no_values: bool,
    },
    /// Compare the effective environment of two profiles or two sources.
    Diff {
        /// Left source token (source id or file path). Omit when using
        /// `--left-profile`.
        left: Option<String>,
        /// Right source token (source id or file path). Omit when using
        /// `--right-profile`.
        right: Option<String>,
        /// Project root to analyze (defaults to the global path or `.`).
        #[arg(long)]
        path: Option<PathBuf>,
        /// Resolve the left side using this profile instead of a source token.
        #[arg(long)]
        left_profile: Option<String>,
        /// Resolve the right side using this profile instead of a source token.
        #[arg(long)]
        right_profile: Option<String>,
        #[arg(long)]
        json: bool,
        /// Include unchanged keys in the output.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        no_values: bool,
        /// Exit non-zero (1) when any difference is found.
        #[arg(long)]
        exit_code: bool,
    },
    /// Scaffold keys present in `.env*` files into example templates,
    /// with their values stripped.
    Sync {
        path: Option<PathBuf>,
        /// Show what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print a shell completion script to stdout.
    Completions {
        /// Target shell (bash, zsh, fish, powershell, elvish).
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReportFormat {
    Markdown,
    Json,
}
