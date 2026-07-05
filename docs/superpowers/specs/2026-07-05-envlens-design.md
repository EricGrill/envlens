# EnvLens Design Spec

Date: 2026-07-05
Status: Approved by Eric (design review in session); implements `envlens-srs.md` in full (FR-001–FR-055) excluding SRS §6 "Future Enhancements".
Target: v0.1.0, ready for submission to awesome-tuis (Dashboards section).

## 1. Overview

EnvLens is a Rust terminal application that scans a project directory, discovers environment-variable sources (`.env*` files, Docker Compose files, `package.json` scripts, CI config files, the current process environment), parses them with source locations, computes effective values under a precedence order, runs diagnostics (duplicates, conflicts, shadowing, missing required, empty, undefined/circular references, secret exposure), and presents the result in a three-pane TUI or as non-interactive CLI output (`check`, `report`).

## 2. Decisions log

Decisions made with Eric during design review (2026-07-05):

| Topic | Decision |
|---|---|
| Scope | Full SRS (FR-001–FR-055). SRS §6 Future Enhancements excluded. |
| Language/stack | Rust, single crate, pure sync core + thin frontends (approach A). |
| `check` strictness (SRS Q6) | Exit 1 on error-severity diagnostics; `--strict` makes warnings fail too. |
| `.env.example` required (SRS Q1) | Yes, variables in example/sample/template files are required by default; config can override. |
| Process env precedence (SRS Q2) | Highest by default; config can override. |
| Compose service grouping (SRS Q3) | Each compose service is a sub-source (`docker-compose.yml[api]`). Differences between services in the same file are info-severity; conflicts across files/sources are warning-severity. |
| Monorepo workspaces (SRS Q4) | Not first-class in v0.1. Recursive discovery with project-relative paths covers nested projects. |
| JSON export values (SRS Q5) | Masked values included by default; `--no-values` omits values entirely. |
| Package script parsing (SRS Q7) | Included. Regex-based (leading `KEY=val` assignments, `cross-env` args), not a shell parser. Each script is a sub-source (`package.json[dev]`). |
| License | Dual MIT OR Apache-2.0. |
| Publishing | Public GitHub repo `EricGrill/envlens`, GitHub Actions CI + tagged release binaries; awesome-tuis PR text prepared for Eric's review before submission; crates.io publish deferred. |
| Tracking | Linear project "EnvLens" on Chainbytes team (CHA-1871…CHA-1879). |

## 3. Architecture

Single cargo package `envlens`. Strict layering: `core` has no knowledge of `tui`, `report`, or `cli`. All core code is synchronous and deterministic; the only I/O in core is reading files during scan/parse and reading `std::env::vars()`.

```
src/
  main.rs            entry: panic hook, dispatch (TUI vs subcommand)
  cli.rs             clap derive definitions
  config.rs          .envlens.yml discovery, parsing, merge
  core/
    mod.rs           pub fn analyze(root, &Config, profile, tracked_files) -> Analysis
    scanner.rs       file discovery + classification
    model.rs         data types (§4)
    parsers/
      mod.rs         Parser dispatch by SourceKind
      dotenv.rs      hand-rolled dotenv parser
      compose.rs     docker-compose environment extraction
      package_json.rs  script inline-env extraction
      process.rs     process environment snapshot
    resolve.rs       precedence, profiles, effective values, ${VAR} expansion
    diagnostics.rs   diagnostic rules (§8 of SRS)
    secrets.rs       secret classification + masking
  report/
    json.rs          machine-readable output for `check --json` / `report --format json`
    markdown.rs      sanitized markdown report
  tui/
    app.rs           App state, Event, update(), run loop
    theme.rs         styles, color degradation, ASCII fallback icons
    views/
      sources.rs     left pane
      variables.rs   right pane
      details.rs     bottom pane
      help.rs        help overlay
      modals.rs      search input, filter menu, sort menu, confirm dialogs, export prompt
```

### Pipeline

`core::analyze()` runs: scan → parse each source → collect occurrences → apply profile + source filter → order sources by precedence → compute effective values → resolve `${VAR}` references → run diagnostics → return `Analysis`.

