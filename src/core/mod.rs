pub mod diagnostics;
pub mod model;
pub mod parsers;
pub mod resolve;
pub mod scanner;
pub mod secrets;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::core::model::{
    Analysis, EnvSource, ParseError, SecretClass, SourceKind, VariableOccurrence,
};
use crate::core::parsers::ci::CiFlavor;
use crate::core::parsers::process::PROCESS_SOURCE_ID;
use crate::core::resolve::ResolveError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct External {
    pub process_env: BTreeMap<String, String>,
    pub tracked_files: Option<BTreeSet<PathBuf>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AnalyzeError {
    #[error("root is unreadable: {0}")]
    RootUnreadable(PathBuf),
    #[error("unknown profile '{0}'")]
    UnknownProfile(String),
    #[error("unknown source '{0}'")]
    UnknownSource(String),
}

pub fn analyze(
    root: &Path,
    config: &Config,
    profile: Option<&str>,
    source_filter: Option<&[String]>,
    external: External,
) -> Result<Analysis, AnalyzeError> {
    if !root.is_dir() || fs::read_dir(root).is_err() {
        return Err(AnalyzeError::RootUnreadable(root.to_path_buf()));
    }

    let mut sources = Vec::new();
    let mut occurrences = Vec::new();
    let mut required: BTreeSet<String> = config.required.iter().cloned().collect();

    for discovered in scanner::scan(root, &config.ignore) {
        let path = root.join(&discovered.rel_path);
        let source_id = path_id(&discovered.rel_path);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => {
                sources.push(source(
                    source_id,
                    discovered.kind,
                    Some(discovered.rel_path),
                    None,
                    vec![ParseError {
                        line: None,
                        message: format!("could not read source: {err}"),
                    }],
                ));
                continue;
            }
        };

        match discovered.kind {
            SourceKind::Dotenv | SourceKind::DotenvExample => {
                let (entries, errors) = parsers::dotenv::parse(&content);
                if config.required_from_examples && discovered.kind == SourceKind::DotenvExample {
                    required.extend(entries.iter().map(|entry| entry.key.clone()));
                }
                occurrences.extend(parsers::occurrences_from_dotenv(&source_id, entries));
                sources.push(source(
                    source_id,
                    discovered.kind,
                    Some(discovered.rel_path),
                    None,
                    errors,
                ));
            }
            SourceKind::Compose => {
                let (services, errors) = parsers::compose::parse(&content);
                if services.is_empty() {
                    sources.push(source(
                        source_id,
                        SourceKind::Compose,
                        Some(discovered.rel_path),
                        None,
                        errors,
                    ));
                } else {
                    for service in services {
                        let id = format!("{source_id}[{}]", service.name);
                        occurrences.extend(parsers::occurrences_from_compose(&id, service.entries));
                        sources.push(source(
                            id,
                            SourceKind::Compose,
                            Some(discovered.rel_path.clone()),
                            Some(service.name),
                            Vec::new(),
                        ));
                    }
                    if !errors.is_empty() {
                        sources.push(source(
                            source_id,
                            SourceKind::Compose,
                            Some(discovered.rel_path),
                            None,
                            errors,
                        ));
                    }
                }
            }
            SourceKind::PackageScript => {
                let (scripts, errors) = parsers::package_json::parse(&content);
                if scripts.is_empty() {
                    sources.push(source(
                        source_id,
                        SourceKind::PackageScript,
                        Some(discovered.rel_path),
                        None,
                        errors,
                    ));
                } else {
                    for script in scripts {
                        let id = format!("{source_id}[{}]", script.script);
                        occurrences.extend(parsers::occurrences_from_scripts(&id, script.entries));
                        sources.push(source(
                            id,
                            SourceKind::PackageScript,
                            Some(discovered.rel_path.clone()),
                            Some(script.script),
                            Vec::new(),
                        ));
                    }
                    if !errors.is_empty() {
                        sources.push(source(
                            source_id,
                            SourceKind::PackageScript,
                            Some(discovered.rel_path),
                            None,
                            errors,
                        ));
                    }
                }
            }
            SourceKind::Ci => {
                let flavor =
                    parsers::ci::flavor_for(&discovered.rel_path).unwrap_or(CiFlavor::CircleCi);
                let (entries, errors) = parsers::ci::parse(&content, flavor);
                occurrences.extend(parsers::occurrences_from_ci(&source_id, entries));
                sources.push(source(
                    source_id,
                    SourceKind::Ci,
                    Some(discovered.rel_path),
                    None,
                    errors,
                ));
            }
            SourceKind::Manifest => {
                sources.push(source(
                    source_id,
                    SourceKind::Manifest,
                    Some(discovered.rel_path),
                    None,
                    Vec::new(),
                ));
            }
            SourceKind::Process => {}
        }
    }

    sources.push(source(
        PROCESS_SOURCE_ID.to_string(),
        SourceKind::Process,
        None,
        None,
        Vec::new(),
    ));
    occurrences.extend(parsers::occurrences_from_process(external.process_env));
    classify_occurrences(&mut occurrences, config);

    resolve::rank_sources(&mut sources, config, profile, source_filter).map_err(
        |err| match err {
            ResolveError::UnknownProfile(name) => AnalyzeError::UnknownProfile(name),
            ResolveError::UnknownSource(name) => AnalyzeError::UnknownSource(name),
        },
    )?;
    let mut variables = resolve::resolve(&sources, occurrences);
    let reference_diagnostics = resolve::expand_references(&mut variables);
    refresh_variable_secret_flags(&mut variables);

    let mut analysis = Analysis {
        root: root.to_path_buf(),
        profile: profile.unwrap_or("default").to_string(),
        sources,
        variables,
        diagnostics: reference_diagnostics,
    };
    diagnostics::run(&mut analysis, &required, external.tracked_files.as_ref());

    Ok(analysis)
}

