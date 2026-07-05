use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::config::Config;
use crate::core::model::{
    Diagnostic, DiagnosticCode, EnvSource, Severity, SourceId, SourceKind, VariableOccurrence,
    VariableSummary,
};

const ESCAPED_DOLLAR_SENTINEL: char = '\u{E000}';

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    #[error("unknown profile '{0}'")]
    UnknownProfile(String),
    #[error("unknown source '{0}'")]
    UnknownSource(String),
}

pub fn rank_sources(
    sources: &mut [EnvSource],
    config: &Config,
    profile: Option<&str>,
    source_filter: Option<&[String]>,
) -> Result<(), ResolveError> {
    let profile_includes = profile
        .map(|name| {
            profile_include_list(config, name)
                .ok_or_else(|| ResolveError::UnknownProfile(name.to_string()))
        })
        .transpose()?;

    let mut ranks = Vec::with_capacity(sources.len());
    for (input_index, source) in sources.iter_mut().enumerate() {
        let profile_match = profile_includes
            .as_ref()
            .and_then(|includes| first_matching_token_index(source, includes));
        source.enabled = profile_includes.is_none() || profile_match.is_some();

        let custom_match = first_matching_token_index(source, &config.precedence);
        let rank = custom_match
            .map(|idx| 1_000 + idx as i64 * 10)
            .or_else(|| profile_match.map(|idx| 10 + idx as i64 * 10))
            .unwrap_or_else(|| default_rank(source) as i64);

        ranks.push((source.id.clone(), sort_key(source, rank, input_index)));
    }

    let rank_by_id: BTreeMap<SourceId, SortKey> = ranks.into_iter().collect();

    if let Some(filter) = source_filter {
        let mut allowed = BTreeSet::new();
        for wanted in filter {
            let matches: Vec<&SourceId> = sources
                .iter()
                .filter(|source| filter_matches_source(source, wanted))
                .map(|source| &source.id)
                .collect();
            if matches.is_empty() || !matches.iter().any(|id| source_enabled(sources, id)) {
                return Err(ResolveError::UnknownSource(wanted.clone()));
            }
            for id in matches {
                allowed.insert(id.clone());
            }
        }
        for source in sources.iter_mut() {
            source.enabled = source.enabled && allowed.contains(&source.id);
        }
    }

    sources.sort_by(|left, right| {
        let left_key = rank_by_id.get(&left.id);
        let right_key = rank_by_id.get(&right.id);
        left_key.cmp(&right_key)
    });

    for (idx, source) in sources.iter_mut().enumerate() {
        source.precedence = ((idx as u32) + 1) * 10;
    }

    Ok(())
}

pub fn resolve(
    sources: &[EnvSource],
    occurrences: Vec<VariableOccurrence>,
) -> Vec<VariableSummary> {
    let source_by_id: BTreeMap<&str, &EnvSource> = sources
        .iter()
        .map(|source| (source.id.as_str(), source))
        .collect();
    let source_order: BTreeMap<&str, usize> = sources
        .iter()
        .enumerate()
        .map(|(idx, source)| (source.id.as_str(), idx))
        .collect();
    let process_values = process_values(&occurrences, &source_by_id);

    let mut grouped: BTreeMap<String, Vec<(usize, VariableOccurrence)>> = BTreeMap::new();
    for (input_order, occurrence) in occurrences.into_iter().enumerate() {
        grouped
            .entry(occurrence.key.clone())
            .or_default()
            .push((input_order, occurrence));
    }

    grouped
        .into_iter()
        .map(|(key, mut keyed_occurrences)| {
            keyed_occurrences.sort_by(|(left_input, left), (right_input, right)| {
                let left_order = source_order
                    .get(left.source_id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX);
                let right_order = source_order
                    .get(right.source_id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX);
                let left_line = left.line.unwrap_or(u32::MAX);
                let right_line = right.line.unwrap_or(u32::MAX);
                if line_orders_document_position(left, right, &source_by_id) {
                    return (left_line, left_order, *left_input).cmp(&(
                        right_line,
                        right_order,
                        *right_input,
                    ));
                }
                (left_order, left_line, *left_input).cmp(&(right_order, right_line, *right_input))
            });

            let mut effective = None;
            for (_, occurrence) in &keyed_occurrences {
                let Some(source) = source_by_id.get(occurrence.source_id.as_str()) else {
                    continue;
                };
                if !source.enabled || !is_value_bearing(source.kind) {
                    continue;
                }
                if occurrence.is_inherited {
                    if let Some(value) = process_values.get(occurrence.key.as_str()) {
                        effective = Some((value.clone(), occurrence.source_id.clone()));
                    }
                } else if let Some(value) = &occurrence.parsed_value {
                    effective = Some((value.clone(), occurrence.source_id.clone()));
                }
            }

            let occurrences: Vec<VariableOccurrence> = keyed_occurrences
                .into_iter()
                .map(|(_, occurrence)| occurrence)
                .collect();
            let is_secret_like = occurrences
                .iter()
                .any(|occurrence| occurrence.secret.is_secret());

            VariableSummary {
                key,
                effective,
                occurrences,
                diagnostics: Vec::new(),
                is_required: false,
                is_missing: false,
                is_secret_like,
            }
        })
        .collect()
}

