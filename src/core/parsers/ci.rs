//! CI config `env`/`variables` parser (spec FR-008, design.md "CI configs").
//!
//! Only the cheap, high-value shapes are extracted: top-level and job-level
//! flat maps. Deeper constructs (build matrices, `strategy:` blocks, GitHub's
//! `secrets`/`vars` contexts) are out of scope for v0.1 and deliberately
//! never walked, so they can't leak spurious entries. `${{ secrets.X }}`
//! GitHub Actions expressions are plain YAML scalars and are kept verbatim
//! as strings — resolving them is out of scope.
//!
//! `CircleCi` is listed as a source but contributes zero variables in v0.1:
//! `parse` short-circuits to `(empty, empty)` without even looking at
//! `content`, so a malformed CircleCi file can never produce an error.
//!
//! serde_yaml has no span information on `Value`, so line numbers are
//! best-effort: a simple whole-file scan for a line starting with `KEY:`
//! (after trimming indentation). This does not disambiguate same-named keys
//! across jobs — CI line precision is explicitly low priority per spec, so a
//! `None` or a first-match line is an acceptable answer either way.

use std::ffi::OsStr;
use std::path::Path;

use serde_yaml::{Mapping, Value};

use crate::core::model::ParseError;
use crate::core::parsers::stringify_scalar;

/// Which CI system a config file's shape should be interpreted as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiFlavor {
    GithubActions,
    GitlabCi,
    CircleCi,
}

/// `(key, value, line)` triples contributed by a CI config's `env`/
/// `variables` maps.
type CiEntries = Vec<(String, String, Option<u32>)>;

/// Parse a CI config document into flat `(key, value, line)` env entries
/// plus any parse errors, per `flavor`. A malformed document (YAML that
/// doesn't parse at all) yields zero entries and exactly one [`ParseError`]
/// with `line: None` — except for [`CiFlavor::CircleCi`], which always
/// returns `(empty, empty)` regardless of content (v0.1 scope: the file is
/// still listed as a source, it just contributes no variables).
pub fn parse(content: &str, flavor: CiFlavor) -> (CiEntries, Vec<ParseError>) {
    match flavor {
        CiFlavor::CircleCi => (Vec::new(), Vec::new()),
        CiFlavor::GithubActions => parse_github_actions(content),
        CiFlavor::GitlabCi => parse_gitlab(content),
    }
}

/// Derive a [`CiFlavor`] from a project-relative path, mirroring the
/// discovery rules in `scanner.rs`'s `classify`: `.github/workflows/*.yml`
/// (by path suffix, so a same-named file elsewhere never matches),
/// `.gitlab-ci.yml` / `circle.yml` (by file name, anywhere), and
/// `.circleci/config.yml` (by path suffix, so a bare root `config.yml`
/// doesn't match). Returns `None` for anything else.
pub fn flavor_for(rel_path: &Path) -> Option<CiFlavor> {
    if is_github_workflow(rel_path) {
        return Some(CiFlavor::GithubActions);
    }
    if is_circleci_config(rel_path) {
        return Some(CiFlavor::CircleCi);
    }

    let file_name = rel_path.file_name()?.to_str()?;
    if file_name == ".gitlab-ci.yml" {
        return Some(CiFlavor::GitlabCi);
    }
    if file_name == "circle.yml" {
        return Some(CiFlavor::CircleCi);
    }

    None
}

/// `.github/workflows/*.yml` or `*.yaml`, matched by relative-path suffix
/// (mirrors `scanner.rs::is_github_workflow`).
fn is_github_workflow(rel_path: &Path) -> bool {
    let has_yaml_extension = matches!(
        rel_path.extension().and_then(OsStr::to_str),
        Some("yml") | Some("yaml")
    );
    has_yaml_extension && parent_matches(rel_path, &[".github", "workflows"])
}

/// `.circleci/config.yml`, matched by relative-path suffix — a bare
/// root-level `config.yml` must never match (mirrors
/// `scanner.rs::is_circleci_config`).
fn is_circleci_config(rel_path: &Path) -> bool {
    rel_path.file_name() == Some(OsStr::new("config.yml"))
        && parent_matches(rel_path, &[".circleci"])
}

