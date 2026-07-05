# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to semantic versioning.

## [0.1.0] - 2026-07-05

### Added

- Initial EnvLens TUI with sources, variables, and details panes.
- Dotenv discovery for standard `.env*` files and example/template files.
- Docker Compose environment parsing for map, list, and inherited-key forms.
- Package script parsing for leading inline env assignments, `cross-env`, and simple `set KEY=value &&` scripts.
- Flat CI env parsing for GitHub Actions and GitLab configuration.
- Process environment source with highest default precedence.
- Deterministic effective-value resolution with built-in profiles, source filtering, custom precedence, and reference expansion.
- Diagnostics for duplicates, conflicts, missing required variables, empty required variables, invalid dotenv lines, undefined references, circular references, unresolved inherited compose keys, shadowed values, and secrets in tracked files.
- Secret-like key/value detection with masked-by-default rendering.
- `envlens check` with human-readable and JSON output, strict mode, `--no-values`, and CI-friendly exit codes.
- `envlens report` with sanitized markdown and JSON output.
- `.envlens.yml` / `.envlens.yaml` / `.config/envlens.yml` discovery, user config merge, config warnings, custom required keys, custom secret patterns, failure threshold, ignores, precedence, and profiles.
- TUI search, filtering, sorting, source toggling, reveal controls, sanitized export, editor open, refresh, help overlay, ASCII mode, and no-color support.
- Fixture-backed CLI integration tests and TUI snapshot tests.
- VHS demo tape plus static screenshot fallback for environments without `vhs`.

### Security

- CLI/report outputs and diagnostic messages mask secret-like values.
- `--no-values` removes value-bearing fields from machine-readable output.
- The crate uses no network stack and does not collect telemetry.