`Analysis` contains: sources (with parse errors), variable summaries (sorted by key), diagnostics, and the resolved profile/precedence used. The TUI, `check`, and `report` all consume `Analysis`; nothing downstream re-derives semantic facts. Re-running `analyze` on identical inputs yields identical output (NFR-019); all maps iterate in sorted or insertion order (`BTreeMap`/`Vec`, never `HashMap` iteration into output). `Analysis` itself contains no timestamps; report timestamps are added at the frontend boundary from a clock that honors `SOURCE_DATE_EPOCH` (reproducible-builds convention), which is how the byte-identical determinism test in §14 passes while normal runs still show a real timestamp.

Core performs no process spawning and no I/O beyond reading discovered files and `std::env::vars_os()`. Facts that require external processes are passed *into* `analyze` as data: the frontend runs `git ls-files -z` whenever a `.git` entry of any type exists at the root (worktrees and submodules use a `.git` *file*) and supplies `tracked_files: Option<BTreeSet<PathBuf>>`; `None` (git absent or command failed for any reason) simply disables the `SecretInTrackedFile` rule.

### Dependencies

Runtime: `ratatui`, `crossterm`, `clap` (derive), `serde`, `serde_yaml`, `serde_json`, `ignore`, `regex`, `anyhow`, `thiserror`, `unicode-width`. Dev: `insta`, `assert_cmd`, `predicates`, `tempfile`. No network crates anywhere in the tree (asserted in CI; NFR-006).

The dotenv parser is hand-rolled: existing crates do not expose line numbers, raw values, inline-comment semantics, or malformed-line reporting required by FR-011/013/014.

## 4. Data model

Mirrors SRS §7, adapted to Rust:

```rust
enum SourceKind { Dotenv, DotenvExample, Compose, PackageScript, Process, Ci }

struct EnvSource {
    id: SourceId,             // stable string, e.g. ".env", "docker-compose.yml[api]", "process"
    kind: SourceKind,
    path: Option<PathBuf>,    // project-relative; None for process
    context: Option<String>,  // compose service name or script name
    precedence: u32,          // higher wins
    enabled: bool,            // toggled by --source / TUI sources pane
    errors: Vec<ParseError>,
}

struct VariableOccurrence {
    key: String,
    raw_value: Option<String>,     // exactly as written; None for bare compose list keys
    parsed_value: Option<String>,  // after quote/escape/reference handling
    source_id: SourceId,
    line: Option<u32>,
    is_empty: bool,                // defined as empty string
    is_inherited: bool,            // compose bare key inheriting from process
    secret: SecretClass,           // None | KeyLike | ValueLike | Both
}

struct VariableSummary {
    key: String,
    effective: Option<(String /*value*/, SourceId)>,
    occurrences: Vec<VariableOccurrence>,   // ordered by precedence asc
    diagnostics: Vec<Diagnostic>,
    is_required: bool,
    is_missing: bool,
    is_secret_like: bool,
}

struct Diagnostic {
    severity: Severity,        // Info | Warning | Error
    code: DiagnosticCode,      // enum of the 10 SRS §8 codes
    message: String,           // actionable, includes sources/lines (NFR-012)
    key: Option<String>,
    source_id: Option<SourceId>,
    line: Option<u32>,
}
```

`DiagnosticCode` is a closed enum matching SRS §8 exactly: `DuplicateKey`, `ConflictingValues`, `MissingRequired`, `EmptyRequired`, `UndefinedReference`, `CircularReference`, `InvalidDotenvLine`, `SecretInTrackedFile`, `InheritedUnresolved`, `ShadowedValue`. (`SecretInTrackedFile` fires only when the frontend supplied `tracked_files` per §3; if git is absent the code is never emitted. This keeps SRS §8 complete without importing §6.6 git-awareness scope.)

Deliberate adaptations of the SRS §7 TypeScript model: FR-019's per-occurrence "warning list" is realized as `Diagnostic`s carrying `source_id` + `line` (the details pane joins them back to occurrences); the SRS `parsedAt` field is dropped because it would violate NFR-019 determinism; the SRS `"config"` source type is unused in v0.1 (nothing parses env vars out of `.envlens.yml` itself). The FR-028 empty/undefined/inherited/unresolved distinction is derived from `is_empty` + `is_inherited` on the occurrence plus joined `UndefinedReference`/`InheritedUnresolved` diagnostics — there is no separate per-occurrence status enum.

## 5. Scanning and discovery

