//! Content-aware hooks for ecosystems whose manifests carry names and workspace layouts.
//!
//! Adapters only read small manifest files and never fail discovery: unreadable or
//! malformed manifests simply yield no information.

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use std::fs;
use std::path::Path;

/// Built-in adapter selected by an ecosystem's `adapter` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adapter {
    Cargo,
    Node,
    Python,
    Go,
    Gradle,
    Dotnet,
    Dart,
}

/// How a workspace root declares its members.
#[derive(Debug, Clone)]
pub enum WorkspaceSpec {
    /// Member paths relative to the root, as globs.
    Globs { include: GlobSet, exclude: GlobSet },
    /// Every nested project of the same ecosystem is a member.
    Nested,
}

impl WorkspaceSpec {
    /// `relative` uses `/` separators and is relative to the workspace root.
    pub fn contains(&self, relative: &str) -> bool {
        match self {
            WorkspaceSpec::Globs { include, exclude } => {
                include.is_match(relative) && !exclude.is_match(relative)
            }
            WorkspaceSpec::Nested => true,
        }
    }
}

/// What an adapter learned about a project directory.
#[derive(Debug, Clone, Default)]
pub struct ProjectInfo {
    pub name: Option<String>,
    pub workspace: Option<WorkspaceSpec>,
}

impl Adapter {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "cargo" => Self::Cargo,
            "node" => Self::Node,
            "python" => Self::Python,
            "go" => Self::Go,
            "gradle" => Self::Gradle,
            "dotnet" => Self::Dotnet,
            "dart" => Self::Dart,
            _ => return None,
        })
    }

    /// Inspects `dir`, whose entry names are `names`.
    pub fn inspect(self, dir: &Path, names: &[&str]) -> ProjectInfo {
        match self {
            Adapter::Cargo => cargo(dir),
            Adapter::Node => node(dir, names),
            Adapter::Python => python(dir),
            Adapter::Go => go(dir, names),
            Adapter::Gradle => gradle(dir, names),
            Adapter::Dotnet => dotnet(names),
            Adapter::Dart => dart(dir),
        }
    }
}

/// Manifests larger than this are not parsed; real ones are a few kilobytes.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

fn read(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    fs::read_to_string(path).ok()
}

fn read_toml(path: &Path) -> Option<toml::Table> {
    read(path)?.parse::<toml::Table>().ok()
}

/// Builds a workspace spec from glob patterns; `!pattern` entries are exclusions.
fn globs_spec<'a>(
    patterns: impl IntoIterator<Item = &'a str>,
    excludes: impl IntoIterator<Item = &'a str>,
) -> Option<WorkspaceSpec> {
    let mut include = GlobSetBuilder::new();
    let mut exclude = GlobSetBuilder::new();
    let mut any = false;
    let add = |builder: &mut GlobSetBuilder, raw: &str| {
        let pattern = raw.trim().trim_start_matches("./").trim_end_matches('/');
        if pattern.is_empty() || pattern == "." {
            return false;
        }
        match GlobBuilder::new(pattern).literal_separator(true).build() {
            Ok(glob) => {
                builder.add(glob);
                true
            }
            Err(_) => false,
        }
    };
    for raw in patterns {
        if let Some(negated) = raw.strip_prefix('!') {
            add(&mut exclude, negated);
        } else if add(&mut include, raw) {
            any = true;
        }
    }
    for raw in excludes {
        add(&mut exclude, raw);
    }
    if !any {
        return None;
    }
    Some(WorkspaceSpec::Globs {
        include: include.build().ok()?,
        exclude: exclude.build().ok()?,
    })
}

fn str_array(value: Option<&toml::Value>) -> Vec<&str> {
    value
        .and_then(toml::Value::as_array)
        .map(|items| items.iter().filter_map(toml::Value::as_str).collect())
        .unwrap_or_default()
}

fn cargo(dir: &Path) -> ProjectInfo {
    let Some(manifest) = read_toml(&dir.join("Cargo.toml")) else {
        return ProjectInfo::default();
    };
    let name = manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    let workspace = manifest.get("workspace").map(|ws| {
        let members = str_array(ws.get("members"));
        let exclude = str_array(ws.get("exclude"));
        globs_spec(members, exclude).unwrap_or(WorkspaceSpec::Nested)
    });
    ProjectInfo { name, workspace }
}

