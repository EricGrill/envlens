//! Hand-rolled dotenv grammar parser (spec FR-010..014).
//!
//! Deliberately not regex-driven beyond the key pattern: multi-line
//! double-quoted values require a manual line cursor that can consume
//! following physical lines, which a single regex pass over the whole file
//! can't express cleanly. Parsing never panics and never aborts on a
//! malformed line — each bad line contributes a [`ParseError`] and parsing
//! continues with the next line, so callers always get partial results.

use crate::core::model::ParseError;

/// A single `KEY=value` assignment parsed out of a dotenv file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DotenvEntry {
    pub key: String,
    /// Original text of the value, unprocessed (quotes included; spans
    /// embedded newlines verbatim for multi-line double-quoted values).
    pub raw_value: String,
    /// Post-quote/escape-processing value, pre-`${VAR}`-expansion.
    pub parsed_value: String,
    /// 1-indexed line on which the entry's `KEY=` starts.
    pub line: u32,
    /// `true` for single-quoted values: no `${VAR}`/`$VAR` expansion later.
    pub no_expand: bool,
}

/// Private-use sentinel standing in for an escaped `\$` inside a
/// double-quoted value, so a later expansion pass (resolve.rs) can tell a
/// real `$` — meant to start a reference — apart from one the author
/// explicitly escaped. Restored to `$` after expansion runs.
///
/// Any occurrence of this exact character already present in the input is
/// stripped up front so the sentinel stays airtight (a documented, narrow
/// limitation: a file that legitimately contains U+E000 will silently lose
/// it).
const SENTINEL: char = '\u{E000}';

/// Parse dotenv-format `content` into entries plus any per-line errors.
/// Parsing is best-effort: a malformed line contributes a [`ParseError`]
/// and parsing continues with the rest of the file.
pub fn parse(content: &str) -> (Vec<DotenvEntry>, Vec<ParseError>) {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    let cleaned: String = content.chars().filter(|&c| c != SENTINEL).collect();

    let physical_lines: Vec<&str> = cleaned
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();

    let mut entries = Vec::new();
    let mut errors = Vec::new();

    let mut i = 0usize;
    while i < physical_lines.len() {
        let line_no = (i + 1) as u32;
        let line = physical_lines[i];
        let after_indent = line.trim_start();

        if after_indent.is_empty() || after_indent.starts_with('#') {
            i += 1;
            continue;
        }

        let mut working = after_indent;
        if let Some(rest) = working.strip_prefix("export")
            && rest.starts_with(|c: char| c.is_whitespace())
        {
            working = rest.trim_start();
        }

        let Some(eq_idx) = working.find('=') else {
            errors.push(ParseError {
                line: Some(line_no),
                message: format!("missing '=' (expected KEY=VALUE): '{}'", working.trim_end()),
            });
            i += 1;
            continue;
        };

        let key_candidate = working[..eq_idx].trim_end();
        if !is_valid_key(key_candidate) {
            errors.push(ParseError {
                line: Some(line_no),
                message: format!(
                    "invalid key '{key_candidate}' (keys match [A-Za-z_][A-Za-z0-9_.]*)"
                ),
            });
            i += 1;
            continue;
        }
        let key = key_candidate.to_string();

        let after_eq = &working[eq_idx + 1..];
        let value_part = after_eq.trim_start();
        // Safe: `value_part` is derived from `line` purely via `trim_start`,
        // `strip_prefix`, and range-indexing — all non-copying subslice
        // operations — so it shares `line`'s allocation and this offset is
        // a valid byte index into `line`.
        let value_col = value_part.as_ptr() as usize - line.as_ptr() as usize;

        if value_part.starts_with('"') {
            let start_col = value_col + 1;
            match scan_double_quoted(&physical_lines, i, start_col) {
                Some((raw, parsed, end_line, end_col)) => {
                    check_trailing(
                        physical_lines[end_line],
                        end_col,
                        line_no,
                        &key,
                        &mut errors,
                    );
                    entries.push(DotenvEntry {
                        key,
                        raw_value: raw,
                        parsed_value: parsed,
                        line: line_no,
                        no_expand: false,
                    });
                    i = end_line + 1;
                }
                None => {
                    errors.push(ParseError {
                        line: Some(line_no),
                        message: format!(
                            "unterminated double-quoted value for key '{key}': missing closing '\"'"
                        ),
                    });
                    i = physical_lines.len();
                }
            }
            continue;
        }

        if value_part.starts_with('\'') {
            let start_col = value_col + 1;
            match scan_single_quoted(line, start_col) {
                Some((raw, parsed, end_col)) => {
                    check_trailing(line, end_col, line_no, &key, &mut errors);
                    entries.push(DotenvEntry {
                        key,
                        raw_value: raw,
                        parsed_value: parsed,
                        line: line_no,
                        no_expand: true,
                    });
                }
                None => {
                    errors.push(ParseError {
                        line: Some(line_no),
                        message: format!(
                            "unterminated single-quoted value for key '{key}': missing closing '\''"
                        ),
                    });
                }
            }
            i += 1;
            continue;
        }

        let value = extract_unquoted_value(after_eq);
        entries.push(DotenvEntry {
            key,
            raw_value: value.clone(),
            parsed_value: value,
            line: line_no,
            no_expand: false,
        });
        i += 1;
    }

    (entries, errors)
}