/// True if `rel_path`'s parent directory chain ends with exactly `expected`,
/// in order.
fn parent_matches(rel_path: &Path, expected: &[&str]) -> bool {
    let Some(parent) = rel_path.parent() else {
        return false;
    };
    let components: Vec<&OsStr> = parent.components().map(|c| c.as_os_str()).collect();
    if components.len() < expected.len() {
        return false;
    }
    let start = components.len() - expected.len();
    components[start..]
        .iter()
        .zip(expected)
        .all(|(actual, expected)| *actual == OsStr::new(expected))
}

fn parse_github_actions(content: &str) -> (CiEntries, Vec<ParseError>) {
    let document: Value = match serde_yaml::from_str(content) {
        Ok(document) => document,
        Err(err) => {
            return (
                Vec::new(),
                vec![ParseError {
                    line: None,
                    message: format!("invalid YAML: {err}"),
                }],
            );
        }
    };

    let mut entries = Vec::new();

    if let Some(env) = document.get("env").and_then(Value::as_mapping) {
        entries.extend(mapping_entries(content, env));
    }

    if let Some(jobs) = document.get("jobs").and_then(Value::as_mapping) {
        for (_, job) in jobs {
            let job_env = job
                .as_mapping()
                .and_then(|job| job.get("env"))
                .and_then(Value::as_mapping);
            if let Some(job_env) = job_env {
                entries.extend(mapping_entries(content, job_env));
            }
        }
    }

    (entries, Vec::new())
}

fn parse_gitlab(content: &str) -> (CiEntries, Vec<ParseError>) {
    let document: Value = match serde_yaml::from_str(content) {
        Ok(document) => document,
        Err(err) => {
            return (
                Vec::new(),
                vec![ParseError {
                    line: None,
                    message: format!("invalid YAML: {err}"),
                }],
            );
        }
    };

    let Some(top_level) = document.as_mapping() else {
        return (Vec::new(), Vec::new());
    };

    let mut entries = Vec::new();

    if let Some(variables) = top_level.get("variables").and_then(Value::as_mapping) {
        entries.extend(mapping_entries(content, variables));
    }

    // A "job" is any top-level mapping key (other than `variables` itself)
    // whose value has a `variables` child mapping — GitLab CI has no
    // separate `jobs:` wrapper the way GitHub Actions does. Dot-prefixed
    // keys (`.template:`) are GitLab's convention for hidden jobs/templates:
    // they're never run directly and their `variables:` only takes effect
    // where another job extends/anchors them, so they're skipped here.
    for (key, value) in top_level {
        let key_str = key.as_str();
        if key_str == Some("variables") {
            continue;
        }
        if key_str.is_some_and(|k| k.starts_with('.')) {
            continue;
        }
        let job_variables = value
            .as_mapping()
            .and_then(|job| job.get("variables"))
            .and_then(Value::as_mapping);
        if let Some(job_variables) = job_variables {
            entries.extend(mapping_entries(content, job_variables));
        }
    }

    (entries, Vec::new())
}

/// Read a flat `key: scalar` mapping into `(key, value, line)` triples,
/// preserving document order. Non-scalar values (nested maps/sequences, e.g.
/// a matrix block that happens to share a mapping with `env:`) are silently
/// dropped rather than emitted as garbage entries.
fn mapping_entries(content: &str, mapping: &Mapping) -> CiEntries {
    mapping
        .iter()
        .filter_map(|(key, value)| {
            let key = key.as_str()?.to_string();
            let value = stringify_scalar(value)?;
            let line = find_key_line(content, &key);
            Some((key, value, line))
        })
        .collect()
}

