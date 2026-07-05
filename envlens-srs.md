# Software Requirements Specification: EnvLens

## 1. Introduction

### 1.1 Purpose

**EnvLens** is a terminal user interface for inspecting, comparing, and debugging environment variable configuration across local development projects.

It helps developers answer questions like:

- Which `.env` file set this value?
- Which value wins after overrides?
- Is a required variable missing?
- Are secrets accidentally exposed?
- Do `.env.local`, `.env.test`, Docker Compose, shell environment, and package scripts disagree?
- Why does the app behave differently locally, in test, or in CI?

### 1.2 Product Scope

EnvLens is a cross-platform CLI/TUI application that scans a project directory, discovers environment variable sources, parses them, and presents a navigable interface showing merged environment state, conflicts, missing variables, duplicate declarations, and potential secret leaks.

The initial version focuses on local project inspection without requiring cloud accounts, databases, or background services.

### 1.3 Target Users

Primary users:

- Full-stack developers
- DevOps engineers
- Open-source maintainers
- AI coding agents working inside repositories
- Developers debugging local-vs-CI or local-vs-production configuration issues

Secondary users:

- Security reviewers
- Technical support engineers
- Developer experience teams

### 1.4 Definitions

| Term | Meaning |
|---|---|
| Environment source | A file or process that defines environment variables |
| Variable | A key-value pair such as `DATABASE_URL=...` |
| Effective value | The value that wins after applying precedence rules |
| Shadowed value | A value overridden by another source |
| Secret-like value | A variable likely containing sensitive material |
| Overlay | A merged view of multiple environment sources |
| Profile | A named inspection mode, e.g. `dev`, `test`, `production` |

---

## 2. Overall Description

### 2.1 Product Perspective

EnvLens is a standalone developer tool. It should work inside any project directory and require minimal configuration.

It is not intended to replace:

- Secret managers
- CI configuration systems
- Full security scanners
- Docker Compose
- dotenv libraries

Instead, it acts as a human-friendly inspection layer over existing configuration sources.

### 2.2 Product Functions

The MVP shall:

1. Discover environment-related files in a project.
2. Parse `.env`-style files.
3. Parse selected environment declarations from `docker-compose.yml`.
4. Parse selected environment declarations from `package.json` scripts where feasible.
5. Read current process environment variables.
6. Display all variables in a TUI.
7. Show source file and line number for each variable.
8. Show effective values according to a configurable precedence order.
9. Detect duplicates, conflicts, missing values, undefined references, and secret-like variables.
10. Mask secret-like values by default.
11. Export a sanitized report.

### 2.3 User Classes

#### Local Developer

Wants to quickly understand why a service fails locally.

Needs:

- Fast scan
- Clear missing-variable warnings
- Safe secret masking
- Source tracing

#### DevOps Engineer

Wants to compare environments and prevent drift.

Needs:

- Environment matrix
- Conflict detection
- Docker Compose support
- CI-friendly output

#### Open Source Maintainer

Wants contributors to diagnose setup problems without leaking secrets.

Needs:

- Sanitized reports
- Simple install
- Minimal configuration

#### Security Reviewer

Wants to detect risky secret exposure patterns.

Needs:

- Secret-like variable detection
- Plaintext exposure warnings
- Exportable findings

---

## 3. Functional Requirements

## 3.1 Project Scanning

### FR-001: Scan Current Directory

EnvLens shall scan the current working directory by default.

Example:

```bash
envlens
```

### FR-002: Scan Specified Directory

EnvLens shall accept a target directory.

```bash
envlens /path/to/project
```

### FR-003: Ignore Heavy Directories

EnvLens shall ignore common large or irrelevant directories by default:

- `.git`
- `node_modules`
- `vendor`
- `.venv`
- `venv`
- `dist`
- `build`
- `.next`
- `coverage`

### FR-004: Configurable Ignore Patterns

EnvLens should support ignore patterns from a config file or CLI option.

Example:

```bash
envlens --ignore tmp --ignore generated
```

---

## 3.2 Source Discovery

### FR-005: Discover Dotenv Files

EnvLens shall detect common dotenv files:

- `.env`
- `.env.local`
- `.env.development`
- `.env.development.local`
- `.env.test`
- `.env.test.local`
- `.env.production`
- `.env.production.local`
- `.env.example`
- `.env.sample`
- `.env.template`

### FR-006: Discover Docker Compose Files

EnvLens shall detect:

