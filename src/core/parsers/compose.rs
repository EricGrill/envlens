//! Docker Compose `environment:` parser (spec FR-015/016).
//!
//! Structure comes from `serde_yaml::Value`: we walk `services.*.environment`
//! accepting either mapping form (`KEY: value`) or list form (`- KEY=value`
//! / bare `- KEY`). `serde_yaml::Mapping` is backed by an `indexmap`, so
//! service order in the output matches document order for free.
//!
//! serde_yaml has no span information on `Value`, so line numbers are
//! recovered with a second pass: a line scan. Each service's block is found
//! by matching its 2-space-indented header line under `services:`; the block
//! ends at the next line whose indentation is `<= 2` spaces (ignoring blank
//! lines and comments). Within that block, an entry's line is the first line
//! whose trimmed text starts with `KEY:`, `- KEY=`, or is exactly `- KEY`
//! (exact key match, never a substring match). A key that can't be found
//! this way gets `line: None` rather than a guess.

use serde_yaml::Value;

use crate::core::model::ParseError;

/// A single service block under `services:` in a compose file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeService {
    pub name: String,
    pub entries: Vec<ComposeEntry>,
}

/// A single environment entry contributed by a service's `environment:` key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeEntry {
    pub key: String,
    /// `None` for a bare inherited key, e.g. `- DATABASE_URL` in list form
    /// (and for an explicit YAML null in map form, e.g. `KEY:` with nothing
    /// after the colon — compose treats both the same way: pass the value
    /// through from the host environment).
    pub value: Option<String>,
    pub line: Option<u32>,
}

/// Parse a docker-compose YAML document into its services' environment
/// entries plus any parse errors. A malformed document (YAML that doesn't
/// parse at all) yields zero services and exactly one [`ParseError`] with
/// `line: None`. A well-formed document with no `services:` key, or services
/// with no `environment:` key, is not an error either — those just produce
/// empty results. A `services:` key that is present but whose value isn't a
/// mapping (e.g. a scalar or sequence) is a spec violation and yields zero
/// services plus exactly one [`ParseError`] with `line: None`.
pub fn parse(content: &str) -> (Vec<ComposeService>, Vec<ParseError>) {
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

    let services_map = match document.get("services") {
        None => return (Vec::new(), Vec::new()),
        Some(value) => match value.as_mapping() {
            Some(mapping) => mapping,
            None => {
                return (
                    Vec::new(),
                    vec![ParseError {
                        line: None,
                        message: "services: must be a mapping of service name to definition"
                            .to_string(),
                    }],
                );
            }
        },
    };

    let lines: Vec<&str> = content.lines().collect();
    let mut cursor = services_section_start(&lines);
    let mut services = Vec::with_capacity(services_map.len());

    for (key, service_value) in services_map {
        let Some(name) = key.as_str() else {
            continue;
        };

        let block_start = find_service_header(&lines, name, cursor);
        let block_end = match block_start {
            Some(start) => {
                let end = block_end_line(&lines, start);
                cursor = end;
                end
            }
            None => lines.len(),
        };

        let mut entries = Vec::new();
        if let Some(environment) = service_value
            .as_mapping()
            .and_then(|m| m.get("environment"))
        {
            for (key, value) in collect_environment(environment) {
                let line =
                    block_start.and_then(|start| find_key_line(&lines, start, block_end, &key));
                entries.push(ComposeEntry { key, value, line });
            }
        }

        services.push(ComposeService {
            name: name.to_string(),
            entries,
        });
    }

    (services, Vec::new())
}

/// Index (0-based, into `lines`) of the line right after a top-level
/// `services:` header, i.e. where the search for the first service header
/// should begin. Falls back to `0` (search the whole file) if no such line
/// is found, which only matters for documents that reached this point via
/// some non-standard structure `serde_yaml` still accepted.
fn services_section_start(lines: &[&str]) -> usize {
    lines
        .iter()
        .position(|line| indent_of(line) == 0 && line.trim_start().starts_with("services:"))
        .map(|idx| idx + 1)
        .unwrap_or(0)
}

/// Number of leading ASCII space characters on `line`.
fn indent_of(line: &str) -> usize {
    line.chars().take_while(|&c| c == ' ').count()
}

