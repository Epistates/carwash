//! Outdated and vulnerable dependency detection across ecosystems.
//!
//! Declared dependencies come from manifests, current versions from lockfiles or installs,
//! and available versions straight from each registry (crates.io sparse index, npm, PyPI, Go
//! module proxy): no toolchain is needed to check. Applying updates goes through the
//! ecosystem's own tool (`cargo update`, `pnpm update`, `uv lock --upgrade-package`, `go get`).
//!
//! Lookups are deduplicated across projects and cached on disk, so checking a directory of
//! hundreds of projects costs one request per distinct package.

mod cargo;
mod go;
mod npm;
mod python;

use crate::ecosystem::{EcoId, Registry};
use crate::tasks::Task;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A package registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Crates,
    Npm,
    Pypi,
    Go,
}

impl Source {
    pub fn key(self) -> &'static str {
        match self {
            Source::Crates => "crates",
            Source::Npm => "npm",
            Source::Pypi => "pypi",
            Source::Go => "go",
        }
    }

    /// Ecosystem name in the OSV schema.
    fn osv(self) -> &'static str {
        match self {
            Source::Crates => "crates.io",
            Source::Npm => "npm",
            Source::Pypi => "PyPI",
            Source::Go => "Go",
        }
    }

    fn adapter(self) -> &'static dyn Adapter {
        match self {
            Source::Crates => &cargo::Cargo,
            Source::Npm => &npm::Npm,
            Source::Pypi => &python::Python,
            Source::Go => &go::Go,
        }
    }

    /// The registry used by projects of the ecosystem with key `ecosystem`.
    pub fn for_ecosystem(ecosystem: &str) -> Option<Self> {
        Some(match ecosystem {
            "rust" => Source::Crates,
            "node" => Source::Npm,
            "python" => Source::Pypi,
            "go" => Source::Go,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DepKind {
    Normal,
    Dev,
    Build,
    Optional,
}

impl DepKind {
    pub fn label(self) -> &'static str {
        match self {
            DepKind::Normal => "",
            DepKind::Dev => "dev",
            DepKind::Build => "build",
            DepKind::Optional => "optional",
        }
    }
}

/// A dependency as written in a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Declared {
    pub name: String,
    /// Version requirement as written (`^1.2`, `>=2,<3`, `v1.4.0`).
    pub requirement: String,
    /// Version currently locked or installed.
    pub current: Option<String>,
    pub kind: DepKind,
    pub source: Source,
    /// Manifest to edit when changing the requirement (a workspace root for inherited deps).
    pub manifest: PathBuf,
}

/// How far a newer version is from the current one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Bump {
    #[default]
    None,
    Patch,
    Minor,
    /// Breaking by semver rules (first non-zero component changes).
    Major,
}

#[derive(Debug, Clone, Serialize)]
pub struct Dependency {
    #[serde(flatten)]
    pub declared: Declared,
    /// Newest version the requirement allows.
    pub wanted: Option<String>,
    /// Newest version published.
    pub latest: Option<String>,
    /// From current to latest.
    pub bump: Bump,
    /// From current to wanted.
    pub wanted_bump: Bump,
    /// OSV advisory ids affecting the current version.
    pub vulnerabilities: Vec<String>,
    pub error: Option<String>,
}

impl Dependency {
    pub fn is_outdated(&self) -> bool {
        self.bump != Bump::None
    }
}

impl ProjectDeps {
    /// Dependencies to list for this project when `checked` projects are shown together:
    /// inherited ones (a workspace `[workspace.dependencies]` entry) are listed under the
    /// project owning the manifest, if it is among them.
    pub fn listed<'a>(&'a self, checked: &'a [PathBuf]) -> impl Iterator<Item = &'a Dependency> {
        self.dependencies.iter().filter(move |dep| {
            let owner = dep.declared.manifest.parent();
            owner == Some(self.path.as_path())
                || !owner.is_some_and(|o| checked.iter().any(|c| c == o))
        })
    }
}

/// Dependencies of one project.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectDeps {
    pub path: PathBuf,
    pub dependencies: Vec<Dependency>,
    pub errors: Vec<String>,
}

/// What to check.
#[derive(Debug, Clone)]
pub struct ProjectSpec {
    pub path: PathBuf,
    pub ecosystems: Vec<EcoId>,
}

/// Published versions of a package.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Releases {
    /// Installable versions (not yanked).
    pub versions: Vec<String>,
    /// The registry's own "latest", when it has one.
    pub latest: Option<String>,
}

