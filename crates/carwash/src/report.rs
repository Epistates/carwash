//! JSON output shapes. Kept separate from the engine model so the CLI contract can stay
//! stable while internals change; `version` is bumped on breaking changes.

use carwash_core::select::{Filter, Policy};
use carwash_core::{
    Artifact, ArtifactKind, Detection, GitState, Project, Registry, Safety, Size, Snapshot,
};
use serde::Serialize;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const VERSION: u32 = 1;

fn unix(t: Option<SystemTime>) -> Option<u64> {
    t.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

#[derive(Serialize)]
pub struct ProjectOut<'a> {
    pub id: u32,
    pub path: &'a Path,
    pub name: &'a str,
    pub ecosystems: Vec<&'a str>,
    pub parent: Option<u32>,
    pub member_of: Option<u32>,
    pub is_workspace: bool,
    pub repo: Option<&'a Path>,
    pub last_activity: Option<u64>,
    pub outside_root: bool,
}

impl<'a> ProjectOut<'a> {
    pub fn new(project: &'a Project, registry: &'a Registry) -> Self {
        Self {
            id: project.id.0,
            path: &project.path,
            name: &project.name,
            ecosystems: project
                .ecosystems
                .iter()
                .map(|&e| registry.ecosystem(e).key.as_str())
                .collect(),
            parent: project.parent.map(|p| p.0),
            member_of: project.member_of.map(|p| p.0),
            is_workspace: project.is_workspace,
            repo: project.repo.as_deref(),
            last_activity: unix(project.last_activity),
            outside_root: project.outside_root,
        }
    }
}

#[derive(Serialize)]
pub struct ArtifactOut<'a> {
    pub id: u32,
    pub path: &'a Path,
    pub project: Option<u32>,
    pub ecosystem: Option<&'a str>,
    pub kind: ArtifactKind,
    pub detected_by: &'static str,
    /// Rule path, for rule-based detections.
    pub rule: Option<&'a str>,
    pub confirmed: bool,
    pub ambiguous: bool,
    pub git: GitState,
    pub safety: Safety,
    /// Why the default policy would not select it, if it would not.
    pub hold: Option<&'static str>,
    pub size: Option<Size>,
    pub modified: Option<u64>,
    pub outside_root: bool,
    pub regenerate: Option<&'a str>,
}

impl<'a> ArtifactOut<'a> {
    pub fn new(
        artifact: &'a Artifact,
        registry: &'a Registry,
        policy: &Policy,
        now: SystemTime,
    ) -> Self {
        let (detected_by, rule) = match artifact.detection {
            Detection::Rule { rule, .. } => ("rule", Some(registry.rule(rule))),
            Detection::CacheDirTag => ("cachedir_tag", None),
            Detection::PythonVenv => ("pyvenv_cfg", None),
            Detection::CondaEnv => ("conda_meta", None),
            Detection::CMakeBuild => ("cmake_cache", None),
            Detection::MesonBuild => ("meson_private", None),
            Detection::CargoFingerprint => ("cargo_fingerprint", None),
            Detection::CarwashLeftover => ("carwash_leftover", None),
        };
        Self {
            id: artifact.id.0,
            path: &artifact.path,
            project: artifact.project.map(|p| p.0),
            ecosystem: artifact
                .ecosystem
                .map(|e| registry.ecosystem(e).key.as_str()),
            kind: artifact.kind,
            detected_by,
            rule: rule.map(|r| r.path.as_str()),
            confirmed: artifact.detection.is_certain(),
            ambiguous: artifact.ambiguous,
            git: artifact.git,
            safety: artifact.safety(),
            hold: policy.hold(artifact, now).map(|h| h.describe()),
            size: artifact.size,
            modified: unix(artifact.last_modified()),
            outside_root: artifact.outside_root,
            regenerate: rule.and_then(|r| r.regenerate.as_deref()),
        }
    }
}

#[derive(Serialize, Default)]
pub struct Totals {
    pub artifacts: usize,
    pub on_disk: u64,
    pub reclaimable: u64,
}

impl Totals {
    pub fn add(&mut self, artifact: &Artifact) {
        self.artifacts += 1;
        if let Some(size) = artifact.size {
            self.on_disk += size.on_disk;
            self.reclaimable += size.reclaimable;
        }
    }
}

#[derive(Serialize)]
pub struct ScanReport<'a> {
    pub version: u32,
    pub root: &'a Path,
    pub elapsed_ms: u128,
    pub directories: u64,
    /// Every artifact matching the filters.
    pub total: Totals,
    /// The subset the default policy would clean.
    pub selectable: Totals,
    pub projects: Vec<ProjectOut<'a>>,
    pub artifacts: Vec<ArtifactOut<'a>>,
    pub warnings: usize,
}

impl<'a> ScanReport<'a> {
    pub fn new(
        root: &'a Path,
        snapshot: &'a Snapshot,
        registry: &'a Registry,
        filter: &Filter,
        policy: &Policy,
    ) -> Self {
        let now = SystemTime::now();
        let mut total = Totals::default();
        let mut selectable = Totals::default();
        let artifacts: Vec<ArtifactOut<'a>> = snapshot
            .artifacts
            .iter()
            .filter(|a| filter.matches(a, now))
            .inspect(|a| {
                total.add(a);
                if policy.hold(a, now).is_none() {
                    selectable.add(a);
                }
            })
            .map(|a| ArtifactOut::new(a, registry, policy, now))
            .collect();
        Self {
            version: VERSION,
            root,
            elapsed_ms: snapshot.elapsed.as_millis(),
            directories: snapshot.dirs,
            total,
            selectable,
            projects: snapshot
                .projects
                .iter()
                .map(|p| ProjectOut::new(p, registry))
                .collect(),
            artifacts,
            warnings: snapshot.warnings.len(),
        }
    }
}
