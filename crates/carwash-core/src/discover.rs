//! Phase 1: parallel discovery of projects and artifact directories.
//!
//! Every directory is read exactly once. Its entry names decide, without any `stat`, whether
//! it is a project (marker files) and which children are artifacts (ecosystem rules).
//! Artifacts are never descended into, so discovery cost is proportional to source trees,
//! not to `node_modules` or `target/`.

use crate::Cancel;
use crate::ecosystem::adapters::WorkspaceSpec;
use crate::ecosystem::{EcoId, Registry, RuleId};
use crate::model::{Artifact, ArtifactId, ArtifactKind, Detection, GitState, Project, ProjectId};
use serde::Serialize;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::SystemTime;

/// Directory-name prefix used while a clean is in flight; leftovers are reported as artifacts.
pub const STAGING_PREFIX: &str = ".carwash-trash-";

const CACHEDIR_TAG_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";
/// Enclosing directories inspected above the scan root.
const MAX_ENCLOSING_LEVELS: usize = 16;
/// Version-control internals: never entered, never artifacts.
const VCS_DIRS: &[&str] = &[".git", ".hg", ".svn", ".jj", ".bzr", "_darcs"];

/// Which hidden directories (that are not artifacts) discovery enters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Hidden {
    Never,
    /// Only inside projects and repositories, where they hold things like agent worktrees
    /// (`.claude/worktrees`) rather than tool homes such as `~/.cargo` or `~/.npm`.
    #[default]
    InsideProjects,
    Always,
}

#[derive(Debug, Clone)]
pub struct DiscoverOptions {
    /// Maximum directory depth below the root.
    pub max_depth: Option<usize>,
    /// Hidden directories to descend into when they are not artifacts.
    pub hidden: Hidden,
    /// Do not cross filesystem boundaries.
    pub same_filesystem: bool,
    /// Absolute paths never entered.
    pub exclude: Vec<PathBuf>,
    /// Report artifacts of projects enclosing the root, e.g. a workspace `target/` when the
    /// scan starts in a member crate.
    pub include_enclosing: bool,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self {
            max_depth: None,
            hidden: Hidden::default(),
            same_filesystem: true,
            exclude: Vec::new(),
            include_enclosing: true,
        }
    }
}

/// Something discovery found.
#[derive(Debug, Clone)]
pub enum Found {
    Project(Project),
    Artifact(Artifact),
    Warning(Warning),
}

#[derive(Debug, Clone, Serialize)]
pub struct Warning {
    pub path: PathBuf,
    pub message: String,
}

/// Live progress counters, readable while discovery runs.
#[derive(Debug, Default)]
pub struct Counters {
    pub dirs: AtomicU64,
    pub projects: AtomicU32,
    pub artifacts: AtomicU32,
    pub warnings: AtomicU32,
    /// Artifacts measured so far.
    pub measured: AtomicU32,
}

/// Runs discovery on the current rayon pool, calling `emit` from worker threads.
///
/// Projects are emitted before anything below them, so consumers can resolve `parent`
/// and `member_of` as events arrive.
pub fn discover(
    root: &Path,
    registry: &Registry,
    options: &DiscoverOptions,
    counters: &Counters,
    cancel: &Cancel,
    emit: &(dyn Fn(Found) + Sync),
) {
    let walker = Walker {
        registry,
        options,
        counters,
        cancel,
        emit,
        root_dev: device(root),
    };
    let ctx = walker.enclosing_context(root);
    rayon::scope(|scope| walker.visit(scope, root.to_path_buf(), ctx, true));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Dir,
    Other,
}

#[derive(Debug)]
struct Entry {
    name: OsString,
    kind: EntryKind,
}

/// Reads a directory without following symlinks; symlinked directories are `Other`.
fn read_entries(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        entries.push(Entry {
            name: entry.file_name(),
            kind: if file_type.is_dir() {
                EntryKind::Dir
            } else {
                EntryKind::Other
            },
        });
    }
    Ok(entries)
}

fn utf8_names(entries: &[Entry]) -> Vec<&str> {
    entries.iter().filter_map(|e| e.name.to_str()).collect()
}

#[cfg(unix)]
fn device(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    fs::symlink_metadata(path).ok().map(|m| m.dev())
}

