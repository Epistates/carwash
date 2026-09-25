//! Declarative ecosystem rules and the compiled registry used during discovery.

pub mod adapters;

use crate::model::ArtifactKind;
use globset::{Glob, GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub use adapters::Adapter;

const BUILTIN_RULES: &str = include_str!("builtin.toml");
const SCHEMA_VERSION: u32 = 1;

/// Index of an ecosystem in its [`Registry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EcoId(pub u16);

/// Index of an artifact rule in its [`Registry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuleId(pub u32);

#[derive(Debug, thiserror::Error)]
pub enum RulesError {
    #[error("invalid rules file: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("unsupported rules schema {found} (expected {SCHEMA_VERSION})")]
    Schema { found: u32 },
    #[error("ecosystem `{ecosystem}`: invalid pattern `{pattern}`: {source}")]
    Pattern {
        ecosystem: String,
        pattern: String,
        source: globset::Error,
    },
    #[error("ecosystem `{ecosystem}`: unknown adapter `{adapter}`")]
    Adapter { ecosystem: String, adapter: String },
    #[error("ecosystem `{ecosystem}`: {message}")]
    Invalid { ecosystem: String, message: String },
}

/// On-disk rules file.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RulesFile {
    pub schema: u32,
    #[serde(default)]
    pub ecosystem: Vec<EcosystemSpec>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EcosystemSpec {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub badge: Option<String>,
    #[serde(default)]
    pub markers: Vec<String>,
    #[serde(default)]
    pub weak_markers: Vec<String>,
    #[serde(default)]
    pub lockfiles: Vec<String>,
    #[serde(default)]
    pub adapter: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<ArtifactSpec>,
    #[serde(default)]
    pub tasks: Vec<TaskSpec>,
}

/// A standard command every project of an ecosystem supports (`cargo test`, `go vet ./...`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub name: String,
    /// Program and arguments. `{pm}` is replaced by the project's JavaScript package manager.
    pub run: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Only offered when this path exists in the project (e.g. `gradlew`).
    #[serde(default)]
    pub when: Option<String>,
    /// Not offered when this path exists.
    #[serde(default)]
    pub unless: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSpec {
    pub path: String,
    pub kind: ArtifactKind,
    #[serde(default)]
    pub anywhere: bool,
    /// Also matches outside projects of this ecosystem; for names specific enough to be
    /// unmistakable (`node_modules`, `__pycache__`).
    #[serde(default)]
    pub global: bool,
    #[serde(default)]
    pub ambiguous: bool,
    #[serde(default)]
    pub confirm: Vec<String>,
    #[serde(default)]
    pub require_confirm: bool,
    #[serde(default)]
    pub regenerate: Option<String>,
}

/// One path segment of a marker or artifact pattern.
#[derive(Debug, Clone)]
enum Segment {
    Exact(String),
    Glob(GlobMatcher),
}

impl Segment {
    fn parse(ecosystem: &str, raw: &str) -> Result<Self, RulesError> {
        if raw.contains(['*', '?', '[', '{']) {
            let glob = glob(raw).map_err(|source| RulesError::Pattern {
                ecosystem: ecosystem.to_owned(),
                pattern: raw.to_owned(),
                source,
            })?;
            Ok(Segment::Glob(glob.compile_matcher()))
        } else {
            Ok(Segment::Exact(raw.to_owned()))
        }
    }

    fn matches(&self, name: &str) -> bool {
        match self {
            Segment::Exact(exact) => exact == name,
            Segment::Glob(glob) => glob.is_match(name),
        }
    }
}

fn glob(raw: &str) -> Result<Glob, globset::Error> {
    GlobBuilder::new(raw).literal_separator(true).build()
}

fn split_segments(ecosystem: &str, raw: &str) -> Result<Vec<Segment>, RulesError> {
    let segments: Vec<&str> = raw.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() || segments.iter().any(|s| *s == "." || *s == "..") {
        return Err(RulesError::Invalid {
            ecosystem: ecosystem.to_owned(),
            message: format!("path `{raw}` must be relative and non-empty"),
        });
    }
    segments
        .into_iter()
        .map(|s| Segment::parse(ecosystem, s))
        .collect()
}

/// A compiled ecosystem.
#[derive(Debug)]
pub struct Ecosystem {
    pub id: EcoId,
    pub key: String,
    pub name: String,
    pub badge: String,
    /// Strong and weak markers as written in the rules.
    pub markers: Vec<String>,
    pub lockfiles: Vec<String>,
    pub adapter: Option<Adapter>,
    pub rules: Vec<RuleId>,
    pub tasks: Vec<TaskSpec>,
    /// Marker and lockfile names checked for activity timestamps.
    pub(crate) activity_files: Vec<String>,
}

/// A compiled artifact rule.
#[derive(Debug)]
pub struct ArtifactRule {
    pub id: RuleId,
    pub eco: EcoId,
    pub path: String,
    segments: Vec<Segment>,
    pub kind: ArtifactKind,
    pub anywhere: bool,
    pub global: bool,
    pub ambiguous: bool,
    confirm: Option<GlobSet>,
    pub require_confirm: bool,
    pub regenerate: Option<String>,
}

impl ArtifactRule {
    pub(crate) fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub(crate) fn segment_matches(&self, index: usize, name: &str) -> bool {
        self.segments.get(index).is_some_and(|s| s.matches(name))
    }

    pub(crate) fn has_confirm(&self) -> bool {
        self.confirm.is_some()
    }

    pub(crate) fn confirms(&self, child_name: &str) -> bool {
        self.confirm
            .as_ref()
            .is_some_and(|c| c.is_match(child_name))
    }
}

#[derive(Debug, Clone)]
struct MarkerRef {
    eco: EcoId,
    weak: bool,
    /// Remaining segments for nested markers such as `ProjectSettings/ProjectVersion.txt`.
    rest: Option<String>,
}

/// Index from a directory entry name to the patterns whose first segment it matches.
#[derive(Debug)]
struct SegmentIndex<T> {
    exact: HashMap<String, Vec<T>>,
    globs: GlobSet,
    glob_targets: Vec<T>,
}

impl<T: Clone> SegmentIndex<T> {
    fn build(entries: Vec<(Segment, String, T)>) -> Self {
        let mut exact: HashMap<String, Vec<T>> = HashMap::new();
        let mut builder = GlobSetBuilder::new();
        let mut glob_targets = Vec::new();
        for (segment, raw, target) in entries {
            match segment {
                Segment::Exact(name) => exact.entry(name).or_default().push(target),
                Segment::Glob(_) => {
                    builder.add(glob(&raw).expect("validated when the segment was parsed"));
                    glob_targets.push(target);
                }
            }
        }
        Self {
            exact,
            globs: builder.build().expect("validated globs"),
            glob_targets,
        }
    }

    fn lookup(&self, name: &str, out: &mut Vec<T>) {
        if let Some(targets) = self.exact.get(name) {
            out.extend(targets.iter().cloned());
        }
        if !self.glob_targets.is_empty() {
            for i in self.globs.matches(name) {
                out.push(self.glob_targets[i].clone());
            }
        }
    }
}

/// Result of checking a directory's entries for project markers.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MarkerMatch {
    /// Ecosystems with a strong marker present.
    pub strong: Vec<EcoId>,
    /// Ecosystems with only weak markers present.
    pub weak: Vec<EcoId>,
}

