//! Project structure and dependency management
//!
//! This module defines the core types for managing Rust projects and their dependencies.
//! It handles project discovery, metadata parsing, and dependency tracking.

use cargo_lock::{Lockfile, Package as LockPackage};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Status of a project's command execution
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectStatus {
    /// Pending execution
    Pending,
    /// Currently running
    Running,
    /// Successfully completed
    Success,
    /// Failed execution
    Failed,
}

/// Status of dependency update checking
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyCheckStatus {
    /// Dependencies have not been checked yet
    NotChecked,
    /// Currently checking for updates
    Checking,
    /// Dependencies have been checked
    Checked,
}

/// Status of git repository
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GitStatus {
    #[default]
    Clean,
    Dirty,
    Unknown,
}

/// Visual status of a project's update check state
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProjectCheckStatus {
    /// Not checked yet or cache invalidated (Gray)
    #[default]
    Unchecked,
    /// Currently being checked for updates (Blue)
    Checking,
    /// Some dependencies are outdated (Yellow)
    HasUpdates,
    /// All dependencies up to date (Green)
    UpToDate,
}

/// Represents a single dependency with version information
#[derive(Debug, Clone)]
pub struct Dependency {
    /// Name of the dependency
    pub name: String,
    /// Currently installed version
    pub current_version: String,
    /// Latest available version, if checked
    pub latest_version: Option<String>,
    /// Current status of update checking
    pub check_status: DependencyCheckStatus,
    /// Timestamp of the last check
    pub last_checked: Option<std::time::SystemTime>,
}

impl From<&LockPackage> for Dependency {
    fn from(pkg: &LockPackage) -> Self {
        Self {
            name: pkg.name.as_str().to_string(),
            current_version: pkg.version.to_string(),
            latest_version: None,
            check_status: DependencyCheckStatus::NotChecked,
            last_checked: None,
        }
    }
}

impl Dependency {
    /// Check if a version string is a pre-release (beta, rc, alpha, etc.)
    pub fn is_prerelease(version: &str) -> bool {
        semver::Version::parse(version)
            .map(|v| !v.pre.is_empty())
            .unwrap_or(false)
    }

    /// Check if this dependency has a newer stable version available
    /// Returns true only if:
    /// - Latest version is available and different from current
    /// - If current is stable, latest must also be stable (ignore pre-releases)
    /// - If current is pre-release, any newer version counts
    pub fn has_stable_update(&self) -> bool {
        let Some(ref latest) = self.latest_version else {
            return false;
        };

        // If versions are the same, no update
        if latest == &self.current_version {
            return false;
        }

        // Parse both versions
        let current_semver = semver::Version::parse(&self.current_version).ok();
        let latest_semver = semver::Version::parse(latest).ok();

        match (current_semver, latest_semver) {
            (Some(current), Some(latest_ver)) => {
                // If current is stable but latest is pre-release, don't flag as update
                if current.pre.is_empty() && !latest_ver.pre.is_empty() {
                    return false;
                }
                // Otherwise, if latest is greater, it's an update
                latest_ver > current
            }
            _ => {
                // Fallback: simple string comparison if parsing fails
                latest != &self.current_version
            }
        }
    }

    /// Get update type description for display
    pub fn update_type(&self) -> Option<&'static str> {
        let latest = self.latest_version.as_ref()?;

        if latest == &self.current_version {
            return None;
        }

        let current_is_pre = Self::is_prerelease(&self.current_version);
        let latest_is_pre = Self::is_prerelease(latest);

        match (current_is_pre, latest_is_pre) {
            (false, true) => Some("pre-release"), // Stable → pre-release (usually skip)
            (true, false) => Some("stable"),      // Pre-release → stable (upgrade!)
            (false, false) => Some("stable"),     // Stable → stable
            (true, true) => Some("pre-release"),  // Pre-release → pre-release
        }
    }

    /// Check if the update requires a major version bump
    /// Returns true if the latest version has a different major version than current
    /// This typically means `cargo update` can't auto-update - requires Cargo.toml change
    pub fn is_major_update(&self) -> bool {
        let Some(ref latest) = self.latest_version else {
            return false;
        };

        let current_semver = semver::Version::parse(&self.current_version).ok();
        let latest_semver = semver::Version::parse(latest).ok();

        match (current_semver, latest_semver) {
            (Some(current), Some(latest_ver)) => {
                // For 0.x versions, minor version changes are breaking
                if current.major == 0 {
                    current.minor != latest_ver.minor || current.major != latest_ver.major
                } else {
                    current.major != latest_ver.major
                }
            }
            _ => false,
        }
    }

    /// Get a note about the update constraint, if any
    pub fn update_note(&self) -> Option<&'static str> {
        if !self.has_stable_update() {
            return None;
        }

        if self.is_major_update() {
            Some("requires Cargo.toml change")
        } else {
            None
        }
    }
}