pub fn expand_references(vars: &mut [VariableSummary]) -> Vec<Diagnostic> {
    let key_to_index: BTreeMap<String, usize> = vars
        .iter()
        .enumerate()
        .map(|(idx, var)| (var.key.clone(), idx))
        .collect();
    let original_values: Vec<Option<String>> = vars
        .iter()
        .map(|var| var.effective.as_ref().map(|(value, _)| value.clone()))
        .collect();
    let no_expand: Vec<bool> = vars
        .iter()
        .map(|var| winning_occurrence(var).is_some_and(|occurrence| occurrence.no_expand))
        .collect();
    let graph = reference_graph(vars, &key_to_index, &original_values, &no_expand);
    let cycle_indices = cycle_indices(&graph);

    let mut diagnostics = Vec::new();
    for idx in &cycle_indices {
        diagnostics.push(circular_reference_diagnostic(&vars[*idx]));
    }

    let mut memo = vec![None; vars.len()];
    let mut expanded_values = original_values.clone();
    let mut expansion = Expansion {
        vars,
        key_to_index: &key_to_index,
        original_values: &original_values,
        no_expand: &no_expand,
        cycle_indices: &cycle_indices,
        memo: &mut memo,
        diagnostics: &mut diagnostics,
    };

    for idx in 0..vars.len() {
        if original_values[idx].is_none() {
            continue;
        }
        if cycle_indices.contains(&idx) {
            expanded_values[idx] = original_values[idx]
                .as_ref()
                .map(|value| restore_escaped_dollars(value));
            continue;
        }
        let value = expansion.expand_one(idx);
        expanded_values[idx] = Some(value);
    }

    for (var, expanded) in vars.iter_mut().zip(expanded_values) {
        if let (Some((value, _)), Some(expanded)) = (&mut var.effective, expanded) {
            *value = expanded;
        }
    }

    diagnostics
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SortKey {
    rank: i64,
    depth: usize,
    directory: String,
    override_flag: u8,
    path: String,
    input_index: usize,
}

fn sort_key(source: &EnvSource, rank: i64, input_index: usize) -> SortKey {
    let path = source_path(source);
    if source.kind == SourceKind::Compose {
        let directory = path.parent().map(path_to_string).unwrap_or_default();
        return SortKey {
            rank,
            depth: component_count(path.parent()),
            directory,
            override_flag: u8::from(is_compose_override(&path)),
            path: path_to_string(&path),
            input_index,
        };
    }

    SortKey {
        rank,
        depth: component_count(Some(path.as_path())),
        directory: String::new(),
        override_flag: 0,
        path: path_to_string(&path),
        input_index,
    }
}

fn source_path(source: &EnvSource) -> PathBuf {
    source
        .path
        .clone()
        .unwrap_or_else(|| PathBuf::from(&source.id))
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn component_count(path: Option<&Path>) -> usize {
    path.map(|path| path.components().count()).unwrap_or(0)
}

fn is_compose_override(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                "docker-compose.override.yml" | "docker-compose.override.yaml"
            )
        })
}

fn default_rank(source: &EnvSource) -> u32 {
    match source.kind {
        SourceKind::Dotenv => dotenv_rank(source),
        SourceKind::DotenvExample | SourceKind::Manifest | SourceKind::Ci => 0,
        SourceKind::Compose => 90,
        SourceKind::PackageScript => 100,
        SourceKind::Process => 110,
    }
}

fn dotenv_rank(source: &EnvSource) -> u32 {
    let path = source_path(source);
    match path.file_name().and_then(|name| name.to_str()) {
        Some(".env") => 10,
        Some(".env.local") => 20,
        Some(".env.development") => 30,
        Some(".env.development.local") => 40,
        Some(".env.test") => 50,
        Some(".env.test.local") => 60,
        Some(".env.production") => 70,
        Some(".env.production.local") => 80,
        _ => 10,
    }
}

fn profile_include_list(config: &Config, name: &str) -> Option<Vec<String>> {
    config
        .profiles
        .get(name)
        .map(|profile| profile.include.clone())
        .or_else(|| builtin_profile(name))
}

fn builtin_profile(name: &str) -> Option<Vec<String>> {
    let include: Vec<&str> = match name {
        "dev" => vec![
            ".env",
            ".env.local",
            ".env.development",
            ".env.development.local",
            "compose",
            "scripts",
            "process",
        ],
        "test" => vec![".env", ".env.test", ".env.test.local", "process"],
        "production" => vec![
            ".env",
            ".env.production",
            ".env.production.local",
            "compose",
            "process",
        ],
        _ => return None,
    };
    Some(include.into_iter().map(ToString::to_string).collect())
}

fn first_matching_token_index(source: &EnvSource, tokens: &[String]) -> Option<usize> {
    tokens
        .iter()
        .position(|token| profile_or_precedence_matches_source(source, token))
}

fn profile_or_precedence_matches_source(source: &EnvSource, token: &str) -> bool {
    filter_matches_source(source, token)
        || source
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == token)
        || kind_alias_matches(source.kind, token)
}