impl MarkerMatch {
    pub fn is_empty(&self) -> bool {
        self.strong.is_empty() && self.weak.is_empty()
    }
}

/// Compiled set of ecosystem rules.
#[derive(Debug)]
pub struct Registry {
    ecosystems: Vec<Ecosystem>,
    rules: Vec<ArtifactRule>,
    markers: SegmentIndex<MarkerRef>,
    /// Root-level rules per ecosystem, indexed by first segment.
    root_rules: Vec<SegmentIndex<RuleId>>,
    /// Rules matching at any depth below a project, per ecosystem.
    anywhere_rules: Vec<SegmentIndex<RuleId>>,
    /// Rules matching anywhere, even outside projects.
    global_rules: SegmentIndex<RuleId>,
    by_key: HashMap<String, EcoId>,
}

impl Registry {
    /// The built-in rules only.
    pub fn builtin() -> Self {
        Self::from_specs(builtin_specs()).expect("built-in rules are valid")
    }

    /// Built-in rules with user overrides applied: an override with an existing `id`
    /// replaces that ecosystem, a new `id` is appended.
    pub fn with_overrides(user_rules: &str) -> Result<Self, RulesError> {
        let file = parse_rules(user_rules)?;
        let mut specs = builtin_specs();
        for spec in file.ecosystem {
            match specs.iter_mut().find(|s| s.id == spec.id) {
                Some(existing) => *existing = spec,
                None => specs.push(spec),
            }
        }
        Self::from_specs(specs)
    }