/// Represents a Rust project with metadata and dependencies
///
/// A project is identified by its `Cargo.toml` file and contains metadata about the project
/// such as name, version, authors, and all dependencies. Projects can be standalone or part
/// of a workspace.
#[derive(Debug, Clone)]
pub struct Project {
    /// The name of the project from Cargo.toml
    pub name: String,
    /// The path to the project's root directory
    pub path: PathBuf,
    /// Current status of command execution
    pub status: ProjectStatus,
    /// Version of the project
    pub version: String,
    /// List of project authors
    pub authors: Vec<String>,
    /// All dependencies of the project
    pub dependencies: Vec<Dependency>,
    /// If part of a workspace, the path to the workspace root
    pub workspace_root: Option<PathBuf>,
    /// If part of a workspace, the name of the workspace
    pub workspace_name: Option<String>,
    /// Hash of Cargo.lock file for cache invalidation
    pub cargo_lock_hash: Option<u64>,
    /// Visual status of update checking for UI display
    pub check_status: ProjectCheckStatus,
    /// Git status of the project
    pub git_status: GitStatus,
    /// Total size of the project directory in bytes (calculated on demand)
    pub total_size: Option<u64>,
    /// Size of the target/ directory in bytes (potential savings from cargo clean)
    pub target_size: Option<u64>,
}

impl Project {
    #[allow(dead_code)] // Kept for potential async git checking in future
    fn check_git_status(path: &Path) -> GitStatus {
        use std::process::Command;
        // Run git status --porcelain to check for modifications
        // We run it on the specific path to handle monorepos correctly
        let output = Command::new("git")
            .arg("status")
            .arg("--porcelain")
            .arg(".") // Check only this directory and subdirectories
            .current_dir(path)
            .output();

        match output {
            Ok(output) if output.status.success() => {
                if output.stdout.is_empty() {
                    GitStatus::Clean
                } else {
                    GitStatus::Dirty
                }
            }
            _ => GitStatus::Unknown,
        }
    }

    fn from_toml(
        path: &Path,
        toml: &CargoToml,
        workspace_root: Option<PathBuf>,
        workspace_name: Option<String>,
    ) -> Option<Self> {
        let package = toml.package.as_ref()?;
        let project_path = path.parent()?.to_path_buf();

        // Collect dependency names from this crate's Cargo.toml
        let mut declared_deps: HashSet<String> = HashSet::new();

        // Add regular dependencies
        for dep_name in toml.dependencies.keys() {
            declared_deps.insert(dep_name.clone());
        }

        // Add dev-dependencies
        for dep_name in toml.dev_dependencies.keys() {
            declared_deps.insert(dep_name.clone());
        }

        // Add build-dependencies
        for dep_name in toml.build_dependencies.keys() {
            declared_deps.insert(dep_name.clone());
        }

        // For workspace members, try to load Cargo.lock from workspace root first
        let lockfile_path = if let Some(ref ws_root) = workspace_root {
            let ws_lockfile = ws_root.join("Cargo.lock");
            if ws_lockfile.exists() {
                ws_lockfile
            } else {
                project_path.join("Cargo.lock")
            }
        } else {
            project_path.join("Cargo.lock")
        };

        let dependencies = if let Ok(lockfile) = Lockfile::load(&lockfile_path) {
            lockfile
                .packages
                .iter()
                // Filter to only dependencies declared in this crate's Cargo.toml
                .filter(|pkg| declared_deps.contains(pkg.name.as_str()))
                .map(Dependency::from)
                .collect()
        } else {
            Vec::new()
        };

        let authors = package.authors_vec();

        Some(Self {
            name: package.name.clone(),
            path: project_path,
            status: ProjectStatus::Pending,
            version: package.version_string(),
            authors,
            dependencies,
            workspace_root,
            workspace_name,
            cargo_lock_hash: None, // No hash available here, will be calculated later
            check_status: ProjectCheckStatus::Unchecked, // Start as unchecked
            git_status: GitStatus::Unknown, // Check git status asynchronously
            total_size: None,      // Calculate on demand
            target_size: None,     // Calculate on demand
        })
    }

