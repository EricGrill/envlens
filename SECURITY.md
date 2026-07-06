# Security Policy

## Supported Versions

Security fixes are provided for the latest released version and the `main` branch.

## Reporting a Vulnerability

Use GitHub private vulnerability reporting when it is available for this
repository. If private reporting is not available, open a GitHub issue with a
minimal description and do not include secrets, credentials, private
configuration files, or exploit details that would expose another system.

Include:

- the affected EnvLens version or commit
- the command or workflow involved
- the expected and actual behavior
- a sanitized reproduction case, if one is available

## Handling Test Data

EnvLens tests and fixtures must use repo-owned fake values such as
`envlensFakeSecretValue12345678`. Do not commit provider-shaped fake
credentials, even if they are invalid, because they create noisy public-history
secret scans.

## Security Posture

EnvLens runs locally, has no telemetry, and does not make network calls during
normal operation. Secret-like values are masked by default in CLI output, TUI
views, diagnostics, markdown reports, JSON exports, and panic output.