fn node(dir: &Path, names: &[&str]) -> ProjectInfo {
    let manifest: Option<serde_json::Value> =
        read(&dir.join("package.json")).and_then(|s| serde_json::from_str(&s).ok());
    let name = manifest
        .as_ref()
        .and_then(|m| m.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_owned);

    let mut patterns: Vec<String> = Vec::new();
    if let Some(workspaces) = manifest.as_ref().and_then(|m| m.get("workspaces")) {
        let list = workspaces.as_array().or_else(|| {
            workspaces
                .get("packages")
                .and_then(serde_json::Value::as_array)
        });
        patterns.extend(
            list.into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned)),
        );
    }
    if names.contains(&"pnpm-workspace.yaml")
        && let Some(yaml) = read(&dir.join("pnpm-workspace.yaml"))
        && let Ok(doc) = yaml_serde::from_str::<yaml_serde::Value>(&yaml)
        && let Some(packages) = doc.get("packages").and_then(|p| p.as_sequence())
    {
        patterns.extend(
            packages
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned)),
        );
    }
    let workspace = globs_spec(patterns.iter().map(String::as_str), []);
    ProjectInfo { name, workspace }
}

fn python(dir: &Path) -> ProjectInfo {
    let Some(pyproject) = read_toml(&dir.join("pyproject.toml")) else {
        return ProjectInfo::default();
    };
    let name = pyproject
        .get("project")
        .and_then(|p| p.get("name"))
        .or_else(|| {
            pyproject
                .get("tool")
                .and_then(|t| t.get("poetry"))
                .and_then(|p| p.get("name"))
        })
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    let workspace = pyproject
        .get("tool")
        .and_then(|t| t.get("uv"))
        .and_then(|uv| uv.get("workspace"))
        .and_then(|ws| globs_spec(str_array(ws.get("members")), str_array(ws.get("exclude"))));
    ProjectInfo { name, workspace }
}

fn go(dir: &Path, names: &[&str]) -> ProjectInfo {
    let name = read(&dir.join("go.mod")).and_then(|gomod| {
        gomod.lines().find_map(|line| {
            let module = line.trim().strip_prefix("module")?.trim();
            let module = module.trim_matches('"');
            module.rsplit('/').next().map(str::to_owned)
        })
    });
    let workspace = if names.contains(&"go.work") {
        read(&dir.join("go.work"))
            .and_then(|work| globs_spec(go_work_uses(&work).iter().map(String::as_str), []))
    } else {
        None
    };
    ProjectInfo { name, workspace }
}

