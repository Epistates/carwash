use cargo_lock::{Lockfile, Package as LockPackage};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectStatus {
    Pending,
    Running,
    Success,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyCheckStatus {
    NotChecked,
    Checking,
    Checked,
}

#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub check_status: DependencyCheckStatus,
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

#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    pub status: ProjectStatus,
    pub version: String,
    pub authors: Vec<String>,
    pub dependencies: Vec<Dependency>,
}

impl Project {
    fn from_toml(path: &Path, toml: &CargoToml) -> Option<Self> {
        let package = toml.package.as_ref()?;
        let project_path = path.parent()?.to_path_buf();
        let lockfile_path = project_path.join("Cargo.lock");
        
        let dependencies = if let Ok(lockfile) = Lockfile::load(&lockfile_path) {
            lockfile.packages.iter()
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
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct CargoToml {
    pub package: Option<Package>,
    pub workspace: Option<Workspace>,
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

pub fn find_rust_projects(path: &str) -> Vec<Project> {
    let mut projects = HashMap::new();
    let mut workspace_paths = HashSet::new();

    // Walk through directory tree looking for Cargo.toml files
    for entry in WalkDir::new(path)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            // Skip common directories that won't contain projects
            let file_name = e.file_name().to_string_lossy();
            !file_name.starts_with('.') && 
            file_name != "target" && 
            file_name != "node_modules"
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() == "Cargo.toml")
    {
        let manifest_path = entry.path();
        
        match fs::read_to_string(manifest_path) {
            Ok(content) => {
                match toml::from_str::<CargoToml>(&content) {
                    Ok(toml) => {
                        // Handle workspace manifests
                        if let Some(workspace) = &toml.workspace {
                            let root_path = manifest_path.parent().unwrap();
                            for member_glob in &workspace.members {
                                let full_glob = root_path.join(member_glob).join("Cargo.toml");
                                if let Some(full_glob_str) = full_glob.to_str().map(|s| s.to_string()) {
                                    if let Ok(paths) = glob::glob(&full_glob_str) {
                                        for member_manifest in paths.filter_map(Result::ok) {
                                            workspace_paths.insert(member_manifest);
                                        }
                                    }
                                    // Silently ignore glob errors during scanning
                                }
                            }
                        }
                        
                        // Add the project if it has a package section
                        if let Some(project) = Project::from_toml(manifest_path, &toml) {
                            projects.insert(manifest_path.to_path_buf(), project);
                        }
                    }
                    Err(_) => {
                        // Silently ignore TOML parse errors - might be config files, etc.
                    }
                }
            }
            Err(_) => {
                // Silently ignore file read errors - might be permissions, etc.
            }
        }
    }
    
    // Filter out workspace members, keeping only root workspace or standalone projects
    let mut result: Vec<Project> = projects.into_iter()
        .filter(|(path, _)| !workspace_paths.contains(path))
        .map(|(_, project)| project)
        .collect();
    
    // Sort by name for consistent ordering
    result.sort_by(|a, b| a.name.cmp(&b.name));
    
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_project_discovery() {
        // This test would need actual test fixtures
        let projects = find_rust_projects(".");
        assert!(!projects.is_empty(), "Should find at least the carwash project");
    }
}