    /// Reload dependencies from Cargo.lock after an update
    ///
    /// This method re-parses Cargo.toml and Cargo.lock from disk to get the latest
    /// dependency versions. It should be called after running `cargo update` to ensure
    /// the in-memory state reflects what's on disk.
    ///
    /// Returns Ok(()) if successful, Err with error message otherwise.
    pub fn reload_dependencies(&mut self) -> Result<(), String> {
        // Re-parse Cargo.toml to get declared dependencies
        let cargo_toml_path = self.path.join("Cargo.toml");
        let toml_content = fs::read_to_string(&cargo_toml_path)
            .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;

        let toml: CargoToml = toml::from_str(&toml_content)
            .map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

        // Collect declared dependency names
        let mut declared_deps: HashSet<String> = HashSet::new();
        for dep_name in toml.dependencies.keys() {
            declared_deps.insert(dep_name.clone());
        }
        for dep_name in toml.dev_dependencies.keys() {
            declared_deps.insert(dep_name.clone());
        }
        for dep_name in toml.build_dependencies.keys() {
            declared_deps.insert(dep_name.clone());
        }

        // Determine which Cargo.lock to use (workspace or project)
        let lockfile_path = if let Some(ref ws_root) = self.workspace_root {
            let ws_lockfile = ws_root.join("Cargo.lock");
            if ws_lockfile.exists() {
                ws_lockfile
            } else {
                self.path.join("Cargo.lock")
            }
        } else {
            self.path.join("Cargo.lock")
        };

        // Re-parse Cargo.lock
        let lockfile = Lockfile::load(&lockfile_path)
            .map_err(|e| format!("Failed to load Cargo.lock: {}", e))?;

        // Extract current versions for declared dependencies
        let mut new_deps: Vec<Dependency> = lockfile
            .packages
            .iter()
            .filter(|pkg| declared_deps.contains(pkg.name.as_str()))
            .map(|pkg| {
                // Try to preserve latest_version, check_status, and last_checked from existing deps
                let existing = self
                    .dependencies
                    .iter()
                    .find(|d| d.name == pkg.name.as_str());

                let mut dep = Dependency::from(pkg);
                if let Some(existing_dep) = existing {
                    // Preserve the cached check results if they exist
                    dep.latest_version = existing_dep.latest_version.clone();
                    dep.check_status = existing_dep.check_status.clone();
                    dep.last_checked = existing_dep.last_checked;
                }
                dep
            })
            .collect();

        // Sort for consistent ordering
        new_deps.sort_by(|a, b| a.name.cmp(&b.name));

        // Update dependencies
        self.dependencies = new_deps;

        Ok(())
    }

    /// Compute the project check status based on current dependencies
    ///
    /// Examines all dependencies to determine if any have available updates.
    /// Uses `has_stable_update()` to properly handle pre-release versions -
    /// a stable version won't be flagged for update to a pre-release.
    /// Returns `HasUpdates` if any dependency has a newer stable version available,
    /// otherwise returns `UpToDate`.
    pub fn compute_check_status_from_deps(deps: &[Dependency]) -> ProjectCheckStatus {
        let has_updates = deps.iter().any(|d| d.has_stable_update());

        if has_updates {
            ProjectCheckStatus::HasUpdates
        } else {
            ProjectCheckStatus::UpToDate
        }
    }

    /// Calculate the total size of the project directory
    pub fn calculate_total_size(&self) -> Option<u64> {
        calculate_directory_size(&self.path)
    }

    /// Calculate the size of the target/ directory (potential cargo clean savings)
    pub fn calculate_target_size(&self) -> Option<u64> {
        let target_path = self.path.join("target");
        if target_path.exists() && target_path.is_dir() {
            calculate_directory_size(&target_path)
        } else {
            Some(0)
        }
    }

    /// Format size in human-readable format (KB, MB, GB, TB)
    pub fn format_size(bytes: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;
        const TB: u64 = GB * 1024;

        if bytes >= TB {
            format!("{:.1}TB", bytes as f64 / TB as f64)
        } else if bytes >= GB {
            format!("{:.1}GB", bytes as f64 / GB as f64)
        } else if bytes >= MB {
            format!("{:.1}MB", bytes as f64 / MB as f64)
        } else if bytes >= KB {
            format!("{:.1}KB", bytes as f64 / KB as f64)
        } else {
            format!("{}B", bytes)
        }
    }
}

/// Calculate the total size of a directory and all its contents
pub fn calculate_directory_size(path: &Path) -> Option<u64> {
    use walkdir::WalkDir;

    WalkDir::new(path)
        .follow_links(false) // Don't follow symlinks to avoid infinite loops
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .reduce(|acc, size| acc + size)
}

#[derive(Debug, Deserialize)]
pub struct CargoToml {
    pub package: Option<Package>,
    pub workspace: Option<Workspace>,
    #[serde(default)]
    pub dependencies: HashMap<String, toml::Value>,
    #[serde(default, rename = "dev-dependencies")]
    pub dev_dependencies: HashMap<String, toml::Value>,
    #[serde(default, rename = "build-dependencies")]
    pub build_dependencies: HashMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
pub struct Package {
    pub name: String,
    /// Version can be a string OR workspace-inherited (version.workspace = true)
    #[serde(default)]
    pub version: Option<toml::Value>,
    /// Authors can be an array OR workspace-inherited (authors.workspace = true)
    #[serde(default)]
    pub authors: Option<toml::Value>,
}

impl Package {
    /// Extract version as a string, handling workspace inheritance
    pub fn version_string(&self) -> String {
        match &self.version {
            Some(toml::Value::String(s)) => s.clone(),
            Some(toml::Value::Table(t))
                if t.get("workspace") == Some(&toml::Value::Boolean(true)) =>
            {
                "workspace".to_string()
            }
            _ => "0.0.0".to_string(),
        }
    }

