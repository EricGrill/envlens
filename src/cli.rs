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
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReportFormat {
    Markdown,
    Json,
}
