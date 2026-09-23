//! Cross-platform caching for dependency update checks
//!
//! This module provides persistent caching of dependency check results,
//! keyed by Cargo.lock file hash to automatically invalidate when dependencies change.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Cached dependency information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedDependency {
    /// The latest version available
    pub latest_version: Option<String>,
    /// When this was cached
    pub cached_at: std::time::SystemTime,
}

/// Cache entry for a project
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectCache {
    /// Hash of the Cargo.lock file this cache is based on
    pub lock_file_hash: u64,
    /// Cached dependency information
    pub dependencies: HashMap<String, CachedDependency>,
}

/// Cross-platform cache directory for carwash
fn app_cache_dir() -> PathBuf {
    if let Some(dirs) = directories::ProjectDirs::from("com", "epistates", "carwash") {
        dirs.cache_dir().to_path_buf()
    } else {
        PathBuf::from("/tmp/carwash-cache")
    }
}

/// Manages cross-platform caching of update check results
#[derive(Clone)]
pub struct UpdateCache {
    cache_dir: PathBuf,
}

impl UpdateCache {
    /// Create a new cache manager
    pub fn new() -> Self {
        Self {
            cache_dir: app_cache_dir(),
        }
    }

    /// Compute hash of a Cargo.lock file
    pub fn hash_cargo_lock(lock_path: &Path) -> Option<u64> {
        let contents = fs::read(lock_path).ok()?;
        let mut hasher = DefaultHasher::new();
        contents.hash(&mut hasher);
        Some(hasher.finish())
    }

    /// Get the cache file path for a project
    fn get_cache_path(&self, project_path: &Path) -> PathBuf {
        // Create a unique cache filename based on project path
        let path_str = project_path.to_string_lossy();
        let mut hasher = DefaultHasher::new();
        path_str.hash(&mut hasher);
        let path_hash = hasher.finish();

        self.cache_dir.join(format!("project_{:x}.json", path_hash))
    }

    /// Load cached dependencies if Cargo.lock hasn't changed
    pub fn load(
        &self,
        project_path: &Path,
        current_lock_hash: u64,
    ) -> Option<HashMap<String, CachedDependency>> {
        let cache_path = self.get_cache_path(project_path);

        let contents = fs::read_to_string(&cache_path).ok()?;
        let cache: ProjectCache = serde_json::from_str(&contents).ok()?;

        // Only return cache if lock file hash matches (not invalidated)
        if cache.lock_file_hash == current_lock_hash {
            Some(cache.dependencies)
        } else {
            None
        }
    }

    /// Save dependency information to cache
    pub fn save(
        &self,
        project_path: &Path,
        lock_hash: u64,
        dependencies: HashMap<String, CachedDependency>,
    ) -> Result<()> {
        // Create cache directory if it doesn't exist
        fs::create_dir_all(&self.cache_dir).context("Failed to create cache directory")?;

        let cache = ProjectCache {
            lock_file_hash: lock_hash,
            dependencies,
        };

        let cache_path = self.get_cache_path(project_path);
        let json = serde_json::to_string(&cache).context("Failed to serialize cache data")?;
        fs::write(&cache_path, json)
            .with_context(|| format!("Failed to write cache file: {}", cache_path.display()))?;

        Ok(())
    }

    /// Clear all cached data
    pub fn clear(&self) -> Result<()> {
        if self.cache_dir.exists() {
            fs::remove_dir_all(&self.cache_dir).with_context(|| {
                format!(
                    "Failed to clear cache directory: {}",
                    self.cache_dir.display()
                )
            })?;
        }
        Ok(())
    }
}

/// A single cached dependency version lookup result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepVersionEntry {
    /// The latest version available on crates.io
    pub latest: String,
    /// Unix timestamp (seconds) when this was checked
    pub checked_at: u64,
}

/// Global per-dependency version cache.
///
/// Keyed by `"name@current_version"` so the same dependency at the same pinned
/// version is never checked twice across any number of projects.
/// When Cargo.lock changes a dep's version, the key changes and a fresh check occurs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DependencyVersionCache {
    entries: HashMap<String, DepVersionEntry>,
}

impl DependencyVersionCache {
    fn cache_key(name: &str, current_version: &str) -> String {
        format!("{}@{}", name, current_version)
    }

    fn cache_file() -> PathBuf {
        app_cache_dir().join("dep_versions.json")
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Load the global dep version cache from disk
    pub fn load() -> Self {
        let path = Self::cache_file();
        let contents = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };
        serde_json::from_str(&contents).unwrap_or_default()
    }