/// `^[A-Za-z_][A-Za-z0-9_.]*$`, checked by hand rather than via `regex`
/// (only the key pattern is speced as a regex candidate; a manual check
/// avoids introducing a fallible `Regex::new` for a single fixed pattern).
fn is_valid_key(candidate: &str) -> bool {
    let mut chars = candidate.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Unquoted value: trim whitespace, and treat a `#` as starting a comment
/// only when it's preceded by whitespace (spec: `K=a#b` keeps `a#b`, but
/// `PORT=3000 # local` keeps only `3000`).
fn extract_unquoted_value(after_eq: &str) -> String {
    let bytes = after_eq.as_bytes();
    let mut comment_at = None;
    for (idx, &b) in bytes.iter().enumerate() {
        if b == b'#' && idx > 0 && matches!(bytes[idx - 1], b' ' | b'\t') {
            comment_at = Some(idx);
            break;
        }
    }
    let value_span = match comment_at {
        Some(idx) => &after_eq[..idx],
        None => after_eq,
    };
    value_span.trim().to_string()
}

/// Scan a double-quoted value starting just past its opening `"` at
/// `physical_lines[start_line][start_col]`. Consumes further physical lines
/// (joined with a real `\n`) until an unescaped closing `"` is found.
/// Returns `(raw_value, parsed_value, end_line, end_col)` on success, where
/// `end_col` is the byte offset in `physical_lines[end_line]` immediately
/// after the closing quote. `None` means the quote was never closed before
/// end of input.
fn scan_double_quoted(
    physical_lines: &[&str],
    start_line: usize,
    start_col: usize,
) -> Option<(String, String, usize, usize)> {
    let mut raw = String::from("\"");
    let mut parsed = String::new();
    let mut line_idx = start_line;
    let mut col = start_col;

    loop {
        let line = physical_lines[line_idx];
        let chars: Vec<char> = line[col..].chars().collect();
        let mut idx = 0;
        let mut closed = false;

        while idx < chars.len() {
            let c = chars[idx];
            if c == '"' {
                raw.push('"');
                idx += 1;
                closed = true;
                break;
            } else if c == '\\' && idx + 1 < chars.len() {
                let next = chars[idx + 1];
                raw.push('\\');
                raw.push(next);
                match next {
                    'n' => parsed.push('\n'),
                    't' => parsed.push('\t'),
                    '"' => parsed.push('"'),
                    '\\' => parsed.push('\\'),
                    '$' => parsed.push(SENTINEL),
                    other => {
                        parsed.push('\\');
                        parsed.push(other);
                    }
                }
                idx += 2;
            } else {
                raw.push(c);
                parsed.push(c);
                idx += 1;
            }
        }

        if closed {
            let consumed_bytes: usize = chars[..idx].iter().map(|c| c.len_utf8()).sum();
            return Some((raw, parsed, line_idx, col + consumed_bytes));
        }

        if line_idx + 1 >= physical_lines.len() {
            return None;
        }
        raw.push('\n');
        parsed.push('\n');
        line_idx += 1;
        col = 0;
    }
}

/// Scan a single-quoted value starting just past its opening `'` at
/// `line[start_col]`. Single-quoted values are single-line and have no
/// escapes at all — `\` is literal. Returns `(raw_value, parsed_value,
/// end_col)` on success, `None` if `line` has no closing `'` after
/// `start_col`.
fn scan_single_quoted(line: &str, start_col: usize) -> Option<(String, String, usize)> {
    let rest = &line[start_col..];
    let rel_idx = rest.find('\'')?;
    let inner = &rest[..rel_idx];
    let raw = format!("'{inner}'");
    let end_col = start_col + rel_idx + 1;
    Some((raw, inner.to_string(), end_col))
}

/// After a closing quote, only trailing whitespace and an optional `#
/// comment` are allowed; anything else is trailing junk. The caller keeps
/// the entry either way — this only records a [`ParseError`].
fn check_trailing(
    line: &str,
    end_col: usize,
    line_no: u32,
    key: &str,
    errors: &mut Vec<ParseError>,
) {
    let trailing = line[end_col..].trim_start();
    if trailing.is_empty() || trailing.starts_with('#') {
        return;
    }
    errors.push(ParseError {
        line: Some(line_no),
        message: format!(
            "unexpected trailing content '{}' after quoted value for key '{key}'",
            trailing.trim_end()
        ),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, raw: &str, parsed: &str, line: u32, no_expand: bool) -> DotenvEntry {
        DotenvEntry {
            key: key.to_string(),
            raw_value: raw.to_string(),
            parsed_value: parsed.to_string(),
            line,
            no_expand,
        }
    }

    #[test]
    fn plain_assignment() {
        let (entries, errors) = parse("KEY=value");
        assert_eq!(entries, vec![entry("KEY", "value", "value", 1, false)]);
        assert!(errors.is_empty());
    }

    #[test]
    fn export_prefix() {
        let (entries, errors) = parse("export KEY=value");
        assert_eq!(entries, vec![entry("KEY", "value", "value", 1, false)]);
        assert!(errors.is_empty());
    }

    #[test]
    fn whitespace_around_equals() {
        let (entries, errors) = parse("KEY = value");
        assert_eq!(entries, vec![entry("KEY", "value", "value", 1, false)]);
        assert!(errors.is_empty());
    }

    #[test]
    fn empty_value() {
        let (entries, errors) = parse("KEY=");
        assert_eq!(entries, vec![entry("KEY", "", "", 1, false)]);
        assert!(errors.is_empty());
    }

    #[test]
    fn double_quoted_value() {
        let (entries, errors) = parse(r#"K="a b""#);
        assert_eq!(entries, vec![entry("K", "\"a b\"", "a b", 1, false)]);
        assert!(errors.is_empty());
    }

    #[test]
    fn double_quoted_escapes() {
        // Written as a raw string so the parser receives literal
        // backslash-n / backslash-t / etc., not real control characters.
        let src = r#"K="l1\nl2\t\"q\" \\ \$HOME""#;
        let (entries, errors) = parse(src);

        // `\n` -> real newline, `\t` -> real tab, `\"` -> literal quote,
        // `\\` -> literal backslash, `\$` -> U+E000 sentinel (restored to
        // `$` post-expansion by resolve.rs in a later task).
        let expected_parsed = "l1\nl2\t\"q\" \\ \u{E000}HOME";
        // raw_value is the untouched original text, backslashes and all.
        let expected_raw = r#""l1\nl2\t\"q\" \\ \$HOME""#;

        assert_eq!(
            entries,
            vec![entry("K", expected_raw, expected_parsed, 1, false)]
        );
        assert!(entries[0].parsed_value.contains('\u{E000}'));
        assert!(errors.is_empty());
    }

    #[test]
    fn single_quoted_is_literal_and_marks_no_expand() {
        let (entries, errors) = parse("K='$NOT ${EXP}'");
        assert_eq!(
            entries,
            vec![entry("K", "'$NOT ${EXP}'", "$NOT ${EXP}", 1, true)]
        );
        assert!(errors.is_empty());
    }

    #[test]
    fn inline_comment_after_whitespace_is_stripped() {
        let (entries, errors) = parse("PORT=3000 # local");
        assert_eq!(entries, vec![entry("PORT", "3000", "3000", 1, false)]);
        assert!(errors.is_empty());
    }

    #[test]
    fn hash_without_preceding_whitespace_is_literal() {
        let (entries, errors) = parse("K=a#b");
        assert_eq!(entries, vec![entry("K", "a#b", "a#b", 1, false)]);
        assert!(errors.is_empty());
    }

    #[test]
    fn full_line_comment_and_blank_lines_produce_nothing() {
        let (entries, errors) = parse("# hi\n\n");
        assert!(entries.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn multiline_double_quoted_value_spans_lines_and_anchors_opening_line() {
        // A REAL newline inside the quotes this time (not an escaped one).
        let content = "K=\"l1\nl2\"";
        let (entries, errors) = parse(content);
        assert_eq!(entries, vec![entry("K", "\"l1\nl2\"", "l1\nl2", 1, false)]);
        assert!(errors.is_empty());
    }

    #[test]
    fn crlf_and_bom_are_stripped() {
        let content = "\u{FEFF}KEY=v\r\n";
        let (entries, errors) = parse(content);
        assert_eq!(entries, vec![entry("KEY", "v", "v", 1, false)]);
        assert!(errors.is_empty());
    }

    #[test]
    fn unicode_value_is_preserved() {
        let (entries, errors) = parse("K=héllo→");
        assert_eq!(entries, vec![entry("K", "héllo→", "héllo→", 1, false)]);
        assert!(errors.is_empty());
    }

    #[test]
    fn invalid_key_with_space_produces_no_entry_and_one_error() {
        let (entries, errors) = parse("DATABASE URL=abc");
        assert!(entries.is_empty());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, Some(1));
        assert_eq!(
            errors[0].message,
            "invalid key 'DATABASE URL' (keys match [A-Za-z_][A-Za-z0-9_.]*)"
        );
    }

    #[test]
    fn missing_key_produces_one_error() {
        let (entries, errors) = parse("=missing");
        assert!(entries.is_empty());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, Some(1));
    }

    #[test]
    fn missing_equals_produces_one_error() {
        let (entries, errors) = parse("KEY");
        assert!(entries.is_empty());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, Some(1));
    }

    #[test]
    fn unterminated_double_quote_anchors_error_at_opening_line() {
        let (entries, errors) = parse("K=\"abc");
        assert!(entries.is_empty());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, Some(1));
    }

    #[test]
    fn unterminated_single_quote_produces_one_error() {
        let (entries, errors) = parse("K='abc");
        assert!(entries.is_empty());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, Some(1));
    }

    #[test]
    fn trailing_junk_after_quote_keeps_entry_and_errors() {
        let (entries, errors) = parse(r#"K="a"b"#);
        assert_eq!(entries, vec![entry("K", "\"a\"", "a", 1, false)]);
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn duplicate_keys_both_returned() {
        let (entries, errors) = parse("A=1\nA=2");
        assert_eq!(
            entries,
            vec![
                entry("A", "1", "1", 1, false),
                entry("A", "2", "2", 2, false),
            ]
        );
        assert!(errors.is_empty());
    }

    #[test]
    fn ten_thousand_char_line_parses_without_panicking() {
        let value = "x".repeat(10_000);
        let content = format!("K={value}");
        let (entries, errors) = parse(&content);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].parsed_value, value);
        assert_eq!(entries[0].raw_value, value);
        assert!(errors.is_empty());
    }

    #[test]
    fn raw_value_preserves_quotes() {
        let (entries, errors) = parse(r#"K="a b""#);
        assert_eq!(entries[0].raw_value, "\"a b\"");
        assert!(errors.is_empty());
    }

    #[test]
    fn preexisting_sentinel_char_is_stripped() {
        // Grammar footnote: any U+E000 already in the input is stripped so
        // the `\$` sentinel stays airtight.
        let content = format!("K=a{SENTINEL}b");
        let (entries, errors) = parse(&content);
        assert_eq!(entries, vec![entry("K", "ab", "ab", 1, false)]);
        assert!(errors.is_empty());
    }
}
