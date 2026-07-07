//! `envlens diff` (issue #7): compare the effective environment produced by
//! two selectors — two profiles or two individual sources — reusing the same
//! precedence/resolution engine the rest of envlens uses. Secret-like values
//! are masked in every output path, exactly like `report`.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::core::model::Analysis;
use crate::core::secrets::{MaskedValue, classify_value};

/// How a single key changed between the two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Removed,
    Changed,
    Unchanged,
}

impl ChangeKind {
    fn sigil(self) -> char {
        match self {
            ChangeKind::Added => '+',
            ChangeKind::Removed => '-',
            ChangeKind::Changed => '~',
            ChangeKind::Unchanged => '=',
        }
    }

    fn color_code(self) -> &'static str {
        match self {
            ChangeKind::Added => "32",
            ChangeKind::Removed => "31",
            ChangeKind::Changed => "33",
            ChangeKind::Unchanged => "0",
        }
    }
}

/// One key's before/after across the two sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    pub key: String,
    pub kind: ChangeKind,
    /// Effective value on the left side, if the key resolves there.
    pub left: Option<String>,
    pub right: Option<String>,
    /// True when either side's value is secret-like; drives masking.
    pub secret: bool,
}

/// The full comparison plus human labels for each side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffResult {
    pub left_label: String,
    pub right_label: String,
    pub entries: Vec<DiffEntry>,
}

impl DiffResult {
    /// True when at least one key was added, removed, or changed.
    pub fn has_changes(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.kind != ChangeKind::Unchanged)
    }
}

/// Build the effective-value map for one analysis: key -> (value, secret?).
fn effective_map(analysis: &Analysis) -> BTreeMap<String, (String, bool)> {
    analysis
        .variables
        .iter()
        .filter_map(|variable| {
            variable.effective.as_ref().map(|(value, _source)| {
                let secret = variable.is_secret_like || classify_value(value);
                (variable.key.clone(), (value.clone(), secret))
            })
        })
        .collect()
}

/// Compare two analyses' effective values.
pub fn compute(
    left: &Analysis,
    right: &Analysis,
    left_label: impl Into<String>,
    right_label: impl Into<String>,
) -> DiffResult {
    let left_map = effective_map(left);
    let right_map = effective_map(right);

    let mut keys: Vec<&String> = left_map.keys().chain(right_map.keys()).collect();
    keys.sort();
    keys.dedup();

    let entries = keys
        .into_iter()
        .map(|key| {
            let left_value = left_map.get(key);
            let right_value = right_map.get(key);
            let secret = left_value.map(|(_, s)| *s).unwrap_or(false)
                || right_value.map(|(_, s)| *s).unwrap_or(false);
            let kind = match (left_value, right_value) {
                (None, Some(_)) => ChangeKind::Added,
                (Some(_), None) => ChangeKind::Removed,
                (Some((l, _)), Some((r, _))) if l != r => ChangeKind::Changed,
                _ => ChangeKind::Unchanged,
            };
            DiffEntry {
                key: key.clone(),
                kind,
                left: left_value.map(|(v, _)| v.clone()),
                right: right_value.map(|(v, _)| v.clone()),
                secret,
            }
        })
        .collect();

    DiffResult {
        left_label: left_label.into(),
        right_label: right_label.into(),
        entries,
    }
}

fn shown_value(value: &str, secret: bool) -> String {
    MaskedValue::new(value, secret, false).to_string()
}

/// Render the human-readable diff. `all` includes unchanged keys; `no_values`
/// hides values entirely (only keys and change kind are shown).
pub fn render_human(result: &DiffResult, all: bool, no_values: bool, color: bool) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "--- {} (left)\n+++ {} (right)\n",
        result.left_label, result.right_label
    ));

    let mut added = 0usize;
    let mut removed = 0usize;
    let mut changed = 0usize;

    for entry in &result.entries {
        match entry.kind {
            ChangeKind::Added => added += 1,
            ChangeKind::Removed => removed += 1,
            ChangeKind::Changed => changed += 1,
            ChangeKind::Unchanged => {}
        }
        if entry.kind == ChangeKind::Unchanged && !all {
            continue;
        }
        let sigil = entry.kind.sigil();
        let body = if no_values {
            entry.key.clone()
        } else {
            match entry.kind {
                ChangeKind::Added => format!(
                    "{} = {}",
                    entry.key,
                    shown_value(entry.right.as_deref().unwrap_or(""), entry.secret)
                ),
                ChangeKind::Removed => format!(
                    "{} = {}",
                    entry.key,
                    shown_value(entry.left.as_deref().unwrap_or(""), entry.secret)
                ),
                ChangeKind::Changed => format!(
                    "{} = {} -> {}",
                    entry.key,
                    shown_value(entry.left.as_deref().unwrap_or(""), entry.secret),
                    shown_value(entry.right.as_deref().unwrap_or(""), entry.secret)
                ),
                ChangeKind::Unchanged => format!(
                    "{} = {}",
                    entry.key,
                    shown_value(entry.left.as_deref().unwrap_or(""), entry.secret)
                ),
            }
        };
        let line = format!("{sigil} {body}");
        if color {
            output.push_str(&format!("\x1b[{}m{line}\x1b[0m\n", entry.kind.color_code()));
        } else {
            output.push_str(&line);
            output.push('\n');
        }
    }

    output.push_str(&format!(
        "\nsummary: {added} added, {removed} removed, {changed} changed\n"
    ));
    output
}

