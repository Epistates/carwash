//! Phase 2: parallel size measurement of artifact directories.
//!
//! Sizes are allocated bytes (`st_blocks * 512` on Unix), not apparent lengths. Hard-linked
//! inodes are counted once, and only count towards `reclaimable` when every link lives
//! inside the measured tree: deleting a pnpm or uv `node_modules`/`.venv` whose files are
//! hard links into a global store frees almost nothing, and the numbers say so.

use crate::Cancel;
use crate::model::Size;
use dashmap::DashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A hard-linked inode seen during measurement.
#[derive(Debug, Clone, Copy)]
struct Linked {
    nlink: u64,
    seen: u64,
    bytes: u64,
}

#[derive(Debug, Default)]
struct Totals {
    single_link_bytes: AtomicU64,
    apparent: AtomicU64,
    files: AtomicU64,
    dirs: AtomicU64,
    errors: AtomicU64,
    newest_secs: AtomicU64,
}

/// File identity for hard-link accounting: `(device, inode)`.
type Identity = (u64, u64);

struct Measurer<'a> {
    totals: Totals,
    linked: DashMap<Identity, Linked>,
    cancel: &'a Cancel,
}

/// Measures `path` on the current rayon pool. Unreadable entries are counted in
/// [`Size::errors`] and otherwise skipped; symlinks are measured, never followed.
pub fn measure(path: &Path, cancel: &Cancel) -> Size {
    let measurer = Measurer {
        totals: Totals::default(),
        linked: DashMap::new(),
        cancel,
    };
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            measurer.account(&meta);
            rayon::scope(|scope| measurer.walk(scope, path.to_path_buf()));
        }
        Ok(meta) => measurer.account(&meta),
        Err(_) => {
            measurer.totals.errors.fetch_add(1, Ordering::Relaxed);
        }
    }
    measurer.finish()
}

impl Measurer<'_> {
    fn walk<'s>(&'s self, scope: &rayon::Scope<'s>, dir: std::path::PathBuf) {
        if self.cancel.is_cancelled() {
            return;
        }
        let Ok(read_dir) = fs::read_dir(&dir) else {
            self.totals.errors.fetch_add(1, Ordering::Relaxed);
            return;
        };
        for entry in read_dir {
            let Ok(entry) = entry else {
                self.totals.errors.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // `DirEntry::metadata` does not traverse symlinks.
            let Ok(meta) = entry.metadata() else {
                self.totals.errors.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            self.account(&meta);
            if meta.is_dir() {
                let child = entry.path();
                scope.spawn(move |scope| self.walk(scope, child));
            }
        }
    }

    fn account(&self, meta: &fs::Metadata) {
        let totals = &self.totals;
        if meta.is_dir() {
            totals.dirs.fetch_add(1, Ordering::Relaxed);
        } else {
            totals.files.fetch_add(1, Ordering::Relaxed);
            totals.apparent.fetch_add(meta.len(), Ordering::Relaxed);
        }
        if let Ok(modified) = meta.modified()
            && let Ok(since_epoch) = modified.duration_since(UNIX_EPOCH)
        {
            totals
                .newest_secs
                .fetch_max(since_epoch.as_secs(), Ordering::Relaxed);
        }
        let bytes = allocated_bytes(meta);
        match link_identity(meta) {
            Some((identity, nlink)) if nlink > 1 && !meta.is_dir() => {
                self.linked
                    .entry(identity)
                    .and_modify(|l| l.seen += 1)
                    .or_insert(Linked {
                        nlink,
                        seen: 1,
                        bytes,
                    });
            }
            _ => {
                totals.single_link_bytes.fetch_add(bytes, Ordering::Relaxed);
            }
        }
    }

    fn finish(self) -> Size {
        let totals = self.totals;
        let single = totals.single_link_bytes.into_inner();
        let mut on_disk = single;
        let mut reclaimable = single;
        for (_, linked) in self.linked {
            on_disk += linked.bytes;
            if linked.seen >= linked.nlink {
                reclaimable += linked.bytes;
            }
        }
        let newest = totals.newest_secs.into_inner();
        Size {
            on_disk,
            reclaimable,
            apparent: totals.apparent.into_inner(),
            files: totals.files.into_inner(),
            dirs: totals.dirs.into_inner(),
            newest: (newest > 0).then(|| UNIX_EPOCH + Duration::from_secs(newest)),
            errors: totals.errors.into_inner(),
        }
    }
}

