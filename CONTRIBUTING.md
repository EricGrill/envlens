# Contributing

EnvLens is a small Rust crate with a deterministic core and thin CLI/TUI/report frontends. Keep changes scoped, tested, and aligned with existing patterns.

## Setup

```sh
cargo build
cargo test
```

No external services are required for the test suite.

## Development Checks

Run these before committing:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
git diff --check
```

For a quick CLI smoke test:

```sh
cargo test --test cli version_flag
cargo run -- check --no-color tests/fixtures/basic
```

## Snapshot Workflow

EnvLens uses `insta` for CLI and TUI snapshots.

```sh
cargo test
cargo insta review
```

Only accept snapshot changes after confirming the rendered behavior is intended and secrets remain masked unless the specific test is intentionally checking transient TUI reveal behavior.

## Fixture Conventions

- Put integration fixtures under `tests/fixtures/<name>/`.
- Keep fixtures small and readable.
- Use fake secrets only. Prefer obvious fake values such as `sk_live_...` test strings that cannot be real credentials.
- Add the smallest fixture that proves the behavior. Reuse existing fixtures when possible.
- Do not rely on the caller's process environment in fixture assertions unless the test explicitly sets it.
- Keep expected source IDs project-relative, for example `.env`, `apps/web/.env`, `docker-compose.yml[api]`, or `package.json[dev]`.

## Style

- Prefer existing modules and helpers over new abstractions.
- Do not add dependencies unless the task explicitly requires it.
- Keep core behavior deterministic: stable ordering, no timestamps in core data, and sanitized output at report boundaries.
- Preserve the security posture: no raw secret values in CLI output, reports, diagnostics, config warnings, or panic output.