- `docker-compose.yml`
- `docker-compose.yaml`
- `compose.yml`
- `compose.yaml`
- `docker-compose.override.yml`
- `docker-compose.override.yaml`

### FR-007: Discover Package Manifests

EnvLens shall detect:

- `package.json`
- `pnpm-workspace.yaml`
- `turbo.json`
- `nx.json`

MVP only requires partial support for `package.json`.

### FR-008: Discover CI Configs

EnvLens should detect CI config files:

- `.github/workflows/*.yml`
- `.github/workflows/*.yaml`
- `.gitlab-ci.yml`
- `circle.yml`
- `.circleci/config.yml`

CI parsing may be limited in MVP.

### FR-009: Read Current Process Environment

EnvLens shall include the current process environment as a source named `process`.

---

## 3.3 Parsing

### FR-010: Parse Basic Dotenv Syntax

EnvLens shall parse dotenv lines in the form:

```env
KEY=value
KEY="value"
KEY='value'
export KEY=value
```

### FR-011: Preserve Source Location

Each parsed variable shall include:

- source name
- absolute or project-relative file path
- line number
- raw value
- parsed value

### FR-012: Support Comments

EnvLens shall ignore full-line comments:

```env
# comment
```

### FR-013: Support Inline Comments

EnvLens should support inline comments when not inside quotes:

```env
PORT=3000 # local server
```

### FR-014: Detect Invalid Lines

EnvLens shall detect malformed dotenv lines and show warnings.

Example invalid lines:

```env
DATABASE URL=abc
=missing_key
KEY
```

### FR-015: Parse Docker Compose Environment Maps

EnvLens shall parse Compose environment declarations like:

```yaml
services:
  api:
    environment:
      NODE_ENV: development
      PORT: "5001"
```

### FR-016: Parse Docker Compose Environment Lists

EnvLens shall parse Compose environment declarations like:

```yaml
services:
  api:
    environment:
      - NODE_ENV=development
      - PORT=5001
      - DATABASE_URL
```

A key without value shall be marked as inherited or unresolved.

### FR-017: Parse Package Scripts for Inline Env

EnvLens should parse simple inline env assignments from package scripts.

Example:

```json
{
  "scripts": {
    "dev": "NODE_ENV=development PORT=3000 next dev"
  }
}
```

### FR-018: Cross-platform Script Parsing

EnvLens should recognize common patterns:

```bash
NODE_ENV=test jest
cross-env NODE_ENV=test jest
```

Windows `set KEY=value && command` support is optional for MVP.

---

## 3.4 Variable Model

### FR-019: Represent Variable Occurrences

Each occurrence shall contain:

- key
- value
- source type
- source path
- line number
- service/script context if applicable
- parsed/unparsed status
- secret classification
- warning list

### FR-020: Group Occurrences by Key

The TUI shall group all occurrences of the same variable key.

### FR-021: Show Effective Value

EnvLens shall calculate an effective value according to precedence rules.

Default precedence, lowest to highest:

1. `.env`
2. `.env.local`
3. `.env.development`
4. `.env.development.local`
5. `.env.test`
6. `.env.test.local`
7. `.env.production`
8. `.env.production.local`
9. Docker Compose environment
10. package script inline environment
11. process environment

### FR-022: Custom Precedence

EnvLens should allow custom precedence through config.

Example:

```yaml
precedence:
  - .env
  - .env.local
  - process
```

### FR-023: Environment Profile

EnvLens shall support selecting a target profile.

Example:

```bash
envlens --profile test
```

For `test`, the default overlay may prioritize:

1. `.env`
2. `.env.test`
3. `.env.test.local`
4. process

---

## 3.5 Diagnostics

### FR-024: Duplicate Key Detection

EnvLens shall flag keys declared multiple times in the same source.

### FR-025: Conflict Detection

EnvLens shall flag keys with multiple different values across sources.

Example:

```text
PORT=.env:3000, docker-compose.yml:5001
```

### FR-026: Shadowed Value Detection

EnvLens shall show when one value overrides another.

### FR-027: Missing Required Variable Detection

EnvLens shall infer required variables from example/template files.

If `.env.example` contains:

```env
DATABASE_URL=
JWT_SECRET=
```

and active sources do not define them, EnvLens shall mark them missing.

### FR-028: Empty Value Detection

EnvLens shall distinguish between:

- undefined
- defined but empty
- inherited from process
- unresolved reference

### FR-029: Variable Reference Detection