fn filter_matches_source(source: &EnvSource, token: &str) -> bool {
    source.id == token
        || source
            .path
            .as_ref()
            .is_some_and(|path| path_to_string(path) == token)
}

fn kind_alias_matches(kind: SourceKind, token: &str) -> bool {
    matches!(
        (kind, token),
        (SourceKind::Compose, "compose")
            | (SourceKind::PackageScript, "scripts")
            | (SourceKind::PackageScript, "package")
            | (SourceKind::PackageScript, "package.json")
            | (SourceKind::Process, "process")
            | (SourceKind::Ci, "ci")
            | (SourceKind::Manifest, "manifest")
            | (SourceKind::Dotenv, "dotenv")
    )
}

fn source_enabled(sources: &[EnvSource], id: &SourceId) -> bool {
    sources
        .iter()
        .find(|source| &source.id == id)
        .is_some_and(|source| source.enabled)
}

fn is_value_bearing(kind: SourceKind) -> bool {
    matches!(
        kind,
        SourceKind::Dotenv | SourceKind::Compose | SourceKind::PackageScript | SourceKind::Process
    )
}

fn line_orders_document_position(
    left: &VariableOccurrence,
    right: &VariableOccurrence,
    source_by_id: &BTreeMap<&str, &EnvSource>,
) -> bool {
    if left.source_id == right.source_id {
        return true;
    }
    let Some(left_source) = source_by_id.get(left.source_id.as_str()) else {
        return false;
    };
    let Some(right_source) = source_by_id.get(right.source_id.as_str()) else {
        return false;
    };
    left_source.kind == right_source.kind
        && matches!(
            left_source.kind,
            SourceKind::Compose | SourceKind::PackageScript
        )
        && left_source.path.is_some()
        && left_source.path == right_source.path
}

fn process_values(
    occurrences: &[VariableOccurrence],
    source_by_id: &BTreeMap<&str, &EnvSource>,
) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for occurrence in occurrences {
        if let Some(source) = source_by_id.get(occurrence.source_id.as_str())
            && source.kind == SourceKind::Process
            && source.enabled
            && let Some(value) = &occurrence.parsed_value
        {
            values.insert(occurrence.key.clone(), value.clone());
        }
    }
    values
}

fn reference_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_.]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)\b"))
        .as_ref()
        .ok()
}

fn referenced_keys(value: &str) -> Vec<String> {
    let Some(re) = reference_regex() else {
        return Vec::new();
    };
    re.captures_iter(value)
        .filter_map(|captures| {
            captures
                .get(1)
                .or_else(|| captures.get(2))
                .map(|matched| matched.as_str().to_string())
        })
        .collect()
}