- Default root: CWD; positional arg overrides (FR-001/002).
- Walk with the `ignore` crate, max depth 8, following the default ignore list from FR-003 (`.git`, `node_modules`, `vendor`, `.venv`, `venv`, `dist`, `build`, `.next`, `coverage`) plus `--ignore` CLI patterns and config `ignore:` patterns (FR-004). Hidden files are still visited (dotenv files are hidden). Symlinked directories are not followed.
- Classification (FR-005–FR-009):
  - Dotenv: exact names `.env`, `.env.local`, `.env.development`, `.env.development.local`, `.env.test`, `.env.test.local`, `.env.production`, `.env.production.local`; example templates `.env.example`, `.env.sample`, `.env.template` become `SourceKind::DotenvExample` (they define required keys, never values).
  - Compose: `docker-compose.yml|yaml`, `compose.yml|yaml`, `docker-compose.override.yml|yaml`.
  - Package manifests: `package.json` parsed for scripts; `pnpm-workspace.yaml`, `turbo.json`, `nx.json` are discovered and listed as sources but contribute no variables in v0.1 (shown with a note in the details pane).
  - CI configs: `.github/workflows/*.yml|yaml`, `.gitlab-ci.yml`, `circle.yml`, `.circleci/config.yml` — discovered and listed; top-level and job-level `env:` maps are parsed for GitHub Actions and GitLab (`variables:`) since that is cheap with serde_yaml; deeper constructs (matrix, secrets contexts) are out of scope and noted as a documented limitation.
  - Process environment: always present as source `process` (FR-009).
- Nested files (monorepos) are discovered and identified by project-relative path (`apps/web/.env`).

### Same-rank ordering (deterministic tie-breaks)

- Dotenv files of the same rank (e.g. two `.env.local` in different subdirectories): shallower path first, then lexicographic path order; the later one wins.
- Compose: base files (`docker-compose.yml|yaml`, `compose.yml|yaml`) share one rank; multiple base files (side-by-side variants or different directories) order by the dotenv path rule. `docker-compose.override.yml|yaml` binds to the base file **in its own directory** and ranks immediately above it. Within one file, services are sub-sources in document order.
- Package scripts: sub-sources `package.json[<script>]` ordered by document order of the scripts map; multiple `package.json` files order by the dotenv path rule.
- Where sub-sources of the same file define the same key with different values (two compose services, two scripts), the *last* sub-source in the above order supplies the source's candidate for effective-value resolution, and the difference is reported at info severity (§2 decisions).

## 6. Parsing rules

### Dotenv (`parsers/dotenv.rs`)

Line grammar (FR-010–FR-014):
- Optional leading whitespace, optional `export `, then `KEY` matching `[A-Za-z_][A-Za-z0-9_.]*`, `=`, then value.
- Values: unquoted (trimmed, inline `#` comment starts a comment only when preceded by whitespace), double-quoted (supports `\n`, `\t`, `\"`, `\\`, `\$` escapes), single-quoted (literal, no escapes or expansion).
- Full-line comments (`#`) and blank lines ignored.
- Malformed lines (`KEY` with no `=` and not a comment, missing key before `=`, space in key like `DATABASE URL=x`) produce a `ParseError` on the source plus an `InvalidDotenvLine` diagnostic with line number; parsing continues (NFR-017/018).
- Raw value (`raw_value`) preserves the original text between `=` and end-of-line; `parsed_value` is post-quote/escape processing, pre-reference-expansion.
- Multi-line quoted values are supported for double quotes (continue until closing quote); a never-closed quote is a `ParseError` anchored at the opening line.

### Compose (`parsers/compose.rs`)

Via `serde_yaml::Value` walking (schema-tolerant): for each `services.<name>`, read `environment` as map (FR-015) or list (FR-016). List entries `KEY=value` parse like dotenv unquoted; bare `KEY` becomes an occurrence with `raw_value: None`, `is_inherited: true` — resolved against the process source at diagnostic time (`InheritedUnresolved` info if the process lacks it). `env_file:` entries are noted in the source details but not chained in v0.1 (the referenced files are usually discovered independently by the scanner anyway). Line numbers come from a line scan: after structural parsing, the raw file is scanned for the literal `KEY:` / `- KEY=` within the service block to supply best-effort line numbers. (This is the primary mechanism, not a fallback — `serde_yaml` is archived and exposes no spans on `Value`; we accept it for structure because it is stable and ubiquitous, and keep line attribution independent of it.)