    /// Extract authors as a vector, handling workspace inheritance
    pub fn authors_vec(&self) -> Vec<String> {
        match &self.authors {
            Some(toml::Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            Some(toml::Value::Table(t))
                if t.get("workspace") == Some(&toml::Value::Boolean(true)) =>
            {
                vec![] // Workspace-inherited, actual values unknown
            }
            _ => vec![],
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Workspace {
    pub members: Vec<String>,
    // Capture all other workspace fields (package, lints, dependencies, etc.)
    // This ensures we can parse ANY workspace Cargo.toml without failures
    #[serde(flatten)]
    pub other: std::collections::HashMap<String, toml::Value>,
}

/// Recursively finds all Rust projects in the given directory path.
///
/// This function scans for Cargo.toml files, identifies workspaces,
/// and returns a sorted list of projects with their dependencies.
///
/// # Arguments
///
/// * `path` - The directory path to scan for Rust projects
///
/// # Returns
///
/// A vector of `Project` structs, sorted by workspace and name.
/// Projects without dependencies in their Cargo.lock are excluded from the results.
///
/// # Examples
///
/// ```
/// use carwash::project::find_rust_projects;
///
/// let projects = find_rust_projects(".");
/// // Projects will be sorted with workspace members grouped together
/// ```
pub fn find_rust_projects(path: &str) -> Vec<Project> {
    let mut projects = HashMap::new();
    let mut workspaces: HashMap<PathBuf, (String, Vec<PathBuf>)> = HashMap::new();

    // Convert relative path to absolute path
    let base_path = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|| PathBuf::from(path))
    };

    // Build gitignore matcher
    let mut builder = ignore::WalkBuilder::new(&base_path);
    builder
        .follow_links(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .hidden(true) // We'll filter hidden ourselves
        .filter_entry(|e| {
            // Skip common directories that won't contain projects
            let file_name = e.file_name().to_string_lossy();
            !file_name.starts_with('.') && file_name != "target" && file_name != "node_modules"
        });

    // First pass: Find all Cargo.toml files and identify workspaces
    for entry in builder
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() == "Cargo.toml")
    {
        let manifest_path = entry.path();

        if let Ok(content) = fs::read_to_string(manifest_path) {
            if let Ok(toml) = toml::from_str::<CargoToml>(&content) {
                // Check if this is a workspace root
                if let Some(workspace) = &toml.workspace {
                    let root_path = manifest_path.parent().unwrap().to_path_buf();
                    let mut member_paths = Vec::new();

                    // Resolve workspace members
                    for member in &workspace.members {
                        let member_path = root_path.join(member).join("Cargo.toml");
                        if member_path.exists() {
                            member_paths.push(member_path);
                        }
                    }

                    // Get workspace name (use directory name as fallback)
                    let workspace_name = if let Some(pkg) = &toml.package {
                        pkg.name.clone()
                    } else {
                        root_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("workspace")
                            .to_string()
                    };

                    workspaces.insert(root_path, (workspace_name, member_paths));
                }
            }
        }
    }

    // Second pass: Create projects with workspace information
    for entry in WalkDir::new(&base_path)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            let file_name = e.file_name().to_string_lossy();
            !file_name.starts_with('.') && file_name != "target" && file_name != "node_modules"
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() == "Cargo.toml")
    {
        let manifest_path = entry.path();

        if let Ok(content) = fs::read_to_string(manifest_path) {
            if let Ok(toml) = toml::from_str::<CargoToml>(&content) {
                // Find if this project belongs to a workspace
                let mut workspace_info: Option<(PathBuf, String)> = None;

                for (ws_root, (ws_name, members)) in &workspaces {
                    if members.contains(&manifest_path.to_path_buf()) {
                        workspace_info = Some((ws_root.clone(), ws_name.clone()));
                        break;
                    }
                }

                // Add the project if it has a package section
                if let Some(project) = Project::from_toml(
                    manifest_path,
                    &toml,
                    workspace_info.as_ref().map(|(root, _)| root.clone()),
                    workspace_info.as_ref().map(|(_, name)| name.clone()),
                ) {
                    projects.insert(manifest_path.to_path_buf(), project);
                }
            }
        }
    }

    // Convert to vec and sort by workspace and name
    let mut result: Vec<Project> = projects.into_values().collect();
    result.sort_by(|a, b| {
        match (&a.workspace_name, &b.workspace_name) {
            (Some(ws_a), Some(ws_b)) if ws_a == ws_b => {
                // Same workspace: sort by project name
                a.name.cmp(&b.name)
            }
            (Some(ws_a), Some(ws_b)) => {
                // Different workspaces: sort by workspace name
                ws_a.cmp(ws_b)
            }
            (Some(_), None) => std::cmp::Ordering::Less, // Workspace projects first
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.name.cmp(&b.name), // Standalone projects sorted by name
        }
    });

    result
}

/// Build a hierarchical tree of projects organized by directory structure
///
/// This function takes a root path and builds a tree where:
/// - Directories are intermediate nodes that can be expanded/collapsed
/// - Projects (Cargo.toml files) are leaf nodes
/// - Subdirectories are only expanded if they contain projects
///
/// # Arguments
///
/// * `path` - The root directory path to scan
///
/// # Returns
///
/// A `TreeNode` representing the root of the directory tree
pub fn build_project_tree(path: &str) -> crate::tree::TreeNode {
    let base_path = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|| PathBuf::from(path))
    };

