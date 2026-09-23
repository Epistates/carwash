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
}

impl CachedSize {
    pub fn age(&self) -> Duration {
        let now = unix_now();
        Duration::from_secs(now.saturating_sub(self.measured_at))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
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
        self.entries.insert(
            path,
            CachedSize {
                size,
                measured_at: unix_now(),
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
}
