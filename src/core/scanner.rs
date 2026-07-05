//! Filesystem discovery of env-relevant sources (spec FR-001..009).
//!
//! Walks a project root looking for dotenv files, compose manifests, package
//! manifests, and CI configs by file name / relative-path suffix. Classifies
//! purely on names during the walk — never opens file contents, so this
//! scales to large repos (NFR-002). `Process` sources are not file-backed and
//! are therefore never produced here.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::core::model::SourceKind;

/// Directories that are never descended into, regardless of `.gitignore`
/// state (the scanner deliberately ignores `.gitignore` so it can still see
/// gitignored `.env` files).
const DEFAULT_IGNORE_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "vendor",
    ".venv",
    "venv",
    "dist",
    "build",
    ".next",
    "coverage",
];

const DOTENV_NAMES: &[&str] = &[
    ".env",
    ".env.local",
    ".env.development",
    ".env.development.local",
    ".env.test",
    ".env.test.local",
    ".env.production",
    ".env.production.local",
];

const DOTENV_EXAMPLE_NAMES: &[&str] = &[".env.example", ".env.sample", ".env.template"];

const COMPOSE_NAMES: &[&str] = &[
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
    "docker-compose.override.yml",
    "docker-compose.override.yaml",
];

const MANIFEST_NAMES: &[&str] = &["pnpm-workspace.yaml", "turbo.json", "nx.json"];

const CI_NAMES: &[&str] = &[".gitlab-ci.yml", "circle.yml"];

/// A single file-backed source discovered by [`scan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// Path relative to the scan root.
    pub rel_path: PathBuf,
    pub kind: SourceKind,
}

/// Walk `root` and return every recognized env-relevant file, sorted by
/// `(depth, rel_path)` (shallower paths first, then lexicographically).
///
/// `extra_ignores` are additional directory names (matched by exact file
/// name, like the built-in ignore list) that are never descended into.
pub fn scan(root: &Path, extra_ignores: &[String]) -> Vec<Discovered> {
    let mut ignored_dirs: Vec<String> = DEFAULT_IGNORE_DIRS
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    ignored_dirs.extend(extra_ignores.iter().cloned());

    let walker = WalkBuilder::new(root)
        .standard_filters(false)
        .hidden(false)
        .max_depth(Some(8))
        .follow_links(false)
        .filter_entry(move |entry| {
            let is_ignored_dir = entry.file_type().is_some_and(|ft| ft.is_dir())
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| ignored_dirs.iter().any(|ignored| ignored == name));
            !is_ignored_dir
        })
        .build();

    let mut discovered: Vec<Discovered> = walker
        .filter_map(|result| result.ok())
        .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
        .filter_map(|entry| {
            let rel_path = entry.path().strip_prefix(root).ok()?.to_path_buf();
            classify(&rel_path).map(|kind| Discovered { rel_path, kind })
        })
        .collect();

    discovered.sort_by(|a, b| {
        let depth = |p: &Path| p.components().count();
        depth(&a.rel_path)
            .cmp(&depth(&b.rel_path))
            .then_with(|| a.rel_path.cmp(&b.rel_path))
    });

    discovered
}

/// Classify a root-relative file path by name, applying the path-suffix
/// exceptions for CI workflow files before falling back to exact file-name
/// matches. Returns `None` for anything not recognized.
fn classify(rel_path: &Path) -> Option<SourceKind> {
    if is_github_workflow(rel_path) || is_circleci_config(rel_path) {
        return Some(SourceKind::Ci);
    }

    let file_name = rel_path.file_name()?.to_str()?;

    if DOTENV_NAMES.contains(&file_name) {
        return Some(SourceKind::Dotenv);
    }
    if DOTENV_EXAMPLE_NAMES.contains(&file_name) {
        return Some(SourceKind::DotenvExample);
    }
    if COMPOSE_NAMES.contains(&file_name) {
        return Some(SourceKind::Compose);
    }
    if file_name == "package.json" {
        return Some(SourceKind::PackageScript);
    }
    if MANIFEST_NAMES.contains(&file_name) {
        return Some(SourceKind::Manifest);
    }
    if CI_NAMES.contains(&file_name) {
        return Some(SourceKind::Ci);
    }

    None
}

/// `.github/workflows/*.yml` or `*.yaml`, matched by relative-path suffix so
/// a same-named file elsewhere is never misclassified.
fn is_github_workflow(rel_path: &Path) -> bool {
    let has_yaml_extension = matches!(
        rel_path.extension().and_then(OsStr::to_str),
        Some("yml") | Some("yaml")
    );
    has_yaml_extension && parent_matches(rel_path, &[".github", "workflows"])
}

/// `.circleci/config.yml`, matched by relative-path suffix — a bare
/// root-level `config.yml` must never match.
fn is_circleci_config(rel_path: &Path) -> bool {
    rel_path.file_name() == Some(OsStr::new("config.yml"))
        && parent_matches(rel_path, &[".circleci"])
}