### Package scripts (`parsers/package_json.rs`)

For each `scripts.<name>` string (FR-017/018): tokenize on whitespace; consume leading `KEY=value` tokens as assignments; if a token is `cross-env`, consume subsequent `KEY=value` tokens; stop at the first non-assignment command token. `set KEY=value &&` (Windows cmd) is recognized and parsed too — SRS §5.2 lists Windows-specific syntax as excluded while FR-018 calls it optional; we include it deliberately because it is a trivial extension of the same tokenizer, not a scope accident. Each script with at least one assignment becomes sub-source `package.json[<script>]`. Line numbers: located by scanning the raw file for the script key (best-effort).

### Process (`parsers/process.rs`)

Snapshot of `std::env::vars_os()`, lossy-decoded, sorted by key. No line numbers. Values participate in masking like any other source.

## 7. Precedence, profiles, references

- Default precedence, lowest → highest, per FR-021: `.env` < `.env.local` < `.env.development` < `.env.development.local` < `.env.test` < `.env.test.local` < `.env.production` < `.env.production.local` < compose < package scripts < process. Example/template sources have no precedence (they define requirements, not values). CI sources also carry no precedence in v0.1 — they are informational occurrences flagged in details, never effective values.
- Profiles (FR-023, FR-055): a profile is an ordered include-list of source ids/patterns. The list order **is** the precedence order for the included sources (lowest → highest, same convention as `precedence:` and FR-021); if the config also sets `precedence:`, that key wins for the sources it names. Built-ins: `dev` (`.env`, `.env.local`, `.env.development`, `.env.development.local`, compose, scripts, process), `test` (`.env`, `.env.test`, `.env.test.local`, process), `production` (`.env`, `.env.production`, `.env.production.local`, compose, process). Config `profiles:` overrides/extends. `--profile X` selects; default is "all sources, default precedence".
- Custom precedence (FR-022, FR-054): config `precedence:` is an ordered list of source names; listed sources rank in that order above unlisted ones, which keep default relative order below.
- Effective value (FR-021): highest-precedence enabled occurrence with a defined value; `is_inherited` occurrences resolve through the process value if present.
- References (FR-029–031): `${VAR}` and `$VAR` (word boundary) in parsed values, except inside single quotes. Expansion resolves against effective values of the active overlay. Undefined → `UndefinedReference` warning and the reference is left verbatim in the expanded value. Cycles detected by DFS over the reference graph → `CircularReference` error on each participating key; cyclic references are not expanded. `\$` escapes expansion in double quotes.

## 8. Diagnostics

All rules run over the `Analysis` in one pass, emitting SRS §8 codes with actionable messages in the NFR-012 style, e.g. `PORT differs across sources: .env:3 (3000), docker-compose.yml[api]:12 (5001). Effective value is 5001 from docker-compose.yml[api].`

Diagnostic messages embed values, so **message composition renders every value through the masking function** — for a secret-like key the message reads `JWT_SECRET differs across sources: .env:2 (••••••••3a), .env.local:5 (••••••••8F)`. This closes the path around the `MaskedValue` report boundary (NFR-005); the §14 secrets fixture includes a *conflicting* secret-like key specifically to regression-test it.

| Rule | Notes |
|---|---|
| `DuplicateKey` (warning) | Same key twice in one source (any parsed source, including example and CI files); last occurrence wins within the source; both are recorded. |
| `ConflictingValues` (warning) | ≥2 distinct defined values across different enabled *value-bearing* sources (dotenv, compose, scripts, process). Example and CI sources never participate — examples are requirements, CI occurrences are informational context shown in the details pane only. Cross-service/cross-script same-file differences downgrade to info (§2 decisions, §5 same-rank ordering). |
| `ShadowedValue` (info) | Any value-bearing occurrence overridden by a higher-precedence one with a different or equal value. Example/CI occurrences neither shadow nor are shadowed. |
| `MissingRequired` (error) | Required key (from example files + config `required:`) with no defined occurrence in enabled *value-bearing* sources (dotenv, compose, scripts, process — same scoping as `ConflictingValues`; a key defined only in a CI file is still missing). |
| `EmptyRequired` (warning) | Required key defined in value-bearing sources but empty in all of them. |
| `UndefinedReference` (warning) | See §7. |
| `CircularReference` (error) | See §7. |
| `InvalidDotenvLine` (warning) | From parser errors. |
| `InheritedUnresolved` (info) | Compose bare key with no process value. |
| `SecretInTrackedFile` (warning) | Secret-like occurrence in a git-tracked file (only when `.git` exists and `git ls-files` succeeds; silent otherwise). |

