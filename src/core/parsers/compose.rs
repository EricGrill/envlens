//! Docker Compose `environment:` parser (spec FR-015/016).
//!
//! Structure comes from `serde_yaml::Value`: we walk `services.*.environment`
//! accepting either mapping form (`KEY: value`) or list form (`- KEY=value`
//! / bare `- KEY`). `serde_yaml::Mapping` is backed by an `indexmap`, so
//! service order in the output matches document order for free. A YAML merge
//! key (`<<: *anchor`) surfaces in the mapping as a literal `"<<"` key with
//! the merged-in mapping as its value; it is skipped rather than emitted as
//! a (garbage) entry.
//!
//! serde_yaml has no span information on `Value`, so line numbers are
//! recovered with a second pass: a line scan. Each service's block is found
//! by matching its 2-space-indented header line under `services:`; the block
//! ends at the next line whose indentation is `<= 2` spaces (ignoring blank
//! lines and comments). Within that block we locate the `environment:`
//! header line at the service's own child indent, then scan ONLY the lines
//! between that header and the end of its sub-block (the next non-blank,
//! non-comment line whose indentation is `<=` the `environment:` line's own
//! indent) for entries. A candidate line only counts as an entry if it sits
//! at the entries' own indent (one level deeper than `environment:`), so
//! text at other indents — a `depends_on:` list item that happens to share
//! an env var's name, or a `KEY:`-looking line inside a deeper block-scalar
//! value body — can never be mistaken for (or steal the line of) a real
//! entry. Entries are matched in document order via a positional cursor that
//! advances past each match, so duplicate keys (`- PORT=1` / `- PORT=2`)
//! resolve to successive lines rather than both pointing at the first
//! occurrence. A key that can't be found this way gets `line: None` rather
//! than a guess.

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
            // `env_bounds` is `(entry_indent, entries_end)`: the indentation
            // entries must sit at, and the exclusive end of the sub-block.
            // `search_cursor` is the next 0-based line to start searching
            // from; it advances past each match so duplicate keys land on
            // successive lines rather than all re-finding the first one.
            let env_bounds = block_start
                .and_then(|start| find_environment_header(&lines, start, block_end))
                .map(|header| {
                    let env_indent = indent_of(lines[header]);
                    let entries_end = next_line_at_or_above(&lines, header, env_indent);
                    (header, env_indent + 2, entries_end)
                });
            let mut search_cursor = env_bounds.map(|(header, _, _)| header + 1);

            for (key, value) in collect_environment(environment) {
                let line = env_bounds.and_then(|(_, entry_indent, entries_end)| {
                    let start = search_cursor?;
                    let (next, line) =
                        find_entry_line(&lines, start, entries_end, entry_indent, &key)?;
                    search_cursor = Some(next);
                    Some(line)
                });
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
    next_line_at_or_above(lines, start, 2)
}

/// 0-based exclusive end of the sub-block that begins right after `start`:
/// the index of the next non-blank, non-comment line whose indentation is
/// `<= threshold`, or `lines.len()` if the sub-block runs to end of file.
fn next_line_at_or_above(lines: &[&str], start: usize, threshold: usize) -> usize {
    lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return false;
            }
            indent_of(line) <= threshold
        })
        .map(|(idx, _)| idx)
        .unwrap_or(lines.len())
}

/// Find the 0-based index of the `environment:` header line within the
/// half-open range `(block_start, block_end)`, at the service's own child
/// indent (`indent_of(lines[block_start]) + 2`). Returns `None` if no such
/// line is found, e.g. because the service block was itself never located.
fn find_environment_header(lines: &[&str], block_start: usize, block_end: usize) -> Option<usize> {
    let target_indent = indent_of(lines[block_start]) + 2;
    let block_end = block_end.min(lines.len());
    (block_start + 1..block_end)
        .find(|&idx| indent_of(lines[idx]) == target_indent && lines[idx].trim() == "environment:")
}

/// Find the 1-based line number of `key` among the environment entries in
/// the half-open range `(search_start, entries_end)` (0-based indices into
/// `lines`), matching only lines that sit exactly at `entry_indent` — this
/// is what keeps a `KEY:`-looking line inside a deeper block-scalar value
/// body, or a line elsewhere at a different indent, from matching. Returns
/// the matched 0-based index (so the caller can advance its search cursor
/// past it, giving duplicate keys distinct lines) alongside the 1-based line
/// number.
fn find_entry_line(
    lines: &[&str],
    search_start: usize,
    entries_end: usize,
    entry_indent: usize,
    key: &str,
) -> Option<(usize, u32)> {
    let entries_end = entries_end.min(lines.len());
    (search_start..entries_end).find_map(|idx| {
        let line = lines[idx];
        if indent_of(line) != entry_indent {
            return None;
        }
        matches_key(line.trim(), key).then(|| (idx + 1, (idx + 1) as u32))
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
                // A YAML merge key (`<<: *anchor`) surfaces here as a
                // literal `"<<"` key whose value is the merged-in mapping,
                // not an environment variable — skip it rather than
                // emitting a garbage `<<` entry with value `None`.
                if key == "<<" {
                    return None;
                }
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

    #[test]
    fn sibling_key_collision_uses_environment_line() {
        // `depends_on` lists a service-ish-looking entry "PORT" (line 4)
        // before the real `environment:` entry named PORT (line 6). The old
        // whole-block text scan would find the `depends_on` line first;
        // scoping the scan to the `environment:` sub-block fixes that.
        let content = "services:\n  api:\n    depends_on:\n      - PORT\n    environment:\n      - PORT=8080\n";
        let (services, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(
            services,
            vec![ComposeService {
                name: "api".to_string(),
                entries: vec![entry("PORT", Some("8080"), Some(6))],
            }]
        );
    }

    #[test]
    fn block_scalar_body_not_matched() {
        // The block-scalar value of MSG contains an indented line that looks
        // like a `KEY:` entry (line 5). It sits deeper than the entries'
        // indent, so indent-aware matching must skip it and attribute the
        // real KEY entry to its own line (6), not the body line.
        let content = "services:\n  api:\n    environment:\n      MSG: |\n        KEY: inside\n      KEY: value\n";
        let (services, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(services.len(), 1);
        assert_eq!(
            services[0].entries,
            vec![
                entry("MSG", Some("KEY: inside\n"), Some(4)),
                entry("KEY", Some("value"), Some(6)),
            ]
        );
    }

    #[test]
    fn duplicate_list_keys_distinct_lines() {
        // Two list-form entries with the same key must resolve to their own
        // distinct lines via the positional search cursor, not both to the
        // first occurrence.
        let content = "services:\n  api:\n    environment:\n      - PORT=1\n      - PORT=2\n";
        let (services, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(services.len(), 1);
        assert_eq!(
            services[0].entries,
            vec![
                entry("PORT", Some("1"), Some(4)),
                entry("PORT", Some("2"), Some(5)),
            ]
        );
    }

    #[test]
    fn merge_key_skipped() {
        // `<<: *defaults` is a YAML merge key; serde_yaml surfaces it in the
        // mapping as a literal "<<" key whose value is the merged-in
        // mapping (a non-scalar), not an environment variable. It must be
        // skipped entirely rather than emitted as a garbage entry.
        let content = "defaults: &defaults\n  A: \"1\"\nservices:\n  api:\n    environment:\n      <<: *defaults\n      KEY: value\n";
        let (services, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(services.len(), 1);
        assert_eq!(
            services[0].entries,
            vec![entry("KEY", Some("value"), Some(7))]
        );
    }
}