/// Find the 0-based index of `name`'s service header line (`^  <name>:`)
/// starting the search at `from`.
fn find_service_header(lines: &[&str], name: &str, from: usize) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .skip(from)
        .find(|(_, line)| is_service_header(line, name))
        .map(|(idx, _)| idx)
}

fn is_service_header(line: &str, name: &str) -> bool {
    if indent_of(line) != 2 {
        return false;
    }
    line[2..]
        .strip_prefix(name)
        .is_some_and(|rest| rest.starts_with(':'))
}

/// 0-based exclusive end of the block started by the header at `start`: the
/// index of the next non-blank, non-comment line with indentation `<= 2`, or
/// `lines.len()` if the block runs to end of file.
fn block_end_line(lines: &[&str], start: usize) -> usize {
    lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return false;
            }
            indent_of(line) <= 2
        })
        .map(|(idx, _)| idx)
        .unwrap_or(lines.len())
}

/// Find the 1-based line number of `key` within the half-open line range
/// `(start, end)` (0-based indices into `lines`; `start` is the service's
/// own header line and is skipped since entries are always nested under it).
fn find_key_line(lines: &[&str], start: usize, end: usize, key: &str) -> Option<u32> {
    let end = end.min(lines.len());
    (start + 1..end).find_map(|idx| {
        let trimmed = lines[idx].trim();
        matches_key(trimmed, key).then(|| (idx + 1) as u32)
    })
}

/// Whether `trimmed` is the line contributing `key`, via one of the three
/// shapes a compose environment entry can take: `KEY:` (map form), `- KEY=`
/// (valued list form), or exactly `- KEY` (bare list form). Matching is
/// exact-key, never substring — `PORT:` must not match a search for `POR`.
fn matches_key(trimmed: &str, key: &str) -> bool {
    if let Some(rest) = trimmed.strip_prefix(key)
        && rest.starts_with(':')
    {
        return true;
    }
    if let Some(rest) = trimmed.strip_prefix("- ")
        && let Some(after_key) = rest.strip_prefix(key)
        && (after_key.starts_with('=') || after_key.is_empty())
    {
        return true;
    }
    false
}

/// Read an `environment:` value (mapping or sequence form) into
/// `(key, value)` pairs, preserving document order. Anything else (a scalar,
/// or an unrecognized nested shape) contributes nothing.
fn collect_environment(environment: &Value) -> Vec<(String, Option<String>)> {
    if let Some(map) = environment.as_mapping() {
        return map
            .iter()
            .filter_map(|(key, value)| {
                let key = key.as_str()?.to_string();
                Some((key, stringify_scalar(value)))
            })
            .collect();
    }

    if let Some(sequence) = environment.as_sequence() {
        return sequence
            .iter()
            .filter_map(Value::as_str)
            .map(|item| match item.split_once('=') {
                Some((key, value)) => (key.to_string(), Some(value.to_string())),
                None => (item.to_string(), None),
            })
            .collect();
    }

    Vec::new()
}

