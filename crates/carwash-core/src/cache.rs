//! Persistent cache of measured artifact sizes.
//!
//! Measuring tens of gigabytes of `target/` and `node_modules` takes a while; the cache lets
//! a frontend show last-known sizes instantly and refine them as fresh measurements arrive.

use crate::model::Size;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const VERSION: u32 = 1;
/// Entries not refreshed for this long are dropped on save.
const MAX_AGE: Duration = Duration::from_secs(60 * 60 * 24 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedSize {
    pub size: Size,
    /// Unix seconds.
    pub measured_at: u64,
    /// The tree's [`Fingerprint`] taken just before measuring, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
}

impl CachedSize {
    /// Time since the measurement, as of `now`.
    pub fn age_at(&self, now: SystemTime) -> Duration {
        let now = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        Duration::from_secs(now.saturating_sub(self.measured_at))
    }
}

/// A cheap summary of a tree's top levels: the newest modification time and the number of
/// entries in the directories at most [`Fingerprint::DEPTH`] levels down.
///
/// Adding or removing entries up there changes it, which is how caches usually change
/// (new packages, toolchains, simulators). Edits deeper down leave it untouched, so an
/// unchanged fingerprint is evidence, not proof, that the tree is the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    /// Nanoseconds since the Unix epoch.
    pub newest: u64,
    pub entries: u64,
}

impl Fingerprint {
    pub const DEPTH: usize = 2;
    /// Trees with more entries than this near the top get no fingerprint.
    const MAX_ENTRIES: u64 = 50_000;

    /// `None` when `path` cannot be read or is too wide near the top.
    pub fn of(path: &Path) -> Option<Self> {
        let mut fingerprint = Self {
            newest: 0,
            entries: 0,
        };
        fingerprint.visit(path, 0, true)?;
        Some(fingerprint)
    }

    fn visit(&mut self, dir: &Path, depth: usize, root: bool) -> Option<()> {
        let modified = std::fs::symlink_metadata(dir)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as u64);
        self.newest = self.newest.max(modified);
        if depth == Self::DEPTH {
            return Some(());
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            // An unreadable subdirectory is part of the summary as it is; the root must read.
            Err(_) if !root => return Some(()),
            Err(_) => return None,
        };
        for entry in entries.flatten() {
            self.entries += 1;
            if self.entries > Self::MAX_ENTRIES {
                return None;
            }
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                self.visit(&entry.path(), depth + 1, false)?;
            }
        }
        Some(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SizeCache {
    version: u32,
    entries: HashMap<PathBuf, CachedSize>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl SizeCache {
    /// Loads the cache; a missing, unreadable or outdated file yields an empty cache.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|cache| cache.version == VERSION)
            .unwrap_or_else(|| Self {
                version: VERSION,
                entries: HashMap::new(),
            })
    }

    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        self.version = VERSION;
        let cutoff = unix_now().saturating_sub(MAX_AGE.as_secs());
        self.entries.retain(|_, entry| entry.measured_at >= cutoff);
        let json = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        crate::paths::write_atomic(path, &json)
    }

    pub fn get(&self, path: &Path) -> Option<&CachedSize> {
        self.entries.get(path)
    }

    pub fn insert(&mut self, path: PathBuf, size: Size) {
        self.insert_with(path, size, None);
    }

    /// Records a size along with the tree's fingerprint from just before measuring.
    pub fn insert_with(&mut self, path: PathBuf, size: Size, fingerprint: Option<Fingerprint>) {
        self.entries.insert(
            path,
            CachedSize {
                size,
                measured_at: unix_now(),
                fingerprint,
            },
        );
    }

    pub fn remove(&mut self, path: &Path) {
        self.entries.remove(path);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_ignores_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sizes.json");
        let mut cache = SizeCache::load(&file);
        assert!(cache.is_empty());
        let size = Size {
            on_disk: 42,
            reclaimable: 40,
            ..Size::default()
        };
        cache.insert(PathBuf::from("/p/target"), size);
        cache.save(&file).unwrap();

        let loaded = SizeCache::load(&file);
        assert_eq!(loaded.get(Path::new("/p/target")).unwrap().size, size);

        std::fs::write(&file, b"{not json").unwrap();
        assert!(SizeCache::load(&file).is_empty());
    }

    #[test]
    fn entries_without_a_fingerprint_still_load() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sizes.json");
        let mut cache = SizeCache::load(&file);
        cache.insert(PathBuf::from("/p"), Size::default());
        cache.save(&file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            !text.contains("fingerprint"),
            "same shape as before: {text}"
        );
        let cache = SizeCache::load(&file);
        let entry = cache.get(Path::new("/p")).expect("old entry loads");
        assert_eq!(entry.fingerprint, None);
    }

    #[test]
    fn fingerprints_notice_changes_near_the_top_only() {
        use filetime::{FileTime, set_file_mtime};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("a/b/c/d")).unwrap();
        let old = FileTime::from_unix_time(1_000_000, 0);
        for sub in ["", "a", "a/b", "a/b/c", "a/b/c/d"] {
            set_file_mtime(root.join(sub), old).unwrap();
        }
        let before = Fingerprint::of(root).unwrap();
        assert_eq!(Fingerprint::of(root), Some(before), "stable");

        // Four levels down: invisible.
        std::fs::write(root.join("a/b/c/d/deep"), b"x").unwrap();
        assert_eq!(Fingerprint::of(root), Some(before));

        // Two levels down (a new package dir, say): noticed.
        std::fs::create_dir(root.join("a/b/new")).unwrap();
        assert_ne!(Fingerprint::of(root), Some(before));

        assert_eq!(Fingerprint::of(&root.join("missing")), None);
    }
}