fn reference_graph(
    vars: &[VariableSummary],
    key_to_index: &BTreeMap<String, usize>,
    original_values: &[Option<String>],
    no_expand: &[bool],
) -> Vec<Vec<usize>> {
    vars.iter()
        .enumerate()
        .map(|(idx, _)| {
            if no_expand[idx] {
                return Vec::new();
            }
            let Some(value) = &original_values[idx] else {
                return Vec::new();
            };
            referenced_keys(value)
                .into_iter()
                .filter_map(|key| key_to_index.get(&key).copied())
                .filter(|target| original_values[*target].is_some())
                .collect()
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visit {
    Unvisited,
    Visiting,
    Done,
}

fn cycle_indices(graph: &[Vec<usize>]) -> BTreeSet<usize> {
    let mut state = vec![Visit::Unvisited; graph.len()];
    let mut stack = Vec::new();
    let mut cycles = BTreeSet::new();
    for idx in 0..graph.len() {
        detect_cycles(idx, graph, &mut state, &mut stack, &mut cycles);
    }
    cycles
}

fn detect_cycles(
    idx: usize,
    graph: &[Vec<usize>],
    state: &mut [Visit],
    stack: &mut Vec<usize>,
    cycles: &mut BTreeSet<usize>,
) {
    match state[idx] {
        Visit::Done => return,
        Visit::Visiting => {
            if let Some(pos) = stack.iter().position(|stack_idx| *stack_idx == idx) {
                for cycle_idx in &stack[pos..] {
                    cycles.insert(*cycle_idx);
                }
            }
            return;
        }
        Visit::Unvisited => {}
    }

    state[idx] = Visit::Visiting;
    stack.push(idx);
    for target in &graph[idx] {
        detect_cycles(*target, graph, state, stack, cycles);
    }
    stack.pop();
    state[idx] = Visit::Done;
}

struct Expansion<'a> {
    vars: &'a [VariableSummary],
    key_to_index: &'a BTreeMap<String, usize>,
    original_values: &'a [Option<String>],
    no_expand: &'a [bool],
    cycle_indices: &'a BTreeSet<usize>,
    memo: &'a mut [Option<String>],
    diagnostics: &'a mut Vec<Diagnostic>,
}

impl Expansion<'_> {
    fn expand_one(&mut self, idx: usize) -> String {
        if let Some(value) = &self.memo[idx] {
            return value.clone();
        }

        let Some(original) = &self.original_values[idx] else {
            return String::new();
        };

        if self.no_expand[idx] {
            let value = restore_escaped_dollars(original);
            self.memo[idx] = Some(value.clone());
            return value;
        }

        let Some(re) = reference_regex() else {
            let value = restore_escaped_dollars(original);
            self.memo[idx] = Some(value.clone());
            return value;
        };

        let mut output = String::new();
        let mut last = 0;
        for captures in re.captures_iter(original) {
            let Some(matched) = captures.get(0) else {
                continue;
            };
            output.push_str(&original[last..matched.start()]);
            let key = captures
                .get(1)
                .or_else(|| captures.get(2))
                .map(|matched| matched.as_str());
            if let Some(key) = key {
                match self.key_to_index.get(key).copied() {
                    Some(target) if self.original_values[target].is_none() => {
                        self.diagnostics
                            .push(undefined_reference_diagnostic(&self.vars[idx], key));
                        output.push_str(matched.as_str());
                    }
                    Some(target) if self.cycle_indices.contains(&target) => {
                        output.push_str(matched.as_str());
                    }
                    Some(target) => {
                        let value = self.expand_one(target);
                        output.push_str(&value);
                    }
                    None => {
                        self.diagnostics
                            .push(undefined_reference_diagnostic(&self.vars[idx], key));
                        output.push_str(matched.as_str());
                    }
                }
            } else {
                output.push_str(matched.as_str());
            }
            last = matched.end();
        }
        output.push_str(&original[last..]);

        let output = restore_escaped_dollars(&output);
        self.memo[idx] = Some(output.clone());
        output
    }
}

fn restore_escaped_dollars(value: &str) -> String {
    value.replace(ESCAPED_DOLLAR_SENTINEL, "$")
}

fn winning_occurrence(var: &VariableSummary) -> Option<&VariableOccurrence> {
    let (_, source_id) = var.effective.as_ref()?;
    var.occurrences
        .iter()
        .rfind(|occurrence| &occurrence.source_id == source_id)
}

fn diagnostic_location(var: &VariableSummary) -> (Option<SourceId>, Option<u32>) {
    winning_occurrence(var)
        .map(|occurrence| (Some(occurrence.source_id.clone()), occurrence.line))
        .unwrap_or((None, None))
}

fn undefined_reference_diagnostic(var: &VariableSummary, missing: &str) -> Diagnostic {
    let (source_id, line) = diagnostic_location(var);
    Diagnostic {
        severity: Severity::Warning,
        code: DiagnosticCode::UndefinedReference,
        message: format!("{} references undefined variable {missing}", var.key),
        key: Some(var.key.clone()),
        source_id,
        line,
    }
}

fn circular_reference_diagnostic(var: &VariableSummary) -> Diagnostic {
    let (source_id, line) = diagnostic_location(var);
    Diagnostic {
        severity: Severity::Error,
        code: DiagnosticCode::CircularReference,
        message: format!("{} participates in a circular reference", var.key),
        key: Some(var.key.clone()),
        source_id,
        line,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;
    use crate::config::Profile;
    use crate::core::model::{
        DiagnosticCode, EnvSource, ParseError, SecretClass, Severity, SourceKind,
        VariableOccurrence,
    };

    fn source(id: &str, kind: SourceKind, path: Option<&str>) -> EnvSource {
        EnvSource {
            id: id.to_string(),
            kind,
            path: path.map(PathBuf::from),
            context: context_from_id(id),
            precedence: 0,
            enabled: true,
            errors: Vec::<ParseError>::new(),
        }
    }

    fn context_from_id(id: &str) -> Option<String> {
        id.rsplit_once('[')
            .and_then(|(_, rest)| rest.strip_suffix(']'))
            .map(ToString::to_string)
    }

    fn occ(key: &str, value: &str, source_id: &str, line: u32) -> VariableOccurrence {
        VariableOccurrence {
            key: key.to_string(),
            raw_value: Some(value.to_string()),
            parsed_value: Some(value.to_string()),
            source_id: source_id.to_string(),
            line: Some(line),
            is_empty: value.is_empty(),
            is_inherited: false,
            no_expand: false,
            secret: SecretClass::None,
        }
    }

    fn occ_no_expand(key: &str, value: &str, source_id: &str, line: u32) -> VariableOccurrence {
        VariableOccurrence {
            no_expand: true,
            ..occ(key, value, source_id, line)
        }
    }

    fn inherited_occ(key: &str, source_id: &str, line: u32) -> VariableOccurrence {
        VariableOccurrence {
            key: key.to_string(),
            raw_value: None,
            parsed_value: None,
            source_id: source_id.to_string(),
            line: Some(line),
            is_empty: false,
            is_inherited: true,
            no_expand: false,
            secret: SecretClass::None,
        }
    }

    fn process_occ(key: &str, value: &str) -> VariableOccurrence {
        VariableOccurrence {
            key: key.to_string(),
            raw_value: Some(value.to_string()),
            parsed_value: Some(value.to_string()),
            source_id: "process".to_string(),
            line: None,
            is_empty: value.is_empty(),
            is_inherited: false,
            no_expand: true,
            secret: SecretClass::None,
        }
    }

    fn rank_and_resolve(
        mut sources: Vec<EnvSource>,
        occurrences: Vec<VariableOccurrence>,
    ) -> Vec<VariableSummary> {
        rank_sources(&mut sources, &Config::default(), None, None).unwrap();
        resolve(&sources, occurrences)
    }

    fn effective<'a>(vars: &'a [VariableSummary], key: &str) -> Option<&'a (String, String)> {
        vars.iter()
            .find(|var| var.key == key)
            .and_then(|var| var.effective.as_ref())
    }

    #[test]
    fn default_precedence_order() {
        let sources = vec![
            source(".env", SourceKind::Dotenv, Some(".env")),
            source(".env.local", SourceKind::Dotenv, Some(".env.local")),
            source(
                "docker-compose.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
            source(
                "package.json[dev]",
                SourceKind::PackageScript,
                Some("package.json"),
            ),
            source("process", SourceKind::Process, None),
        ];
        let occurrences = vec![
            occ("PORT", "3000", ".env", 1),
            occ("PORT", "5001", ".env.local", 1),
            occ("PORT", "5002", "docker-compose.yml[api]", 7),
            occ("PORT", "5003", "package.json[dev]", 3),
            process_occ("PORT", "9999"),
        ];

        let vars = rank_and_resolve(sources, occurrences);

        assert_eq!(
            effective(&vars, "PORT"),
            Some(&("9999".to_string(), "process".to_string()))
        );
        let order: Vec<&str> = vars[0]
            .occurrences
            .iter()
            .map(|occurrence| occurrence.source_id.as_str())
            .collect();
        assert_eq!(
            order,
            vec![
                ".env",
                ".env.local",
                "docker-compose.yml[api]",
                "package.json[dev]",
                "process"
            ]
        );
    }

    #[test]
    fn same_rank_path_tiebreak_deeper_path_wins() {
        let sources = vec![
            source(".env", SourceKind::Dotenv, Some(".env")),
            source("apps/web/.env", SourceKind::Dotenv, Some("apps/web/.env")),
        ];
        let occurrences = vec![
            occ("API_URL", "root", ".env", 1),
            occ("API_URL", "web", "apps/web/.env", 1),
        ];

        let vars = rank_and_resolve(sources, occurrences);

        assert_eq!(
            effective(&vars, "API_URL"),
            Some(&("web".to_string(), "apps/web/.env".to_string()))
        );
    }

    #[test]
    fn compose_override_beats_base() {
        let sources = vec![
            source(
                "docker-compose.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
            source(
                "docker-compose.override.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.override.yml"),
            ),
        ];
        let occurrences = vec![
            occ("PORT", "5001", "docker-compose.yml[api]", 4),
            occ("PORT", "5002", "docker-compose.override.yml[api]", 4),
        ];

        let vars = rank_and_resolve(sources, occurrences);

        assert_eq!(
            effective(&vars, "PORT"),
            Some(&(
                "5002".to_string(),
                "docker-compose.override.yml[api]".to_string()
            ))
        );
    }

    #[test]
    fn two_base_compose_files_use_path_rule() {
        let sources = vec![
            source(
                "compose.yaml[api]",
                SourceKind::Compose,
                Some("compose.yaml"),
            ),
            source(
                "docker-compose.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
        ];
        let occurrences = vec![
            occ("PORT", "from-compose", "compose.yaml[api]", 4),
            occ("PORT", "from-docker-compose", "docker-compose.yml[api]", 4),
        ];

        let vars = rank_and_resolve(sources, occurrences);

        assert_eq!(
            effective(&vars, "PORT"),
            Some(&(
                "from-docker-compose".to_string(),
                "docker-compose.yml[api]".to_string()
            ))
        );
    }

    #[test]
    fn compose_override_binds_per_directory() {
        let mut sources = vec![
            source(
                "apps/web/docker-compose.override.yml[api]",
                SourceKind::Compose,
                Some("apps/web/docker-compose.override.yml"),
            ),
            source(
                "docker-compose.override.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.override.yml"),
            ),
            source(
                "apps/web/docker-compose.yml[api]",
                SourceKind::Compose,
                Some("apps/web/docker-compose.yml"),
            ),
            source(
                "docker-compose.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
        ];

        rank_sources(&mut sources, &Config::default(), None, None).unwrap();

        let ordered_ids: Vec<&str> = sources.iter().map(|source| source.id.as_str()).collect();
        assert_eq!(
            ordered_ids,
            vec![
                "docker-compose.yml[api]",
                "docker-compose.override.yml[api]",
                "apps/web/docker-compose.yml[api]",
                "apps/web/docker-compose.override.yml[api]",
            ]
        );
    }

    #[test]
    fn subsource_document_order_last_wins() {
        let sources = vec![
            source(
                "docker-compose.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
            source(
                "docker-compose.yml[worker]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
        ];
        let occurrences = vec![
            occ("PORT", "api", "docker-compose.yml[api]", 4),
            occ("PORT", "worker", "docker-compose.yml[worker]", 9),
        ];

        let vars = rank_and_resolve(sources, occurrences);

        assert_eq!(
            effective(&vars, "PORT"),
            Some(&(
                "worker".to_string(),
                "docker-compose.yml[worker]".to_string()
            ))
        );
    }

    #[test]
    fn subsource_document_order_last_wins_even_when_sources_are_reversed() {
        let sources = vec![
            source(
                "docker-compose.yml[worker]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
            source(
                "docker-compose.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
        ];
        let occurrences = vec![
            occ("PORT", "worker", "docker-compose.yml[worker]", 9),
            occ("PORT", "api", "docker-compose.yml[api]", 4),
        ];

        let vars = rank_and_resolve(sources, occurrences);

        assert_eq!(
            effective(&vars, "PORT"),
            Some(&(
                "worker".to_string(),
                "docker-compose.yml[worker]".to_string()
            ))
        );
    }

    #[test]
    fn same_source_duplicate_last_wins() {
        let sources = vec![source(".env", SourceKind::Dotenv, Some(".env"))];
        let occurrences = vec![
            occ("PORT", "3000", ".env", 1),
            occ("PORT", "5001", ".env", 2),
        ];

        let vars = rank_and_resolve(sources, occurrences);

        assert_eq!(
            effective(&vars, "PORT"),
            Some(&("5001".to_string(), ".env".to_string()))
        );
    }

    #[test]
    fn same_source_duplicate_last_wins_by_line_even_when_input_is_reversed() {
        let sources = vec![source(".env", SourceKind::Dotenv, Some(".env"))];
        let occurrences = vec![
            occ("PORT", "5001", ".env", 2),
            occ("PORT", "3000", ".env", 1),
        ];

        let vars = rank_and_resolve(sources, occurrences);

        assert_eq!(
            effective(&vars, "PORT"),
            Some(&("5001".to_string(), ".env".to_string()))
        );
    }

    #[test]
    fn profile_include_order_is_precedence() {
        let mut config = Config::default();
        config.profiles.insert(
            "inverted".to_string(),
            Profile {
                include: vec!["process".to_string(), ".env".to_string()],
            },
        );
        let mut sources = vec![
            source(".env", SourceKind::Dotenv, Some(".env")),
            source(".env.local", SourceKind::Dotenv, Some(".env.local")),
            source("process", SourceKind::Process, None),
        ];
        let occurrences = vec![occ("PORT", "3000", ".env", 1), process_occ("PORT", "9999")];

        rank_sources(&mut sources, &config, Some("inverted"), None).unwrap();
        let vars = resolve(&sources, occurrences);

        assert_eq!(
            effective(&vars, "PORT"),
            Some(&("3000".to_string(), ".env".to_string()))
        );
        assert!(
            !sources
                .iter()
                .find(|source| source.id == ".env.local")
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn source_filter_narrows_to_matching_source() {
        let mut sources = vec![
            source(".env", SourceKind::Dotenv, Some(".env")),
            source(".env.local", SourceKind::Dotenv, Some(".env.local")),
        ];
        let filter = vec![".env".to_string()];
        let occurrences = vec![
            occ("PORT", "3000", ".env", 1),
            occ("PORT", "5001", ".env.local", 1),
        ];

        rank_sources(&mut sources, &Config::default(), None, Some(&filter)).unwrap();
        let vars = resolve(&sources, occurrences);

        assert_eq!(
            effective(&vars, "PORT"),
            Some(&("3000".to_string(), ".env".to_string()))
        );
        assert!(
            !sources
                .iter()
                .find(|source| source.id == ".env.local")
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn unknown_source_filter_is_error() {
        let mut sources = vec![source(".env", SourceKind::Dotenv, Some(".env"))];
        let filter = vec!["missing.env".to_string()];

        assert_eq!(
            rank_sources(&mut sources, &Config::default(), None, Some(&filter)),
            Err(ResolveError::UnknownSource("missing.env".to_string()))
        );
    }

    #[test]
    fn filter_naming_profile_excluded_source_is_error() {
        let mut sources = vec![
            source(".env", SourceKind::Dotenv, Some(".env")),
            source(
                "docker-compose.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
        ];
        let filter = vec!["docker-compose.yml".to_string()];

        assert_eq!(
            rank_sources(
                &mut sources,
                &Config::default(),
                Some("test"),
                Some(&filter)
            ),
            Err(ResolveError::UnknownSource(
                "docker-compose.yml".to_string()
            ))
        );
    }

    #[test]
    fn builtin_profiles() {
        let mut sources = vec![
            source(".env", SourceKind::Dotenv, Some(".env")),
            source(".env.local", SourceKind::Dotenv, Some(".env.local")),
            source(".env.test", SourceKind::Dotenv, Some(".env.test")),
            source(
                "docker-compose.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
            source("process", SourceKind::Process, None),
        ];

        rank_sources(&mut sources, &Config::default(), Some("test"), None).unwrap();

        assert!(
            sources
                .iter()
                .find(|source| source.id == ".env")
                .unwrap()
                .enabled
        );
        assert!(
            sources
                .iter()
                .find(|source| source.id == ".env.test")
                .unwrap()
                .enabled
        );
        assert!(
            sources
                .iter()
                .find(|source| source.id == "process")
                .unwrap()
                .enabled
        );
        assert!(
            !sources
                .iter()
                .find(|source| source.id == ".env.local")
                .unwrap()
                .enabled
        );
        assert!(
            !sources
                .iter()
                .find(|source| source.id == "docker-compose.yml[api]")
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn custom_precedence_key_wins() {
        let config = Config {
            precedence: vec![".env.local".to_string(), ".env".to_string()],
            ..Config::default()
        };
        let sources = vec![
            source(".env", SourceKind::Dotenv, Some(".env")),
            source(".env.local", SourceKind::Dotenv, Some(".env.local")),
        ];
        let occurrences = vec![
            occ("PORT", "3000", ".env", 1),
            occ("PORT", "5001", ".env.local", 1),
        ];

        let mut ranked_sources = sources;
        rank_sources(&mut ranked_sources, &config, None, None).unwrap();
        let vars = resolve(&ranked_sources, occurrences);

        assert_eq!(
            effective(&vars, "PORT"),
            Some(&("3000".to_string(), ".env".to_string()))
        );
    }

    #[test]
    fn example_and_ci_never_effective() {
        let sources = vec![
            source(
                ".env.example",
                SourceKind::DotenvExample,
                Some(".env.example"),
            ),
            source(
                ".github/workflows/ci.yml",
                SourceKind::Ci,
                Some(".github/workflows/ci.yml"),
            ),
        ];
        let occurrences = vec![
            occ("PORT", "3000", ".env.example", 1),
            occ("PORT", "5001", ".github/workflows/ci.yml", 4),
        ];

        let vars = rank_and_resolve(sources, occurrences);

        assert_eq!(effective(&vars, "PORT"), None);
    }

    #[test]
    fn expansion_resolves_braced_and_word_boundary_references() {
        let sources = vec![source(".env", SourceKind::Dotenv, Some(".env"))];
        let occurrences = vec![
            occ("HOST", "localhost", ".env", 1),
            occ("PORT", "3000", ".env", 2),
            occ("API_URL", "${HOST}:$PORT", ".env", 3),
        ];
        let mut vars = rank_and_resolve(sources, occurrences);

        let diagnostics = expand_references(&mut vars);

        assert!(diagnostics.is_empty());
        assert_eq!(
            effective(&vars, "API_URL"),
            Some(&("localhost:3000".to_string(), ".env".to_string()))
        );
    }

    #[test]
    fn undefined_reference_warns_and_keeps_reference() {
        let sources = vec![source(".env", SourceKind::Dotenv, Some(".env"))];
        let occurrences = vec![occ("API_URL", "${MISSING}", ".env", 3)];
        let mut vars = rank_and_resolve(sources, occurrences);

        let diagnostics = expand_references(&mut vars);

        assert_eq!(
            effective(&vars, "API_URL"),
            Some(&("${MISSING}".to_string(), ".env".to_string()))
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, DiagnosticCode::UndefinedReference);
        assert_eq!(diagnostics[0].severity, Severity::Warning);
        assert_eq!(diagnostics[0].key.as_deref(), Some("API_URL"));
        assert_eq!(diagnostics[0].source_id.as_deref(), Some(".env"));
        assert_eq!(diagnostics[0].line, Some(3));
    }

    #[test]
    fn circular_reference_errors_on_all_participants_and_leaves_values_unexpanded() {
        let sources = vec![source(".env", SourceKind::Dotenv, Some(".env"))];
        let occurrences = vec![occ("A", "${B}", ".env", 1), occ("B", "${A}", ".env", 2)];
        let mut vars = rank_and_resolve(sources, occurrences);

        let diagnostics = expand_references(&mut vars);

        let mut cycle_keys: Vec<String> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == DiagnosticCode::CircularReference)
            .filter_map(|diagnostic| diagnostic.key.clone())
            .collect();
        cycle_keys.sort();
        assert_eq!(cycle_keys, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(
            effective(&vars, "A"),
            Some(&("${B}".to_string(), ".env".to_string()))
        );
        assert_eq!(
            effective(&vars, "B"),
            Some(&("${A}".to_string(), ".env".to_string()))
        );
    }

    #[test]
    fn reference_to_existing_cyclic_variable_is_not_undefined() {
        let sources = vec![source(".env", SourceKind::Dotenv, Some(".env"))];
        let occurrences = vec![
            occ("A", "${B}", ".env", 1),
            occ("B", "${A}", ".env", 2),
            occ("C", "${A}", ".env", 3),
        ];
        let mut vars = rank_and_resolve(sources, occurrences);

        let diagnostics = expand_references(&mut vars);

        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != DiagnosticCode::UndefinedReference)
        );
        assert_eq!(
            effective(&vars, "C"),
            Some(&("${A}".to_string(), ".env".to_string()))
        );
    }

    #[test]
    fn no_expand_respected_and_escaped_dollar_sentinel_restored() {
        let sources = vec![source(".env", SourceKind::Dotenv, Some(".env"))];
        let occurrences = vec![
            occ("X", "expanded", ".env", 1),
            occ_no_expand("SINGLE", "${X}", ".env", 2),
            occ("ESCAPED", "\u{E000}X", ".env", 3),
        ];
        let mut vars = rank_and_resolve(sources, occurrences);

        let diagnostics = expand_references(&mut vars);

        assert!(diagnostics.is_empty());
        assert_eq!(
            effective(&vars, "SINGLE"),
            Some(&("${X}".to_string(), ".env".to_string()))
        );
        assert_eq!(
            effective(&vars, "ESCAPED"),
            Some(&("$X".to_string(), ".env".to_string()))
        );
    }

    #[test]
    fn inherited_resolution_uses_process_value_at_inheriting_source_rank() {
        let sources = vec![
            source(
                "docker-compose.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
            source("process", SourceKind::Process, None),
        ];
        let occurrences = vec![
            inherited_occ("DATABASE_URL", "docker-compose.yml[api]", 5),
            process_occ("DATABASE_URL", "postgres://host/db"),
        ];

        let vars = rank_and_resolve(sources, occurrences);

        assert_eq!(
            effective(&vars, "DATABASE_URL"),
            Some(&("postgres://host/db".to_string(), "process".to_string()))
        );
        assert!(
            vars[0]
                .occurrences
                .iter()
                .any(|occurrence| occurrence.is_inherited)
        );
    }

    #[test]
    fn inherited_resolution_does_not_use_disabled_process_source() {
        let mut sources = vec![
            source(
                "docker-compose.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
            source("process", SourceKind::Process, None),
        ];
        let filter = vec!["docker-compose.yml".to_string()];
        let occurrences = vec![
            inherited_occ("DATABASE_URL", "docker-compose.yml[api]", 5),
            process_occ("DATABASE_URL", "postgres://host/db"),
        ];

        rank_sources(&mut sources, &Config::default(), None, Some(&filter)).unwrap();
        let vars = resolve(&sources, occurrences);

        assert_eq!(effective(&vars, "DATABASE_URL"), None);
    }

    #[test]
    fn unknown_profile_is_error() {
        let mut sources = vec![source(".env", SourceKind::Dotenv, Some(".env"))];

        assert_eq!(
            rank_sources(&mut sources, &Config::default(), Some("missing"), None),
            Err(ResolveError::UnknownProfile("missing".to_string()))
        );
    }

    #[test]
    fn resolve_outputs_variables_sorted_by_key() {
        let sources = vec![source(".env", SourceKind::Dotenv, Some(".env"))];
        let occurrences = vec![occ("ZED", "1", ".env", 1), occ("ALPHA", "1", ".env", 2)];

        let vars = rank_and_resolve(sources, occurrences);
        let keys: Vec<&str> = vars.iter().map(|var| var.key.as_str()).collect();

        assert_eq!(keys, vec!["ALPHA", "ZED"]);
    }

    #[test]
    fn secret_like_summary_reflects_occurrence_secret_flags() {
        let sources = vec![source(".env", SourceKind::Dotenv, Some(".env"))];
        let mut secret = occ("JWT_SECRET", "abc", ".env", 1);
        secret.secret = SecretClass::KeyLike;

        let vars = rank_and_resolve(sources, vec![secret]);

        assert!(vars[0].is_secret_like);
    }

    #[test]
    fn source_filter_matches_file_path_for_all_subsources() {
        let mut sources = vec![
            source(
                "docker-compose.yml[api]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
            source(
                "docker-compose.yml[worker]",
                SourceKind::Compose,
                Some("docker-compose.yml"),
            ),
            source(".env", SourceKind::Dotenv, Some(".env")),
        ];
        let filter = vec!["docker-compose.yml".to_string()];

        rank_sources(&mut sources, &Config::default(), None, Some(&filter)).unwrap();

        assert!(
            sources
                .iter()
                .filter(|source| source.path.as_deref()
                    == Some(PathBuf::from("docker-compose.yml").as_path()))
                .all(|source| source.enabled)
        );
        assert!(
            !sources
                .iter()
                .find(|source| source.id == ".env")
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn rank_sources_is_deterministic_across_input_order() {
        let mut first = vec![
            source("process", SourceKind::Process, None),
            source(".env.local", SourceKind::Dotenv, Some(".env.local")),
            source(".env", SourceKind::Dotenv, Some(".env")),
        ];
        let mut second = first.clone();
        second.reverse();

        rank_sources(&mut first, &Config::default(), None, None).unwrap();
        rank_sources(&mut second, &Config::default(), None, None).unwrap();

        let first_ids: Vec<&str> = first.iter().map(|source| source.id.as_str()).collect();
        let second_ids: Vec<&str> = second.iter().map(|source| source.id.as_str()).collect();
        assert_eq!(first_ids, second_ids);
    }

    #[test]
    fn custom_profile_definition_is_not_mutated_by_ranking() {
        let config = Config {
            profiles: BTreeMap::from([(
                "custom".to_string(),
                Profile {
                    include: vec![".env".to_string()],
                },
            )]),
            ..Config::default()
        };
        let original = config.clone();
        let mut sources = vec![source(".env", SourceKind::Dotenv, Some(".env"))];

        rank_sources(&mut sources, &config, Some("custom"), None).unwrap();

        assert_eq!(config, original);
    }
}
