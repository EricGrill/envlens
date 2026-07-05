use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn tracked_files(root: &Path) -> Option<BTreeSet<PathBuf>> {
    if !root.join(".git").exists() {
        return None;
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("ls-files")
        .arg("-z")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files = stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect();
    Some(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn tracked_files_contains_cargo_toml_in_this_repo() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let tracked = tracked_files(root).expect("repo should return tracked files");

        assert!(tracked.contains(&PathBuf::from("Cargo.toml")));
    }

    #[test]
    fn tracked_files_none_for_non_git_directory() {
        let dir = TempDir::new().expect("tempdir");

        assert_eq!(tracked_files(dir.path()), None);
    }
}