    pub fn from_specs(specs: Vec<EcosystemSpec>) -> Result<Self, RulesError> {
        let mut ecosystems = Vec::with_capacity(specs.len());
        let mut rules = Vec::new();
        let mut marker_entries = Vec::new();
        let mut root_entries: Vec<Vec<(Segment, String, RuleId)>> = Vec::new();
        let mut anywhere_entries: Vec<Vec<(Segment, String, RuleId)>> = Vec::new();
        let mut global_entries = Vec::new();
        let mut by_key = HashMap::new();

        for (index, spec) in specs.into_iter().enumerate() {
            let eco = EcoId(u16::try_from(index).map_err(|_| RulesError::Invalid {
                ecosystem: spec.id.clone(),
                message: "too many ecosystems".into(),
            })?);
            if by_key.insert(spec.id.clone(), eco).is_some() {
                return Err(RulesError::Invalid {
                    ecosystem: spec.id,
                    message: "duplicate id".into(),
                });
            }
            if let Some(task) = spec
                .tasks
                .iter()
                .find(|t| t.run.is_empty() || t.name.is_empty())
            {
                return Err(RulesError::Invalid {
                    ecosystem: spec.id,
                    message: format!("task `{}` needs a name and a command", task.name),
                });
            }
            if spec.markers.is_empty() && spec.weak_markers.is_empty() {
                return Err(RulesError::Invalid {
                    ecosystem: spec.id,
                    message: "needs at least one marker".into(),
                });
            }
            let adapter = spec
                .adapter
                .as_deref()
                .map(|name| {
                    Adapter::parse(name).ok_or_else(|| RulesError::Adapter {
                        ecosystem: spec.id.clone(),
                        adapter: name.to_owned(),
                    })
                })
                .transpose()?;

            for (raw, weak) in spec
                .markers
                .iter()
                .map(|m| (m, false))
                .chain(spec.weak_markers.iter().map(|m| (m, true)))
            {
                let (first, rest) = match raw.split_once('/') {
                    Some((first, rest)) => (first, Some(rest.to_owned())),
                    None => (raw.as_str(), None),
                };
                marker_entries.push((
                    Segment::parse(&spec.id, first)?,
                    first.to_owned(),
                    MarkerRef { eco, weak, rest },
                ));
            }

            let mut roots = Vec::new();
            let mut anywhere = Vec::new();
            let mut eco_rules = Vec::new();
            for artifact in &spec.artifacts {
                let id = RuleId(rules.len() as u32);
                let segments = split_segments(&spec.id, &artifact.path)?;
                if (artifact.anywhere || artifact.global) && segments.len() != 1 {
                    return Err(RulesError::Invalid {
                        ecosystem: spec.id.clone(),
                        message: format!(
                            "`anywhere`/`global` artifacts must be a single segment: `{}`",
                            artifact.path
                        ),
                    });
                }
                let first_raw = artifact
                    .path
                    .split('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let entry = (segments[0].clone(), first_raw, id);
                if artifact.global {
                    global_entries.push(entry.clone());
                }
                if artifact.anywhere {
                    anywhere.push(entry);
                } else {
                    roots.push(entry);
                }
                let confirm = if artifact.confirm.is_empty() {
                    None
                } else {
                    let mut builder = GlobSetBuilder::new();
                    for pattern in &artifact.confirm {
                        builder.add(glob(pattern).map_err(|source| RulesError::Pattern {
                            ecosystem: spec.id.clone(),
                            pattern: pattern.clone(),
                            source,
                        })?);
                    }
                    Some(builder.build().map_err(|source| RulesError::Pattern {
                        ecosystem: spec.id.clone(),
                        pattern: artifact.confirm.join(", "),
                        source,
                    })?)
                };
                if artifact.require_confirm && confirm.is_none() {
                    return Err(RulesError::Invalid {
                        ecosystem: spec.id.clone(),
                        message: format!(
                            "`{}` requires confirmation but lists none",
                            artifact.path
                        ),
                    });
                }
                rules.push(ArtifactRule {
                    id,
                    eco,
                    path: artifact.path.clone(),
                    segments,
                    kind: artifact.kind,
                    anywhere: artifact.anywhere,
                    global: artifact.global,
                    ambiguous: artifact.ambiguous,
                    confirm,
                    require_confirm: artifact.require_confirm,
                    regenerate: artifact.regenerate.clone(),
                });
                eco_rules.push(id);
            }
            root_entries.push(roots);
            anywhere_entries.push(anywhere);

            let activity_files = spec
                .markers
                .iter()
                .chain(&spec.lockfiles)
                .filter(|m| !m.contains(['*', '?', '[', '{', '/']))
                .cloned()
                .collect();
            let badge = spec
                .badge
                .clone()
                .unwrap_or_else(|| spec.id.chars().take(2).collect());
            ecosystems.push(Ecosystem {
                id: eco,
                key: spec.id,
                name: spec.name,
                badge,
                markers: spec
                    .markers
                    .iter()
                    .chain(&spec.weak_markers)
                    .cloned()
                    .collect(),
                lockfiles: spec.lockfiles,
                adapter,
                rules: eco_rules,
                tasks: spec.tasks,
                activity_files,
            });
        }

        Ok(Self {
            ecosystems,
            rules,
            markers: SegmentIndex::build(marker_entries),
            root_rules: root_entries.into_iter().map(SegmentIndex::build).collect(),
            anywhere_rules: anywhere_entries
                .into_iter()
                .map(SegmentIndex::build)
                .collect(),
            global_rules: SegmentIndex::build(global_entries),
            by_key,
        })
    }

    pub fn ecosystems(&self) -> &[Ecosystem] {
        &self.ecosystems
    }

    pub fn ecosystem(&self, id: EcoId) -> &Ecosystem {
        &self.ecosystems[usize::from(id.0)]
    }

    pub fn find(&self, key: &str) -> Option<EcoId> {
        self.by_key.get(key).copied()
    }

    pub fn rule(&self, id: RuleId) -> &ArtifactRule {
        &self.rules[id.0 as usize]
    }

    pub fn rules(&self) -> &[ArtifactRule] {
        &self.rules
    }

    /// Checks directory entry names for project markers. `dir` is used to resolve nested
    /// markers, which cost one `stat` each and only when their first segment is present.
    pub fn detect_markers<'a>(
        &self,
        dir: &Path,
        names: impl IntoIterator<Item = &'a str>,
    ) -> MarkerMatch {
        let mut refs = Vec::new();
        for name in names {
            let start = refs.len();
            self.markers.lookup(name, &mut refs);
            // Resolve nested markers relative to the matching entry.
            let mut i = start;
            while i < refs.len() {
                let keep = match &refs[i].rest {
                    Some(rest) => dir.join(name).join(rest).exists(),
                    None => true,
                };
                if keep {
                    i += 1;
                } else {
                    refs.swap_remove(i);
                }
            }
        }
        let mut result = MarkerMatch::default();
        for marker in &refs {
            if !marker.weak && !result.strong.contains(&marker.eco) {
                result.strong.push(marker.eco);
            }
        }
        for marker in &refs {
            if marker.weak
                && !result.strong.contains(&marker.eco)
                && !result.weak.contains(&marker.eco)
            {
                result.weak.push(marker.eco);
            }
        }
        result.strong.sort_unstable();
        result.weak.sort_unstable();
        result
    }