pub fn re_resolve(
    analysis: &mut Analysis,
    config: &Config,
    profile: Option<&str>,
    tracked: Option<&BTreeSet<PathBuf>>,
) {
    let enabled_by_id: BTreeMap<String, bool> = analysis
        .sources
        .iter()
        .map(|source| (source.id.clone(), source.enabled))
        .collect();
    let mut occurrences: Vec<VariableOccurrence> = analysis
        .variables
        .iter()
        .flat_map(|variable| variable.occurrences.iter().cloned())
        .collect();
    classify_occurrences(&mut occurrences, config);
    let required = required_keys_from_existing_sources(analysis, config, &occurrences);

    let _ = resolve::rank_sources(&mut analysis.sources, config, profile, None);
    for source in &mut analysis.sources {
        if let Some(enabled) = enabled_by_id.get(&source.id) {
            source.enabled = *enabled;
        }
    }

    let mut variables = resolve::resolve(&analysis.sources, occurrences);
    let reference_diagnostics = resolve::expand_references(&mut variables);
    refresh_variable_secret_flags(&mut variables);

    analysis.profile = profile.unwrap_or("default").to_string();
    analysis.variables = variables;
    analysis.diagnostics = reference_diagnostics;
    diagnostics::run(analysis, &required, tracked);
}

fn source(
    id: String,
    kind: SourceKind,
    path: Option<PathBuf>,
    context: Option<String>,
    errors: Vec<ParseError>,
) -> EnvSource {
    EnvSource {
        id,
        kind,
        path,
        context,
        precedence: 0,
        enabled: true,
        errors,
    }
}