EnvLens shall detect references like:

```env
API_URL=${HOST}:${PORT}
```

### FR-030: Undefined Reference Warning

EnvLens shall warn when a variable references another variable that is not defined in the active overlay.

### FR-031: Circular Reference Warning

EnvLens should detect simple circular references.

Example:

```env
A=${B}
B=${A}
```

### FR-032: Secret-like Key Detection

EnvLens shall classify keys as secret-like if they contain case-insensitive terms such as:

- `SECRET`
- `TOKEN`
- `PASSWORD`
- `PASS`
- `PRIVATE`
- `KEY`
- `CREDENTIAL`
- `AUTH`
- `SESSION`
- `COOKIE`

### FR-033: Secret-like Value Detection

EnvLens should classify values as secret-like if they resemble:

- JWTs
- private keys
- long random tokens
- API keys
- connection strings with embedded passwords

### FR-034: Secret Masking

Secret-like values shall be masked by default.

Example:

```text
JWT_SECRET = sk_••••••••••12
```

### FR-035: Explicit Reveal

Users shall be able to reveal a masked value only via explicit action.

Examples:

- Press `r` to reveal selected value.
- Press `R` to reveal all values after confirmation.

### FR-036: Sanitized Export

EnvLens shall export reports with secrets masked by default.

---

## 3.6 TUI Requirements

### FR-037: Main Layout

The main TUI shall have at least three panes:

1. Sources pane
2. Variables pane
3. Details/diagnostics pane

Suggested layout:

```text
┌ Sources ───────────────┐ ┌ Variables ──────────────────────────────┐
│ process                │ │ ⚠ DATABASE_URL      conflict             │
│ .env                   │ │ ✓ NODE_ENV          development          │
│ .env.local             │ │ ⚠ JWT_SECRET        missing/example      │
│ docker-compose.yml     │ │ 🔒 STRIPE_API_KEY   masked              │
└────────────────────────┘ └─────────────────────────────────────────┘
┌ Details ────────────────────────────────────────────────────────────┐
│ DATABASE_URL                                                        │
│ effective: postgres://user:••••@localhost:5432/app                  │
│                                                                     │
│ Sources:                                                            │
│ .env:4                postgres://user:••••@localhost:5432/app       │
│ .env.local:2          postgres://user:••••@localdb:5432/app         │
│ docker-compose.yml:9  postgres://user:••••@db:5432/app             │
│                                                                     │
│ Warnings: value differs across sources                              │
└─────────────────────────────────────────────────────────────────────┘
```

### FR-038: Keyboard Navigation

The TUI shall support keyboard navigation.

Minimum keys:

| Key | Action |
|---|---|
| `q` | Quit |
| `?` | Help |
| `↑/↓` or `j/k` | Move selection |
| `Tab` | Switch pane |
| `/` | Search variables |
| `f` | Filter diagnostics |
| `r` | Reveal selected secret |
| `e` | Export sanitized report |
| `o` | Open source file if editor integration is available |
| `Enter` | Expand/collapse details |

### FR-039: Search

Users shall be able to search variables by key.

### FR-040: Filtering

Users shall be able to filter by:

- all variables
- warnings only
- missing only
- conflicts only
- secrets only
- source
- profile

### FR-041: Sort Options

Users should be able to sort by:

- key
- severity
- source count
- effective source
- secret classification

### FR-042: Help View

The TUI shall include an in-app help screen.

### FR-043: Non-interactive Mode

EnvLens shall provide non-interactive output for CI or scripting.

Examples:

```bash
envlens check
envlens check --json
envlens report --format markdown
```

---

## 3.7 CLI Requirements

### FR-044: Default Command Opens TUI

```bash
envlens
```

opens the interactive TUI.

### FR-045: Check Command

```bash
envlens check
```

runs diagnostics and exits with a status code.

### FR-046: Exit Codes

| Code | Meaning |
|---:|---|
| 0 | No errors |
| 1 | Warnings/errors found |
| 2 | Invalid CLI usage |
| 3 | Parse failure |
| 4 | Internal error |

### FR-047: JSON Output

```bash
envlens check --json
```

shall output machine-readable diagnostics.

### FR-048: Markdown Report

```bash
envlens report --format markdown
```

shall output a sanitized markdown report.

### FR-049: Profile Selection

```bash
envlens --profile test
```

shall inspect variables using the selected profile.

### FR-050: Source Selection

```bash
envlens --source .env.local --source docker-compose.yml
```

