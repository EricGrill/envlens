//! Property tests (issue #11): the parsers and the full analysis pipeline are
//! pure functions over untrusted text. Fixture tests prove known-good cases;
//! these prove that *arbitrary* input can neither panic nor hang. The crate
//! denies `unwrap_used`/`expect_used`, so a surviving panic here is exactly the
//! class of bug static lints can't catch.

use std::collections::{BTreeMap, BTreeSet};

use envlens::config::Config;
use envlens::core::parsers::{compose, direnv, dockerfile, dotenv};
use envlens::core::{External, analyze};
use proptest::prelude::*;
use tempfile::TempDir;

proptest! {
    // Parsers: never panic on arbitrary Unicode input.
    #[test]
    fn dotenv_parse_never_panics(input in "\\PC*") {
        let _ = dotenv::parse(&input);
    }

    #[test]
    fn compose_parse_never_panics(input in "\\PC*") {
        let _ = compose::parse(&input);
    }

    #[test]
    fn dockerfile_parse_never_panics(input in "\\PC*") {
        let _ = dockerfile::parse(&input);
    }

    #[test]
    fn direnv_parse_never_panics(input in "\\PC*") {
        let _ = direnv::parse(&input);
    }

    // Adversarial reference graphs: `${VAR}` expansion must always terminate,
    // even for self-referential and cyclic graphs. We build a `.env` whose
    // values reference each other arbitrarily and run the whole pipeline; a
    // missing termination guard would hang the test instead of returning.
    #[test]
    fn reference_expansion_terminates(
        keys in prop::collection::vec("[A-Z][A-Z0-9_]{0,4}", 1..8),
        edges in prop::collection::vec((0usize..8, 0usize..8), 0..24),
    ) {
        let mut lines = String::new();
        for (i, key) in keys.iter().enumerate() {
            // Every value references one other key by index, forming an
            // arbitrary (possibly cyclic / self-referential) graph.
            let target = edges
                .iter()
                .find(|(from, _)| *from == i)
                .map(|(_, to)| keys[to % keys.len()].clone())
                .unwrap_or_else(|| keys[(i + 1) % keys.len()].clone());
            lines.push_str(&format!("{key}=${{{target}}}\n"));
        }

        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".env"), &lines).unwrap();

        let external = External {
            process_env: BTreeMap::new(),
            tracked_files: None,
        };
        // The assertion is simply that this returns at all.
        let result = analyze(dir.path(), &Config::default(), None, None, external);
        prop_assert!(result.is_ok());
    }
}

proptest! {
    // Filesystem-touching cases are slower; cap them.
    #![proptest_config(ProptestConfig::with_cases(48))]

    // The full analysis pipeline never panics on an arbitrary `.env` file.
    #[test]
    fn analyze_never_panics_on_arbitrary_dotenv(input in "\\PC{0,400}") {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".env"), &input).unwrap();
        let external = External {
            process_env: BTreeMap::new(),
            tracked_files: Some(BTreeSet::new()),
        };
        let _ = analyze(dir.path(), &Config::default(), None, None, external);
    }
}
