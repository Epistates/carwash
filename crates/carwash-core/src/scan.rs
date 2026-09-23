//! The scan pipeline: discovery, then git inspection and measurement in parallel, as one
//! stream of events.

use crate::Cancel;
use crate::clean::{self, CleanEvent, CleanItem, CleanOptions, CleanReport};
use crate::discover::{self, Counters, DiscoverOptions, Found, Warning};
use crate::ecosystem::Registry;
use crate::git;
use crate::measure;
use crate::model::{Artifact, ArtifactId, GitState, Project, Size};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub discover: DiscoverOptions,
    /// Ask git whether artifacts are ignored or contain tracked files.
    pub git: bool,
    /// Measure artifact sizes.
    pub measure: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            discover: DiscoverOptions::default(),
            git: true,
            measure: true,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ScanEvent {
    Project(Project),
    Artifact(Artifact),
    Warning(Warning),
    DiscoveryFinished { dirs: u64, elapsed: Duration },
    Git { id: ArtifactId, state: GitState },
    Measured { id: ArtifactId, size: Size },
    Finished { elapsed: Duration },
}

/// Artifact location needed by the git and measurement phases.
#[derive(Debug, Clone)]
pub struct Target {
    pub id: ArtifactId,
    pub path: PathBuf,
    pub repo: Option<PathBuf>,
}

impl From<&Artifact> for Target {
    fn from(a: &Artifact) -> Self {
        Self {
            id: a.id,
            path: a.path.clone(),
            repo: a.repo.clone(),
        }
    }
}

/// Owns the rule registry and the I/O thread pool.
#[derive(Debug, Clone)]
pub struct Engine {
    registry: Arc<Registry>,
    pool: Arc<rayon::ThreadPool>,
}

/// Directory walking is latency-bound, so more threads than cores keeps the disk queue full.
fn default_threads() -> usize {
    let cores = std::thread::available_parallelism().map_or(4, usize::from);
    (cores * 2).clamp(4, 32)
}

impl Engine {
    pub fn new(registry: Registry) -> Self {
        Self::with_threads(registry, None).expect("thread pool")
    }

    pub fn with_threads(
        registry: Registry,
        threads: Option<usize>,
    ) -> Result<Self, rayon::ThreadPoolBuildError> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads.unwrap_or_else(default_threads))
            .thread_name(|i| format!("carwash-io-{i}"))
            .build()?;
        Ok(Self {
            registry: Arc::new(registry),
            pool: Arc::new(pool),
        })
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    /// Runs the full pipeline, blocking the calling thread. `counters` may be read
    /// concurrently for progress display.
    pub fn scan(
        &self,
        root: &Path,
        options: &ScanOptions,
        counters: &Counters,
        cancel: &Cancel,
        sink: &(dyn Fn(ScanEvent) + Sync),
    ) {
        let started = Instant::now();
        let targets = Mutex::new(Vec::new());
        self.pool.install(|| {
            discover::discover(
                root,
                &self.registry,
                &options.discover,
                counters,
                cancel,
                &|found| match found {
                    Found::Project(p) => sink(ScanEvent::Project(p)),
                    Found::Artifact(a) => {
                        targets.lock().expect("targets lock").push(Target::from(&a));
                        sink(ScanEvent::Artifact(a));
                    }
                    Found::Warning(w) => sink(ScanEvent::Warning(w)),
                },
            );
        });
        sink(ScanEvent::DiscoveryFinished {
            dirs: counters.dirs.load(Ordering::Relaxed),
            elapsed: started.elapsed(),
        });
        let targets = targets.into_inner().expect("targets lock");
        if !cancel.is_cancelled() {
            self.pool.install(|| {
                rayon::join(
                    || {
                        if options.git {
                            inspect_git(&targets, sink);
                        }
                    },
                    || {
                        if options.measure {
                            measure_all(&targets, counters, cancel, sink);
                        }
                    },
                );
            });
        }
        sink(ScanEvent::Finished {
            elapsed: started.elapsed(),
        });
    }

    /// Re-measures specific artifacts, emitting `ScanEvent::Measured`.
    pub fn measure(
        &self,
        targets: &[Target],
        counters: &Counters,
        cancel: &Cancel,
        sink: &(dyn Fn(ScanEvent) + Sync),
    ) {
        self.pool
            .install(|| measure_all(targets, counters, cancel, sink));
    }

    /// Deletes artifacts; see [`clean::clean`].
    pub fn clean(
        &self,
        items: &[CleanItem],
        options: &CleanOptions,
        cancel: &Cancel,
        emit: &(dyn Fn(CleanEvent) + Sync),
    ) -> CleanReport {
        self.pool
            .install(|| clean::clean(items, options, cancel, emit))
    }
}