should restrict inspection to selected sources.

---

## 3.8 Configuration

### FR-051: Config File Discovery

EnvLens should discover config files in this order:

1. `.envlens.yml`
2. `.envlens.yaml`
3. `.config/envlens.yml`
4. user config directory

### FR-052: Configurable Secret Patterns

Users should be able to define custom secret key patterns.

Example:

```yaml
secret_patterns:
  - "SUPABASE_.*"
  - ".*_TOKEN"
```

### FR-053: Configurable Required Variables

Users should be able to define required variables.

```yaml
required:
  - DATABASE_URL
  - JWT_SECRET
  - NODE_ENV
```

### FR-054: Configurable Precedence

Users should be able to define source precedence.

### FR-055: Configurable Profiles

Users should be able to define profiles.

```yaml
profiles:
  dev:
    include:
      - .env
      - .env.local
  test:
    include:
      - .env
      - .env.test
```

---

## 4. Non-Functional Requirements

## 4.1 Performance

### NFR-001: Startup Time

For repositories with fewer than 500 files, EnvLens should start in under 500 ms on a typical developer laptop.

### NFR-002: Large Repository Handling

EnvLens should remain responsive in repositories with up to 50,000 files by using ignore patterns and targeted source discovery.

### NFR-003: Incremental Refresh

EnvLens should support manual refresh.

File watching is optional for MVP.

---

## 4.2 Security

### NFR-004: Secrets Masked by Default

Secret-like values must never be shown in full by default.

### NFR-005: Sanitized Logs

EnvLens must not write unmasked secrets to logs, crash reports, or exported reports.

### NFR-006: No Network Calls by Default

EnvLens must not make network requests during normal operation.

### NFR-007: Local-only Processing

All parsing and diagnostics must happen locally.

### NFR-008: Explicit Secret Reveal

Any reveal operation must be user-initiated.

### NFR-009: Clipboard Safety

If copy-to-clipboard is added, copying secrets must require explicit action.

---

## 4.3 Usability

### NFR-010: Zero Configuration

EnvLens must produce useful results without a config file.

### NFR-011: Safe Defaults

The default behavior must avoid leaking secrets.

### NFR-012: Clear Diagnostics

Warnings should be actionable and explain where the issue comes from.

Bad:

```text
Conflict detected.
```

Good:

```text
PORT differs across sources: .env:3000, docker-compose.yml:5001. Effective value is 5001 from docker-compose.yml.
```

### NFR-013: Keyboard-first UX

All core actions must be usable without a mouse.

---

## 4.4 Portability

### NFR-014: Supported Platforms

MVP should support:

- macOS
- Linux

Windows support should be considered but is not required for MVP.

### NFR-015: Terminal Compatibility

EnvLens should support common terminal emulators:

- Terminal.app
- iTerm2
- Alacritty
- Ghostty
- Kitty
- WezTerm
- GNOME Terminal

### NFR-016: Color Degradation

EnvLens should remain usable in low-color or no-color terminals.

---

## 4.5 Reliability

### NFR-017: Graceful Parse Failures

A malformed source file must not crash the application.

### NFR-018: Partial Results

If one source fails to parse, EnvLens should still display results from other sources.

### NFR-019: Deterministic Output

Given the same sources and config, EnvLens shall produce deterministic diagnostics.

---

## 5. MVP Scope

### 5.1 MVP Included

The first version shall include:

- TUI app
- Scan current or specified directory
- Parse `.env` files
- Parse `.env.example` / `.env.sample` as required variable templates
- Parse basic Docker Compose `environment`
- Parse current process environment
- Variable list view
- Source/detail view
- Effective value calculation
- Duplicate/conflict/missing/empty/undefined-reference diagnostics
- Secret masking
- Search
- Warnings-only filter
- Sanitized markdown export
- `envlens check --json`

### 5.2 MVP Excluded

The first version shall not include:

- Cloud secret manager integrations
- GitHub Actions deep parsing
- Full shell parser for package scripts
- Windows-specific environment syntax
- Automatic file watching
- Editing `.env` files from the TUI
- Storing secrets
- Sending telemetry
- Network calls
- AI-generated fixes

---

## 6. Future Enhancements

### 6.1 Secret Manager Integrations

Potential integrations:

- Doppler
- 1Password
- AWS Secrets Manager
- GCP Secret Manager
- Azure Key Vault
- HashiCorp Vault
- macOS Keychain
- Bitwarden

