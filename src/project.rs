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
}

impl Project {
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

        let authors = package.authors.clone().unwrap_or_default();

        Some(Self {
            name: package.name.clone(),
            path: project_path,
            status: ProjectStatus::Pending,
            version: package.version.clone(),
            authors,
            dependencies,
            workspace_root,
            workspace_name,
            cargo_lock_hash: None, // No hash available here, will be calculated later
            check_status: ProjectCheckStatus::Unchecked, // Start as unchecked
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
    /// Returns `HasUpdates` if any dependency has a newer version available,
    /// otherwise returns `UpToDate`.
    pub fn compute_check_status_from_deps(deps: &[Dependency]) -> ProjectCheckStatus {
        let has_updates = deps.iter().any(|d| {
            d.latest_version
                .as_ref()
                .map(|latest| latest != &d.current_version)
                .unwrap_or(false)
        });

        if has_updates {
            ProjectCheckStatus::HasUpdates
        } else {
            ProjectCheckStatus::UpToDate
        }
    }
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
    pub version: String,
    #[serde(default)]
    pub authors: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct Workspace {
    pub members: Vec<String>,
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

    // First pass: Find all Cargo.toml files and identify workspaces
    for entry in WalkDir::new(&base_path)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            // Skip common directories that won't contain projects
            let file_name = e.file_name().to_string_lossy();
            !file_name.starts_with('.') && file_name != "target" && file_name != "node_modules"
        })
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
}
