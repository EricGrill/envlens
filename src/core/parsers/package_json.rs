//! `package.json` `scripts` inline-env-assignment parser (spec FR-017/018).
//!
//! For each `scripts.<name>` string, the value is tokenized on whitespace and
//! only *leading* `KEY=value` tokens are treated as environment assignments —
//! this is a regex-shaped heuristic, not a shell parser, so an assignment
//! appearing after the command name (`node FOO=bar`) is left alone as just an
//! argument. Two shell idioms get special-cased because they're common and
//! trivial extensions of the same tokenizer: `cross-env KEY=value ... cmd`
//! (skipping cross-env's own `--flags`) and the Windows cmd form `set
//! KEY=value && cmd`. A script with zero leading assignments contributes no
//! sub-source at all — `eslint .` is not `package.json[lint]` with an empty
//! entry list, it's simply absent from the results.
//!
//! serde_json has no span information on `Value`, so line numbers are
//! recovered with a best-effort raw-text scan for the literal `"<script>":`
//! — this can in principle match an unrelated same-named key elsewhere in
//! the file, but that tradeoff is accepted for v0.1 (see compose.rs for the
//! more careful scan this project uses when precision actually matters).

use crate::core::model::ParseError;

/// A single `scripts.<name>` entry that contributed at least one leading
/// environment assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptEnv {
    pub script: String,
    /// `(key, value, line)` in the order the assignments appear in the
    /// script string.
    pub entries: Vec<(String, String, Option<u32>)>,
}

/// Parse a `package.json` document's `scripts` map into per-script
/// environment assignments plus any parse errors. A malformed document
/// (JSON that doesn't parse at all) yields zero scripts and exactly one
/// [`ParseError`] with `line: None`. A well-formed document with no
/// `scripts` key, or scripts with zero leading assignments, is not an error
/// — those just produce empty/omitted results.
pub fn parse(content: &str) -> (Vec<ScriptEnv>, Vec<ParseError>) {
    let document: serde_json::Value = match serde_json::from_str(content) {
        Ok(document) => document,
        Err(err) => {
            return (
                Vec::new(),
                vec![ParseError {
                    line: None,
                    message: format!("invalid JSON: {err}"),
                }],
            );
        }
    };

    let Some(scripts) = document
        .get("scripts")
        .and_then(serde_json::Value::as_object)
    else {
        return (Vec::new(), Vec::new());
    };

    let mut results = Vec::new();
    for (name, value) in scripts {
        let Some(text) = value.as_str() else {
            continue;
        };

        let assignments = leading_assignments(text);
        if assignments.is_empty() {
            continue;
        }

        let line = find_script_line(content, name);
        let entries = assignments
            .into_iter()
            .map(|(key, value)| (key, value, line))
            .collect();
        results.push(ScriptEnv {
            script: name.clone(),
            entries,
        });
    }

    (results, Vec::new())
}

/// Tokenize `script` on whitespace and consume leading `KEY=value`
/// assignments, per the three recognized shapes: plain leading assignments,
/// `cross-env KEY=value... cmd` (skipping cross-env's own `--flags`), and
/// `set KEY=value && cmd` (a single assignment only). Stops at the first
/// token that isn't part of one of those shapes.
fn leading_assignments(script: &str) -> Vec<(String, String)> {
    let tokens: Vec<&str> = script.split_whitespace().collect();
    let Some(&first) = tokens.first() else {
        return Vec::new();
    };

    if first == "cross-env" {
        let mut assignments = Vec::new();
        for &token in &tokens[1..] {
            if token.starts_with('-') {
                // cross-env's own flag, e.g. `--silent`; skip and keep
                // looking for assignments.
                continue;
            }
            match split_assignment(token) {
                Some(pair) => assignments.push(pair),
                None => break,
            }
        }
        return assignments;
    }

    if first == "set" {
        return match tokens.get(1).and_then(|token| split_assignment(token)) {
            Some(pair) => vec![pair],
            None => Vec::new(),
        };
    }

    let mut assignments = Vec::new();
    for &token in &tokens {
        match split_assignment(token) {
            Some(pair) => assignments.push(pair),
            None => break,
        }
    }
    assignments
}