/// Render a scalar YAML value the way it would read in a `.env` file:
/// strings pass through unchanged, booleans/numbers use their canonical
/// decimal text (no `5001.0`-style float artifacts for integers). A YAML
/// null (`KEY:` with nothing after the colon) is treated the same as a bare
/// list-form key: `None`, meaning "inherit from the host environment".
/// Non-scalar values (nested maps/sequences) are dropped as unsupported.
fn stringify_scalar(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, value: Option<&str>, line: Option<u32>) -> ComposeEntry {
        ComposeEntry {
            key: key.to_string(),
            value: value.map(str::to_string),
            line,
        }
    }

    #[test]
    fn map_form() {
        let content = "services:\n  api:\n    environment:\n      NODE_ENV: development\n      PORT: \"5001\"\n";
        let (services, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(
            services,
            vec![ComposeService {
                name: "api".to_string(),
                entries: vec![
                    entry("NODE_ENV", Some("development"), Some(4)),
                    entry("PORT", Some("5001"), Some(5)),
                ],
            }]
        );
    }

    #[test]
    fn map_form_stringifies_numeric_and_bool_scalars() {
        let content = "services:\n  api:\n    environment:\n      PORT: 5001\n      DEBUG: true\n";
        let (services, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(services.len(), 1);
        assert_eq!(
            services[0].entries,
            vec![
                entry("PORT", Some("5001"), Some(4)),
                entry("DEBUG", Some("true"), Some(5)),
            ]
        );
    }

    #[test]
    fn map_form_null_value_is_inherited() {
        let content =
            "services:\n  api:\n    environment:\n      NODE_ENV:\n      PORT: \"5001\"\n";
        let (services, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(
            services,
            vec![ComposeService {
                name: "api".to_string(),
                entries: vec![
                    entry("NODE_ENV", None, Some(4)),
                    entry("PORT", Some("5001"), Some(5)),
                ],
            }]
        );
    }

    #[test]
    fn list_form() {
        let content = "services:\n  api:\n    environment:\n      - NODE_ENV=development\n      - PORT=5001\n      - DATABASE_URL\n";
        let (services, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(
            services,
            vec![ComposeService {
                name: "api".to_string(),
                entries: vec![
                    entry("NODE_ENV", Some("development"), Some(4)),
                    entry("PORT", Some("5001"), Some(5)),
                    entry("DATABASE_URL", None, Some(6)),
                ],
            }]
        );
    }

    #[test]
    fn services_in_document_order() {
        let content = "services:\n  web:\n    environment:\n      NODE_ENV: production\n  api:\n    environment:\n      PORT: \"5001\"\n";
        let (services, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(
            services,
            vec![
                ComposeService {
                    name: "web".to_string(),
                    entries: vec![entry("NODE_ENV", Some("production"), Some(4))],
                },
                ComposeService {
                    name: "api".to_string(),
                    entries: vec![entry("PORT", Some("5001"), Some(7))],
                },
            ]
        );
    }

    #[test]
    fn line_numbers_from_scan() {
        // Both services share the "NODE_ENV" key: this is a regression check
        // that each service's scan is scoped to its own block rather than
        // finding the first match anywhere in the file.
        let content = "services:\n  api:\n    environment:\n      NODE_ENV: development\n  web:\n    environment:\n      NODE_ENV: production\n      PORT: \"8080\"\n";
        let (services, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(services.len(), 2);

        assert_eq!(services[0].name, "api");
        assert_eq!(
            services[0].entries,
            vec![entry("NODE_ENV", Some("development"), Some(4))]
        );

        assert_eq!(services[1].name, "web");
        assert_eq!(
            services[1].entries,
            vec![
                entry("NODE_ENV", Some("production"), Some(7)),
                entry("PORT", Some("8080"), Some(8)),
            ]
        );
    }

    #[test]
    fn env_file_noted_not_parsed() {
        let content = "services:\n  api:\n    env_file: .env.docker\n";
        let (services, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(
            services,
            vec![ComposeService {
                name: "api".to_string(),
                entries: Vec::new(),
            }]
        );
    }

    #[test]
    fn env_file_alongside_environment_only_yields_environment_entries() {
        let content = "services:\n  api:\n    env_file: .env.docker\n    environment:\n      NODE_ENV: development\n";
        let (services, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(
            services,
            vec![ComposeService {
                name: "api".to_string(),
                entries: vec![entry("NODE_ENV", Some("development"), Some(5))],
            }]
        );
    }

    #[test]
    fn malformed_yaml_is_one_error() {
        let content = "services: [";
        let (services, errors) = parse(content);

        assert!(services.is_empty());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, None);
    }

    #[test]
    fn non_mapping_services_is_one_error() {
        let content = "services: \"x\"\n";
        let (services, errors) = parse(content);

        assert_eq!(services.len(), 0);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, None);
    }

    #[test]
    fn no_services_key_no_error() {
        let content = "version: \"3.9\"\n";
        let (services, errors) = parse(content);

        assert!(services.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn no_environment_key() {
        let content = "services:\n  api:\n    image: myimage\n";
        let (services, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(
            services,
            vec![ComposeService {
                name: "api".to_string(),
                entries: Vec::new(),
            }]
        );
    }
}
