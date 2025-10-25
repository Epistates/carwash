//! Cross-platform caching for dependency update checks
//!
//! This module provides persistent caching of dependency check results,
//! keyed by Cargo.lock file hash to automatically invalidate when dependencies change.

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

/// Manages cross-platform caching of update check results
pub struct UpdateCache {
    cache_dir: PathBuf,
}

impl UpdateCache {
    /// Create a new cache manager
    pub fn new() -> Self {
        let cache_dir = if let Some(cache_base) =
            directories::ProjectDirs::from("com", "epistates", "carwash")
        {
            cache_base.cache_dir().to_path_buf()
        } else {
            // Fallback to temp directory if project dirs not available
            std::path::PathBuf::from("/tmp/carwash-cache")
        };

        Self { cache_dir }
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
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Create cache directory if it doesn't exist
        fs::create_dir_all(&self.cache_dir)?;

        let cache = ProjectCache {
            lock_file_hash: lock_hash,
            dependencies,
        };

        let cache_path = self.get_cache_path(project_path);
        let json = serde_json::to_string(&cache)?;
        fs::write(cache_path, json)?;

        Ok(())
    }

    /// Clear all cached data
    pub fn clear(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.cache_dir.exists() {
            fs::remove_dir_all(&self.cache_dir)?;
        }
        Ok(())
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
}
