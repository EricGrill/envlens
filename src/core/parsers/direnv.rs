//! direnv `.envrc` parser (issue #9).
//!
//! `.envrc` files are shell scripts, so we only extract the literal
//! assignments that map cleanly to environment variables:
//!
//! - `export KEY=value`
//! - `KEY=value`
//!
//! Everything else — `PATH_add bin`, `layout python`, `use nix`, `source_up`,
//! values built from command substitution (`$(...)` / backticks) — is skipped
//! silently, without producing a parse error, because it is expected shell and
//! not a defect. This keeps `.envrc` support best-effort and noise-free.

/// A single literal assignment from a `.envrc`, as
/// `(key, value, 1-indexed line)`.
pub type DirenvEntry = (String, String, Option<u32>);

/// Parse `.envrc` `content`, returning only the literal `KEY=value`
/// assignments. Non-literal shell lines are skipped without error.
pub fn parse(content: &str) -> Vec<DirenvEntry> {
    let mut entries = Vec::new();

    for (idx, raw) in content.split('\n').enumerate() {
        let line_no = (idx + 1) as u32;
        let line = raw.strip_suffix('\r').unwrap_or(raw).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Optional leading `export ` (or `export\t`).
        let assignment = match line.strip_prefix("export") {
            Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim_start(),
            Some(_) => continue, // e.g. `exportfoo` — not an export statement
            None => line,
        };

        let Some((key, raw_value)) = assignment.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !is_valid_key(key) {
            continue;
        }

        let value = raw_value.trim();
        if is_non_literal(value) {
            continue;
        }
        entries.push((key.to_string(), unquote(value), Some(line_no)));
    }

    entries
}

/// Skip values that are not plain literals: command substitution, backticks,
/// or variable expansion we can't resolve statically.
fn is_non_literal(value: &str) -> bool {
    value.contains("$(") || value.contains('`')
}

/// Strip a single matching pair of surrounding quotes.
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

fn is_valid_key(key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
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
    fn export_and_bare_assignments() {
        assert_eq!(
            keyvals("export FOO=bar\nBAZ=qux\n"),
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "qux".to_string()),
            ]
        );
    }

    #[test]
    fn quoted_values_unquoted() {
        assert_eq!(
            keyvals("export MSG=\"hello world\"\nexport NAME='eric'\n"),
            vec![
                ("MSG".to_string(), "hello world".to_string()),
                ("NAME".to_string(), "eric".to_string()),
            ]
        );
    }

    #[test]
    fn skips_shell_directives_without_error() {
        let content = "\
layout python
use nix
PATH_add bin
source_up
export REAL=1
";
        assert_eq!(
            keyvals(content),
            vec![("REAL".to_string(), "1".to_string())]
        );
    }

    #[test]
    fn skips_command_substitution_values() {
        let content = "export DYNAMIC=$(date)\nexport BACKTICK=`whoami`\nexport STATIC=ok\n";
        assert_eq!(
            keyvals(content),
            vec![("STATIC".to_string(), "ok".to_string())]
        );
    }

    #[test]
    fn line_numbers_are_reported() {
        let content = "# comment\n\nexport A=1\nlayout ruby\nexport B=2\n";
        assert_eq!(
            parse(content),
            vec![
                ("A".to_string(), "1".to_string(), Some(3)),
                ("B".to_string(), "2".to_string(), Some(5)),
            ]
        );
    }

    #[test]
    fn never_panics_on_odd_input() {
        for input in [
            "",
            "export",
            "export ",
            "=",
            "export =x",
            "9BAD=1",
            "export\t",
        ] {
            let _ = parse(input);
        }
    }
}