#[derive(Serialize)]
struct JsonEntry {
    key: String,
    kind: ChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    left: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    right: Option<String>,
}

#[derive(Serialize)]
struct JsonSummary {
    added: usize,
    removed: usize,
    changed: usize,
    unchanged: usize,
}

#[derive(Serialize)]
struct JsonDiff {
    left: String,
    right: String,
    changes: Vec<JsonEntry>,
    summary: JsonSummary,
}

/// Render the diff as JSON. Secret values are masked; `no_values` drops the
/// value fields entirely. Unchanged keys are always omitted from JSON.
pub fn render_json(result: &DiffResult, no_values: bool) -> Result<String, serde_json::Error> {
    let mut summary = JsonSummary {
        added: 0,
        removed: 0,
        changed: 0,
        unchanged: 0,
    };
    let mut changes = Vec::new();
    for entry in &result.entries {
        match entry.kind {
            ChangeKind::Added => summary.added += 1,
            ChangeKind::Removed => summary.removed += 1,
            ChangeKind::Changed => summary.changed += 1,
            ChangeKind::Unchanged => {
                summary.unchanged += 1;
                continue;
            }
        }
        let (left, right) = if no_values {
            (None, None)
        } else {
            (
                entry
                    .left
                    .as_ref()
                    .map(|value| shown_value(value, entry.secret)),
                entry
                    .right
                    .as_ref()
                    .map(|value| shown_value(value, entry.secret)),
            )
        };
        changes.push(JsonEntry {
            key: entry.key.clone(),
            kind: entry.kind,
            left,
            right,
        });
    }

    let doc = JsonDiff {
        left: result.left_label.clone(),
        right: result.right_label.clone(),
        changes,
        summary,
    };
    serde_json::to_string_pretty(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{SourceId, VariableSummary};

    fn analysis_with(vars: &[(&str, &str, bool)]) -> Analysis {
        let variables = vars
            .iter()
            .map(|(key, value, secret)| VariableSummary {
                key: key.to_string(),
                effective: Some((value.to_string(), "src".to_string() as SourceId)),
                occurrences: Vec::new(),
                diagnostics: Vec::new(),
                is_required: false,
                is_missing: false,
                is_secret_like: *secret,
            })
            .collect();
        Analysis {
            root: std::path::PathBuf::from("."),
            profile: "default".to_string(),
            sources: Vec::new(),
            variables,
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn classifies_added_removed_changed_unchanged() {
        let left = analysis_with(&[
            ("SAME", "1", false),
            ("GONE", "x", false),
            ("MOD", "a", false),
        ]);
        let right = analysis_with(&[
            ("SAME", "1", false),
            ("NEW", "y", false),
            ("MOD", "b", false),
        ]);

        let result = compute(&left, &right, "left", "right");
        let kinds: Vec<(&str, ChangeKind)> = result
            .entries
            .iter()
            .map(|entry| (entry.key.as_str(), entry.kind))
            .collect();

        assert_eq!(
            kinds,
            vec![
                ("GONE", ChangeKind::Removed),
                ("MOD", ChangeKind::Changed),
                ("NEW", ChangeKind::Added),
                ("SAME", ChangeKind::Unchanged),
            ]
        );
        assert!(result.has_changes());
    }

    #[test]
    fn identical_sides_have_no_changes() {
        let left = analysis_with(&[("A", "1", false)]);
        let right = analysis_with(&[("A", "1", false)]);
        assert!(!compute(&left, &right, "l", "r").has_changes());
    }

    #[test]
    fn secret_values_are_masked_in_human_output() {
        let left = analysis_with(&[("TOKEN", "supersecretvalue123", true)]);
        let right = analysis_with(&[("TOKEN", "differentsecret456", true)]);
        let result = compute(&left, &right, "l", "r");
        let text = render_human(&result, false, false, false);
        assert!(!text.contains("supersecretvalue123"));
        assert!(!text.contains("differentsecret456"));
        assert!(text.contains('•'));
    }

    #[test]
    fn no_values_hides_values_but_shows_keys() {
        let left = analysis_with(&[("PLAIN", "one", false)]);
        let right = analysis_with(&[("PLAIN", "two", false)]);
        let result = compute(&left, &right, "l", "r");
        let text = render_human(&result, false, true, false);
        assert!(text.contains("PLAIN"));
        assert!(!text.contains("one"));
        assert!(!text.contains("two"));
    }

    #[test]
    fn json_omits_unchanged_and_masks_secrets() {
        let left = analysis_with(&[("A", "1", false), ("T", "supersecretvalue123", true)]);
        let right = analysis_with(&[("A", "1", false), ("T", "othersecretvalue456", true)]);
        let result = compute(&left, &right, "l", "r");
        let json = render_json(&result, false).unwrap();
        assert!(!json.contains("\"A\""));
        assert!(!json.contains("supersecretvalue123"));
        assert!(json.contains("\"changed\""));
    }
}