Empty-vs-undefined-vs-inherited-vs-unresolved distinctions (FR-028) are carried on the occurrence and rendered distinctly in the details pane and reports.

## 9. Secrets

- Key classification (FR-032): key split into segments on `_`, `.`, `-`, and lower→upper case boundaries; a key is secret-like if any segment case-insensitively equals one of: `secret`, `token`, `password`, `pass`, `passwd`, `pwd`, `private`, `key`, `credential`, `credentials`, `auth`, `session`, `cookie`, `apikey`. Segment matching means `PUBLIC_KEY` matches (`KEY` segment) but `KEYBOARD_LAYOUT` does not. Config `secret_patterns:` adds user regexes (FR-052).
- Value classification (FR-033): JWT shape (`eyJ` + two dot-separated base64url parts), PEM headers (`-----BEGIN … PRIVATE KEY-----`), known credential prefixes (`sk_live_`, `sk_test_`, `pk_live_`, `AKIA`, `ghp_`, `gho_`, `github_pat_`, `xoxb-`, `xoxp-`, `glpat-`, `AIza`), URLs with `user:pass@`, and strings ≥ 20 chars with Shannon entropy > 3.5 bits/char and no whitespace.
- Masking (FR-034): values ≥ 8 chars render as up-to-3-char recognizable prefix (only if the value matches a known-prefix pattern) + `•` run capped at 10 + last 2 chars, e.g. `sk_••••••••••8F`. Values < 8 chars render as `••••••••` (fixed width, length-hiding). ASCII mode uses `*`.
- Reveal (FR-035, NFR-008): TUI-only state; `r` toggles selected, `R` reveals all after a y/N confirm modal, `Esc` re-masks (per the transient-state ordering in §12). Reveal state never persists and never reaches report writers. There is deliberately **no** CLI flag to emit unmasked values — exports are always sanitized (FR-036), and anyone needing raw values already has the source files.
- Sanitized output (FR-036, NFR-004/005): `report/` and all log/panic output receive only pre-masked strings. A `MaskedValue` newtype whose `Display` is the masked form is used at the report boundary so accidental leakage is a type error.

## 10. Configuration

Discovery order (FR-051), first found wins for project config: `.envlens.yml`, `.envlens.yaml`, `.config/envlens.yml`; plus user config at `~/.config/envlens/config.yml` (XDG) merged underneath (project keys override user keys; lists replace, not concatenate). `--config PATH` bypasses discovery.

```yaml
ignore: [tmp, generated]          # FR-004
required: [DATABASE_URL, NODE_ENV] # FR-053 (adds to example-file inference)
required_from_examples: true       # default true (SRS Q1 decision)
secret_patterns: ["SUPABASE_.*"]   # FR-052
precedence: [.env, .env.local, process] # FR-054
fail_on: error                     # error | warning (Q6 decision; --strict == warning)
profiles:                          # FR-055
  dev:  { include: [.env, .env.local, process] }
  test: { include: [.env, .env.test, process] }
```

Unknown keys produce a warning (not an error). A malformed config file falls back to defaults with a visible warning (NFR-010: zero-config always works).

## 11. CLI

```
envlens [PATH] [--profile P] [--source S]... [--ignore G]... [--config F] [--no-color] [--ascii]
envlens check  [PATH] [--json] [--strict] [--no-values] [common flags]
envlens report [PATH] --format markdown|json [--out FILE] [--no-values] [common flags]
```

Flag interactions: `--source` intersects with the active profile's include list (a profile selects the candidate set; `--source` narrows it further). Naming a source outside the profile, or one that does not exist in the project, is a usage error → exit 2, even though this validation runs after clap (exit 2 means "the invocation is wrong", not "clap said so"). `--strict` overrides config `fail_on:` when both are present. `--no-values` applies to both `check --json` and `report` (both formats). An empty project (no sources discovered beyond `process`) is not an error: the TUI opens with the sources pane showing `process` and a "no project env sources found" notice; `check` reports zero findings and exits 0 unless config `required:` entries are missing (then normal diagnostics apply).

