//! `envlens sync` (issue #8): turn the "key present in `.env` but missing from
//! `.env.example`" diagnostic into a fix. Missing keys are appended to the
//! example template **with their values stripped** — sync never writes a real
//! value (and therefore never a secret) into a tracked example file, and never
//! modifies or reorders existing lines.
//!
//! Scoping is per-directory: an example file receives only the keys from the
//! `.env*` files that live beside it. Running sync twice is a no-op.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::core::model::{Analysis, SourceKind};

/// One example file that would gain keys (or be created).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Root-relative path of the example file.
    pub path: PathBuf,
    /// Keys to append, sorted, values stripped.
    pub added_keys: Vec<String>,
    /// True when the file does not yet exist and would be created.
    pub created: bool,
}

/// The set of example-file edits sync would make.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncPlan {
    pub changes: Vec<FileChange>,
}

impl SyncPlan {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Directory key used to group `.env*` files with the example beside them.
fn dir_of(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// Compute the sync plan from an analysis: which keys each `.env.example`
/// (`.sample`/`.template`) is missing relative to the real `.env*` files in the
/// same directory. If no example file exists anywhere, plan to create a root
/// `.env.example` from every discovered dotenv key.
pub fn plan(analysis: &Analysis) -> SyncPlan {
    let kind_by_id: BTreeMap<&str, SourceKind> = analysis
        .sources
        .iter()
        .map(|source| (source.id.as_str(), source.kind))
        .collect();
    let path_by_id: BTreeMap<&str, &Path> = analysis
        .sources
        .iter()
        .filter_map(|source| {
            source
                .path
                .as_deref()
                .map(|path| (source.id.as_str(), path))
        })
        .collect();

    // Real dotenv keys grouped by directory.
    let mut dotenv_keys: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    // Keys already present in each example source (by source id).
    let mut example_keys: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for variable in &analysis.variables {
        for occurrence in &variable.occurrences {
            match kind_by_id.get(occurrence.source_id.as_str()) {
                Some(SourceKind::Dotenv) => {
                    if let Some(path) = path_by_id.get(occurrence.source_id.as_str()) {
                        dotenv_keys
                            .entry(dir_of(path))
                            .or_default()
                            .insert(occurrence.key.clone());
                    }
                }
                Some(SourceKind::DotenvExample) => {
                    example_keys
                        .entry(occurrence.source_id.clone())
                        .or_default()
                        .insert(occurrence.key.clone());
                }
                _ => {}
            }
        }
    }

    let example_sources: Vec<(&str, &Path)> = analysis
        .sources
        .iter()
        .filter(|source| source.kind == SourceKind::DotenvExample)
        .filter_map(|source| {
            source
                .path
                .as_deref()
                .map(|path| (source.id.as_str(), path))
        })
        .collect();

    let mut changes = Vec::new();

    if example_sources.is_empty() {
        // Nothing to sync into: bootstrap a single root example from all keys.
        let mut all_keys: BTreeSet<String> = BTreeSet::new();
        for keys in dotenv_keys.values() {
            all_keys.extend(keys.iter().cloned());
        }
        if !all_keys.is_empty() {
            changes.push(FileChange {
                path: PathBuf::from(".env.example"),
                added_keys: all_keys.into_iter().collect(),
                created: true,
            });
        }
        return SyncPlan { changes };
    }

    for (id, path) in example_sources {
        let dir = dir_of(path);
        let Some(known) = dotenv_keys.get(&dir) else {
            continue;
        };
        let existing = example_keys.get(id).cloned().unwrap_or_default();
        let missing: Vec<String> = known.difference(&existing).cloned().collect();
        if !missing.is_empty() {
            changes.push(FileChange {
                path: path.to_path_buf(),
                added_keys: missing,
                created: false,
            });
        }
    }

    changes.sort_by(|a, b| a.path.cmp(&b.path));
    SyncPlan { changes }
}

/// Apply the plan by appending stripped keys to each example file (creating it
/// when `created`). Existing content is preserved verbatim; new keys are
/// appended after a trailing newline is ensured.
pub fn apply(root: &Path, plan: &SyncPlan) -> std::io::Result<()> {
    for change in &plan.changes {
        let full = root.join(&change.path);
        let mut content = if change.created {
            String::from("# Generated by `envlens sync`. Fill in real values.\n")
        } else {
            let existing = std::fs::read_to_string(&full)?;
            let mut buffer = existing;
            if !buffer.is_empty() && !buffer.ends_with('\n') {
                buffer.push('\n');
            }
            buffer
        };
        for key in &change.added_keys {
            content.push_str(key);
            content.push_str("=\n");
        }
        std::fs::write(&full, content)?;
    }
    Ok(())
}

/// Human-readable summary of the plan. `dry_run` flips the verb tense.
pub fn render_plan(plan: &SyncPlan, dry_run: bool) -> String {
    if plan.is_empty() {
        return "sync: nothing to do — every example file is up to date.\n".to_string();
    }

    let mut output = String::new();
    let mut total = 0usize;
    for change in &plan.changes {
        let verb = match (dry_run, change.created) {
            (true, true) => "would create",
            (true, false) => "would update",
            (false, true) => "created",
            (false, false) => "updated",
        };
        output.push_str(&format!(
            "{verb} {} (+{} keys)\n",
            change.path.display(),
            change.added_keys.len()
        ));
        for key in &change.added_keys {
            output.push_str(&format!("  + {key}=\n"));
            total += 1;
        }
    }
    let files = plan.changes.len();
    output.push_str(&format!(
        "\nsummary: {total} keys across {files} file(s){}\n",
        if dry_run {
            " (dry run, nothing written)"
        } else {
            ""
        }
    ));
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::core::{External, analyze};
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn analyze_dir(root: &Path) -> Analysis {
        analyze(
            root,
            &Config::default(),
            None,
            None,
            External {
                process_env: BTreeMap::new(),
                tracked_files: None,
            },
        )
        .expect("analyze")
    }

    #[test]
    fn adds_missing_keys_with_values_stripped() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".env"),
            "API_URL=http://x\nSECRET_TOKEN=supersecretvalue1\n",
        )
        .unwrap();
        std::fs::write(root.join(".env.example"), "API_URL=\n").unwrap();

