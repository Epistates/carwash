//! Safe deletion of artifact directories.
//!
//! Every target is re-validated immediately before deletion (it may have changed since it
//! was scanned). Permanent deletion first renames the directory to a staging name next to
//! it, which is atomic and instant, then removes the staged tree; an interrupted clean leaves
//! a `.carwash-trash-*` directory that the next scan reports as a leftover artifact.
//!
//! A program still writing into a directory can refill it while it is being emptied, and
//! the final `rmdir` then fails with "directory not empty". Once the directory has been
//! renamed aside, nothing new can find it by its old path, so the removal is tried again a
//! few times; if the writer outlasts that, the failure says so, says where the remainder is,
//! and the other targets are cleaned regardless.

use crate::Cancel;
use crate::discover::STAGING_PREFIX;
use crate::model::ArtifactId;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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

/// How long to wait before each further attempt at a directory that was refilled while it
/// was being removed. A build tool flushing its last files is done well within this.
const REFILL_WAITS: [Duration; 3] = [
    Duration::from_millis(50),
    Duration::from_millis(250),
    Duration::from_millis(1000),
];

/// A removal that did not finish.
#[derive(Debug)]
struct Unfinished {
    /// What is left to remove: the staging directory, or the artifact itself when it could
    /// not be renamed.
    left: PathBuf,
    staged: bool,
    error: io::Error,
}

impl Unfinished {
    /// What happened, for the person reading the result: the cause in plain words where
    /// it is known, and where the remainder is.
    fn describe(&self) -> String {
        let refilled = self.error.kind() == io::ErrorKind::DirectoryNotEmpty;
        match (refilled, self.staged) {
            (true, true) => format!(
                "another program was still writing into it, so part of it is left in {}; the next scan offers to finish removing it. Nothing else was affected",
                self.left.display()
            ),
            (true, false) => "another program is still writing into it, so it was left where it is; clean it again once that program has finished. Nothing else was affected".to_owned(),
            (false, true) => format!(
                "{} (what is left of it is in {})",
                self.error,
                self.left.display()
            ),
            (false, false) => self.error.to_string(),
        }
    }
}

/// Removes the tree at `target` with `remove`. When the tree was renamed aside (`staged`)
/// and it was refilled while being emptied, waits (`pause`) and tries again, up to
/// [`REFILL_WAITS`]: what arrives now comes from a program that already had the directory
/// open, and it stops when that program does. A tree still at its own path is not retried,
/// since that would only race a program that is using it.
fn remove_tree(
    target: &Path,
    staged: bool,
    remove: &dyn Fn(&Path) -> io::Result<()>,
    pause: &dyn Fn(Duration),
) -> io::Result<()> {
    let mut result = remove(target);
    if staged {
        for wait in REFILL_WAITS {
            match &result {
                Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {
                    pause(wait);
                    result = remove(target);
                }
                _ => break,
            }
        }
    }
    result
}

/// Removes `path`; on failure, says what is left to remove and why.
fn remove_permanently(path: &Path) -> Result<(), Unfinished> {
    // Rename first so the artifact disappears atomically; fall back to deleting in place
    // when renaming is not possible (for example on some network filesystems).
    let (target, staged) = match staging_path(path) {
        Some(staged) if fs::rename(path, &staged).is_ok() => (staged, true),
        _ => (path.to_path_buf(), false),
    };
    // std removes a symlink found at `target` without following it, should the artifact
    // have been swapped for one after validation, and unlinks files without opening them
    // (named pipes, sockets and unreadable files included). It walks relative to directory
    // handles, so nothing swapped in deeper is followed either (CVE-2022-21658).
    remove_tree(
        &target,
        staged,
        &|tree| fs::remove_dir_all(tree),
        &std::thread::sleep,
    )
    .map_err(|error| Unfinished {
        left: target,
        staged,
        error,
    })
}