/// Split `token` into `(key, value)` if it looks like `KEY=value`, where
/// `KEY` matches `[A-Za-z_][A-Za-z0-9_.]*` and the split is on the FIRST
/// `=`. Returns `None` for a token with no `=`, or whose text before the
/// first `=` isn't a valid key (e.g. `next` has no `=` at all; `&&` isn't a
/// valid key either).
fn split_assignment(token: &str) -> Option<(String, String)> {
    let eq_idx = token.find('=')?;
    let key = &token[..eq_idx];
    if !is_valid_key(key) {
        return None;
    }
    Some((key.to_string(), token[eq_idx + 1..].to_string()))
}

/// `^[A-Za-z_][A-Za-z0-9_.]*$`, checked by hand (same grammar as the dotenv
/// parser's key pattern).
fn is_valid_key(candidate: &str) -> bool {
    let mut chars = candidate.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Best-effort 1-indexed line number for the `scripts.<name>` entry: a
/// raw-text scan for the literal `"<name>":`. Can in principle match an
/// unrelated same-named key elsewhere in the file; `None` if not found at
/// all.
fn find_script_line(content: &str, name: &str) -> Option<u32> {
    let needle = format!("\"{name}\":");
    content
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(&needle))
        .map(|(idx, _)| (idx + 1) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_assignments() {
        let content = r#"{"scripts":{"dev":"NODE_ENV=development PORT=3000 next dev"}}"#;
        let (scripts, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].script, "dev");
        let pairs: Vec<(String, String)> = scripts[0]
            .entries
            .iter()
            .map(|(k, v, _)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("NODE_ENV".to_string(), "development".to_string()),
                ("PORT".to_string(), "3000".to_string()),
            ]
        );
    }

    #[test]
    fn cross_env() {
        let content = r#"{"scripts":{"test":"cross-env NODE_ENV=test jest"}}"#;
        let (scripts, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].script, "test");
        let pairs: Vec<(String, String)> = scripts[0]
            .entries
            .iter()
            .map(|(k, v, _)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(pairs, vec![("NODE_ENV".to_string(), "test".to_string())]);
    }

    #[test]
    fn cross_env_skips_its_own_flags() {
        let content = r#"{"scripts":{"test":"cross-env --silent NODE_ENV=test jest"}}"#;
        let (scripts, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(scripts.len(), 1);
        let pairs: Vec<(String, String)> = scripts[0]
            .entries
            .iter()
            .map(|(k, v, _)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(pairs, vec![("NODE_ENV".to_string(), "test".to_string())]);
    }

    #[test]
    fn windows_set() {
        let content = r#"{"scripts":{"win":"set NODE_ENV=production && node app"}}"#;
        let (scripts, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].script, "win");
        let pairs: Vec<(String, String)> = scripts[0]
            .entries
            .iter()
            .map(|(k, v, _)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(
            pairs,
            vec![("NODE_ENV".to_string(), "production".to_string())]
        );
    }

    #[test]
    fn no_assignments_no_subsource() {
        let content = r#"{"scripts":{"lint":"eslint ."}}"#;
        let (scripts, errors) = parse(content);

        assert!(errors.is_empty());
        assert!(
            scripts.is_empty(),
            "a script with zero env assignments must not appear in results, got {scripts:?}"
        );
    }

    #[test]
    fn not_an_assignment_midway() {
        let content = r#"{"scripts":{"x":"node FOO=bar"}}"#;
        let (scripts, errors) = parse(content);

        assert!(errors.is_empty());
        assert!(
            scripts.is_empty(),
            "assignments must lead the script; 'node' is the command, got {scripts:?}"
        );
    }

    #[test]
    fn scripts_in_document_order() {
        // Keys are deliberately NOT alphabetical ("zebra" before "apple"):
        // if the `preserve_order` feature on `serde_json` were ever dropped,
        // this test would fail because the map would iterate alphabetically
        // instead of in document order. That's the point of the test.
        let content = r#"{"scripts":{"zebra":"NODE_ENV=z zcmd","apple":"NODE_ENV=a acmd"}}"#;
        let (scripts, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(scripts.len(), 2);
        assert_eq!(scripts[0].script, "zebra");
        assert_eq!(scripts[1].script, "apple");
    }

    #[test]
    fn line_numbers_located_by_raw_scan() {
        // Same non-alphabetical ordering as `scripts_in_document_order`, so
        // this test also guards `preserve_order` rather than passing by
        // coincidence.
        let content = "{\n  \"scripts\": {\n    \"zebra\": \"NODE_ENV=z zcmd\",\n    \"apple\": \"NODE_ENV=a acmd\"\n  }\n}\n";
        let (scripts, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(scripts.len(), 2);
        assert_eq!(scripts[0].script, "zebra");
        assert_eq!(scripts[0].entries[0].2, Some(3));
        assert_eq!(scripts[1].script, "apple");
        assert_eq!(scripts[1].entries[0].2, Some(4));
    }

    #[test]
    fn invalid_json_is_one_error() {
        let content = "{ this is not json ";
        let (scripts, errors) = parse(content);

        assert!(scripts.is_empty());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, None);
    }

    #[test]
    fn no_scripts_key_is_not_an_error() {
        let content = r#"{"name":"pkg"}"#;
        let (scripts, errors) = parse(content);

        assert!(scripts.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn only_assignments_no_command() {
        let content = r#"{"scripts":{"env":"NODE_ENV=production"}}"#;
        let (scripts, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(scripts.len(), 1);
        let pairs: Vec<(String, String)> = scripts[0]
            .entries
            .iter()
            .map(|(k, v, _)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(
            pairs,
            vec![("NODE_ENV".to_string(), "production".to_string())]
        );
    }

    #[test]
    fn empty_value_assignment() {
        let content = r#"{"scripts":{"x":"FOO= node app"}}"#;
        let (scripts, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(scripts.len(), 1);
        let pairs: Vec<(String, String)> = scripts[0]
            .entries
            .iter()
            .map(|(k, v, _)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(pairs, vec![("FOO".to_string(), String::new())]);
    }

    #[test]
    fn cross_env_without_assignment() {
        let content = r#"{"scripts":{"x":"cross-env jest"}}"#;
        let (scripts, errors) = parse(content);

        assert!(errors.is_empty());
        assert!(
            scripts.is_empty(),
            "cross-env with no leading assignment must omit the script, got {scripts:?}"
        );
    }

    #[test]
    fn set_without_assignment() {
        let content = r#"{"scripts":{"x":"set && node"}}"#;
        let (scripts, errors) = parse(content);

        assert!(errors.is_empty());
        assert!(
            scripts.is_empty(),
            "set with no assignment token must omit the script, got {scripts:?}"
        );
    }

    /// Documents a known v0.1 limitation: the tokenizer is a whitespace
    /// splitter, not a shell parser, so it has no concept of quoting. A
    /// quoted value containing a space (`FOO="a b" cmd`) is truncated at the
    /// first whitespace, leaving the leading quote character in the value
    /// (`"a`). This test locks that behavior in place so that changing it
    /// later is a conscious decision, not an accidental regression.
    #[test]
    fn quoted_value_truncated_documented() {
        let content = r#"{"scripts":{"x":"FOO=\"a b\" cmd"}}"#;
        let (scripts, errors) = parse(content);

        assert!(errors.is_empty());
        assert_eq!(scripts.len(), 1);
        let pairs: Vec<(String, String)> = scripts[0]
            .entries
            .iter()
            .map(|(k, v, _)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(pairs, vec![("FOO".to_string(), "\"a".to_string())]);
    }
}