    /// Evict entries older than `max_age_secs`
    pub fn evict_stale(&mut self, max_age_secs: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.entries
            .retain(|_, entry| now.saturating_sub(entry.checked_at) <= max_age_secs);
    }

    /// Save the global dep version cache to disk (evicts entries older than 7 days)
    pub fn save(&mut self) -> Result<()> {
        self.evict_stale(7 * 24 * 3600);
        let dir = app_cache_dir();
        fs::create_dir_all(&dir).context("Failed to create cache directory")?;
        let json = serde_json::to_string(self).context("Failed to serialize dep cache")?;
        fs::write(Self::cache_file(), json).context("Failed to write dep cache")?;
        Ok(())
    }

    /// Look up a cached latest version for a dependency.
    /// Returns `None` if not cached or if the entry is older than `max_age`.
    pub fn lookup(
        &self,
        name: &str,
        current_version: &str,
        max_age: std::time::Duration,
    ) -> Option<&DepVersionEntry> {
        let key = Self::cache_key(name, current_version);
        let entry = self.entries.get(&key)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now.saturating_sub(entry.checked_at) > max_age.as_secs() {
            None
        } else {
            Some(entry)
        }
    }

    /// Insert or update a cached version entry
    pub fn insert(&mut self, name: &str, current_version: &str, latest: String) {
        let key = Self::cache_key(name, current_version);
        let checked_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.entries
            .insert(key, DepVersionEntry { latest, checked_at });
    }

    /// One-time migration: read old per-project cache files and convert to per-dep format.
    /// Deletes old files after successful migration.
    pub fn migrate_from_project_caches() -> Self {
        let dir = app_cache_dir();
        let cache = Self::default();

        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return cache,
        };

        let mut old_files = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.starts_with("project_")
                && name.ends_with(".json")
                && let Ok(contents) = fs::read_to_string(&path)
                && serde_json::from_str::<ProjectCache>(&contents).is_ok()
            {
                // Old per-project cache file — can't migrate entries because the old format
                // doesn't store current_version (needed for the new key). Mark for cleanup;
                // deps will be re-checked on first use.
                old_files.push(path);
            }
        }

        // Clean up old cache files
        for file in old_files {
            let _ = fs::remove_file(file);
        }

        cache
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_path_consistency() {
        let cache = UpdateCache::new();
        let path = Path::new("/home/user/project1");

        // Same path should generate same cache filename
        let path1 = cache.get_cache_path(path);
        let path2 = cache.get_cache_path(path);
        assert_eq!(path1, path2);
    }

    #[test]
    fn test_different_projects_different_cache() {
        let cache = UpdateCache::new();
        let path1 = Path::new("/home/user/project1");
        let path2 = Path::new("/home/user/project2");

        // Different paths should generate different cache filenames
        let cache1 = cache.get_cache_path(path1);
        let cache2 = cache.get_cache_path(path2);
        assert_ne!(cache1, cache2);
    }

    #[test]
    fn test_dep_version_cache_insert_and_lookup() {
        let mut cache = DependencyVersionCache::default();
        cache.insert("serde", "1.0.200", "1.0.228".to_string());

        let entry = cache
            .lookup("serde", "1.0.200", std::time::Duration::from_secs(300))
            .expect("should find entry");
        assert_eq!(entry.latest, "1.0.228");
    }

    #[test]
    fn test_dep_version_cache_different_versions() {
        let mut cache = DependencyVersionCache::default();
        cache.insert("serde", "1.0.200", "1.0.228".to_string());
        cache.insert("serde", "1.0.210", "1.0.228".to_string());

        // Both entries exist independently
        assert!(
            cache
                .lookup("serde", "1.0.200", std::time::Duration::from_secs(300))
                .is_some()
        );
        assert!(
            cache
                .lookup("serde", "1.0.210", std::time::Duration::from_secs(300))
                .is_some()
        );

        // Different version key returns None
        assert!(
            cache
                .lookup("serde", "1.0.100", std::time::Duration::from_secs(300))
                .is_none()
        );
    }

    #[test]
    fn test_dep_version_cache_expired() {
        let mut cache = DependencyVersionCache::default();
        let key = DependencyVersionCache::cache_key("tokio", "1.42.0");
        cache.entries.insert(
            key,
            DepVersionEntry {
                latest: "1.43.0".to_string(),
                checked_at: 0, // epoch = very old
            },
        );

        // Should be expired with any reasonable max_age
        assert!(
            cache
                .lookup("tokio", "1.42.0", std::time::Duration::from_secs(300))
                .is_none()
        );
    }
}
