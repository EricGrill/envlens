//! envlens: a TUI to inspect, compare, and debug environment variables
//! across `.env` files, Docker Compose, package scripts, and your shell.

pub mod cli;
pub mod config;
pub mod core;
pub mod diff;
pub mod git;
pub mod report;
pub mod sync;
pub mod tui;