    // Build ONLY the root level - no recursive scanning!
    // Children will be loaded lazily when directories are expanded
    build_tree_level_only(&base_path, 0)
}

/// Build a single level of the tree (non-recursive)
fn build_tree_level_only(dir_path: &Path, depth: usize) -> crate::tree::TreeNode {
    let dir_name = dir_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("projects")
        .to_string();

    let mut node =
        crate::tree::TreeNode::directory(dir_name.clone(), dir_path.to_path_buf(), depth);
    node.children_loaded = false; // Mark as not loaded yet
    // Expand root by default, and also auto-expand "crates" directories
    node.expanded = depth == 0 || dir_name == "crates";

    node
}

/// Check if a path should be ignored based on gitignore rules
fn should_ignore_path(path: &Path, parent_dir: &Path) -> bool {
    // Build a gitignore matcher for the parent directory
    let mut builder = ignore::gitignore::GitignoreBuilder::new(parent_dir);

    // Add .gitignore from parent if it exists
    let gitignore_path = parent_dir.join(".gitignore");
    if gitignore_path.exists() {
        let _ = builder.add(&gitignore_path);
    }

    let gitignore = match builder.build() {
        Ok(gi) => gi,
        Err(_) => return false, // If we can't build gitignore, don't ignore
    };

    // Check if path is ignored
    gitignore.matched(path, path.is_dir()).is_ignore()
}

/// Check if a directory contains any Rust projects (recursively, but shallow scan)
/// This does a quick check to see if we should show a directory even when show_all_folders=false
fn directory_contains_rust_projects(path: &Path) -> bool {
    // Check if this directory itself is a Rust project or workspace
    let cargo_toml_path = path.join("Cargo.toml");
    if cargo_toml_path.exists() {
        return true;
    }

    // Check immediate children for Rust projects
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.filter_map(|e| e.ok()) {
            let child_path = entry.path();

            // Skip if ignored by gitignore
            if should_ignore_path(&child_path, path) {
                continue;
            }

            // Skip hidden and common non-project directories
            if let Some(name) = child_path.file_name() {
                let name_str = name.to_string_lossy();
                if name_str.starts_with('.')
                    || name_str == "target"
                    || name_str == "node_modules"
                    || name_str == "vendor"
                    || name_str == "third_party"
                {
                    continue;
                }
            }

            // Check if child directory is a Rust project/workspace OR contains Rust projects
            if child_path.is_dir() {
                if child_path.join("Cargo.toml").exists() {
                    return true;
                }
                // Recursively check one more level for nested projects (e.g., crates/)
                if let Ok(nested_entries) = std::fs::read_dir(&child_path) {
                    for nested in nested_entries.filter_map(|e| e.ok()) {
                        if nested.path().is_dir() && nested.path().join("Cargo.toml").exists() {
                            return true;
                        }
                    }
                }
            }
        }
    }

    false
}

