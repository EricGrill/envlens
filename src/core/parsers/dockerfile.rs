//! Dockerfile `ENV` / `ARG` instruction parser (issue #9).
//!
//! Recognizes the two literal assignment forms Docker supports and nothing
//! more — this is a lightweight extractor, not a full Dockerfile grammar:
//!
//! - `ENV KEY value` (legacy space form: everything after the key is the value)
//! - `ENV KEY=value KEY2=value2 ...` (one or more `key=value` pairs on a line)
//! - `ARG KEY` / `ARG KEY=default`
//!
//! Line continuations (`\` at end of line) are joined before parsing. Parsing
//! never panics; unrecognized instructions contribute nothing (an `ARG` with
//! no default contributes a key with an empty value, matching "declared but
//! unset").

/// A single assignment extracted from a Dockerfile, as
/// `(key, value, 1-indexed line)`.
pub type DockerfileEntry = (String, String, Option<u32>);

/// Parse Dockerfile `content`, returning every `ENV`/`ARG` assignment.
pub fn parse(content: &str) -> Vec<DockerfileEntry> {
    let mut entries = Vec::new();

    for (line_no, logical) in logical_lines(content) {
        let trimmed = logical.trim_start();
        let Some((instruction, rest)) = split_instruction(trimmed) else {
            continue;
        };
        match instruction.to_ascii_uppercase().as_str() {
            "ENV" => parse_env(rest, line_no, &mut entries),
            "ARG" => parse_arg(rest, line_no, &mut entries),
            _ => {}
        }
    }

    entries
}

/// Join backslash-continued physical lines into logical lines, tagging each
/// with the 1-indexed physical line where it started. Comment lines (`#…`)
/// and blank lines are dropped.
fn logical_lines(content: &str) -> Vec<(u32, String)> {
    let physical: Vec<&str> = content
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();

    let mut logical = Vec::new();
    let mut i = 0usize;
    while i < physical.len() {
        let start = (i + 1) as u32;
        let mut buffer = String::new();
        loop {
            let line = physical[i];
            if let Some(without_slash) = line.strip_suffix('\\') {
                buffer.push_str(without_slash);
                buffer.push(' ');
                i += 1;
                if i >= physical.len() {
                    break;
                }
            } else {
                buffer.push_str(line);
                i += 1;
                break;
            }
        }
        let trimmed = buffer.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            logical.push((start, trimmed.to_string()));
        }
    }
    logical
}

/// Split a logical line into `(instruction, rest)` on the first run of
/// whitespace, e.g. `"ENV FOO bar"` -> `("ENV", "FOO bar")`.
fn split_instruction(line: &str) -> Option<(&str, &str)> {
    let idx = line.find(char::is_whitespace)?;
    let (instruction, rest) = line.split_at(idx);
    Some((instruction, rest.trim_start()))
}

fn parse_env(rest: &str, line_no: u32, entries: &mut Vec<DockerfileEntry>) {
    // `ENV` has two forms. If the first token contains `=`, it's the modern
    // `key=value key2=value2` form; otherwise it's the legacy
    // `key rest-of-line-is-value` form.
    let first = rest.split_whitespace().next().unwrap_or("");
    if first.contains('=') {
        for (key, value) in split_pairs(rest) {
            entries.push((key, value, Some(line_no)));
        }
    } else if let Some(idx) = rest.find(char::is_whitespace) {
        let key = rest[..idx].to_string();
        let value = unquote(rest[idx..].trim());
        if is_valid_key(&key) {
            entries.push((key, value, Some(line_no)));
        }
    }
}

fn parse_arg(rest: &str, line_no: u32, entries: &mut Vec<DockerfileEntry>) {
    // Only the first token matters: `ARG KEY` or `ARG KEY=default`. Any
    // trailing tokens are additional ARGs on Docker >= 20.10 only in the
    // rare multi-arg form, which we intentionally do not split.
    let token = rest.split_whitespace().next().unwrap_or("");
    if token.is_empty() {
        return;
    }
    let (key, value) = match token.split_once('=') {
        Some((k, v)) => (k.to_string(), unquote(v)),
        None => (token.to_string(), String::new()),
    };
    if is_valid_key(&key) {
        entries.push((key, value, Some(line_no)));
    }
}