### 6.2 CI Provider Support

Support deeper parsing for:

- GitHub Actions
- GitLab CI
- CircleCI
- Buildkite
- Jenkins
- Vercel
- Netlify
- Fly.io
- Railway
- Render

### 6.3 Framework Presets

Add framework-specific required-variable detection for:

- Next.js
- Vite
- Remix
- Rails
- Django
- FastAPI
- Laravel
- Phoenix

### 6.4 Editor Integration

Open selected variable source in:

```bash
$EDITOR +line file
```

### 6.5 Fix Suggestions

Suggest actions like:

- Add missing key to `.env.local`
- Remove duplicate key
- Move secret from `.env` to `.env.local`
- Add key to `.gitignore`
- Add placeholder to `.env.example`

### 6.6 Git Awareness

Detect whether dotenv files are tracked by git and warn if sensitive files are committed.

Example warning:

```text
.env.local appears to be tracked by git and contains secret-like values.
```

### 6.7 Watch Mode

Automatically refresh when env files change.

```bash
envlens --watch
```

---

## 7. Data Model

### 7.1 Source

```ts
type EnvSource = {
  id: string;
  type: "dotenv" | "compose" | "package-script" | "process" | "ci" | "config";
  path?: string;
  name: string;
  profile?: string;
  precedence: number;
  parsedAt: string;
  errors: ParseError[];
};
```

### 7.2 Variable Occurrence

```ts
type VariableOccurrence = {
  key: string;
  rawValue?: string;
  parsedValue?: string;
  sourceId: string;
  path?: string;
  line?: number;
  context?: string;
  isEmpty: boolean;
  isInherited: boolean;
  isSecretLike: boolean;
};
```

### 7.3 Variable Summary

```ts
type VariableSummary = {
  key: string;
  effectiveValue?: string;
  effectiveSourceId?: string;
  occurrences: VariableOccurrence[];
  diagnostics: Diagnostic[];
  isRequired: boolean;
  isMissing: boolean;
  isSecretLike: boolean;
};
```

### 7.4 Diagnostic

```ts
type Diagnostic = {
  severity: "info" | "warning" | "error";
  code: string;
  message: string;
  key?: string;
  sourceId?: string;
  path?: string;
  line?: number;
};
```

---

## 8. Diagnostic Codes

| Code | Severity | Description |
|---|---|---|
| `DUPLICATE_KEY` | warning | Same key appears multiple times in one source |
| `CONFLICTING_VALUES` | warning | Same key has different values across sources |
| `MISSING_REQUIRED` | error | Required key is not defined |
| `EMPTY_REQUIRED` | warning | Required key is defined but empty |
| `UNDEFINED_REFERENCE` | warning | Variable references undefined variable |
| `CIRCULAR_REFERENCE` | error | Variable references form a cycle |
| `INVALID_DOTENV_LINE` | warning | Source line could not be parsed |
| `SECRET_IN_TRACKED_FILE` | warning | Secret-like value appears in tracked file |
| `INHERITED_UNRESOLVED` | info | Compose variable inherits from process but process has no value |
| `SHADOWED_VALUE` | info | Value is overridden by higher-precedence source |

---

## 9. User Stories

### US-001: Inspect Local Config

As a developer, I want to run `envlens` in my project so I can see which environment variables are active.

Acceptance criteria:

- The app opens in a TUI.
- It lists discovered sources.
- It lists variables.
- Selecting a variable shows all sources.

### US-002: Find Missing Variables

As a developer, I want EnvLens to compare `.env.example` with my local env so I can see what setup values I forgot.

Acceptance criteria:

- Variables in `.env.example` are treated as required.
- Missing keys are marked clearly.
- `envlens check` exits nonzero when required variables are missing.

### US-003: Debug Conflicting Ports

As a developer, I want to know why my app is using a different port than expected.

Acceptance criteria:

- `PORT` from all sources is shown.
- The effective source is highlighted.
- Shadowed values are shown.

### US-004: Avoid Leaking Secrets

As a maintainer, I want exports to mask secrets so contributors can safely paste reports into issues.

Acceptance criteria:

- Secret-like values are masked in the UI by default.
- Secret-like values are masked in markdown and JSON exports by default.
- Reveal requires explicit action.

### US-005: CI Check

As a DevOps engineer, I want to run EnvLens in CI so config drift can fail a build.

Acceptance criteria:

- `envlens check --json` emits valid JSON.
- Exit status reflects diagnostic severity.
- No TUI is required.

