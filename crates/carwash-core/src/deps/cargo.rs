//! Rust: Cargo.toml requirements, Cargo.lock versions, the crates.io sparse index.

use super::{
    Adapter, Bump, Declared, DepKind, Dependency, Releases, Source, find_upwards, read_capped,
    semver_bump, task,
};
use crate::tasks::Task;
use semver::{Version, VersionReq};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Cargo;

const TABLES: [(&str, DepKind); 3] = [
    ("dependencies", DepKind::Normal),
    ("dev-dependencies", DepKind::Dev),
    ("build-dependencies", DepKind::Build),
];

fn read_toml(path: &Path) -> Option<toml::Table> {
    read_capped(path)?.parse().ok()
}

/// The workspace root manifest above `dir` (or `dir` itself), if any.
fn workspace_root(dir: &Path) -> Option<(PathBuf, toml::Table)> {
    dir.ancestors().take(8).find_map(|d| {
        let manifest = d.join("Cargo.toml");
        let table = read_toml(&manifest)?;
        table.contains_key("workspace").then_some((manifest, table))
    })
}

/// Crates.io versions per package name from the nearest Cargo.lock.
fn locked_versions(dir: &Path) -> HashMap<String, Vec<Version>> {
    let mut versions: HashMap<String, Vec<Version>> = HashMap::new();
    let Some(lock) = find_upwards(dir, "Cargo.lock", 8).and_then(|p| read_toml(&p)) else {
        return versions;
    };
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default();
    for package in packages {
        let from_crates_io = package
            .get("source")
            .and_then(toml::Value::as_str)
            .is_some_and(|s| {
                s.contains("crates.io-index") || s.starts_with("sparse+https://index.crates.io")
            });
        if !from_crates_io {
            continue;
        }
        if let (Some(name), Some(Ok(version))) = (
            package.get("name").and_then(toml::Value::as_str),
            package
                .get("version")
                .and_then(toml::Value::as_str)
                .map(Version::parse),
        ) {
            versions.entry(name.to_owned()).or_default().push(version);
        }
    }
    versions
}

/// (crate name, requirement) for a dependency entry on crates.io; `None` for path/git/other.
fn entry(
    key: &str,
    value: &toml::Value,
    workspace_deps: Option<&toml::Table>,
) -> Option<(String, String)> {
    match value {
        toml::Value::String(req) => Some((key.to_owned(), req.clone())),
        toml::Value::Table(table) => {
            if table.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                let inherited = workspace_deps?.get(key)?;
                return entry(key, inherited, None);
            }
            if table.contains_key("path")
                || table.contains_key("git")
                || table.contains_key("registry")
            {
                return None;
            }
            let name = table
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(key);
            let req = table.get("version").and_then(toml::Value::as_str)?;
            Some((name.to_owned(), req.to_owned()))
        }
        _ => None,
    }
}