/// Best-effort 1-indexed line number for `key`: the first line in `content`
/// whose trimmed text starts with `<key>:`. This is a whole-file scan with
/// no job scoping, so a key repeated across jobs always resolves to its
/// first occurrence — acceptable since CI line precision is explicitly
/// low-priority (see module docs). `None` if not found at all.
fn find_key_line(content: &str, key: &str) -> Option<u32> {
    let needle = format!("{key}:");
    content
        .lines()
        .enumerate()
        .find(|(_, line)| line.trim_start().starts_with(&needle))
        .map(|(idx, _)| (idx + 1) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triple(key: &str, value: &str) -> (String, String) {
        (key.to_string(), value.to_string())
    }

    fn without_lines(entries: &[(String, String, Option<u32>)]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(k, v, _)| (k.clone(), v.clone()))
            .collect()
    }

    #[test]
    fn github_actions_env() {
        let content = "env:\n  NODE_ENV: production\njobs:\n  build:\n    env:\n      API_KEY: ${{ secrets.API_KEY }}\n  test:\n    steps: []\n";
        let (entries, errors) = parse(content, CiFlavor::GithubActions);

        assert!(errors.is_empty());
        assert_eq!(
            without_lines(&entries),
            vec![
                triple("NODE_ENV", "production"),
                triple("API_KEY", "${{ secrets.API_KEY }}"),
            ]
        );
    }

    #[test]
    fn gitlab_variables() {
        let content = "variables:\n  NODE_ENV: production\nbuild:\n  variables:\n    API_KEY: secret123\ntest:\n  script: echo hi\n";
        let (entries, errors) = parse(content, CiFlavor::GitlabCi);

        assert!(errors.is_empty());
        assert_eq!(
            without_lines(&entries),
            vec![
                triple("NODE_ENV", "production"),
                triple("API_KEY", "secret123"),
            ]
        );
    }

    #[test]
    fn gitlab_hidden_job_template_skipped() {
        let content = ".template:\n  variables:\n    TEMPLATE_VAR: unused\ndeploy:\n  variables:\n    DEPLOY_VAR: value\n";
        let (entries, errors) = parse(content, CiFlavor::GitlabCi);

        assert!(errors.is_empty());
        assert_eq!(without_lines(&entries), vec![triple("DEPLOY_VAR", "value")]);
    }

    #[test]
    fn circleci_returns_empty() {
        let content = "version: 2.1\njobs:\n  build:\n    docker:\n      - image: cimg/node:18.0\n    environment:\n      NODE_ENV: production\n";
        let (entries, errors) = parse(content, CiFlavor::CircleCi);

        assert!(entries.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn deeper_constructs_ignored() {
        let content = "jobs:\n  build:\n    strategy:\n      matrix:\n        node: [14, 16]\n    env:\n      NODE_ENV: test\n";
        let (entries, errors) = parse(content, CiFlavor::GithubActions);

        assert!(errors.is_empty());
        assert_eq!(without_lines(&entries), vec![triple("NODE_ENV", "test")]);
    }

    #[test]
    fn malformed_yaml_one_error_github_actions() {
        let content = "env: [";
        let (entries, errors) = parse(content, CiFlavor::GithubActions);

        assert!(entries.is_empty());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, None);
    }

    #[test]
    fn malformed_yaml_one_error_gitlab() {
        let content = "variables: [";
        let (entries, errors) = parse(content, CiFlavor::GitlabCi);

        assert!(entries.is_empty());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, None);
    }

    #[test]
    fn circleci_never_errors_even_on_malformed_content() {
        let content = "not: valid: yaml: [";
        let (entries, errors) = parse(content, CiFlavor::CircleCi);

        assert!(entries.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn no_env_key_no_error() {
        let content = "jobs:\n  build:\n    steps: []\n";
        let (entries, errors) = parse(content, CiFlavor::GithubActions);

        assert!(entries.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn flavor_for_dispatch() {
        assert_eq!(
            flavor_for(Path::new(".github/workflows/ci.yml")),
            Some(CiFlavor::GithubActions)
        );
        assert_eq!(
            flavor_for(Path::new(".github/workflows/release.yaml")),
            Some(CiFlavor::GithubActions)
        );
        assert_eq!(
            flavor_for(Path::new(".gitlab-ci.yml")),
            Some(CiFlavor::GitlabCi)
        );
        assert_eq!(
            flavor_for(Path::new("circle.yml")),
            Some(CiFlavor::CircleCi)
        );
        assert_eq!(
            flavor_for(Path::new(".circleci/config.yml")),
            Some(CiFlavor::CircleCi)
        );
        assert_eq!(flavor_for(Path::new("README.md")), None);
        assert_eq!(flavor_for(Path::new("config.yml")), None);
    }
}