---

## 10. Example CLI Usage

```bash
# Open TUI in current project
envlens

# Open TUI for a specific path
envlens ~/code/my-app

# Run diagnostics only
envlens check

# Machine-readable diagnostics
envlens check --json

# Use test profile
envlens --profile test

# Export sanitized markdown report
envlens report --format markdown > env-report.md

# Include process environment but hide dotenv files
envlens --source process

# Reveal values only in interactive mode
envlens
```

---

## 11. Example Sanitized Markdown Report

```markdown
# EnvLens Report

Project: my-app  
Generated: 2026-07-05T01:13:38Z  
Profile: development

## Summary

- Sources scanned: 4
- Variables found: 37
- Required variables missing: 2
- Conflicts: 3
- Secret-like variables: 8

## Errors

### MISSING_REQUIRED: DATABASE_URL

`DATABASE_URL` is listed in `.env.example` but not defined in active sources.

### MISSING_REQUIRED: JWT_SECRET

`JWT_SECRET` is listed in `.env.example` but not defined in active sources.

## Warnings

### CONFLICTING_VALUES: PORT

Effective value: `5001`

| Source | Line | Value |
|---|---:|---|
| `.env` | 2 | `3000` |
| `.env.local` | 2 | `5001` |

### SECRET_LIKE: STRIPE_API_KEY

Value is masked.

| Source | Line | Value |
|---|---:|---|
| `.env.local` | 5 | `sk_live_••••••••8F` |
```

---

## 12. Suggested Technical Architecture

### 12.1 Recommended Stack

For fastest MVP:

- **Language:** Python 3.11+
- **TUI:** Textual
- **Parsing:** Python stdlib + `pyyaml`
- **Packaging:** `uv` or `pipx`
- **Testing:** `pytest`

Alternative single-binary implementation:

- **Language:** Go
- **TUI:** Bubble Tea
- **YAML:** `gopkg.in/yaml.v3`
- **Packaging:** Homebrew + GitHub releases

### 12.2 High-Level Modules

```text
envlens/
  cli.py
  app.py
  scanner.py
  parsers/
    dotenv.py
    compose.py
    package_json.py
    process.py
  model.py
  diagnostics.py
  masking.py
  report.py
  config.py
  tui/
    screens.py
    widgets.py
```

### 12.3 Processing Pipeline

```text
scan project
  ↓
discover sources
  ↓
parse sources
  ↓
normalize variable occurrences
  ↓
apply profile and precedence
  ↓
compute effective values
  ↓
run diagnostics
  ↓
render TUI or export report
```

---

## 13. Acceptance Criteria for MVP Release

EnvLens v0.1.0 is acceptable when:

1. `envlens` opens a usable TUI.
2. `.env`, `.env.local`, `.env.example`, and process env are parsed.
3. Docker Compose environment blocks are parsed.
4. Variables show source file and line.
5. Effective values are calculated.
6. Conflicts are detected.
7. Missing required variables are detected from `.env.example`.
8. Secret-like values are masked by default.
9. Search and warning filter work.
10. `envlens check --json` produces valid JSON.
11. `envlens report --format markdown` produces a sanitized report.
12. Unit tests cover parsers, precedence, diagnostics, and masking.
13. The tool does not make network calls.
14. The tool does not write unmasked secrets to disk.

---

## 14. Open Questions

1. Should `.env.example` variables always be considered required, or only when configured?
2. Should process environment have highest precedence by default?
3. Should Docker Compose variables be grouped by service?
4. Should EnvLens support monorepos as first-class workspaces in MVP?
5. Should JSON export include masked values only, or optionally omit values entirely?
6. Should `envlens check` fail on warnings, errors only, or configurable severity?
7. Should package script parsing be included in v0.1 or deferred?

---

## 15. Recommended MVP Build Plan

### Day 1 Scope

Build a shippable MVP with:

1. CLI skeleton
2. `.env` parser
3. source scanner
4. variable model
5. precedence resolver
6. diagnostics engine
7. Textual TUI with sources, variables, and details panes
8. markdown export
9. JSON check command
10. unit tests for core logic

Defer:

- package script parsing
- CI parsing
- config file support
- file watching
- editor integration

### Suggested First Milestone

```bash
envlens fixtures/basic
```

Should show:

- `.env`
- `.env.local`
- `.env.example`
- effective value per key
- missing required variables
- masked secrets
- conflict warnings
