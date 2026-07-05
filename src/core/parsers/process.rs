//! Process-environment capture (spec FR-009).
//!
//! `capture()` reads the real OS environment via [`std::env::vars_os`]. It is
//! deliberately just a data-capture function: nothing in `core` calls it.
//! Frontends (e.g. `main.rs`) call `capture()` once and pass the resulting
//! map into the pipeline, keeping `core`'s parsers pure/testable and the one
//! process-global side effect isolated to the edge of the program.

use std::collections::BTreeMap;

/// Snapshot the current process environment.
///
/// Keys and values are lossily decoded from `OsString` via
/// [`std::ffi::OsStr::to_string_lossy`], so non-UTF-8 environment entries are
/// preserved as best-effort text (replacement characters for invalid bytes)
/// rather than dropped or causing a panic. Collected into a `BTreeMap` for a
/// deterministic, sorted-by-key snapshot.
pub fn capture() -> BTreeMap<String, String> {
    std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_includes_path_and_lookup_works() {
        let env = capture();
        assert!(
            env.contains_key("PATH"),
            "expected PATH to be present in any test environment"
        );
        assert!(env.get("PATH").is_some_and(|v| !v.is_empty()));
    }
}