/// Load children for a directory node (lazy loading) - Async friendly version
/// Returns a vector of children nodes
pub fn load_directory_children_async(
    dir_path: &Path,
    depth: usize,
    show_all_folders: bool,
) -> Vec<crate::tree::TreeNode> {
    let mut children = Vec::new();

    // Scan for Rust projects directly in this directory
    if let Ok(entries) = std::fs::read_dir(dir_path) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();

            // Skip if ignored by gitignore
            if should_ignore_path(&path, dir_path) {
                continue;
            }

            let file_name = match path.file_name() {
                Some(name) => name.to_string_lossy(),
                None => continue,
            };

            // Skip hidden directories and common build directories
            if file_name.starts_with('.')
                || file_name == "target"
                || file_name == "node_modules"
                || file_name == "vendor"
                || file_name == "third_party"
            {
                continue;
            }

            if path.is_dir() {
                // Check if this directory IS a Rust project
                let cargo_toml_path = path.join("Cargo.toml");
                if cargo_toml_path.exists() {
                    // Parse Cargo.toml to check if it's a workspace-only or a real project
                    if let Ok(content) = std::fs::read_to_string(&cargo_toml_path) {
                        if let Ok(toml) = toml::from_str::<CargoToml>(&content) {
                            // Check if this has a workspace section (with or without package)
                            // Workspaces should be expandable directories to show their members
                            if toml.workspace.is_some() {
                                // It's a workspace root - treat as expandable directory
                                // Even if it also has a [package] section, prioritize showing members
                                if show_all_folders || directory_contains_rust_projects(&path) {
                                    let mut dir_node = build_tree_level_only(&path, depth + 1);
                                    // Load children eagerly so "crates" dirs can be auto-expanded
                                    // NOTE: For async version, we might want to skip this eager loading or make it recursive?
                                    // For now, let's keep it consistent but synchronous for this sublevel
                                    load_directory_children(&mut dir_node, show_all_folders);
                                    // But keep the workspace itself collapsed - user can expand with h/l or ←/→
                                    dir_node.expanded = false;
                                    children.push(dir_node);
                                }
                            } else if toml.package.is_some() {
                                // It's a standalone project (no workspace) - add as project node
                                let project = Project {
                                    name: toml
                                        .package
                                        .as_ref()
                                        .map(|p| p.name.clone())
                                        .unwrap_or_else(|| file_name.to_string()),
                                    path: path.clone(),
                                    version: toml
                                        .package
                                        .as_ref()
                                        .map(|p| p.version_string())
                                        .unwrap_or_default(),
                                    authors: toml
                                        .package
                                        .as_ref()
                                        .map(|p| p.authors_vec())
                                        .unwrap_or_default(),
                                    dependencies: Vec::new(), // Will be loaded on-demand when needed
                                    workspace_root: None,
                                    workspace_name: None,
                                    cargo_lock_hash: None,
                                    status: ProjectStatus::Pending,
                                    check_status: ProjectCheckStatus::Unchecked,
                                    git_status: GitStatus::Unknown, // Check asynchronously
                                    total_size: None,               // Calculate on demand
                                    target_size: None,              // Calculate on demand
                                };
                                let project_node =
                                    crate::tree::TreeNode::project(project, depth + 1);
                                children.push(project_node);
                            }
                        }
                    }
                } else {
                    // No Cargo.toml - check if we should show it as a directory
                    if show_all_folders || directory_contains_rust_projects(&path) {
                        let dir_node = build_tree_level_only(&path, depth + 1);
                        // Don't override expanded state - build_tree_level_only sets it correctly
                        // (e.g., "crates" directories are auto-expanded)
                        children.push(dir_node);
                    }
                }
            }
        }
    }

    // Sort children
    children.sort_by(|a, b| match (&a.node_type, &b.node_type) {
        (
            crate::tree::TreeNodeType::Directory { name: a_name, .. },
            crate::tree::TreeNodeType::Directory { name: b_name, .. },
        ) => a_name.cmp(b_name),
        (
            crate::tree::TreeNodeType::Project(a_proj),
            crate::tree::TreeNodeType::Project(b_proj),
        ) => a_proj.name.cmp(&b_proj.name),
        (crate::tree::TreeNodeType::Directory { .. }, crate::tree::TreeNodeType::Project(_)) => {
            std::cmp::Ordering::Less
        }
        (crate::tree::TreeNodeType::Project(_), crate::tree::TreeNodeType::Directory { .. }) => {
            std::cmp::Ordering::Greater
        }
    });

    children
}

