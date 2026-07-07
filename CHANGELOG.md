# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to semantic versioning.

## [Unreleased]

### Added

- `envlens diff` compares the effective environment of two profiles
  (`--left-profile`/`--right-profile`) or two sources (`LEFT RIGHT` tokens),
  with added/removed/changed classification, human and `--json` output,
  secret masking, `--all`, `--no-values`, and a git-style `--exit-code`
  (issue #7).
- `envlens sync` scaffolds keys present in `.env*` files into example
  templates with their values stripped; `--dry-run` previews changes. Never
  writes real (secret) values and is idempotent (issue #8).
- Source discovery for `Dockerfile`/`Containerfile` `ENV`/`ARG` instructions
  and direnv `.envrc` literal assignments; non-literal `.envrc` shell lines
  are skipped without error (issue #9).
- `envlens completions <shell>` prints a shell completion script for bash,
  zsh, fish, powershell, or elvish (issue #10).

### Testing

- Property tests (proptest) asserting the dotenv/compose/Dockerfile/direnv
  parsers never panic on arbitrary input and that `${VAR}` reference
  expansion always terminates on adversarial reference graphs (issue #11).

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