/// True if `rel_path`'s parent directory chain ends with exactly `expected`,
/// in order (e.g. `expected = [".github", "workflows"]` matches
/// `.github/workflows/x.yml` and `a/b/.github/workflows/x.yml`, since only
/// the suffix of the parent chain is checked).
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
        .zip(expected.iter())
        .all(|(component, name)| **component == *OsStr::new(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Create an empty file at `root/rel`, creating parent directories as
    /// needed. `rel` uses `/` as a separator regardless of platform.
    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(&path, "").expect("write file");
    }

    fn scan_default(root: &Path) -> Vec<(PathBuf, SourceKind)> {
        scan(root, &[])
            .into_iter()
            .map(|d| (d.rel_path, d.kind))
            .collect()
    }

    fn sorted(mut v: Vec<(PathBuf, SourceKind)>) -> Vec<(PathBuf, SourceKind)> {
        v.sort();
        v
    }

    #[test]
    fn finds_all_dotenv_variants() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();

        let dotenv_files = [
            ".env",
            ".env.local",
            ".env.development",
            ".env.development.local",
            ".env.test",
            ".env.test.local",
            ".env.production",
            ".env.production.local",
        ];
        let example_files = [".env.example", ".env.sample", ".env.template"];

        for f in dotenv_files.iter().chain(example_files.iter()) {
            touch(root, f);
        }

        let mut expected: Vec<(PathBuf, SourceKind)> = dotenv_files
            .iter()
            .map(|f| (PathBuf::from(f), SourceKind::Dotenv))
            .collect();
        expected.extend(
            example_files
                .iter()
                .map(|f| (PathBuf::from(f), SourceKind::DotenvExample)),
        );

        assert_eq!(sorted(scan_default(root)), sorted(expected));
    }

    #[test]
    fn finds_compose_package_ci_manifests() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();

        let files: &[(&str, SourceKind)] = &[
            ("docker-compose.yml", SourceKind::Compose),
            ("compose.yaml", SourceKind::Compose),
            ("docker-compose.override.yml", SourceKind::Compose),
            ("package.json", SourceKind::PackageScript),
            ("pnpm-workspace.yaml", SourceKind::Manifest),
            ("turbo.json", SourceKind::Manifest),
            ("nx.json", SourceKind::Manifest),
            (".github/workflows/test.yml", SourceKind::Ci),
            (".gitlab-ci.yml", SourceKind::Ci),
            (".circleci/config.yml", SourceKind::Ci),
            ("circle.yml", SourceKind::Ci),
        ];

        for (f, _) in files {
            touch(root, f);
        }

        let expected: Vec<(PathBuf, SourceKind)> = files
            .iter()
            .map(|(f, kind)| (PathBuf::from(f), *kind))
            .collect();

        assert_eq!(sorted(scan_default(root)), sorted(expected));
    }

    #[test]
    fn bare_config_yml_not_ci() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();

        touch(root, "config.yml");
        touch(root, "other/config.yml");
        touch(root, ".circleci/config.yml");

        let expected = vec![(PathBuf::from(".circleci/config.yml"), SourceKind::Ci)];

        assert_eq!(sorted(scan_default(root)), sorted(expected));
    }

    #[test]
    fn ignores_default_dirs() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();

        let ignored_dirs = [
            "node_modules",
            ".git",
            "vendor",
            ".venv",
            "venv",
            "dist",
            "build",
            ".next",
            "coverage",
        ];
        for d in ignored_dirs {
            touch(root, &format!("{d}/.env"));
        }
        touch(root, ".env");

        let expected = vec![(PathBuf::from(".env"), SourceKind::Dotenv)];

        assert_eq!(sorted(scan_default(root)), sorted(expected));
    }

    #[test]
    fn extra_ignores_respected() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();

        touch(root, "tmp/.env");
        touch(root, ".env");

        let result: Vec<(PathBuf, SourceKind)> = scan(root, &["tmp".to_string()])
            .into_iter()
            .map(|d| (d.rel_path, d.kind))
            .collect();

        assert_eq!(result, vec![(PathBuf::from(".env"), SourceKind::Dotenv)]);
    }

    #[test]
    fn nested_discovery_and_order() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();

        touch(root, "apps/web/.env");
        touch(root, ".env");

        let result = scan_default(root);

        assert_eq!(
            result,
            vec![
                (PathBuf::from(".env"), SourceKind::Dotenv),
                (PathBuf::from("apps/web/.env"), SourceKind::Dotenv),
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn does_not_follow_symlinked_dirs() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().expect("tempdir");
        let root = dir.path().join("root");
        fs::create_dir_all(&root).expect("create root");

        // The real target lives *outside* the scan root, so the only way to
        // reach it is through the symlink below.
        let real_dir = dir.path().join("real_target");
        fs::create_dir_all(&real_dir).expect("create real dir");
        fs::write(real_dir.join(".env"), "").expect("write file");

        symlink(&real_dir, root.join("linked")).expect("create symlink");

        assert_eq!(scan_default(&root), Vec::new());
    }

    #[test]
    fn random_files_not_classified() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();

        touch(root, "README.md");
        touch(root, "env.txt");
        touch(root, ".envrc");

        assert_eq!(scan_default(root), Vec::new());
    }
}