- Bare `envlens` opens the TUI (FR-044). `check` prints human-readable diagnostics (or `--json`) and exits per threshold (FR-045). `report` writes sanitized markdown per SRS §11 or JSON.
- Exit codes (FR-046): 0 no findings ≥ threshold; 1 findings ≥ threshold (threshold = error, or warning with `--strict`/`fail_on: warning`); 2 CLI usage error (clap); 3 environment failure — target path missing/unreadable, i.e. no analysis possible (per-file parse errors are diagnostics, not exit 3); 4 internal error via panic hook (message passes through masking, exits 4).
- `check --json` schema: `{ version, generated_at, root, profile, summary: {sources, variables, errors, warnings, infos, secrets, missing_required}, sources: [...], variables: [...], diagnostics: [...] }` — stable field order, documented in README. `--no-values` removes `value`-bearing fields entirely (Q5 decision).
- stdout is data; all human chrome (progress, warnings about config) goes to stderr. `NO_COLOR` env and `--no-color` disable ANSI.

## 12. TUI

Elm-style: `App` state struct; `Event` enum (key, resize, tick); `update(&mut App, Event)`; `draw(&App, Frame)`. Crossterm raw mode + alternate screen; terminal restored on panic via hook + `Drop` guard.

- Layout per FR-037: sources pane (left, ~28 cols), variables pane (right), details pane (bottom, ~40% height). Focus cycles with `Tab` (FR-038); focused pane has highlighted border.
- Variables pane rows: status icon (`✓` ok, `⚠` warning, `✗` error, `🔒` secret; ASCII `+ ! x #`), key, effective value (masked as needed), effective source. Sorted per active sort (key default; severity, source count, effective source, secret status — FR-041 via `s` menu).
- Sources pane: each source with occurrence count and parse-error badge; `Space` toggles enabled → re-resolve (not re-parse). Selecting a source filters the variables pane to it (FR-040 "by source").
- Details pane for the selected key: effective value + winner, all occurrences (source, line, value — masked/revealed, empty/inherited/unresolved annotations), diagnostics with messages.
- Keys (FR-038): `q` quit, `?` help overlay, `↑/↓`/`j/k` move, `Tab` pane switch, `/` incremental substring search on keys, `f` filter cycle (all → warnings → missing → conflicts → secrets), `s` sort menu, `r` reveal selected, `R` reveal all (confirm), `e` export sanitized markdown (prompts for path, default `envlens-report.md`), `o` open effective source in `$EDITOR +line file` (suspend/restore terminal; disabled with a status message if `$EDITOR` unset), `Enter` expand/collapse occurrence detail, `Ctrl+r` re-scan (NFR-003).
- `Esc` dismisses transient state top-down, one layer per press: open modal/menu first, then active search, then reveal state (re-mask all), then nothing. This resolves the FR-035/FR-039 overlap deterministically.
- Profile filtering (FR-040) is satisfied by launch-time selection (`--profile`); in-TUI profile switching is deliberately out of scope for v0.1 (it would invalidate most view state for a rarely-toggled setting). Filtering by source is in-TUI via the sources pane.
- Help overlay lists all keys + active profile/config path (FR-042).
- Color degradation (NFR-016): styles defined in `theme.rs` with a low-color variant selected when the terminal reports < 256 colors or `NO_COLOR`; icons swap to ASCII with `--ascii` or when the locale is not UTF-8.

## 13. Error handling

- Core is `Result`-based (`thiserror` error enums); per-file failures degrade to `ParseError` + diagnostics, never abort the pipeline (NFR-017/018).
- Frontends use `anyhow` with context. `unwrap`/`expect` denied by clippy lint config outside tests.
- Panic hook: restores terminal, prints a masked one-line error + issue-reporting pointer, exits 4 (NFR-005 — panic output cannot contain raw values because payloads are our own messages, and the hook additionally truncates).

## 14. Testing