#[cfg(not(unix))]
fn device(_path: &Path) -> Option<u64> {
    None
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::symlink_metadata(path).and_then(|m| m.modified()).ok()
}

fn is_cachedir_tag(path: &Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; CACHEDIR_TAG_SIGNATURE.len()];
    fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok_and(|()| buf == CACHEDIR_TAG_SIGNATURE)
}

#[derive(Debug, Clone)]
struct Owner {
    id: ProjectId,
    ecos: Arc<[EcoId]>,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    rule: RuleId,
    /// Index of the segment the next directory level must match.
    next: usize,
    owner: Option<ProjectId>,
}

#[derive(Debug, Clone)]
struct WorkspaceCtx {
    id: ProjectId,
    ecos: Arc<[EcoId]>,
    root: Arc<Path>,
    spec: Arc<WorkspaceSpec>,
}

/// State inherited from ancestors.
#[derive(Debug, Clone)]
struct Ctx {
    depth: usize,
    owner: Option<Owner>,
    /// Ecosystems of every enclosing project (suppresses nested weak markers).
    inside: Arc<[EcoId]>,
    /// Enclosing ecosystems that have `anywhere` rules.
    anywhere: Arc<[EcoId]>,
    /// Multi-segment rules partially matched by ancestors.
    pending: Vec<Pending>,
    repo: Option<Arc<Path>>,
    workspaces: Arc<[WorkspaceCtx]>,
    /// Entered only to finish a pending rule (hidden directories such as `.nx`).
    pending_only: bool,
    outside_root: bool,
}

impl Ctx {
    fn root() -> Self {
        Self {
            depth: 0,
            owner: None,
            inside: Arc::from([]),
            anywhere: Arc::from([]),
            pending: Vec::new(),
            repo: None,
            workspaces: Arc::from([]),
            pending_only: false,
            outside_root: false,
        }
    }
}

/// A child directory matched by at least one rule, awaiting content checks.
struct Candidate {
    rule: RuleId,
    owner: Option<ProjectId>,
}

struct Walker<'a> {
    registry: &'a Registry,
    options: &'a DiscoverOptions,
    counters: &'a Counters,
    cancel: &'a Cancel,
    emit: &'a (dyn Fn(Found) + Sync),
    root_dev: Option<u64>,
}

impl<'a> Walker<'a> {
    fn warn(&self, path: &Path, error: &io::Error) {
        self.counters.warnings.fetch_add(1, Ordering::Relaxed);
        (self.emit)(Found::Warning(Warning {
            path: path.to_path_buf(),
            message: error.to_string(),
        }));
    }

    fn visit<'s>(&'s self, scope: &rayon::Scope<'s>, dir: PathBuf, mut ctx: Ctx, is_root: bool)
    where
        'a: 's,
    {
        if self.cancel.is_cancelled() {
            return;
        }
        self.counters.dirs.fetch_add(1, Ordering::Relaxed);
        let entries = match read_entries(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                self.warn(&dir, &error);
                return;
            }
        };
        let names = utf8_names(&entries);

        let mut here: Option<Owner> = None;
        if !ctx.pending_only {
            if names.contains(&".git") {
                ctx.repo = Some(Arc::from(dir.as_path()));
            }
            let ecos = self.project_ecosystems(&dir, &names, &ctx.inside);
            if ecos.is_empty() {
                if !is_root
                    && !names.iter().any(|n| VCS_DIRS.contains(n))
                    && let Some((detection, kind, eco)) = self.content_detection(&dir, &names)
                {
                    let owner = ctx.owner.as_ref().map(|o| o.id);
                    self.emit_artifact(dir, owner, eco, kind, detection, false, &ctx);
                    return;
                }
            } else {
                let owner = self.enter_project(&dir, &names, ecos, &mut ctx);
                here = Some(owner);
            }
        }

