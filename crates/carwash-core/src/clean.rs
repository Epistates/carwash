//! Safe deletion of artifact directories.
//!
//! Every target is re-validated immediately before deletion (it may have changed since it
//! was scanned). Permanent deletion first renames the directory to a staging name next to
//! it, which is atomic and instant, then removes the staged tree; an interrupted clean leaves
//! a `.carwash-trash-*` directory that the next scan reports as a leftover artifact.

use crate::Cancel;
use crate::discover::STAGING_PREFIX;
use crate::model::ArtifactId;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeleteMode {
    /// Remove immediately; frees space right away.
    #[default]
    Permanent,
    /// Move to the platform trash; recoverable, but frees nothing until the trash is emptied.
    Trash,
}

#[derive(Debug, Clone)]
pub struct CleanItem {
    pub id: ArtifactId,
    pub path: PathBuf,
    /// Bytes expected to be freed (the measured reclaimable size), for reporting.
    pub expected_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct CleanOptions {
    pub mode: DeleteMode,
    /// Targets must lie strictly inside one of these directories.
    pub allowed_roots: Vec<PathBuf>,
    /// Validate and report without deleting anything.
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub enum CleanEvent {
    Started { id: ArtifactId },
    Removed { id: ArtifactId, bytes: u64 },
    Failed { id: ArtifactId, error: String },
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CleanReport {
    pub removed: usize,
    pub failed: usize,
    pub bytes: u64,
    /// Free space on the filesystem of the first target, before and after.
    pub free_before: Option<u64>,
    pub free_after: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum RefusalReason {
    #[error("no longer exists")]
    Missing,
    #[error("is not a directory")]
    NotDirectory,
    #[error("is a symlink")]
    Symlink,
    #[error("is outside the scanned roots")]
    OutsideRoots,
    #[error("is a scan root itself")]
    IsRoot,
    #[error("contains a git repository")]
    ContainsRepository,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Checks that `path` is still a plain directory strictly inside an allowed root.
pub fn validate(path: &Path, allowed_roots: &[PathBuf]) -> Result<(), RefusalReason> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(RefusalReason::Missing),
        Err(e) => return Err(e.into()),
    };
    if meta.file_type().is_symlink() {
        return Err(RefusalReason::Symlink);
    }
    if !meta.is_dir() {
        return Err(RefusalReason::NotDirectory);
    }
    let canonical = fs::canonicalize(path)?;
    let mut inside = false;
    for root in allowed_roots {
        let Ok(root) = fs::canonicalize(root) else {
            continue;
        };
        if canonical == root {
            return Err(RefusalReason::IsRoot);
        }
        if canonical.starts_with(&root) {
            inside = true;
        }
    }
    if !inside {
        return Err(RefusalReason::OutsideRoots);
    }
    if fs::symlink_metadata(path.join(".git")).is_ok() {
        return Err(RefusalReason::ContainsRepository);
    }
    Ok(())
}

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

fn staging_path(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let name = path.file_name()?.to_string_lossy();
    let n = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
    Some(parent.join(format!("{STAGING_PREFIX}{}-{n}-{name}", std::process::id())))
}

fn remove_permanently(path: &Path) -> io::Result<()> {
    // Rename first so the artifact disappears atomically; fall back to deleting in place
    // when renaming is not possible (for example on some network filesystems).
    let target = match staging_path(path) {
        Some(staged) if fs::rename(path, &staged).is_ok() => staged,
        _ => path.to_path_buf(),
    };
    remove_dir_all::remove_dir_all(&target)
}

/// Deletes `items` in parallel on the current rayon pool.
pub fn clean(
    items: &[CleanItem],
    options: &CleanOptions,
    cancel: &Cancel,
    emit: &(dyn Fn(CleanEvent) + Sync),
) -> CleanReport {
    let probe = items
        .first()
        .and_then(|i| i.path.parent().map(Path::to_path_buf));
    let free_before = probe.as_deref().and_then(|p| fs4::available_space(p).ok());

    let removed = AtomicU64::new(0);
    let failed = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);

    items.par_iter().for_each(|item| {
        if cancel.is_cancelled() {
            return;
        }
        emit(CleanEvent::Started { id: item.id });
        let result = validate(&item.path, &options.allowed_roots)
            .map_err(|e| e.to_string())
            .and_then(|()| {
                if options.dry_run {
                    return Ok(());
                }
                match options.mode {
                    DeleteMode::Permanent => {
                        remove_permanently(&item.path).map_err(|e| e.to_string())
                    }
                    DeleteMode::Trash => trash::delete(&item.path).map_err(|e| e.to_string()),
                }
            });
        match result {
            Ok(()) => {
                removed.fetch_add(1, Ordering::Relaxed);
                bytes.fetch_add(item.expected_bytes, Ordering::Relaxed);
                emit(CleanEvent::Removed {
                    id: item.id,
                    bytes: item.expected_bytes,
                });
            }
            Err(error) => {
                failed.fetch_add(1, Ordering::Relaxed);
                emit(CleanEvent::Failed { id: item.id, error });
            }
        }
    });

    CleanReport {
        removed: removed.into_inner() as usize,
        failed: failed.into_inner() as usize,
        bytes: bytes.into_inner(),
        free_before,
        free_after: probe.as_deref().and_then(|p| fs4::available_space(p).ok()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn item(id: u32, path: PathBuf) -> CleanItem {
        CleanItem {
            id: ArtifactId(id),
            path,
            expected_bytes: 100,
        }
    }

    fn run(
        items: &[CleanItem],
        roots: &[PathBuf],
        dry_run: bool,
    ) -> (CleanReport, Vec<CleanEvent>) {
        let events = Mutex::new(Vec::new());
        let report = clean(
            items,
            &CleanOptions {
                mode: DeleteMode::Permanent,
                allowed_roots: roots.to_vec(),
                dry_run,
            },
            &Cancel::new(),
            &|e| events.lock().unwrap().push(e),
        );
        (report, events.into_inner().unwrap())
    }

    #[test]
    fn deletes_directories_and_leaves_no_staging() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("project/target");
        fs::create_dir_all(target.join("debug/deps")).unwrap();
        fs::write(target.join("debug/deps/lib.rlib"), "x").unwrap();

        let (report, _) = run(
            &[item(0, target.clone())],
            &[root.path().to_path_buf()],
            false,
        );
        assert_eq!((report.removed, report.failed, report.bytes), (1, 0, 100));
        assert!(!target.exists());
        let leftovers: Vec<_> = fs::read_dir(root.path().join("project")).unwrap().collect();
        assert!(leftovers.is_empty(), "staging directory removed");
    }

    #[test]
    fn dry_run_touches_nothing() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("node_modules");
        fs::create_dir(&target).unwrap();
        let (report, _) = run(
            &[item(0, target.clone())],
            &[root.path().to_path_buf()],
            true,
        );
        assert_eq!(report.removed, 1);
        assert!(target.exists());
    }

    #[test]
    fn refuses_outside_roots_the_root_itself_and_repositories() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let repo = root.path().join("vendored");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let outside = other.path().join("target");
        fs::create_dir(&outside).unwrap();

        let roots = [root.path().to_path_buf()];
        assert!(matches!(
            validate(&outside, &roots),
            Err(RefusalReason::OutsideRoots)
        ));
        assert!(matches!(
            validate(root.path(), &roots),
            Err(RefusalReason::IsRoot)
        ));
        assert!(matches!(
            validate(&repo, &roots),
            Err(RefusalReason::ContainsRepository)
        ));
        assert!(matches!(
            validate(&root.path().join("missing"), &roots),
            Err(RefusalReason::Missing)
        ));

        let (report, events) = run(&[item(0, outside.clone())], &roots, false);
        assert_eq!(report.failed, 1);
        assert!(outside.exists());
        assert!(
            events
                .iter()
                .any(|e| matches!(e, CleanEvent::Failed { .. }))
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_artifacts_are_refused_and_targets_survive() {
        let root = tempfile::tempdir().unwrap();
        let precious = root.path().join("src");
        fs::create_dir(&precious).unwrap();
        fs::write(precious.join("main.rs"), "fn main() {}").unwrap();
        let link = root.path().join("node_modules");
        std::os::unix::fs::symlink(&precious, &link).unwrap();

        let (report, _) = run(
            &[item(0, link.clone())],
            &[root.path().to_path_buf()],
            false,
        );
        assert_eq!(report.failed, 1);
        assert!(precious.join("main.rs").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_inside_artifacts_are_removed_without_following() {
        let root = tempfile::tempdir().unwrap();
        let precious = root.path().join("shared");
        fs::create_dir(&precious).unwrap();
        fs::write(precious.join("keep"), "data").unwrap();
        let modules = root.path().join("app/node_modules");
        fs::create_dir_all(&modules).unwrap();
        std::os::unix::fs::symlink(&precious, modules.join("linked-pkg")).unwrap();
        std::os::unix::fs::symlink(root.path().join("dangling"), modules.join("broken")).unwrap();

        let (report, _) = run(
            &[item(0, modules.clone())],
            &[root.path().to_path_buf()],
            false,
        );
        assert_eq!(report.removed, 1);
        assert!(!modules.exists());
        assert!(precious.join("keep").exists(), "symlink target untouched");
    }
}