- **Unit (in-module):** dotenv parser table tests covering every FR-010–014 form plus pathological inputs (unterminated quotes, CRLF, BOM, unicode keys, 10k-char lines); compose map/list/bare-key; package-script tokenizer incl. `cross-env` and `set … &&`; precedence/profile resolution; reference expansion + cycles; segment-based secret matching (incl. `KEYBOARD_LAYOUT` negative case); masking widths; config merge.
- **Fixtures:** `tests/fixtures/{basic,conflicts,secrets,compose,scripts,invalid,monorepo,ci}` — small real project trees, committed. `fixtures/basic` satisfies the SRS §15 milestone assertions.
- **Integration (`tests/cli.rs`, assert_cmd):** exit codes 0/1/2/3 paths, `--strict`, `check --json` parses and matches a documented schema (serde round-trip + insta snapshot), markdown report golden file, determinism (two runs with `SOURCE_DATE_EPOCH` pinned → byte-identical stdout), `--no-values` truly value-free, planted fake secrets (e.g. `envlensFakeHistoricalSecret…`) never appear unmasked in any export mode, empty-directory behavior (exit 0, `process`-only source list).
- **TUI snapshots (`tests/tui.rs`, insta + `TestBackend`):** initial render on `fixtures/basic`, search active, each filter mode, details with masked vs revealed secret, help overlay, ASCII/no-color mode, narrow-terminal (80×24) render.
- **Supply-chain/NFR checks in CI:** `cargo tree` asserted to contain no network stack (`reqwest|hyper|curl|ureq`); `cargo deny` advisories optional-but-included if setup friction is low, otherwise `cargo audit` in CI.
- Coverage goal: parsers/resolve/diagnostics/secrets ≥ 90% line coverage (measured with `cargo llvm-cov` in CI, informational not gating).

## 15. Performance & portability

- Startup budget (NFR-001): scan+parse for <500-file repos well under 500 ms — the `ignore` walker prunes FR-003 dirs before descent; parsing is linear over small files. 50k-file repos (NFR-002) stay responsive because classification happens on file names during the walk, and only classified sources are opened/read.
- Platforms (NFR-014): macOS + Linux tier 1 (CI-tested); Windows compiles (crossterm supports it) but is untested/best-effort in v0.1.
- Terminals (NFR-015): crossterm covers the listed emulators; degradation per §12.

## 16. CI/CD, release, submission

- `.github/workflows/ci.yml`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` on `ubuntu-latest` + `macos-latest`, stable toolchain; network-crate tree assertion.
- `.github/workflows/release.yml`: on `v*` tags — build `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-gnu`; strip, tarball + `sha256sums.txt`, attach to GitHub Release.
- Repo content: `README.md` (features, screenshots/GIF, install from release binaries or `cargo install --git`, usage, config reference, keybindings table), `LICENSE-MIT`, `LICENSE-APACHE`, `CHANGELOG.md` (keep-a-changelog), `.gitignore`, `CONTRIBUTING.md` (brief), fixtures, `docs/`.
- Demo media: VHS tape (`docs/demo.tape`) rendering a GIF against `tests/fixtures/basic` if `vhs` is available locally; otherwise a static screenshot captured from the TUI.
- awesome-tuis: prepared PR (branch on a fork of `rothgar/awesome-tuis`) adding under **Dashboards**, alphabetical position: `- [envlens](https://github.com/EricGrill/envlens) - Inspect, compare, and debug environment variables across .env files, Docker Compose, package scripts, and your shell`. Eric reviews before submission.

## 17. Out of scope (v0.1)

Everything in SRS §5.2 and §6: secret-manager integrations, deep CI parsing (beyond flat `env:`/`variables:` maps), full shell parsing, file watching, editing files from the TUI, telemetry, network calls, AI fixes, first-class monorepo workspaces, crates.io publish (deferred, name reserved by later publish), Homebrew tap.

## 18. Implementation phasing

The implementation plan should phase the work rather than treat it flat: (1) scaffold + core pipeline (scanner, parsers, model, resolve, diagnostics, secrets) with unit tests; (2) CLI frontends (`check`, `report`) with integration tests — this makes the core exercisable end-to-end before any TUI code exists; (3) TUI with snapshot tests; (4) config file + profiles wiring across all frontends; (5) CI/CD, release, README/demo, awesome-tuis PR prep. Each phase leaves the repo green (`fmt`, `clippy`, `test`).

## 19. Acceptance criteria mapping

SRS §13 items 1–14 map to: TUI (§12), parsers (§6), compose (§6), source/line display (§12 details pane), effective values (§7), conflicts (§8), missing-required (§8), masking (§9), search/filter (§12), `check --json` (§11), markdown report (§11), test suite (§14), no-network (§3 deps + CI assertion), no unmasked secrets on disk (§9 `MaskedValue` boundary + §14 security tests).
