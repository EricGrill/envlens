//! Project configuration model and discovery (spec §10, FR-051..055).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_yaml::{Mapping, Value};

/// What severity of finding should make the run fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailOn {
    Error,
    Warning,
}

impl<'de> Deserialize<'de> for FailOn {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "error" => Ok(Self::Error),
            "warning" => Ok(Self::Warning),
            _ => Err(serde::de::Error::custom("expected 'error' or 'warning'")),
        }
    }
}

/// A named profile: an ordered include list whose order *is* the precedence
/// (lowest first, highest last), matching the built-in profile semantics.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Profile {
    /// Ordered source tokens (filenames like `.env.local`, or the aliases
    /// `compose` / `scripts` / `process`). Order is precedence, lowest first.
    pub include: Vec<String>,
}

/// Project configuration. Constructed in-process for now; file loading lands
/// in a later task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub ignore: Vec<String>,
    pub required: Vec<String>,
    pub required_from_examples: bool,
    pub secret_patterns: Vec<String>,
    /// Source tokens to lift above their default rank, lowest first.
    pub precedence: Vec<String>,
    pub fail_on: FailOn,
    pub profiles: BTreeMap<String, Profile>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ignore: Vec::new(),
            required: Vec::new(),
            required_from_examples: true,
            secret_patterns: Vec::new(),
            precedence: Vec::new(),
            fail_on: FailOn::Error,
            profiles: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: Config,
    pub warnings: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PartialConfig {
    ignore: Option<Vec<String>>,
    required: Option<Vec<String>>,
    required_from_examples: Option<bool>,
    secret_patterns: Option<Vec<String>>,
    precedence: Option<Vec<String>>,
    fail_on: Option<FailOn>,
    profiles: Option<BTreeMap<String, PartialProfile>>,
}

#[derive(Debug, Default, Deserialize)]
struct PartialProfile {
    include: Option<Vec<String>>,
}

impl PartialConfig {
    fn merge_into(self, config: &mut Config) {
        if let Some(ignore) = self.ignore {
            config.ignore = ignore;
        }
        if let Some(required) = self.required {
            config.required = required;
        }
        if let Some(required_from_examples) = self.required_from_examples {
            config.required_from_examples = required_from_examples;
        }
        if let Some(secret_patterns) = self.secret_patterns {
            config.secret_patterns = secret_patterns;
        }
        if let Some(precedence) = self.precedence {
            config.precedence = precedence;
        }
        if let Some(fail_on) = self.fail_on {
            config.fail_on = fail_on;
        }
        if let Some(profiles) = self.profiles {
            for (name, profile) in profiles {
                let entry = config.profiles.entry(name).or_default();
                if let Some(include) = profile.include {
                    entry.include = include;
                }
            }
        }
    }
}

pub fn parse_str(input: &str) -> LoadedConfig {
    let (partial, warnings) = parse_partial_source(input, None);
    let mut config = Config::default();
    partial.merge_into(&mut config);
    LoadedConfig { config, warnings }
}

pub fn load_file(path: &Path) -> LoadedConfig {
    let (partial, warnings) = load_partial_file(path);
    let mut config = Config::default();
    partial.merge_into(&mut config);
    LoadedConfig { config, warnings }
}

pub fn discover(root: &Path, xdg: Option<PathBuf>, home: Option<PathBuf>) -> LoadedConfig {
    let mut config = Config::default();
    let mut warnings = Vec::new();

    if let Some(user_path) = user_config_path(xdg, home)
        && user_path.is_file()
    {
        let (partial, file_warnings) = load_partial_file(&user_path);
        partial.merge_into(&mut config);
        warnings.extend(file_warnings);
    }

    if let Some(project_path) = project_config_path(root) {
        let (partial, file_warnings) = load_partial_file(&project_path);
        partial.merge_into(&mut config);
        warnings.extend(file_warnings);
    }

    LoadedConfig { config, warnings }
}

fn load_partial_file(path: &Path) -> (PartialConfig, Vec<String>) {
    match fs::read_to_string(path) {
        Ok(input) => parse_partial_source(&input, Some(path)),
        Err(err) => (
            PartialConfig::default(),
            vec![format!(
                "could not read config {}: {err}; using defaults",
                path.display()
            )],
        ),
    }
}

fn parse_partial_source(input: &str, path: Option<&Path>) -> (PartialConfig, Vec<String>) {
    let value = match serde_yaml::from_str::<Value>(input) {
        Ok(Value::Null) => Value::Mapping(Mapping::new()),
        Ok(value) => value,
        Err(err) => {
            return (
                PartialConfig::default(),
                vec![format_parse_warning(path, &err.to_string())],
            );
        }
    };

    let warnings = unknown_key_warnings(&value, path);
    let partial = match serde_yaml::from_value::<PartialConfig>(value) {
        Ok(partial) => partial,
        Err(err) => {
            return (
                PartialConfig::default(),
                vec![format_parse_warning(path, &err.to_string())],
            );
        }
    };

    (partial, warnings)
}

fn format_parse_warning(path: Option<&Path>, err: &str) -> String {
    match path {
        Some(path) => format!(
            "could not parse config {}: {err}; using defaults",
            path.display()
        ),
        None => format!("could not parse config: {err}; using defaults"),
    }
}

