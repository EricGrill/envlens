//! Parsers that turn raw source-file contents into structured entries
//! (spec §6). Each format gets its own submodule; mapping parsed entries
//! into [`crate::core::model::VariableOccurrence`]s is added alongside the
//! consumers that need it.

pub mod compose;
pub mod dotenv;
pub mod process;

use std::collections::BTreeMap;

use crate::core::model::{SecretClass, VariableOccurrence};
use process::PROCESS_SOURCE_ID;

/// Map dotenv entries from a single source into occurrences. Purely
/// mechanical field-for-field copying — secret classification is applied by
/// a later pass, so `secret` is always [`SecretClass::None`] here.
pub fn occurrences_from_dotenv(
    source_id: &str,
    entries: Vec<dotenv::DotenvEntry>,
) -> Vec<VariableOccurrence> {
    entries
        .into_iter()
        .map(|entry| {
            let is_empty = entry.parsed_value.is_empty();
            VariableOccurrence {
                key: entry.key,
                raw_value: Some(entry.raw_value),
                parsed_value: Some(entry.parsed_value),
                source_id: source_id.to_string(),
                line: Some(entry.line),
                is_empty,
                is_inherited: false,
                no_expand: entry.no_expand,
                secret: SecretClass::None,
            }
        })
        .collect()
}

/// Map a captured process environment (see [`process::capture`]) into
/// occurrences. Process values are raw text straight from the OS: `raw_value`
/// and `parsed_value` are identical, there's no source line, and no
/// `${VAR}` expansion is ever applied. `source_id` is always
/// [`PROCESS_SOURCE_ID`].
pub fn occurrences_from_process(env: BTreeMap<String, String>) -> Vec<VariableOccurrence> {
    env.into_iter()
        .map(|(key, value)| {
            let is_empty = value.is_empty();
            VariableOccurrence {
                key,
                raw_value: Some(value.clone()),
                parsed_value: Some(value),
                source_id: PROCESS_SOURCE_ID.to_string(),
                line: None,
                is_empty,
                is_inherited: false,
                no_expand: true,
                secret: SecretClass::None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotenv_entry_maps_to_occurrence() {
        let entries = vec![dotenv::DotenvEntry {
            key: "KEY".to_string(),
            raw_value: "\"val\"".to_string(),
            parsed_value: "val".to_string(),
            line: 3,
            no_expand: false,
        }];

        let occurrences = occurrences_from_dotenv(".env", entries);

        assert_eq!(
            occurrences,
            vec![VariableOccurrence {
                key: "KEY".to_string(),
                raw_value: Some("\"val\"".to_string()),
                parsed_value: Some("val".to_string()),
                source_id: ".env".to_string(),
                line: Some(3),
                is_empty: false,
                is_inherited: false,
                no_expand: false,
                secret: SecretClass::None,
            }]
        );
    }

    #[test]
    fn dotenv_entry_with_empty_parsed_value_sets_is_empty() {
        let entries = vec![dotenv::DotenvEntry {
            key: "EMPTY".to_string(),
            raw_value: String::new(),
            parsed_value: String::new(),
            line: 1,
            no_expand: false,
        }];

        let occurrences = occurrences_from_dotenv(".env", entries);

        assert_eq!(
            occurrences,
            vec![VariableOccurrence {
                key: "EMPTY".to_string(),
                raw_value: Some(String::new()),
                parsed_value: Some(String::new()),
                source_id: ".env".to_string(),
                line: Some(1),
                is_empty: true,
                is_inherited: false,
                no_expand: false,
                secret: SecretClass::None,
            }]
        );
    }

    #[test]
    fn process_env_maps_to_occurrence() {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "/usr/bin".to_string());

        let occurrences = occurrences_from_process(env);

        assert_eq!(
            occurrences,
            vec![VariableOccurrence {
                key: "PATH".to_string(),
                raw_value: Some("/usr/bin".to_string()),
                parsed_value: Some("/usr/bin".to_string()),
                source_id: "process".to_string(),
                line: None,
                is_empty: false,
                is_inherited: false,
                no_expand: true,
                secret: SecretClass::None,
            }]
        );
    }

    #[test]
    fn process_env_with_empty_value_sets_is_empty() {
        let mut env = BTreeMap::new();
        env.insert("EMPTY".to_string(), String::new());

        let occurrences = occurrences_from_process(env);

        assert_eq!(
            occurrences,
            vec![VariableOccurrence {
                key: "EMPTY".to_string(),
                raw_value: Some(String::new()),
                parsed_value: Some(String::new()),
                source_id: "process".to_string(),
                line: None,
                is_empty: true,
                is_inherited: false,
                no_expand: true,
                secret: SecretClass::None,
            }]
        );
    }
}