fn path_id(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn required_keys_from_existing_sources(
    analysis: &Analysis,
    config: &Config,
    occurrences: &[VariableOccurrence],
) -> BTreeSet<String> {
    let mut required: BTreeSet<String> = config.required.iter().cloned().collect();
    if !config.required_from_examples {
        return required;
    }

    let kind_by_id: BTreeMap<&str, SourceKind> = analysis
        .sources
        .iter()
        .map(|source| (source.id.as_str(), source.kind))
        .collect();
    required.extend(
        occurrences
            .iter()
            .filter(|occurrence| {
                kind_by_id.get(occurrence.source_id.as_str()) == Some(&SourceKind::DotenvExample)
            })
            .map(|occurrence| occurrence.key.clone()),
    );
    required
}

fn classify_occurrences(occurrences: &mut [VariableOccurrence], config: &Config) {
    let extra_patterns: Vec<regex::Regex> = config
        .secret_patterns
        .iter()
        .filter_map(|pattern| regex::Regex::new(pattern).ok())
        .collect();

    for occurrence in occurrences {
        let key_like = secrets::classify_key(&occurrence.key, &extra_patterns);
        let value_like = occurrence
            .parsed_value
            .as_deref()
            .is_some_and(secrets::classify_value);
        occurrence.secret = match (key_like, value_like) {
            (true, true) => SecretClass::Both,
            (true, false) => SecretClass::KeyLike,
            (false, true) => SecretClass::ValueLike,
            (false, false) => SecretClass::None,
        };
    }
}

fn refresh_variable_secret_flags(variables: &mut [model::VariableSummary]) {
    for var in variables {
        var.is_secret_like = var
            .occurrences
            .iter()
            .any(|occurrence| occurrence.secret.is_secret())
            || var
                .effective
                .as_ref()
                .is_some_and(|(value, _)| secrets::classify_value(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{DiagnosticCode, SourceKind};

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    fn empty_external() -> External {
        External {
            process_env: BTreeMap::new(),
            tracked_files: None,
        }
    }

    fn variable<'a>(analysis: &'a Analysis, key: &str) -> &'a model::VariableSummary {
        analysis
            .variables
            .iter()
            .find(|var| var.key == key)
            .unwrap_or_else(|| panic!("missing variable {key}"))
    }

    #[test]
    fn basic_fixture_full_pipeline() {
        let analysis = analyze(
            &fixture("basic"),
            &Config::default(),
            None,
            None,
            empty_external(),
        )
        .unwrap();

        let keys: Vec<&str> = analysis
            .variables
            .iter()
            .map(|var| var.key.as_str())
            .collect();
        assert_eq!(
            keys,
            vec![
                "API_URL",
                "DATABASE_URL",
                "JWT_SECRET",
                "NODE_ENV",
                "PORT",
                "REDIS_URL",
                "STRIPE_API_KEY"
            ]
        );
        assert_eq!(
            variable(&analysis, "PORT").effective.as_ref(),
            Some(&("5001".to_string(), ".env.local".to_string()))
        );
        let missing: Vec<&str> = analysis
            .variables
            .iter()
            .filter(|var| var.is_missing)
            .map(|var| var.key.as_str())
            .collect();
        assert_eq!(missing, vec!["JWT_SECRET", "REDIS_URL"]);
        assert!(variable(&analysis, "STRIPE_API_KEY").is_secret_like);
        assert!(
            variable(&analysis, "API_URL")
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::UndefinedReference)
        );
        assert!(analysis.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::ConflictingValues
            && diagnostic.key.as_deref() == Some("PORT")));
    }

    #[test]
    fn analyze_missing_root_is_error() {
        let missing = fixture("does-not-exist");

        assert_eq!(
            analyze(&missing, &Config::default(), None, None, empty_external()),
            Err(AnalyzeError::RootUnreadable(missing))
        );
    }

    #[test]
    fn invalid_fixture_partial_results() {
        let analysis = analyze(
            &fixture("invalid"),
            &Config::default(),
            None,
            None,
            empty_external(),
        )
        .unwrap();

        assert_eq!(
            variable(&analysis, "VALID").effective.as_ref(),
            Some(&("ok".to_string(), ".env".to_string()))
        );
        assert!(
            analysis
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::InvalidDotenvLine)
        );
    }

    #[test]
    fn empty_fixture() {
        let analysis = analyze(
            &fixture("empty"),
            &Config::default(),
            None,
            None,
            empty_external(),
        )
        .unwrap();

        assert_eq!(analysis.sources.len(), 1);
        assert_eq!(analysis.sources[0].id, "process");
        assert_eq!(analysis.sources[0].kind, SourceKind::Process);
        assert!(analysis.variables.is_empty());
        assert!(analysis.diagnostics.is_empty());
    }

    #[test]
    fn determinism_two_runs_equal() {
        let root = fixture("basic");

        let first = analyze(&root, &Config::default(), None, None, empty_external()).unwrap();
        let second = analyze(&root, &Config::default(), None, None, empty_external()).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn manifest_sources_listed() {
        let analysis = analyze(
            &fixture("monorepo"),
            &Config::default(),
            None,
            None,
            empty_external(),
        )
        .unwrap();

        let manifests: Vec<&str> = analysis
            .sources
            .iter()
            .filter(|source| source.kind == SourceKind::Manifest)
            .map(|source| source.id.as_str())
            .collect();
        assert_eq!(manifests, vec!["pnpm-workspace.yaml", "turbo.json"]);
        assert!(!analysis.variables.iter().any(|var| {
            var.occurrences.iter().any(|occurrence| {
                occurrence.source_id == "pnpm-workspace.yaml"
                    || occurrence.source_id == "turbo.json"
            })
        }));
        assert!(
            !analysis
                .diagnostics
                .iter()
                .any(
                    |diagnostic| diagnostic.source_id.as_deref() == Some("pnpm-workspace.yaml")
                        || diagnostic.source_id.as_deref() == Some("turbo.json")
                )
        );
    }

    #[test]
    fn expanded_secret_effective_value_is_masked_in_diagnostics() {
        let root = fixture("expanded-secret");
        let analysis = analyze(&root, &Config::default(), None, None, empty_external()).unwrap();
        let public_alias = variable(&analysis, "PUBLIC_ALIAS");

        assert!(public_alias.is_secret_like);
        assert_eq!(
            public_alias.effective.as_ref(),
            Some(&(
                "envlensFakeHistoricalSecret".to_string(),
                ".env.local".to_string()
            ))
        );
        let conflict = public_alias
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == DiagnosticCode::ConflictingValues)
            .expect("PUBLIC_ALIAS should conflict across sources");
        assert!(conflict.message.contains('•'));
        assert!(!conflict.message.contains("envlensFakeHistoricalSecret"));
    }

    #[test]
    fn re_resolve_preserves_toggles_and_tracked_secret_diagnostics() {
        let tracked = Some(BTreeSet::from([PathBuf::from(".env.local")]));
        let mut analysis = analyze(
            &fixture("basic"),
            &Config::default(),
            None,
            None,
            External {
                process_env: BTreeMap::new(),
                tracked_files: tracked.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            variable(&analysis, "PORT").effective.as_ref(),
            Some(&("5001".to_string(), ".env.local".to_string()))
        );

        let env_local = analysis
            .sources
            .iter_mut()
            .find(|source| source.id == ".env.local")
            .unwrap_or_else(|| panic!("missing .env.local"));
        env_local.enabled = false;
        re_resolve(&mut analysis, &Config::default(), None, tracked.as_ref());

        assert!(
            !analysis
                .sources
                .iter()
                .find(|source| source.id == ".env.local")
                .unwrap_or_else(|| panic!("missing .env.local"))
                .enabled
        );
        assert_eq!(
            variable(&analysis, "PORT").effective.as_ref(),
            Some(&("3000".to_string(), ".env".to_string()))
        );
        assert!(analysis.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::SecretInTrackedFile
                && diagnostic.key.as_deref() == Some("STRIPE_API_KEY")
        }));
    }
}