/// Split a `key=value key2="value two"` run into pairs, honoring simple
/// double/single quoting so a quoted value may contain spaces.
fn split_pairs(input: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        // Read key up to '='.
        let key_start = i;
        while i < chars.len() && chars[i] != '=' && !chars[i].is_whitespace() {
            i += 1;
        }
        let key: String = chars[key_start..i].iter().collect();
        if i >= chars.len() || chars[i] != '=' {
            // Bare token with no '='; skip it.
            continue;
        }
        i += 1; // consume '='
        // Read value, honoring quotes.
        let mut value = String::new();
        if i < chars.len() && (chars[i] == '"' || chars[i] == '\'') {
            let quote = chars[i];
            i += 1;
            while i < chars.len() && chars[i] != quote {
                value.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1; // consume closing quote
            }
        } else {
            while i < chars.len() && !chars[i].is_whitespace() {
                value.push(chars[i]);
                i += 1;
            }
        }
        if is_valid_key(&key) {
            pairs.push((key, value));
        }
    }
    pairs
}

/// Strip a single matching pair of surrounding quotes from a legacy-form value.
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

/// Docker env/arg keys follow the same shape as shell identifiers.
fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyvals(content: &str) -> Vec<(String, String)> {
        parse(content).into_iter().map(|(k, v, _)| (k, v)).collect()
    }

    #[test]
    fn legacy_space_form() {
        assert_eq!(
            keyvals("ENV NODE_ENV production\n"),
            vec![("NODE_ENV".to_string(), "production".to_string())]
        );
    }

    #[test]
    fn legacy_space_form_value_with_spaces() {
        assert_eq!(
            keyvals("ENV GREETING hello there world\n"),
            vec![("GREETING".to_string(), "hello there world".to_string())]
        );
    }

    #[test]
    fn modern_equals_form_multiple_pairs() {
        assert_eq!(
            keyvals("ENV A=1 B=2 C=3\n"),
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "2".to_string()),
                ("C".to_string(), "3".to_string()),
            ]
        );
    }

    #[test]
    fn quoted_value_with_spaces() {
        assert_eq!(
            keyvals("ENV MSG=\"hello world\" PORT=8080\n"),
            vec![
                ("MSG".to_string(), "hello world".to_string()),
                ("PORT".to_string(), "8080".to_string()),
            ]
        );
    }

    #[test]
    fn arg_with_and_without_default() {
        assert_eq!(
            keyvals("ARG VERSION=1.2.3\nARG BUILD_ID\n"),
            vec![
                ("VERSION".to_string(), "1.2.3".to_string()),
                ("BUILD_ID".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn line_continuation_joined() {
        let entries = parse("ENV A=1 \\\n    B=2\n");
        assert_eq!(
            entries
                .iter()
                .map(|(k, v, _)| (k.as_str(), v.as_str()))
                .collect::<Vec<_>>(),
            vec![("A", "1"), ("B", "2")]
        );
        // Both pairs report the physical start line of the instruction.
        assert!(entries.iter().all(|(_, _, line)| *line == Some(1)));
    }

    #[test]
    fn instruction_keyword_is_case_insensitive() {
        assert_eq!(
            keyvals("env FOO=bar\n"),
            vec![("FOO".to_string(), "bar".to_string())]
        );
    }

    #[test]
    fn ignores_other_instructions_and_comments() {
        let content = "# a comment\nFROM rust:1\nRUN echo hi\nWORKDIR /app\nENV KEEP=1\n";
        assert_eq!(
            keyvals(content),
            vec![("KEEP".to_string(), "1".to_string())]
        );
    }

    #[test]
    fn never_panics_on_odd_input() {
        for input in ["ENV", "ENV ", "ENV =", "ARG =x", "ENV 9BAD=1", "ENV ="] {
            let _ = parse(input);
        }
    }
}