/// Extracts `use` directives from a go.work file, both single-line and block forms.
fn go_work_uses(source: &str) -> Vec<String> {
    let mut uses = Vec::new();
    let mut in_block = false;
    for line in source.lines() {
        let line = line.split("//").next().unwrap_or_default().trim();
        if in_block {
            if line.starts_with(')') {
                in_block = false;
            } else if !line.is_empty() {
                uses.push(line.trim_matches('"').to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("use") {
            let rest = rest.trim();
            if rest.starts_with('(') {
                in_block = true;
            } else if !rest.is_empty() {
                uses.push(rest.trim_matches('"').to_owned());
            }
        }
    }
    uses
}

fn gradle(dir: &Path, names: &[&str]) -> ProjectInfo {
    let settings = ["settings.gradle.kts", "settings.gradle"]
        .into_iter()
        .find(|s| names.contains(s));
    let Some(settings) = settings else {
        return ProjectInfo::default();
    };
    let name = read(&dir.join(settings)).and_then(|source| {
        source.lines().find_map(|line| {
            let rest = line.trim().strip_prefix("rootProject.name")?;
            let value = rest.trim_start().strip_prefix('=')?.trim();
            let value = value.trim_matches(|c| c == '"' || c == '\'');
            (!value.is_empty()).then(|| value.to_owned())
        })
    });
    ProjectInfo {
        name,
        workspace: Some(WorkspaceSpec::Nested),
    }
}

fn dotnet(names: &[&str]) -> ProjectInfo {
    let solution = names
        .iter()
        .find(|n| n.ends_with(".sln") || n.ends_with(".slnx"));
    let project = solution.or_else(|| {
        names
            .iter()
            .find(|n| n.ends_with(".csproj") || n.ends_with(".fsproj") || n.ends_with(".vbproj"))
    });
    ProjectInfo {
        name: project
            .and_then(|n| Path::new(n).file_stem())
            .and_then(|s| s.to_str())
            .map(str::to_owned),
        workspace: solution.map(|_| WorkspaceSpec::Nested),
    }
}

fn dart(dir: &Path) -> ProjectInfo {
    let Some(doc) = read(&dir.join("pubspec.yaml"))
        .and_then(|s| yaml_serde::from_str::<yaml_serde::Value>(&s).ok())
    else {
        return ProjectInfo::default();
    };
    let name = doc.get("name").and_then(|n| n.as_str()).map(str::to_owned);
    let workspace = doc
        .get("workspace")
        .and_then(|w| w.as_sequence())
        .and_then(|members| globs_spec(members.iter().filter_map(|m| m.as_str()), []));
    ProjectInfo { name, workspace }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, contents: &str) {
        fs::write(dir.join(name), contents).unwrap();
    }

    #[test]
    fn cargo_workspace_members_and_excludes() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\", \"tools/cli\"]\nexclude = [\"crates/legacy\"]\n",
        );
        let info = Adapter::Cargo.inspect(dir.path(), &["Cargo.toml"]);
        let ws = info.workspace.expect("workspace");
        assert!(ws.contains("crates/core"));
        assert!(ws.contains("tools/cli"));
        assert!(!ws.contains("crates/legacy"));
        assert!(!ws.contains("crates/core/nested"));
        assert!(info.name.is_none());
    }

    #[test]
    fn cargo_package_name() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        );
        let info = Adapter::Cargo.inspect(dir.path(), &["Cargo.toml"]);
        assert_eq!(info.name.as_deref(), Some("demo"));
        assert!(info.workspace.is_none());
    }

    #[test]
    fn node_workspaces_from_package_json_and_pnpm() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"mono","workspaces":{"packages":["apps/*"]}}"#,
        );
        write(
            dir.path(),
            "pnpm-workspace.yaml",
            "packages:\n  - 'packages/**'\n  - '!packages/private'\n",
        );
        let info = Adapter::Node.inspect(dir.path(), &["package.json", "pnpm-workspace.yaml"]);
        assert_eq!(info.name.as_deref(), Some("mono"));
        let ws = info.workspace.unwrap();
        assert!(ws.contains("apps/web"));
        assert!(ws.contains("packages/ui/button"));
        assert!(!ws.contains("packages/private"));
        assert!(!ws.contains("docs"));
    }

    #[test]
    fn python_names_and_uv_workspace() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "pyproject.toml",
            "[project]\nname = \"svc\"\n[tool.uv.workspace]\nmembers = [\"libs/*\"]\n",
        );
        let info = Adapter::Python.inspect(dir.path(), &["pyproject.toml"]);
        assert_eq!(info.name.as_deref(), Some("svc"));
        assert!(info.workspace.unwrap().contains("libs/core"));
    }

    #[test]
    fn go_module_name_and_work_uses() {
        assert_eq!(
            go_work_uses("go 1.22\n\nuse (\n  ./api // comment\n  \"./cli\"\n)\nuse ./tools\n"),
            vec!["./api", "./cli", "./tools"]
        );
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "go.mod",
            "module github.com/acme/widget\n\ngo 1.22\n",
        );
        let info = Adapter::Go.inspect(dir.path(), &["go.mod"]);
        assert_eq!(info.name.as_deref(), Some("widget"));
    }

    #[test]
    fn gradle_root_project_name() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "settings.gradle.kts",
            "rootProject.name = \"shop\"\ninclude(\":app\")\n",
        );
        let info = Adapter::Gradle.inspect(dir.path(), &["settings.gradle.kts"]);
        assert_eq!(info.name.as_deref(), Some("shop"));
        assert!(matches!(info.workspace, Some(WorkspaceSpec::Nested)));
    }

    #[test]
    fn malformed_manifests_yield_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Cargo.toml", "not = [valid");
        write(dir.path(), "package.json", "{");
        assert!(Adapter::Cargo.inspect(dir.path(), &[]).name.is_none());
        assert!(Adapter::Node.inspect(dir.path(), &[]).name.is_none());
    }
}