impl Adapter for Cargo {
    fn declared(&self, dir: &Path) -> Result<Vec<Declared>, String> {
        let manifest = dir.join("Cargo.toml");
        let table = read_toml(&manifest).ok_or_else(|| "cannot read Cargo.toml".to_owned())?;
        let root = workspace_root(dir);
        let workspace_deps = root
            .as_ref()
            .and_then(|(_, t)| t.get("workspace"))
            .and_then(|w| w.get("dependencies"))
            .and_then(toml::Value::as_table);
        let locked = locked_versions(dir);

        let mut sections: Vec<(&toml::Table, DepKind)> = Vec::new();
        for (key, kind) in TABLES {
            if let Some(t) = table.get(key).and_then(toml::Value::as_table) {
                sections.push((t, kind));
            }
        }
        if let Some(targets) = table.get("target").and_then(toml::Value::as_table) {
            for target in targets.values() {
                for (key, kind) in TABLES {
                    if let Some(t) = target.get(key).and_then(toml::Value::as_table) {
                        sections.push((t, kind));
                    }
                }
            }
        }
        // The workspace root owns `[workspace.dependencies]`.
        let is_root = root.as_ref().is_some_and(|(m, _)| m == &manifest);
        let root_deps = if is_root { workspace_deps } else { None };

        let mut out: Vec<Declared> = Vec::new();
        let mut push = |key: &str, value: &toml::Value, kind: DepKind, own: bool| {
            let Some((name, requirement)) = entry(key, value, workspace_deps) else {
                return;
            };
            if out.iter().any(|d| d.name == name) {
                return;
            }
            let inherited = !own
                || value
                    .get("workspace")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false);
            let edit = if inherited {
                root.as_ref().map_or(manifest.clone(), |(m, _)| m.clone())
            } else {
                manifest.clone()
            };
            // Only a locked version satisfying the requirement is this dependency's: another
            // one belongs to some other crate in the graph (the dependency may be unused).
            let current = VersionReq::parse(&requirement).ok().and_then(|req| {
                locked
                    .get(&name)?
                    .iter()
                    .filter(|v| req.matches(v))
                    .max()
                    .map(Version::to_string)
            });
            out.push(Declared {
                name,
                requirement,
                current,
                kind,
                source: Source::Crates,
                manifest: edit,
            });
        };
        for (section, kind) in sections {
            for (key, value) in section {
                push(key, value, kind, true);
            }
        }
        if let Some(deps) = root_deps {
            for (key, value) in deps {
                push(key, value, DepKind::Normal, false);
            }
        }
        Ok(out)
    }

    fn url(&self, name: &str) -> String {
        let name = name.to_ascii_lowercase();
        let prefix = match name.len() {
            1 => "1".to_owned(),
            2 => "2".to_owned(),
            3 => format!("3/{}", &name[..1]),
            _ => format!("{}/{}", &name[..2], &name[2..4]),
        };
        format!("https://index.crates.io/{prefix}/{name}")
    }

    fn parse(&self, body: &str) -> Result<Releases, String> {
        let mut versions = Vec::new();
        for line in body.lines().filter(|l| !l.trim().is_empty()) {
            let record: serde_json::Value =
                serde_json::from_str(line).map_err(|e| format!("bad index entry: {e}"))?;
            if record["yanked"].as_bool() == Some(true) {
                continue;
            }
            if let Some(version) = record["vers"].as_str() {
                versions.push(version.to_owned());
            }
        }
        Ok(Releases {
            versions,
            latest: None,
        })
    }

    fn resolve(&self, dep: &Declared, releases: &Releases) -> (Option<String>, Option<String>) {
        let mut versions: Vec<Version> = releases
            .versions
            .iter()
            .filter_map(|v| Version::parse(v).ok())
            .collect();
        versions.sort();
        let current_is_pre = dep
            .current
            .as_deref()
            .and_then(|c| Version::parse(c).ok())
            .is_some_and(|c| !c.pre.is_empty());
        let wanted = VersionReq::parse(&dep.requirement).ok().and_then(|req| {
            versions
                .iter()
                .rev()
                .find(|v| req.matches(v))
                .map(Version::to_string)
        });
        let latest = versions
            .iter()
            .rev()
            .find(|v| v.pre.is_empty() || current_is_pre)
            .map(Version::to_string);
        (wanted, latest)
    }

    fn bump(&self, from: &str, to: &str) -> Bump {
        match (Version::parse(from), Version::parse(to)) {
            (Ok(a), Ok(b)) => semver_bump((a.major, a.minor, a.patch), (b.major, b.minor, b.patch)),
            _ => Bump::None,
        }
    }

    fn update(&self, dir: &Path, deps: &[&Dependency], latest: bool) -> Result<Vec<Task>, String> {
        if latest {
            for dep in deps {
                let Some(version) = &dep.latest else { continue };
                raise_requirement(&dep.declared.manifest, &dep.declared.name, version)?;
            }
        }
        let mut argv = vec!["cargo".to_owned(), "update".to_owned()];
        for dep in deps {
            argv.push("-p".into());
            argv.push(dep.declared.name.clone());
        }
        let names: Vec<&str> = deps.iter().map(|d| d.declared.name.as_str()).collect();
        let description = if latest {
            format!("raise requirements and update {}", names.join(", "))
        } else {
            format!("update {} within their requirements", names.join(", "))
        };
        Ok(vec![task(dir, "update", argv, description)])
    }
}