#[cfg(unix)]
fn allocated_bytes(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.blocks() * 512
}

#[cfg(not(unix))]
fn allocated_bytes(meta: &fs::Metadata) -> u64 {
    // Round up to a 4 KiB cluster; there is no portable allocation query.
    meta.len().div_ceil(4096) * 4096
}

#[cfg(unix)]
fn link_identity(meta: &fs::Metadata) -> Option<(Identity, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some(((meta.dev(), meta.ino()), meta.nlink()))
}

#[cfg(not(unix))]
fn link_identity(_meta: &fs::Metadata) -> Option<(Identity, u64)> {
    None
}

/// Newest of two optional timestamps.
pub fn newest(a: Option<SystemTime>, b: Option<SystemTime>) -> Option<SystemTime> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: usize) {
        fs::write(path, vec![7u8; bytes]).unwrap();
    }

    #[test]
    fn counts_files_dirs_and_allocation() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a/b")).unwrap();
        write(&dir.path().join("a/one"), 10_000);
        write(&dir.path().join("a/b/two"), 20_000);
        let size = measure(dir.path(), &Cancel::new());
        assert_eq!(size.files, 2);
        assert_eq!(size.dirs, 3);
        assert_eq!(size.apparent, 30_000);
        assert!(
            size.on_disk >= 30_000,
            "allocation covers content: {size:?}"
        );
        assert_eq!(size.on_disk, size.reclaimable);
        assert_eq!(size.errors, 0);
        assert!(size.newest.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_count_once_and_shared_links_are_not_reclaimable() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        let project = root.path().join("project");
        fs::create_dir_all(&store).unwrap();
        fs::create_dir_all(project.join("node_modules")).unwrap();
        write(&store.join("pkg"), 64 * 1024);
        fs::hard_link(store.join("pkg"), project.join("node_modules/pkg")).unwrap();
        // Two links inside the tree to one inode.
        write(&project.join("node_modules/own"), 64 * 1024);
        fs::hard_link(
            project.join("node_modules/own"),
            project.join("node_modules/own2"),
        )
        .unwrap();

        let size = measure(&project.join("node_modules"), &Cancel::new());
        assert_eq!(size.files, 3);
        let dir_bytes = measure_dir_only(&project.join("node_modules"));
        // `own` counted once; `pkg` counted in on_disk but not reclaimable.
        assert_eq!(size.shared(), allocated(&store.join("pkg")));
        assert_eq!(
            size.reclaimable,
            allocated(&project.join("node_modules/own")) + dir_bytes
        );
    }

    #[cfg(unix)]
    fn allocated(path: &Path) -> u64 {
        allocated_bytes(&fs::symlink_metadata(path).unwrap())
    }

    #[cfg(unix)]
    fn measure_dir_only(path: &Path) -> u64 {
        allocated(path)
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside");
        let tree = root.path().join("tree");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(&tree).unwrap();
        write(&outside.join("big"), 1024 * 1024);
        std::os::unix::fs::symlink(&outside, tree.join("link")).unwrap();
        let size = measure(&tree, &Cancel::new());
        assert!(size.apparent < 1024 * 1024);
        assert_eq!(size.files, 1, "the symlink itself");
    }

    #[test]
    fn missing_paths_report_an_error() {
        let size = measure(Path::new("/definitely/not/here"), &Cancel::new());
        assert_eq!(size.errors, 1);
        assert_eq!(size.on_disk, 0);
    }
}
