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

/// Manages cross-platform caching of update check results
#[derive(Clone)]
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

        // Debug logging
        use std::io::Write;
        let mut log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/carwash-debug.log")
            .ok();

        let contents = match fs::read_to_string(&cache_path) {
            Ok(c) => c,
            Err(e) => {
                if let Some(ref mut f) = log_file {
                    let _ = writeln!(f, "  [CACHE] Failed to read cache file {}: {}",
                                   cache_path.display(), e);
                }
                return None;
            }
        };

        let cache: ProjectCache = match serde_json::from_str(&contents) {
            Ok(c) => c,
            Err(e) => {
                if let Some(ref mut f) = log_file {
                    let _ = writeln!(f, "  [CACHE] Failed to parse cache file {}: {}",
                                   cache_path.display(), e);
                }
                return None;
            }
        };

        // Only return cache if lock file hash matches (not invalidated)
        if cache.lock_file_hash == current_lock_hash {
            if let Some(ref mut f) = log_file {
                let _ = writeln!(f, "  [CACHE] Hash match! Loaded {} deps from {}",
                               cache.dependencies.len(), cache_path.display());
            }
            Some(cache.dependencies)
        } else {
            if let Some(ref mut f) = log_file {
                let _ = writeln!(f, "  [CACHE] Hash mismatch! cached={:x}, current={:x} for {}",
                               cache.lock_file_hash, current_lock_hash, cache_path.display());
            }
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
        fs::create_dir_all(&self.cache_dir)
            .context("Failed to create cache directory")?;

        let cache = ProjectCache {
            lock_file_hash: lock_hash,
            dependencies,
        };

        let cache_path = self.get_cache_path(project_path);
        let json = serde_json::to_string(&cache)
            .context("Failed to serialize cache data")?;
        fs::write(&cache_path, json)
            .with_context(|| format!("Failed to write cache file: {}", cache_path.display()))?;

        Ok(())
    }

    /// Clear all cached data
    pub fn clear(&self) -> Result<()> {
        if self.cache_dir.exists() {
            fs::remove_dir_all(&self.cache_dir)
                .with_context(|| format!("Failed to clear cache directory: {}", self.cache_dir.display()))?;
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