fn inspect_git(targets: &[Target], sink: &(dyn Fn(ScanEvent) + Sync)) {
    let mut by_repo: HashMap<&Path, Vec<&Target>> = HashMap::new();
    for target in targets {
        match &target.repo {
            Some(repo) => by_repo.entry(repo.as_path()).or_default().push(target),
            None => sink(ScanEvent::Git {
                id: target.id,
                state: GitState::NotInRepo,
            }),
        }
    }
    by_repo.into_par_iter().for_each(|(repo, targets)| {
        let paths: Vec<PathBuf> = targets.iter().map(|t| t.path.clone()).collect();
        for (target, state) in targets.iter().zip(git::inspect(repo, &paths)) {
            sink(ScanEvent::Git {
                id: target.id,
                state,
            });
        }
    });
}

fn measure_all(
    targets: &[Target],
    counters: &Counters,
    cancel: &Cancel,
    sink: &(dyn Fn(ScanEvent) + Sync),
) {
    targets.par_iter().for_each(|target| {
        if cancel.is_cancelled() {
            return;
        }
        let size = measure::measure(&target.path, cancel);
        if cancel.is_cancelled() {
            return;
        }
        counters.measured.fetch_add(1, Ordering::Relaxed);
        sink(ScanEvent::Measured {
            id: target.id,
            size,
        });
    });
}

/// Everything a scan produced, assembled from its events.
#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    pub projects: Vec<Project>,
    pub artifacts: Vec<Artifact>,
    pub warnings: Vec<Warning>,
    pub dirs: u64,
    pub elapsed: Duration,
    project_index: HashMap<crate::model::ProjectId, usize>,
    artifact_index: HashMap<ArtifactId, usize>,
}

impl Snapshot {
    pub fn apply(&mut self, event: ScanEvent) {
        match event {
            ScanEvent::Project(p) => {
                self.project_index.insert(p.id, self.projects.len());
                self.projects.push(p);
            }
            ScanEvent::Artifact(a) => {
                self.artifact_index.insert(a.id, self.artifacts.len());
                self.artifacts.push(a);
            }
            ScanEvent::Warning(w) => self.warnings.push(w),
            ScanEvent::DiscoveryFinished { dirs, .. } => self.dirs = dirs,
            ScanEvent::Git { id, state } => {
                if let Some(a) = self.artifact_mut(id) {
                    a.git = state;
                }
            }
            ScanEvent::Measured { id, size } => {
                if let Some(a) = self.artifact_mut(id) {
                    a.size = Some(size);
                }
            }
            ScanEvent::Finished { elapsed } => self.elapsed = elapsed,
        }
    }

    pub fn project(&self, id: crate::model::ProjectId) -> Option<&Project> {
        self.project_index.get(&id).map(|&i| &self.projects[i])
    }

    pub fn artifact(&self, id: ArtifactId) -> Option<&Artifact> {
        self.artifact_index.get(&id).map(|&i| &self.artifacts[i])
    }

    pub fn artifact_mut(&mut self, id: ArtifactId) -> Option<&mut Artifact> {
        self.artifact_index
            .get(&id)
            .copied()
            .map(|i| &mut self.artifacts[i])
    }

    /// Runs a scan to completion and collects it.
    pub fn collect(engine: &Engine, root: &Path, options: &ScanOptions, cancel: &Cancel) -> Self {
        let snapshot = Mutex::new(Snapshot::default());
        engine.scan(root, options, &Counters::default(), cancel, &|event| {
            snapshot.lock().expect("snapshot lock").apply(event);
        });
        let mut snapshot = snapshot.into_inner().expect("snapshot lock");
        snapshot.sort();
        snapshot
    }

    /// Orders projects and artifacts by path for stable output.
    pub fn sort(&mut self) {
        self.projects.sort_by(|a, b| a.path.cmp(&b.path));
        self.artifacts.sort_by(|a, b| a.path.cmp(&b.path));
        self.project_index = self
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id, i))
            .collect();
        self.artifact_index = self
            .artifacts
            .iter()
            .enumerate()
            .map(|(i, a)| (a.id, i))
            .collect();
    }
}
