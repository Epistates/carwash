//! Domain model shared by every carwash frontend.

use crate::ecosystem::{EcoId, RuleId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

/// Stable identifier of a project within one scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(pub u32);

/// Stable identifier of an artifact within one scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArtifactId(pub u32);

/// A directory recognised as a project by one or more ecosystems.
#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub id: ProjectId,
    pub path: PathBuf,
    /// Manifest name when an adapter could read it, otherwise the directory name.
    pub name: String,
    pub ecosystems: Vec<EcoId>,
    /// Nearest enclosing project, if any.
    pub parent: Option<ProjectId>,
    /// Workspace/monorepo this project is a declared member of.
    pub member_of: Option<ProjectId>,
    /// True when the project declares workspace members (Cargo workspace, pnpm workspace, go.work...).
    pub is_workspace: bool,
    /// Root of the git repository containing the project.
    pub repo: Option<PathBuf>,
    /// Newest modification time among manifests and lockfiles.
    #[serde(with = "unix_secs_opt")]
    pub last_activity: Option<SystemTime>,
    /// Project lies above the scan root (the scan started inside it).
    pub outside_root: bool,
}

/// What an artifact directory contains, which drives how costly it is to regenerate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    /// Compiler and bundler output.
    Build,
    /// Installed third-party packages.
    Dependencies,
    /// Tool caches.
    Cache,
    /// Virtual environments and toolchain switches.
    Environment,
    /// Reports, logs, coverage and other generated output.
    Other,
    /// Half-deleted directories left behind by an interrupted carwash clean.
    Leftover,
}

impl ArtifactKind {
    pub const ALL: [Self; 6] = [
        Self::Build,
        Self::Dependencies,
        Self::Cache,
        Self::Environment,
        Self::Other,
        Self::Leftover,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Dependencies => "deps",
            Self::Cache => "cache",
            Self::Environment => "env",
            Self::Other => "other",
            Self::Leftover => "leftover",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "build" => Self::Build,
            "deps" | "dependencies" => Self::Dependencies,
            "cache" => Self::Cache,
            "env" | "environment" => Self::Environment,
            "other" => Self::Other,
            "leftover" => Self::Leftover,
            _ => return None,
        })
    }
}

/// How an artifact was recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Detection {
    /// Matched an ecosystem rule; `confirmed` when a confirming child entry was present.
    Rule { rule: RuleId, confirmed: bool },
    /// Contains a `CACHEDIR.TAG` with a valid signature.
    CacheDirTag,
    /// Contains `pyvenv.cfg`.
    PythonVenv,
    /// Contains `conda-meta/`.
    CondaEnv,
    /// Contains `CMakeCache.txt`.
    CMakeBuild,
    /// Contains `meson-private/` and `build.ninja`.
    MesonBuild,
    /// A `target/` holding Cargo's `debug/.fingerprint` or `release/.fingerprint`.
    CargoFingerprint,
    /// Named like a carwash staging directory.
    CarwashLeftover,
}

impl Detection {
    /// Detected by content rather than by a possibly-generic name.
    pub fn is_certain(self) -> bool {
        match self {
            Detection::Rule { confirmed, .. } => confirmed,
            _ => true,
        }
    }
}

/// Git's view of an artifact directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "files")]
pub enum GitState {
    /// Not yet inspected, or git is unavailable.
    #[default]
    Unknown,
    NotInRepo,
    /// Matched by an ignore rule and contains no tracked files.
    Ignored,
    /// Neither ignored nor tracked; it would show up in `git status`.
    Untracked,
    /// Contains this many tracked files.
    Tracked(u64),
}

/// A deletable directory.
#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    pub id: ArtifactId,
    pub path: PathBuf,
    /// Owning project; `None` for content-detected directories outside any project.
    pub project: Option<ProjectId>,
    pub ecosystem: Option<EcoId>,
    pub kind: ArtifactKind,
    pub detection: Detection,
    /// Generic name that some repositories commit (`build`, `dist`, `vendor`...).
    pub ambiguous: bool,
    /// Modification time of the directory itself; refined by measurement.
    #[serde(with = "unix_secs_opt")]
    pub modified: Option<SystemTime>,
    pub git: GitState,
    /// Root of the git repository containing the artifact.
    pub repo: Option<PathBuf>,
    pub size: Option<Size>,
    /// Lies above the scan root, e.g. a workspace `target/` when scanning a member.
    pub outside_root: bool,
}

impl Artifact {
    pub fn name(&self) -> &str {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
    }

    /// Newest known modification time, preferring the measured value.
    pub fn last_modified(&self) -> Option<SystemTime> {
        self.size.and_then(|s| s.newest).or(self.modified)
    }

    pub fn safety(&self) -> Safety {
        crate::safety::assess(self)
    }
}

/// Sizes of an artifact directory tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    /// Allocated bytes, counting each hard-linked inode once.
    pub on_disk: u64,
    /// Bytes freed by deleting the directory: hard-linked files are only counted when
    /// every link lives inside it.
    pub reclaimable: u64,
    /// Sum of file lengths.
    pub apparent: u64,
    pub files: u64,
    pub dirs: u64,
    /// Newest modification time anywhere in the tree.
    #[serde(with = "unix_secs_opt")]
    pub newest: Option<SystemTime>,
    /// Entries that could not be read; the totals are lower bounds when non-zero.
    pub errors: u64,
}

impl Size {
    /// Bytes shared with something outside the directory (for example a pnpm store).
    pub fn shared(&self) -> u64 {
        self.on_disk.saturating_sub(self.reclaimable)
    }
}

/// Whether an artifact may be deleted without extra confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "level", content = "reason")]
pub enum Safety {
    Safe,
    /// Deletable, but only after the user looks at it.
    Review(ReviewReason),
    /// Never deleted by carwash.
    Protected(ProtectReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    /// Generic name without confirming content, and git does not say it is ignored.
    GenericName,
    /// Visible to git as untracked: it may be work in progress.
    UntrackedInGit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectReason {
    /// Contains files tracked by git.
    TrackedFiles,
}

impl Safety {
    pub fn describe(self) -> &'static str {
        match self {
            Safety::Safe => "safe",
            Safety::Review(ReviewReason::GenericName) => "generic name, not confirmed by git",
            Safety::Review(ReviewReason::UntrackedInGit) => "not ignored by git",
            Safety::Protected(ProtectReason::TrackedFiles) => "contains git-tracked files",
        }
    }

    pub fn is_deletable(self) -> bool {
        !matches!(self, Safety::Protected(_))
    }
}

pub(crate) mod unix_secs_opt {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
        match t.and_then(|t| t.duration_since(UNIX_EPOCH).ok()) {
            Some(d) => s.serialize_some(&d.as_secs()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SystemTime>, D::Error> {
        Ok(Option::<u64>::deserialize(d)?.map(|secs| UNIX_EPOCH + Duration::from_secs(secs)))
    }
}