/// The process (EMFILE) or system (ENFILE) ran out of file handles. Each removal holds one
/// per directory level, so deep trees removed side by side can exhaust a low limit;
/// removing one tree at a time needs only as many as it is deep.
fn out_of_file_handles(error: &io::Error) -> bool {
    // No stable `io::ErrorKind` covers these yet.
    if cfg!(unix) {
        matches!(error.raw_os_error(), Some(23 | 24))
    } else if cfg!(windows) {
        error.raw_os_error() == Some(4) // ERROR_TOO_MANY_OPEN_FILES
    } else {
        false
    }
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
    let finish = |item: &CleanItem, result: Result<(), String>| match result {
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
    };
    // Removals that ran out of file handles, with what is left of each, for a second pass.
    let retry = Mutex::new(Vec::new());

    items.par_iter().for_each(|item| {
        if cancel.is_cancelled() {
            return;
        }
        emit(CleanEvent::Started { id: item.id });
        if let Err(refusal) = validate(&item.path, &options.allowed_roots) {
            return finish(item, Err(refusal.to_string()));
        }
        if options.dry_run {
            return finish(item, Ok(()));
        }
        let result = match options.mode {
            DeleteMode::Permanent => match remove_permanently(&item.path) {
                Ok(()) => Ok(()),
                Err(unfinished) if out_of_file_handles(&unfinished.error) => {
                    retry.lock().expect("retry list").push((item, unfinished));
                    return;
                }
                Err(unfinished) => Err(unfinished.describe()),
            },
            DeleteMode::Trash => trash::delete(&item.path).map_err(|e| e.to_string()),
        };
        finish(item, result);
    });

    for (item, unfinished) in retry.into_inner().expect("retry list") {
        if cancel.is_cancelled() {
            break;
        }
        let Unfinished { left, staged, .. } = unfinished;
        let result = remove_tree(
            &left,
            staged,
            &|tree| fs::remove_dir_all(tree),
            &std::thread::sleep,
        )
        .map_err(|error| {
            Unfinished {
                left,
                staged,
                error,
            }
            .describe()
        });
        finish(item, result);
    }

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

    #[cfg(unix)]
    #[test]
    fn recognises_running_out_of_file_handles() {
        assert!(out_of_file_handles(&io::Error::from_raw_os_error(24)));
        assert!(out_of_file_handles(&io::Error::from_raw_os_error(23)));
        assert!(!out_of_file_handles(&io::Error::from_raw_os_error(13)));
        assert!(!out_of_file_handles(&io::Error::other("custom")));
    }

    fn refilled() -> io::Error {
        io::Error::from(io::ErrorKind::DirectoryNotEmpty)
    }

    /// Runs `remove_tree` against a removal that fails with `failures` in turn and then
    /// succeeds; returns the result, the attempts made and the waits between them.
    fn attempt(staged: bool, failures: Vec<io::Error>) -> (io::Result<()>, usize, Vec<Duration>) {
        let failures = Mutex::new(failures.into_iter());
        let attempts = AtomicU64::new(0);
        let waits = Mutex::new(Vec::new());
        let result = remove_tree(
            Path::new("staged"),
            staged,
            &|_| {
                attempts.fetch_add(1, Ordering::Relaxed);
                failures.lock().unwrap().next().map_or(Ok(()), Err)
            },
            &|wait| waits.lock().unwrap().push(wait),
        );
        (
            result,
            attempts.into_inner() as usize,
            waits.into_inner().unwrap(),
        )
    }

    #[test]
    fn a_staged_tree_refilled_by_a_writer_is_tried_again() {
        let (result, attempts, waits) = attempt(true, vec![refilled(), refilled()]);
        assert!(result.is_ok());
        assert_eq!(attempts, 3);
        assert_eq!(waits, REFILL_WAITS[..2]);
    }

    #[test]
    fn retrying_is_bounded() {
        let endless = std::iter::repeat_with(refilled).take(10).collect();
        let (result, attempts, waits) = attempt(true, endless);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::DirectoryNotEmpty);
        assert_eq!(attempts, 1 + REFILL_WAITS.len());
        assert_eq!(waits, REFILL_WAITS);
    }

    #[test]
    fn only_refills_of_a_staged_tree_are_retried() {
        // In place, a retry would race the program still using the directory.
        let (result, attempts, waits) = attempt(false, vec![refilled()]);
        assert!(result.is_err());
        assert_eq!((attempts, waits.len()), (1, 0));
        // Other failures do not get better by waiting.
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        let (result, attempts, _) = attempt(true, vec![denied]);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(attempts, 1);
    }

    #[test]
    fn a_refill_is_explained_with_where_the_remainder_is() {
        let unfinished = |staged, error| Unfinished {
            left: PathBuf::from("/p/.carwash-trash-1-0-node_modules"),
            staged,
            error,
        };
        let staged = unfinished(true, refilled()).describe();
        assert!(
            staged.starts_with("another program was still writing into it"),
            "{staged}"
        );
        assert!(
            staged.contains("/p/.carwash-trash-1-0-node_modules"),
            "{staged}"
        );
        assert!(staged.ends_with("Nothing else was affected"), "{staged}");
        assert!(!staged.contains("os error"), "{staged}");
        let in_place = unfinished(false, refilled()).describe();
        assert!(in_place.contains("left where it is"), "{in_place}");
        let denied = unfinished(true, io::Error::from(io::ErrorKind::PermissionDenied)).describe();
        assert!(
            denied.contains("what is left of it is in /p/.carwash-trash"),
            "{denied}"
        );
    }

    /// The failure this guards against: a program that holds the directory open (a build
    /// tool finishing its writes) keeps adding files while the directory is being emptied.
    #[cfg(unix)]
    #[test]
    fn a_directory_refilled_through_an_open_handle_is_still_removed() {
        use rustix::fs::{Mode, OFlags, open, openat};
        use std::time::Instant;
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("project/node_modules");
        fs::create_dir_all(target.join("pkg")).unwrap();
        for n in 0..200 {
            fs::write(target.join(format!("pkg/file-{n}")), "x").unwrap();
        }
        let handle = open(&target, OFlags::DIRECTORY | OFlags::RDONLY, Mode::empty()).unwrap();
        let until = Instant::now() + Duration::from_millis(300);
        let writer = std::thread::spawn(move || {
            let mut written = 0u32;
            // Creating a file fails once the directory is gone.
            while Instant::now() < until
                && openat(
                    &handle,
                    format!("late-{written}"),
                    OFlags::CREATE | OFlags::WRONLY,
                    Mode::from_raw_mode(0o644),
                )
                .is_ok()
            {
                written += 1;
            }
            written
        });
        let (report, events) = run(
            &[item(0, target.clone())],
            &[root.path().to_path_buf()],
            false,
        );
        let written = writer.join().unwrap();
        assert_eq!(
            (report.removed, report.failed),
            (1, 0),
            "{events:?} with {written} files written meanwhile"
        );
        let left: Vec<_> = fs::read_dir(root.path().join("project")).unwrap().collect();
        assert!(left.is_empty(), "nothing staged is left: {left:?}");
    }

    /// One target that cannot be removed does not stop the others.
    #[cfg(unix)]
    #[test]
    fn a_failed_removal_does_not_stop_the_rest() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let stuck = root.path().join("a/node_modules");
        let fine = root.path().join("b/node_modules");
        for dir in [&stuck, &fine] {
            fs::create_dir_all(dir.join("pkg")).unwrap();
            fs::write(dir.join("pkg/index.js"), "x").unwrap();
        }
        // A directory whose entries cannot be unlinked.
        let locked = stuck.join("pkg");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let (report, events) = run(
            &[item(0, stuck.clone()), item(1, fine.clone())],
            &[root.path().to_path_buf()],
            false,
        );
        let leftover = fs::read_dir(root.path().join("a"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .next();
        if let Some(leftover) = &leftover {
            let _ = fs::set_permissions(leftover.join("pkg"), fs::Permissions::from_mode(0o755));
        }
        if rustix::process::geteuid().is_root() {
            // Root removes it regardless; there is nothing to observe.
            return;
        }
        assert_eq!((report.removed, report.failed), (1, 1));
        assert!(!fine.exists(), "the other target was cleaned");
        let failure = events
            .iter()
            .find_map(|e| match e {
                CleanEvent::Failed { error, .. } => Some(error.clone()),
                _ => None,
            })
            .expect("a failure event");
        let leftover = leftover.expect("the staged remainder");
        assert!(
            failure.contains(&leftover.display().to_string()),
            "says where the remainder is: {failure}"
        );
    }

    /// A directory swapped for a symlink after validation must cost only the link: the
    /// staged path is removed without resolving it.
    #[cfg(unix)]
    #[test]
    fn removal_never_follows_a_swapped_in_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        fs::create_dir(&victim).unwrap();
        fs::write(victim.join("precious"), "keep").unwrap();
        let swapped = dir.path().join("project").join("target");
        fs::create_dir(swapped.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&victim, &swapped).unwrap();
        // The link itself may or may not be reported as removed; the target must survive.
        let _ = remove_permanently(&swapped);
        assert!(victim.join("precious").exists(), "symlink target deleted");
        assert!(fs::symlink_metadata(&swapped).is_err(), "link left behind");
    }

    /// Named pipes, sockets and unreadable files are unlinked without being opened.
    #[cfg(unix)]
    #[test]
    fn removes_special_and_unreadable_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir_all(target.join("deep")).unwrap();
        let fifo = target.join("deep/pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|s| s.success());
        let _socket = std::os::unix::net::UnixListener::bind(target.join("sock")).unwrap();
        let locked = target.join("locked");
        fs::write(&locked, "x").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        assert!(made || cfg!(not(target_os = "macos")), "mkfifo failed");
        remove_permanently(&target).unwrap();
        assert!(!target.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0, "staging left");
    }

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
