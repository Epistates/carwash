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
    pub workspace_root: Option<PathBuf>,
    pub workspace_name: Option<String>,
}

impl Project {
    fn from_toml(path: &Path, toml: &CargoToml, workspace_root: Option<PathBuf>, workspace_name: Option<String>) -> Option<Self> {
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
            lockfile.packages.iter()
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
        })
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

pub fn find_rust_projects(path: &str) -> Vec<Project> {
    let mut projects = HashMap::new();
    let mut workspaces: HashMap<PathBuf, (String, Vec<PathBuf>)> = HashMap::new();

    // First pass: Find all Cargo.toml files and identify workspaces
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
        
        if let Ok(content) = fs::read_to_string(manifest_path) {
            if let Ok(toml) = toml::from_str::<CargoToml>(&content) {
                // Check if this is a workspace root
                if let Some(workspace) = &toml.workspace {
                    let root_path = manifest_path.parent().unwrap().to_path_buf();
                    let mut member_paths = Vec::new();
                    
                    // Resolve workspace members
                    for member_glob in &workspace.members {
                        let full_glob = root_path.join(member_glob).join("Cargo.toml");
                        if let Some(full_glob_str) = full_glob.to_str() {
                            if let Ok(paths) = glob::glob(full_glob_str) {
                                for member_manifest in paths.filter_map(Result::ok) {
                                    member_paths.push(member_manifest);
                                }
                            }
                        }
                    }
                    
                    // Get workspace name (use directory name as fallback)
                    let workspace_name = if let Some(pkg) = &toml.package {
                        pkg.name.clone()
                    } else {
                        root_path.file_name()
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
    for entry in WalkDir::new(path)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            let file_name = e.file_name().to_string_lossy();
            !file_name.starts_with('.') && 
            file_name != "target" && 
            file_name != "node_modules"
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
                    workspace_info.as_ref().map(|(_, name)| name.clone())
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
}