    /// Root-level rules of `eco` whose first segment matches `name`.
    pub(crate) fn root_rules_for(&self, eco: EcoId, name: &str, out: &mut Vec<RuleId>) {
        self.root_rules[usize::from(eco.0)].lookup(name, out);
    }

    /// `anywhere` rules of `eco` matching `name`.
    pub(crate) fn anywhere_rules_for(&self, eco: EcoId, name: &str, out: &mut Vec<RuleId>) {
        self.anywhere_rules[usize::from(eco.0)].lookup(name, out);
    }

    /// `global` rules of any ecosystem matching `name`.
    pub(crate) fn global_rules_for(&self, name: &str, out: &mut Vec<RuleId>) {
        self.global_rules.lookup(name, out);
    }

    pub(crate) fn has_anywhere_rules(&self, eco: EcoId) -> bool {
        let index = &self.anywhere_rules[usize::from(eco.0)];
        !index.exact.is_empty() || !index.glob_targets.is_empty()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::builtin()
    }
}

fn parse_rules(source: &str) -> Result<RulesFile, RulesError> {
    let file: RulesFile = toml::from_str(source)?;
    if file.schema != SCHEMA_VERSION {
        return Err(RulesError::Schema { found: file.schema });
    }
    Ok(file)
}

/// The built-in ecosystem specifications.
pub fn builtin_specs() -> Vec<EcosystemSpec> {
    parse_rules(BUILTIN_RULES)
        .expect("built-in rules parse")
        .ecosystem
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_rules_compile() {
        let registry = Registry::builtin();
        assert!(registry.ecosystems().len() >= 40);
        assert!(registry.find("rust").is_some());
        assert!(registry.find("node").is_some());
    }

    #[test]
    fn markers_match_exact_glob_and_weak() {
        let registry = Registry::builtin();
        let rust = registry.find("rust").unwrap();
        let dotnet = registry.find("dotnet").unwrap();
        let cmake = registry.find("cmake").unwrap();
        let found = registry.detect_markers(
            Path::new("/nonexistent"),
            ["Cargo.toml", "App.csproj", "CMakeLists.txt", "README.md"],
        );
        assert_eq!(found.strong, vec![rust, dotnet]);
        assert_eq!(found.weak, vec![cmake]);
    }

    #[test]
    fn nested_markers_require_the_full_path() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Registry::builtin();
        std::fs::create_dir(dir.path().join("ProjectSettings")).unwrap();
        assert!(
            registry
                .detect_markers(dir.path(), ["ProjectSettings"])
                .is_empty()
        );
        std::fs::write(dir.path().join("ProjectSettings/ProjectVersion.txt"), "").unwrap();
        let found = registry.detect_markers(dir.path(), ["ProjectSettings"]);
        assert_eq!(found.strong, vec![registry.find("unity").unwrap()]);
    }

    #[test]
    fn root_rules_lookup_by_first_segment() {
        let registry = Registry::builtin();
        let node = registry.find("node").unwrap();
        let mut out = Vec::new();
        registry.root_rules_for(node, "node_modules", &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(registry.rule(out[0]).path, "node_modules");
        out.clear();
        registry.root_rules_for(node, ".nx", &mut out);
        assert_eq!(
            out.len(),
            2,
            "both .nx/cache and .nx/workspace-data start with .nx"
        );
    }

    #[test]
    fn overrides_replace_and_extend() {
        let registry = Registry::with_overrides(
            r#"
            schema = 1
            [[ecosystem]]
            id = "rust"
            name = "Rust (custom)"
            markers = ["Cargo.toml"]
            artifacts = [{ path = "target", kind = "build" }, { path = "out", kind = "build" }]

            [[ecosystem]]
            id = "acme"
            name = "Acme"
            markers = ["acme.yml"]
            artifacts = [{ path = ".acme", kind = "cache" }]
            "#,
        )
        .unwrap();
        let rust = registry.find("rust").unwrap();
        assert_eq!(registry.ecosystem(rust).name, "Rust (custom)");
        assert_eq!(registry.ecosystem(rust).rules.len(), 2);
        assert!(registry.find("acme").is_some());
    }

    #[test]
    fn invalid_rules_are_rejected() {
        let bad_path = r#"
            schema = 1
            [[ecosystem]]
            id = "x"
            name = "X"
            markers = ["x"]
            artifacts = [{ path = "../escape", kind = "build" }]
        "#;
        assert!(matches!(
            Registry::with_overrides(bad_path),
            Err(RulesError::Invalid { .. })
        ));
        assert!(matches!(
            Registry::with_overrides("schema = 2"),
            Err(RulesError::Schema { found: 2 })
        ));
    }
}