        let nearest = ctx.owner.as_ref().map(|o| o.id);
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut matched: Vec<RuleId> = Vec::new();
        for entry in entries.iter().filter(|e| e.kind == EntryKind::Dir) {
            let child = dir.join(&entry.name);
            if self.options.exclude.iter().any(|ex| child.starts_with(ex)) {
                continue;
            }
            let Some(name) = entry.name.to_str() else {
                // Non-UTF-8 names never match a rule but may contain projects.
                if !ctx.pending_only {
                    self.descend(scope, child, &ctx, Vec::new(), false);
                }
                continue;
            };
            if VCS_DIRS.contains(&name) {
                continue;
            }

            candidates.clear();
            let mut child_pending = Vec::new();
            if !ctx.pending_only {
                if name.starts_with(STAGING_PREFIX) {
                    self.emit_artifact(
                        child,
                        nearest,
                        None,
                        ArtifactKind::Leftover,
                        Detection::CarwashLeftover,
                        false,
                        &ctx,
                    );
                    continue;
                }
                if let Some(owner) = &here {
                    for &eco in owner.ecos.iter() {
                        matched.clear();
                        self.registry.root_rules_for(eco, name, &mut matched);
                        for &rule in &matched {
                            if self.registry.rule(rule).segment_count() == 1 {
                                candidates.push(Candidate {
                                    rule,
                                    owner: Some(owner.id),
                                });
                            } else {
                                child_pending.push(Pending {
                                    rule,
                                    next: 1,
                                    owner: Some(owner.id),
                                });
                            }
                        }
                    }
                }
                matched.clear();
                for &eco in ctx.anywhere.iter() {
                    self.registry.anywhere_rules_for(eco, name, &mut matched);
                }
                self.registry.global_rules_for(name, &mut matched);
                candidates.extend(matched.iter().map(|&rule| Candidate {
                    rule,
                    owner: nearest,
                }));
            }
            for pending in &ctx.pending {
                let rule = self.registry.rule(pending.rule);
                if rule.segment_matches(pending.next, name) {
                    if pending.next + 1 == rule.segment_count() {
                        candidates.push(Candidate {
                            rule: pending.rule,
                            owner: pending.owner,
                        });
                    } else {
                        child_pending.push(Pending {
                            next: pending.next + 1,
                            ..*pending
                        });
                    }
                }
            }

            if !candidates.is_empty()
                && let Some((rule, confirmed, owner)) = self.evaluate(&child, &candidates)
            {
                let spec = self.registry.rule(rule);
                self.emit_artifact(
                    child,
                    owner,
                    Some(spec.eco),
                    spec.kind,
                    Detection::Rule { rule, confirmed },
                    spec.ambiguous,
                    &ctx,
                );
                continue;
            }

            let enter_hidden = match self.options.hidden {
                Hidden::Never => false,
                Hidden::InsideProjects => ctx.owner.is_some() || ctx.repo.is_some(),
                Hidden::Always => true,
            };
            let normal = !ctx.pending_only && (!name.starts_with('.') || enter_hidden);
            if normal || !child_pending.is_empty() {
                self.descend(scope, child, &ctx, child_pending, !normal);
            }
        }
    }

    fn descend<'s>(
        &'s self,
        scope: &rayon::Scope<'s>,
        child: PathBuf,
        ctx: &Ctx,
        pending: Vec<Pending>,
        pending_only: bool,
    ) where
        'a: 's,
    {
        if self.options.max_depth.is_some_and(|max| ctx.depth >= max) {
            return;
        }
        if self.options.same_filesystem
            && self.root_dev.is_some()
            && device(&child) != self.root_dev
        {
            return;
        }
        let child_ctx = Ctx {
            depth: ctx.depth + 1,
            pending,
            pending_only,
            ..ctx.clone()
        };
        scope.spawn(move |scope| self.visit(scope, child, child_ctx, false));
    }

    /// Ecosystems whose markers are present, dropping weak markers inside a project of the
    /// same ecosystem.
    fn project_ecosystems(&self, dir: &Path, names: &[&str], inside: &[EcoId]) -> Vec<EcoId> {
        let markers = self.registry.detect_markers(dir, names.iter().copied());
        let mut ecos = markers.strong;
        ecos.extend(markers.weak.into_iter().filter(|eco| !inside.contains(eco)));
        ecos.sort_unstable();
        ecos
    }

    /// Recognises artifact directories by content rather than by name.
    fn content_detection(
        &self,
        dir: &Path,
        names: &[&str],
    ) -> Option<(Detection, ArtifactKind, Option<EcoId>)> {
        let has = |name: &str| names.contains(&name);
        if has("pyvenv.cfg") {
            return Some((
                Detection::PythonVenv,
                ArtifactKind::Environment,
                self.registry.find("python"),
            ));
        }
        if has("conda-meta") {
            return Some((
                Detection::CondaEnv,
                ArtifactKind::Environment,
                self.registry.find("python"),
            ));
        }
        if has("CMakeCache.txt") {
            return Some((
                Detection::CMakeBuild,
                ArtifactKind::Build,
                self.registry.find("cmake"),
            ));
        }
        if has("meson-private") && has("build.ninja") {
            return Some((
                Detection::MesonBuild,
                ArtifactKind::Build,
                self.registry.find("meson"),
            ));
        }
        if has("CACHEDIR.TAG") && is_cachedir_tag(&dir.join("CACHEDIR.TAG")) {
            let is_cargo_target = has(".rustc_info.json");
            let (kind, eco) = if is_cargo_target {
                (ArtifactKind::Build, self.registry.find("rust"))
            } else {
                (ArtifactKind::Cache, None)
            };
            return Some((Detection::CacheDirTag, kind, eco));
        }
        // A Cargo target directory whose manifest is gone and that predates CACHEDIR.TAG.
        if dir.file_name().is_some_and(|n| n == "target")
            && ["debug", "release"]
                .iter()
                .any(|profile| has(profile) && dir.join(profile).join(".fingerprint").is_dir())
        {
            return Some((
                Detection::CargoFingerprint,
                ArtifactKind::Build,
                self.registry.find("rust"),
            ));
        }
        None
    }

    /// Checks a rule-matched child's content and picks the most convincing rule.
    fn evaluate(
        &self,
        child: &Path,
        candidates: &[Candidate],
    ) -> Option<(RuleId, bool, Option<ProjectId>)> {
        let entries = read_entries(child).ok()?;
        let names = utf8_names(&entries);
        if names.iter().any(|n| VCS_DIRS.contains(n)) {
            return None;
        }
        let is_project = !self
            .registry
            .detect_markers(child, names.iter().copied())
            .strong
            .is_empty();

        let mut best: Option<(u8, RuleId, bool, Option<ProjectId>)> = None;
        for candidate in candidates {
            let rule = self.registry.rule(candidate.rule);
            let confirmed = rule.has_confirm() && names.iter().any(|n| rule.confirms(n));
            if rule.require_confirm && !confirmed {
                continue;
            }
            if rule.ambiguous && is_project {
                continue;
            }
            let score = u8::from(confirmed) * 2 + u8::from(!rule.ambiguous);
            if best.as_ref().is_none_or(|(s, ..)| score > *s) {
                best = Some((score, candidate.rule, confirmed, candidate.owner));
            }
        }
        best.map(|(_, rule, confirmed, owner)| (rule, confirmed, owner))
    }

    /// Creates the project for `dir` and updates `ctx` to describe its subtree.
    fn enter_project(&self, dir: &Path, names: &[&str], ecos: Vec<EcoId>, ctx: &mut Ctx) -> Owner {
        let id = ProjectId(self.counters.projects.fetch_add(1, Ordering::Relaxed));

        let mut name = None;
        let mut workspace = None;
        for &eco in &ecos {
            if let Some(adapter) = self.registry.ecosystem(eco).adapter {
                let info = adapter.inspect(dir, names);
                name = name.or(info.name);
                workspace = workspace.or(info.workspace);
            }
        }
        let name = name.unwrap_or_else(|| {
            dir.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| dir.display().to_string())
        });

        let member_of = ctx.workspaces.iter().rev().find_map(|ws| {
            let shares_ecosystem = ws.ecos.iter().any(|e| ecos.contains(e));
            let relative = dir.strip_prefix(&*ws.root).ok()?;
            let relative = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            (shares_ecosystem && !relative.is_empty() && ws.spec.contains(&relative))
                .then_some(ws.id)
        });

        let last_activity = ecos
            .iter()
            .flat_map(|&eco| &self.registry.ecosystem(eco).activity_files)
            .filter(|file| names.contains(&file.as_str()))
            .filter_map(|file| modified(&dir.join(file)))
            .max();

        let ecos: Arc<[EcoId]> = Arc::from(ecos);
        (self.emit)(Found::Project(Project {
            id,
            path: dir.to_path_buf(),
            name,
            ecosystems: ecos.to_vec(),
            parent: ctx.owner.as_ref().map(|o| o.id),
            member_of,
            is_workspace: workspace.is_some(),
            repo: ctx.repo.as_deref().map(Path::to_path_buf),
            last_activity,
            outside_root: ctx.outside_root,
        }));

        let owner = Owner {
            id,
            ecos: ecos.clone(),
        };
        let mut inside = ctx.inside.to_vec();
        for &eco in ecos.iter() {
            if !inside.contains(&eco) {
                inside.push(eco);
            }
        }
        ctx.anywhere = inside
            .iter()
            .copied()
            .filter(|&eco| self.registry.has_anywhere_rules(eco))
            .collect();
        ctx.inside = Arc::from(inside);
        if let Some(spec) = workspace {
            let mut workspaces = ctx.workspaces.to_vec();
            workspaces.push(WorkspaceCtx {
                id,
                ecos,
                root: Arc::from(dir),
                spec: Arc::new(spec),
            });
            ctx.workspaces = Arc::from(workspaces);
        }
        ctx.owner = Some(owner.clone());
        owner
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_artifact(
        &self,
        path: PathBuf,
        project: Option<ProjectId>,
        ecosystem: Option<EcoId>,
        kind: ArtifactKind,
        detection: Detection,
        ambiguous: bool,
        ctx: &Ctx,
    ) {
        let id = ArtifactId(self.counters.artifacts.fetch_add(1, Ordering::Relaxed));
        (self.emit)(Found::Artifact(Artifact {
            id,
            modified: modified(&path),
            path,
            project,
            ecosystem,
            kind,
            detection,
            ambiguous,
            git: GitState::Unknown,
            repo: ctx.repo.as_deref().map(Path::to_path_buf),
            size: None,
            outside_root: ctx.outside_root,
        }));
    }

    /// Walks from the root's parent upwards (to the enclosing repository root, stopping
    /// below the home directory) so a scan started inside a project knows about it.
    fn enclosing_context(&self, root: &Path) -> Ctx {
        let home = etcetera::home_dir().ok();
        let mut chain = Vec::new();
        let mut current = root.parent();
        while let Some(dir) = current {
            if home.as_deref() == Some(dir) || chain.len() >= MAX_ENCLOSING_LEVELS {
                break;
            }
            chain.push(dir);
            if dir.join(".git").exists() {
                break;
            }
            current = dir.parent();
        }

        let mut ctx = Ctx::root();
        ctx.outside_root = true;
        for (index, dir) in chain.iter().enumerate().rev() {
            let Ok(entries) = read_entries(dir) else {
                continue;
            };
            let names = utf8_names(&entries);
            if names.contains(&".git") {
                ctx.repo = Some(Arc::from(*dir));
            }
            let ecos = self.project_ecosystems(dir, &names, &ctx.inside);
            if ecos.is_empty() {
                continue;
            }
            let owner = self.enter_project(dir, &names, ecos, &mut ctx);
            if !self.options.include_enclosing {
                continue;
            }
            // The child leading back towards the scan root is not a candidate.
            let towards_root = if index == 0 { root } else { chain[index - 1] };
            let mut matched = Vec::new();
            for entry in entries.iter().filter(|e| e.kind == EntryKind::Dir) {
                let Some(name) = entry.name.to_str() else {
                    continue;
                };
                let child = dir.join(name);
                if child == towards_root {
                    continue;
                }
                matched.clear();
                for &eco in owner.ecos.iter() {
                    self.registry.root_rules_for(eco, name, &mut matched);
                }
                let candidates: Vec<Candidate> = matched
                    .iter()
                    .filter(|&&rule| self.registry.rule(rule).segment_count() == 1)
                    .map(|&rule| Candidate {
                        rule,
                        owner: Some(owner.id),
                    })
                    .collect();
                if candidates.is_empty() {
                    continue;
                }
                if let Some((rule, confirmed, owner)) = self.evaluate(&child, &candidates) {
                    let spec = self.registry.rule(rule);
                    self.emit_artifact(
                        child,
                        owner,
                        Some(spec.eco),
                        spec.kind,
                        Detection::Rule { rule, confirmed },
                        spec.ambiguous,
                        &ctx,
                    );
                }
            }
        }
        ctx.outside_root = false;
        ctx
    }
}