/// Rewrites the requirement of `name` in `manifest` to `version`, keeping formatting.
fn raise_requirement(manifest: &Path, name: &str, version: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(manifest).map_err(|e| e.to_string())?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    let mut changed = false;
    let mut edit = |table: &mut toml_edit::Table| {
        for (key, item) in table.iter_mut() {
            let matches_name = key.get() == name
                || item
                    .get("package")
                    .and_then(|p| p.as_str())
                    .is_some_and(|p| p == name);
            if !matches_name {
                continue;
            }
            if item.is_str() {
                *item = toml_edit::value(version);
                changed = true;
            } else if let Some(inline) = item.as_inline_table_mut() {
                if inline.contains_key("version") {
                    inline.insert("version", version.into());
                    changed = true;
                }
            } else if let Some(t) = item.as_table_mut()
                && t.contains_key("version")
            {
                t.insert("version", toml_edit::value(version));
                changed = true;
            }
        }
    };
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(t) = doc.get_mut(key).and_then(|i| i.as_table_mut()) {
            edit(t);
        }
    }
    if let Some(t) = doc
        .get_mut("workspace")
        .and_then(|w| w.get_mut("dependencies"))
        .and_then(|i| i.as_table_mut())
    {
        edit(t);
    }
    if let Some(targets) = doc.get_mut("target").and_then(|i| i.as_table_mut()) {
        for (_, target) in targets.iter_mut() {
            for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(t) = target.get_mut(key).and_then(|i| i.as_table_mut()) {
                    edit(t);
                }
            }
        }
    }
    if !changed {
        return Err(format!("`{name}` not found in {}", manifest.display()));
    }
    std::fs::write(manifest, doc.to_string()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, name: &str, contents: &str) {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    const LOCK: &str = r#"
version = 4
[[package]]
name = "serde"
version = "1.0.200"
source = "registry+https://github.com/rust-lang/crates.io-index"
[[package]]
name = "rand"
version = "0.7.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
[[package]]
name = "rand"
version = "0.8.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
[[package]]
name = "local"
version = "0.1.0"
"#;

    #[test]
    fn declared_handles_renames_workspaces_targets_and_skips_local() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"app\"]\n[workspace.dependencies]\nserde = \"1\"\n",
        );
        write(dir.path(), "Cargo.lock", LOCK);
        write(
            dir.path(),
            "app/Cargo.toml",
            r#"
[package]
name = "app"
[dependencies]
serde = { workspace = true }
random = { package = "rand", version = "0.8" }
local = { path = "../local" }
[target.'cfg(unix)'.dev-dependencies]
libc = "0.2"
"#,
        );
        let deps = Cargo.declared(&dir.path().join("app")).unwrap();
        let by_name = |n: &str| deps.iter().find(|d| d.name == n).unwrap();
        assert_eq!(deps.len(), 3, "{deps:?}");
        assert_eq!(by_name("serde").current.as_deref(), Some("1.0.200"));
        assert_eq!(by_name("serde").manifest, dir.path().join("Cargo.toml"));
        assert_eq!(
            by_name("rand").current.as_deref(),
            Some("0.8.5"),
            "picks the version matching 0.8"
        );
        assert_eq!(by_name("libc").kind, DepKind::Dev);
        assert!(by_name("libc").current.is_none());
    }

    #[test]
    fn sparse_index_paths() {
        assert_eq!(Cargo.url("a"), "https://index.crates.io/1/a");
        assert_eq!(Cargo.url("ab"), "https://index.crates.io/2/ab");
        assert_eq!(Cargo.url("abc"), "https://index.crates.io/3/a/abc");
        assert_eq!(Cargo.url("Serde"), "https://index.crates.io/se/rd/serde");
    }

    #[test]
    fn resolves_wanted_and_latest_skipping_yanked_and_prereleases() {
        let body = [
            r#"{"name":"x","vers":"1.0.0","yanked":false}"#,
            r#"{"name":"x","vers":"1.4.0","yanked":false}"#,
            r#"{"name":"x","vers":"1.5.0","yanked":true}"#,
            r#"{"name":"x","vers":"2.1.0","yanked":false}"#,
            r#"{"name":"x","vers":"3.0.0-beta.1","yanked":false}"#,
        ]
        .join("\n");
        let releases = Cargo.parse(&body).unwrap();
        let dep = Declared {
            name: "x".into(),
            requirement: "1.0".into(),
            current: Some("1.0.0".into()),
            kind: DepKind::Normal,
            source: Source::Crates,
            manifest: PathBuf::new(),
        };
        assert_eq!(
            Cargo.resolve(&dep, &releases),
            (Some("1.4.0".into()), Some("2.1.0".into()))
        );
        assert_eq!(Cargo.bump("1.0.0", "2.1.0"), Bump::Major);
    }

    #[test]
    fn raising_requirements_preserves_formatting() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("Cargo.toml");
        fs::write(
            &manifest,
            "[package]\nname = \"a\" # keep\n\n[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nclap = \"3\"\n",
        )
        .unwrap();
        raise_requirement(&manifest, "clap", "4.6.0").unwrap();
        raise_requirement(&manifest, "serde", "2.0.0").unwrap();
        let text = fs::read_to_string(&manifest).unwrap();
        assert!(text.contains("name = \"a\" # keep"));
        assert!(text.contains("clap = \"4.6.0\""));
        assert!(
            text.contains("version = \"2.0.0\", features = [\"derive\"]"),
            "{text}"
        );
        assert!(raise_requirement(&manifest, "missing", "1").is_err());
    }
}