fn unknown_key_warnings(value: &Value, path: Option<&Path>) -> Vec<String> {
    let mut warnings = Vec::new();
    let Some(mapping) = value.as_mapping() else {
        return warnings;
    };

    for key in mapping.keys().filter_map(Value::as_str) {
        if !matches!(
            key,
            "ignore"
                | "required"
                | "required_from_examples"
                | "secret_patterns"
                | "precedence"
                | "fail_on"
                | "profiles"
        ) {
            warnings.push(format_unknown_key_warning(path, key));
        }
    }

    if let Some(profiles) = mapping
        .get(Value::String("profiles".to_string()))
        .and_then(Value::as_mapping)
    {
        for (profile_name, profile_value) in profiles {
            let Some(profile_name) = profile_name.as_str() else {
                continue;
            };
            let Some(profile_mapping) = profile_value.as_mapping() else {
                continue;
            };
            for key in profile_mapping.keys().filter_map(Value::as_str) {
                if key != "include" {
                    warnings.push(format_unknown_key_warning(
                        path,
                        &format!("profiles.{profile_name}.{key}"),
                    ));
                }
            }
        }
    }

    warnings
}

fn format_unknown_key_warning(path: Option<&Path>, key: &str) -> String {
    match path {
        Some(path) => format!("unknown config key '{key}' in {}", path.display()),
        None => format!("unknown config key '{key}'"),
    }
}

fn project_config_path(root: &Path) -> Option<PathBuf> {
    [".envlens.yml", ".envlens.yaml", ".config/envlens.yml"]
        .into_iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
}

fn user_config_path(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg.map(|path| path.join("envlens").join("config.yml"))
        .or_else(|| home.map(|path| path.join(".config").join("envlens").join("config.yml")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_from_spec_parses_to_config() {
        let loaded = parse_str(
            r#"
required:
  - DATABASE_URL
secret_patterns:
  - "(?i)(secret|token|key)"
ignore:
  - tmp
profiles:
  web:
    include:
      - .env
      - docker-compose.yml[web]
precedence:
  - .env
  - .env.local
  - process
fail_on: warning
"#,
        );

        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.config.required, vec!["DATABASE_URL"]);
        assert_eq!(
            loaded.config.secret_patterns,
            vec!["(?i)(secret|token|key)"]
        );
        assert_eq!(loaded.config.ignore, vec!["tmp"]);
        assert_eq!(
            loaded.config.profiles["web"].include,
            vec![".env", "docker-compose.yml[web]"]
        );
        assert_eq!(
            loaded.config.precedence,
            vec![".env", ".env.local", "process"]
        );
        assert_eq!(loaded.config.fail_on, FailOn::Warning);
    }

    #[test]
    fn unknown_keys_warn_without_failing() {
        let loaded = parse_str(
            r#"
required: [DATABASE_URL]
mystery: true
profiles:
  web:
    include: [.env]
    extra: nope
"#,
        );

        assert_eq!(loaded.config.required, vec!["DATABASE_URL"]);
        assert_eq!(loaded.config.profiles["web"].include, vec![".env"]);
        assert_eq!(loaded.warnings.len(), 2);
        assert!(loaded.warnings[0].contains("mystery"));
        assert!(loaded.warnings[1].contains("profiles.web.extra"));
    }

    #[test]
    fn malformed_yaml_returns_default_with_warning() {
        let loaded = parse_str("required: [DATABASE_URL\n");

        assert_eq!(loaded.config, Config::default());
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].contains("could not parse config"));
    }

    #[test]
    fn merge_semantics_project_over_user_and_lists_replace() {
        let (user, user_warnings) = parse_partial_source(
            r#"
required: [USER_ONLY]
ignore: [vendor]
profiles:
  web:
    include: [.env]
fail_on: error
"#,
            None,
        );
        let (project, project_warnings) = parse_partial_source(
            r#"
required: [PROJECT_ONLY]
profiles:
  web:
    include: [.env.local]
fail_on: warning
"#,
            None,
        );
        assert!(user_warnings.is_empty());
        assert!(project_warnings.is_empty());
        let mut merged = Config::default();
        user.merge_into(&mut merged);
        project.merge_into(&mut merged);

        assert_eq!(merged.required, vec!["PROJECT_ONLY"]);
        assert_eq!(merged.ignore, vec!["vendor"]);
        assert_eq!(merged.profiles["web"].include, vec![".env.local"]);
        assert_eq!(merged.fail_on, FailOn::Warning);
    }

    #[test]
    fn discovery_first_project_file_wins_and_user_config_merges_underneath() {
        let tempdir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let root = tempdir.path().join("project");
        let xdg = tempdir.path().join("xdg");
        fs::create_dir_all(root.join(".config")).unwrap_or_else(|err| panic!("mkdir: {err}"));
        fs::create_dir_all(xdg.join("envlens")).unwrap_or_else(|err| panic!("mkdir: {err}"));
        fs::write(
            xdg.join("envlens/config.yml"),
            "required: [USER]\nignore: [cache]\n",
        )
        .unwrap_or_else(|err| panic!("write: {err}"));
        fs::write(root.join(".envlens.yml"), "required: [YML]\n")
            .unwrap_or_else(|err| panic!("write: {err}"));
        fs::write(root.join(".envlens.yaml"), "required: [YAML]\n")
            .unwrap_or_else(|err| panic!("write: {err}"));
        fs::write(root.join(".config/envlens.yml"), "required: [NESTED]\n")
            .unwrap_or_else(|err| panic!("write: {err}"));

        let loaded = discover(&root, Some(xdg), None);

        assert_eq!(loaded.config.required, vec!["YML"]);
        assert_eq!(loaded.config.ignore, vec!["cache"]);
    }
}