/// Load children for a directory node (lazy loading)
/// This scans only the immediate children (1 level deep)
///
/// If `show_all_folders` is false, only directories containing Rust projects (Cargo.toml) are shown.
/// If `show_all_folders` is true, all directories are shown.
pub fn load_directory_children(node: &mut crate::tree::TreeNode, show_all_folders: bool) {
    if node.children_loaded {
        return; // Already loaded
    }

    let dir_path = match &node.node_type {
        crate::tree::TreeNodeType::Directory { path, .. } => path,
        _ => return, // Not a directory
    };

    node.children = load_directory_children_async(dir_path, node.depth, show_all_folders);
    node.children_loaded = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_project_discovery() {
        // Test that the discovery function runs without panicking
        let _projects = find_rust_projects(".");
        // In CI or clean environments, there might be no projects with dependencies
        // This is fine - we just verify the function completes without panic
    }

    #[test]
    fn test_cargo_toml_parsing() {
        // Verify we can parse a basic Cargo.toml structure
        let toml_content = r#"
[package]
name = "test-project"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = "1.0"
"#;
        let parsed: Result<CargoToml, _> = toml::from_str(toml_content);
        assert!(parsed.is_ok(), "Should parse valid Cargo.toml");

        let cargo_toml = parsed.unwrap();
        assert!(cargo_toml.package.is_some());
        assert_eq!(cargo_toml.dependencies.len(), 1);
        assert!(cargo_toml.dependencies.contains_key("serde"));
    }

    #[test]
    fn test_cargo_toml_parsing_with_dev_dependencies() {
        let toml_content = r#"
[package]
name = "test-project"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = "1.0"

[dev-dependencies]
tokio = { version = "1.0", features = ["full"] }
"#;
        let parsed: Result<CargoToml, _> = toml::from_str(toml_content);
        assert!(parsed.is_ok());

        let cargo_toml = parsed.unwrap();
        assert_eq!(cargo_toml.dependencies.len(), 1);
        assert_eq!(cargo_toml.dev_dependencies.len(), 1);
        assert!(cargo_toml.dev_dependencies.contains_key("tokio"));
    }

    #[test]
    fn test_cargo_toml_parsing_workspace() {
        let toml_content = r#"
[workspace]
members = ["crate1", "crate2"]
"#;
        let parsed: Result<CargoToml, _> = toml::from_str(toml_content);
        assert!(parsed.is_ok());

        let cargo_toml = parsed.unwrap();
        assert!(cargo_toml.workspace.is_some());
        assert_eq!(cargo_toml.workspace.unwrap().members.len(), 2);
    }

    #[test]
    fn test_cargo_toml_workspace_inherited_fields() {
        // Test parsing Cargo.toml with workspace-inherited fields (like turbovault)
        let toml_content = r#"
[package]
name = "test-crate"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
publish = true

[dependencies]
serde = { workspace = true }
"#;
        let cargo_toml: Result<CargoToml, _> = toml::from_str(toml_content);
        assert!(
            cargo_toml.is_ok(),
            "Should parse workspace-inherited fields: {:?}",
            cargo_toml.err()
        );

        let parsed = cargo_toml.unwrap();
        assert!(parsed.package.is_some());

        let package = parsed.package.unwrap();
        assert_eq!(package.name, "test-crate");
        assert_eq!(package.version_string(), "workspace");
        assert_eq!(package.authors_vec(), Vec::<String>::new()); // workspace-inherited, so empty
    }

    #[test]
    fn test_project_status_default() {
        let status = ProjectStatus::Pending;
        assert_eq!(status, ProjectStatus::Pending);
    }

    #[test]
    fn test_dependency_check_status() {
        let status = DependencyCheckStatus::NotChecked;
        assert_eq!(status, DependencyCheckStatus::NotChecked);

        let checking = DependencyCheckStatus::Checking;
        assert_eq!(checking, DependencyCheckStatus::Checking);

        let checked = DependencyCheckStatus::Checked;
        assert_eq!(checked, DependencyCheckStatus::Checked);
    }

    #[test]
    fn test_dependency_from_lock_package() {
        use cargo_lock::{Package as LockPackage, Version};

        let pkg = LockPackage {
            name: "test-crate".parse().unwrap(),
            version: Version::parse("1.2.3").unwrap(),
            source: None,
            checksum: None,
            dependencies: Vec::new(),
            replace: None,
        };

        let dep = Dependency::from(&pkg);
        assert_eq!(dep.name, "test-crate");
        assert_eq!(dep.current_version, "1.2.3");
        assert_eq!(dep.check_status, DependencyCheckStatus::NotChecked);
        assert!(dep.latest_version.is_none());
    }

    #[test]
    fn test_is_prerelease() {
        // Stable versions
        assert!(!Dependency::is_prerelease("1.0.0"));
        assert!(!Dependency::is_prerelease("2.3.4"));
        assert!(!Dependency::is_prerelease("0.1.0"));

        // Pre-release versions
        assert!(Dependency::is_prerelease("1.0.0-beta.1"));
        assert!(Dependency::is_prerelease("2.0.0-rc.1"));
        assert!(Dependency::is_prerelease("1.0.0-alpha"));
        assert!(Dependency::is_prerelease("0.5.0-pre"));

        // Invalid versions (fallback to false)
        assert!(!Dependency::is_prerelease("not-a-version"));
    }

    #[test]
    fn test_has_stable_update_stable_to_stable() {
        // Stable → newer stable = update
        let dep = Dependency {
            name: "test".into(),
            current_version: "1.0.0".into(),
            latest_version: Some("1.1.0".into()),
            check_status: DependencyCheckStatus::Checked,
            last_checked: None,
        };
        assert!(dep.has_stable_update());
    }

    #[test]
    fn test_has_stable_update_stable_to_prerelease() {
        // Stable → pre-release = NO update (user is on stable, don't suggest beta)
        let dep = Dependency {
            name: "test".into(),
            current_version: "1.0.0".into(),
            latest_version: Some("2.0.0-beta.1".into()),
            check_status: DependencyCheckStatus::Checked,
            last_checked: None,
        };
        assert!(!dep.has_stable_update());
    }

    #[test]
    fn test_has_stable_update_prerelease_to_stable() {
        // Pre-release → stable = update (user should upgrade to stable)
        let dep = Dependency {
            name: "test".into(),
            current_version: "2.0.0-beta.1".into(),
            latest_version: Some("2.0.0".into()),
            check_status: DependencyCheckStatus::Checked,
            last_checked: None,
        };
        assert!(dep.has_stable_update());
    }

    #[test]
    fn test_has_stable_update_prerelease_to_newer_prerelease() {
        // Pre-release → newer pre-release = update
        let dep = Dependency {
            name: "test".into(),
            current_version: "2.0.0-beta.1".into(),
            latest_version: Some("2.0.0-beta.2".into()),
            check_status: DependencyCheckStatus::Checked,
            last_checked: None,
        };
        assert!(dep.has_stable_update());
    }

    #[test]
    fn test_has_stable_update_same_version() {
        // Same version = no update
        let dep = Dependency {
            name: "test".into(),
            current_version: "1.0.0".into(),
            latest_version: Some("1.0.0".into()),
            check_status: DependencyCheckStatus::Checked,
            last_checked: None,
        };
        assert!(!dep.has_stable_update());
    }

    #[test]
    fn test_has_stable_update_no_latest() {
        // No latest version = no update
        let dep = Dependency {
            name: "test".into(),
            current_version: "1.0.0".into(),
            latest_version: None,
            check_status: DependencyCheckStatus::NotChecked,
            last_checked: None,
        };
        assert!(!dep.has_stable_update());
    }

    #[test]
    fn test_update_type() {
        // Stable → stable
        let dep1 = Dependency {
            name: "test".into(),
            current_version: "1.0.0".into(),
            latest_version: Some("2.0.0".into()),
            check_status: DependencyCheckStatus::Checked,
            last_checked: None,
        };
        assert_eq!(dep1.update_type(), Some("stable"));

        // Stable → pre-release
        let dep2 = Dependency {
            name: "test".into(),
            current_version: "1.0.0".into(),
            latest_version: Some("2.0.0-beta.1".into()),
            check_status: DependencyCheckStatus::Checked,
            last_checked: None,
        };
        assert_eq!(dep2.update_type(), Some("pre-release"));

        // Pre-release → stable
        let dep3 = Dependency {
            name: "test".into(),
            current_version: "2.0.0-beta.1".into(),
            latest_version: Some("2.0.0".into()),
            check_status: DependencyCheckStatus::Checked,
            last_checked: None,
        };
        assert_eq!(dep3.update_type(), Some("stable"));

        // Same version = no update type
        let dep4 = Dependency {
            name: "test".into(),
            current_version: "1.0.0".into(),
            latest_version: Some("1.0.0".into()),
            check_status: DependencyCheckStatus::Checked,
            last_checked: None,
        };
        assert_eq!(dep4.update_type(), None);
    }

    #[test]
    fn test_empty_cargo_toml() {
        let toml_content = r#"
[dependencies]
"#;
        let parsed: Result<CargoToml, _> = toml::from_str(toml_content);
        assert!(parsed.is_ok());

        let cargo_toml = parsed.unwrap();
        assert!(cargo_toml.package.is_none());
        assert!(cargo_toml.dependencies.is_empty());
    }

    #[test]
    fn test_relative_path_resolution() {
        // Test that relative paths like "." are properly resolved
        let cwd = std::env::current_dir().ok().unwrap_or_default();
        let cwd_str = cwd.to_string_lossy();

        // Both should work - the current implementation should resolve them
        let projects_dot = find_rust_projects(".");
        let projects_cwd = find_rust_projects(cwd_str.as_ref());

        // Both should find the same number of projects
        assert_eq!(projects_dot.len(), projects_cwd.len());

        // Verify that projects are properly discovered
        // (the exact number depends on the test environment)
        let _ = projects_dot;
    }

    #[test]
    fn test_build_project_tree() {
        // Test that tree building doesn't panic
        let tree = build_project_tree(".");

        // Tree should have a root node
        assert_eq!(tree.node_type.name(), "carwash");

        // Root should be a directory
        assert!(tree.node_type.is_directory());

        // Root should be expanded by default
        assert!(tree.expanded);

        // Tree structure should be created successfully
        let _ = tree;
    }

    #[test]
    fn test_tree_navigation() {
        // Test tree flattening
        let tree = build_project_tree(".");
        let flattened = crate::tree::FlattenedTree::from_tree(&tree);

        // Flattened tree should have at least the root node
        assert!(!flattened.items.is_empty());
    }
}