/// Per-ecosystem behaviour.
trait Adapter: Sync {
    fn declared(&self, dir: &Path) -> Result<Vec<Declared>, String>;
    fn url(&self, name: &str) -> String;
    fn accept(&self) -> Option<&'static str> {
        None
    }
    fn parse(&self, body: &str) -> Result<Releases, String>;
    /// (wanted, latest) for `dep` given published `releases`.
    fn resolve(&self, dep: &Declared, releases: &Releases) -> (Option<String>, Option<String>);
    fn bump(&self, from: &str, to: &str) -> Bump;
    /// Commands (and manifest edits, for `latest`) that apply updates to `deps`.
    fn update(&self, dir: &Path, deps: &[&Dependency], latest: bool) -> Result<Vec<Task>, String>;
}

/// Semver distance, where a change in the first non-zero component is breaking.
pub(crate) fn semver_bump(from: (u64, u64, u64), to: (u64, u64, u64)) -> Bump {
    if to <= from {
        return Bump::None;
    }
    let breaking = match from {
        (0, 0, _) => true,
        (0, minor, _) => to.0 != 0 || to.1 != minor,
        (major, ..) => to.0 != major,
    };
    if breaking {
        Bump::Major
    } else if to.1 != from.1 {
        Bump::Minor
    } else {
        Bump::Patch
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: HashMap<String, CachedReleases>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedReleases {
    releases: Releases,
    fetched_at: u64,
}

/// A project's path, declared dependencies and the errors met reading them.
type Collected = (PathBuf, Vec<Declared>, Vec<String>);
/// Advisory ids by (registry, package, version).
type Advisories = HashMap<(Source, String, String), Vec<String>>;

const CACHE_VERSION: u32 = 1;
const WORKERS: usize = 16;
const USER_AGENT: &str = concat!(
    "carwash/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/epistates/carwash)"
);

/// Progress of a check, for display.
#[derive(Debug, Default)]
pub struct CheckProgress {
    pub total: AtomicUsize,
    pub done: AtomicUsize,
}

/// Checks dependencies against registries, with a persistent cache.
pub struct Checker {
    agent: ureq::Agent,
    cache: Mutex<CacheFile>,
    cache_path: Option<PathBuf>,
    ttl: Duration,
    osv_url: String,
}

impl std::fmt::Debug for Checker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checker")
            .field("cache_path", &self.cache_path)
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

impl Checker {
    /// `cache_path` persists registry responses; entries younger than `ttl` are reused.
    pub fn new(cache_path: Option<PathBuf>, ttl: Duration) -> Self {
        let cache = cache_path
            .as_deref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|c| c.version == CACHE_VERSION)
            .unwrap_or(CacheFile {
                version: CACHE_VERSION,
                entries: HashMap::new(),
            });
        let agent = ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .build()
            .new_agent();
        Self {
            agent,
            cache: Mutex::new(cache),
            cache_path,
            ttl,
            osv_url: "https://api.osv.dev/v1/querybatch".into(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.cache_path else {
            return Ok(());
        };
        let mut cache = self.cache.lock().expect("deps cache lock");
        let cutoff = unix_now().saturating_sub(30 * 86_400);
        cache.entries.retain(|_, e| e.fetched_at >= cutoff);
        let json = serde_json::to_vec(&*cache).map_err(std::io::Error::other)?;
        crate::paths::write_atomic(path, &json)
    }

    fn get(&self, url: &str, accept: Option<&str>) -> Result<String, String> {
        let mut request = self.agent.get(url);
        if let Some(accept) = accept {
            request = request.header("Accept", accept);
        }
        let response = request.call().map_err(|e| e.to_string())?;
        let status = response.status();
        if status == 404 || status == 410 {
            return Err("not found in the registry".into());
        }
        if !status.is_success() {
            return Err(format!("registry returned {status}"));
        }
        response
            .into_body()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| e.to_string())
    }

    fn releases(&self, source: Source, name: &str, refresh: bool) -> Result<Releases, String> {
        let key = format!("{}:{name}", source.key());
        if !refresh {
            let cache = self.cache.lock().expect("deps cache lock");
            if let Some(entry) = cache.entries.get(&key)
                && unix_now().saturating_sub(entry.fetched_at) < self.ttl.as_secs()
            {
                return Ok(entry.releases.clone());
            }
        }
        let adapter = source.adapter();
        let body = self.get(&adapter.url(name), adapter.accept())?;
        let releases = adapter.parse(&body)?;
        self.cache.lock().expect("deps cache lock").entries.insert(
            key,
            CachedReleases {
                releases: releases.clone(),
                fetched_at: unix_now(),
            },
        );
        Ok(releases)
    }

    /// Checks every project. Registry lookups are shared across projects.
    pub fn check(
        &self,
        projects: &[ProjectSpec],
        registry: &Registry,
        vulnerabilities: bool,
        refresh: bool,
        progress: &CheckProgress,
    ) -> Vec<ProjectDeps> {
        // Reading manifests and lockfiles is I/O-bound too: spread it over the workers.
        let slots: Vec<Mutex<Option<Collected>>> =
            projects.iter().map(|_| Mutex::new(None)).collect();
        let next_project = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..WORKERS.min(projects.len()) {
                scope.spawn(|| {
                    loop {
                        let index = next_project.fetch_add(1, Ordering::Relaxed);
                        let Some(project) = projects.get(index) else {
                            break;
                        };
                        let mut deps = Vec::new();
                        let mut errors = Vec::new();
                        for source in project
                            .ecosystems
                            .iter()
                            .filter_map(|&e| Source::for_ecosystem(&registry.ecosystem(e).key))
                        {
                            match source.adapter().declared(&project.path) {
                                Ok(found) => deps.extend(found),
                                Err(error) => errors.push(error),
                            }
                        }
                        *slots[index].lock().expect("slot lock") =
                            Some((project.path.clone(), deps, errors));
                    }
                });
            }
        });
        let declared: Vec<Collected> = slots
            .into_iter()
            .filter_map(|slot| slot.into_inner().expect("slot lock"))
            .collect();

        let mut unique: Vec<(Source, String)> = declared
            .iter()
            .flat_map(|(_, deps, _)| deps.iter().map(|d| (d.source, d.name.clone())))
            .collect();
        unique.sort();
        unique.dedup();
        progress.total.store(unique.len(), Ordering::Relaxed);

        let fetched: Mutex<HashMap<(Source, String), Result<Releases, String>>> =
            Mutex::new(HashMap::new());
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..WORKERS.min(unique.len()) {
                scope.spawn(|| {
                    while let Some((source, name)) =
                        unique.get(next.fetch_add(1, Ordering::Relaxed))
                    {
                        let result = self.releases(*source, name, refresh);
                        fetched
                            .lock()
                            .expect("fetched lock")
                            .insert((*source, name.clone()), result);
                        progress.done.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
        });
        let fetched = fetched.into_inner().expect("fetched lock");

        let mut results: Vec<ProjectDeps> = declared
            .into_iter()
            .map(|(path, deps, errors)| {
                let dependencies = deps
                    .into_iter()
                    .map(|declared| resolve(declared, &fetched))
                    .collect();
                ProjectDeps {
                    path,
                    dependencies,
                    errors,
                }
            })
            .collect();

        if vulnerabilities {
            match self.vulnerabilities(&results) {
                Ok(found) => {
                    for project in &mut results {
                        for dep in &mut project.dependencies {
                            if let Some(current) = &dep.declared.current
                                && let Some(ids) = found.get(&(
                                    dep.declared.source,
                                    dep.declared.name.clone(),
                                    current.clone(),
                                ))
                            {
                                dep.vulnerabilities = ids.clone();
                            }
                        }
                    }
                }
                Err(error) => {
                    for project in &mut results {
                        project
                            .errors
                            .push(format!("vulnerability check failed: {error}"));
                    }
                }
            }
        }
        results
    }

    /// Queries OSV for every distinct (package, current version).
    fn vulnerabilities(&self, projects: &[ProjectDeps]) -> Result<Advisories, String> {
        let mut queries: Vec<(Source, String, String)> = projects
            .iter()
            .flat_map(|p| &p.dependencies)
            .filter_map(|d| {
                d.declared
                    .current
                    .clone()
                    .map(|v| (d.declared.source, d.declared.name.clone(), v))
            })
            .collect();
        queries.sort();
        queries.dedup();
        let mut found = HashMap::new();
        for chunk in queries.chunks(1000) {
            let body = serde_json::json!({
                "queries": chunk.iter().map(|(source, name, version)| serde_json::json!({
                    "package": { "name": name, "ecosystem": source.osv() },
                    "version": version.trim_start_matches('v'),
                })).collect::<Vec<_>>()
            });
            let response = self
                .agent
                .post(&self.osv_url)
                .send_json(&body)
                .map_err(|e| e.to_string())?;
            if !response.status().is_success() {
                return Err(format!("OSV returned {}", response.status()));
            }
            let parsed: serde_json::Value = response
                .into_body()
                .read_json()
                .map_err(|e| e.to_string())?;
            let results = parsed["results"].as_array().cloned().unwrap_or_default();
            for (query, result) in chunk.iter().zip(results) {
                let ids: Vec<String> = result["vulns"]
                    .as_array()
                    .map(|v| {
                        v.iter()
                            .filter_map(|x| x["id"].as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                if !ids.is_empty() {
                    found.insert(query.clone(), ids);
                }
            }
        }
        Ok(found)
    }
}

fn resolve(
    declared: Declared,
    fetched: &HashMap<(Source, String), Result<Releases, String>>,
) -> Dependency {
    let adapter = declared.source.adapter();
    let mut dep = Dependency {
        declared,
        wanted: None,
        latest: None,
        bump: Bump::None,
        wanted_bump: Bump::None,
        vulnerabilities: Vec::new(),
        error: None,
    };
    match fetched.get(&(dep.declared.source, dep.declared.name.clone())) {
        Some(Ok(releases)) => {
            let (wanted, latest) = adapter.resolve(&dep.declared, releases);
            if let Some(current) = &dep.declared.current {
                if let Some(latest) = &latest {
                    dep.bump = adapter.bump(current, latest);
                }
                if let Some(wanted) = &wanted {
                    dep.wanted_bump = adapter.bump(current, wanted);
                }
            }
            dep.wanted = wanted;
            dep.latest = latest;
        }
        Some(Err(error)) => dep.error = Some(error.clone()),
        None => dep.error = Some("not checked".into()),
    }
    dep
}

/// Commands that update `deps` (all from the project at `dir`, one ecosystem each).
/// With `latest`, requirements are raised in the manifests first where the tool needs it.
pub fn update_tasks(dir: &Path, deps: &[&Dependency], latest: bool) -> Result<Vec<Task>, String> {
    let mut by_source: Vec<(Source, Vec<&Dependency>)> = Vec::new();
    for dep in deps {
        match by_source
            .iter_mut()
            .find(|(s, _)| *s == dep.declared.source)
        {
            Some((_, list)) => list.push(dep),
            None => by_source.push((dep.declared.source, vec![dep])),
        }
    }
    let mut tasks = Vec::new();
    for (source, list) in by_source {
        tasks.extend(source.adapter().update(dir, &list, latest)?);
    }
    Ok(tasks)
}

/// Finds `name` in `dir` or its ancestors (lockfiles live at workspace roots).
pub(crate) fn find_upwards(dir: &Path, name: &str, levels: usize) -> Option<PathBuf> {
    dir.ancestors()
        .take(levels)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

pub(crate) fn read_capped(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 64 * 1024 * 1024 {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

pub(crate) fn task(dir: &Path, name: &str, argv: Vec<String>, description: String) -> Task {
    let mut argv = argv.into_iter();
    Task {
        name: name.to_owned(),
        source: "update".into(),
        program: argv.next().unwrap_or_default(),
        args: argv.collect(),
        description: Some(description),
        cwd: dir.to_path_buf(),
        standard: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_bumps_follow_breaking_rules() {
        assert_eq!(semver_bump((1, 2, 3), (1, 2, 3)), Bump::None);
        assert_eq!(semver_bump((1, 2, 3), (1, 2, 4)), Bump::Patch);
        assert_eq!(semver_bump((1, 2, 3), (1, 3, 0)), Bump::Minor);
        assert_eq!(semver_bump((1, 2, 3), (2, 0, 0)), Bump::Major);
        assert_eq!(semver_bump((0, 3, 1), (0, 3, 2)), Bump::Patch);
        assert_eq!(semver_bump((0, 3, 1), (0, 4, 0)), Bump::Major);
        assert_eq!(semver_bump((0, 0, 3), (0, 0, 4)), Bump::Major);
        assert_eq!(semver_bump((2, 0, 0), (1, 9, 9)), Bump::None);
    }

    #[test]
    fn resolve_records_registry_errors() {
        let declared = Declared {
            name: "missing".into(),
            requirement: "1".into(),
            current: Some("1.0.0".into()),
            kind: DepKind::Normal,
            source: Source::Crates,
            manifest: PathBuf::from("Cargo.toml"),
        };
        let mut fetched = HashMap::new();
        fetched.insert(
            (Source::Crates, "missing".to_string()),
            Err("not found in the registry".to_string()),
        );
        let dep = resolve(declared, &fetched);
        assert_eq!(dep.error.as_deref(), Some("not found in the registry"));
        assert!(!dep.is_outdated());
    }
}