        let analysis = analyze_dir(root);
        let plan = plan(&analysis);
        assert_eq!(plan.changes.len(), 1);
        assert_eq!(plan.changes[0].added_keys, vec!["SECRET_TOKEN".to_string()]);

        apply(root, &plan).unwrap();
        let after = std::fs::read_to_string(root.join(".env.example")).unwrap();
        assert!(after.contains("SECRET_TOKEN=\n"));
        // Never writes the real (secret) value.
        assert!(!after.contains("supersecretvalue1"));
        // Preserves the pre-existing key.
        assert!(after.contains("API_URL=\n"));
    }

    #[test]
    fn is_idempotent() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "A=1\nB=2\n").unwrap();
        std::fs::write(root.join(".env.example"), "A=\n").unwrap();

        apply(root, &plan(&analyze_dir(root))).unwrap();
        // Second run sees no missing keys.
        let second = plan(&analyze_dir(root));
        assert!(second.is_empty(), "expected no-op on second run");
    }

    #[test]
    fn no_op_when_example_is_complete() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "A=1\n").unwrap();
        std::fs::write(root.join(".env.example"), "A=\n").unwrap();

        assert!(plan(&analyze_dir(root)).is_empty());
    }

    #[test]
    fn bootstraps_example_when_none_exists() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "A=1\nB=2\n").unwrap();

        let plan = plan(&analyze_dir(root));
        assert_eq!(plan.changes.len(), 1);
        assert!(plan.changes[0].created);
        assert_eq!(
            plan.changes[0].added_keys,
            vec!["A".to_string(), "B".to_string()]
        );

        apply(root, &plan).unwrap();
        assert!(root.join(".env.example").exists());
    }

    #[test]
    fn dry_run_does_not_write() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "A=1\n").unwrap();

        let plan = plan(&analyze_dir(root));
        let text = render_plan(&plan, true);
        assert!(text.contains("would create"));
        assert!(!root.join(".env.example").exists());
    }
}
