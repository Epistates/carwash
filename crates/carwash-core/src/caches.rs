//! Per-user caches outside any project: package stores, toolchains, IDE and SDK caches.
//!
//! Entries are data (`caches.toml`), resolved against the environment and platform. Each says
//! whether deleting the directory is safe and which command the tool offers to prune it.

use crate::model::Size;
use crate::tasks::Task;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const BUILTIN: &str = include_str!("caches.toml");
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CacheSpec {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub ecosystem: Option<String>,
    pub paths: Vec<String>,
    #[serde(default)]
    pub delete: bool,
    #[serde(default)]
    pub prune: Option<Vec<String>>,
    #[serde(default)]
    pub children: bool,
    #[serde(default)]
    pub child_prune: Option<Vec<String>>,
    #[serde(default)]
    pub keep: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CachesFile {
    schema: u32,
    #[serde(default)]
    cache: Vec<CacheSpec>,
}

fn parse(source: &str) -> Result<Vec<CacheSpec>, String> {
    let file: CachesFile = toml::from_str(source).map_err(|e| e.to_string())?;
    if file.schema != SCHEMA_VERSION {
        return Err(format!("unsupported caches schema {}", file.schema));
    }
    Ok(file.cache)
}

pub fn builtin_specs() -> Vec<CacheSpec> {
    parse(BUILTIN).expect("built-in caches parse")
}

/// Built-ins with user entries applied (same `id` replaces, new ids are added).
pub fn specs_with_overrides(user: &str) -> Result<Vec<CacheSpec>, String> {
    let mut specs = builtin_specs();
    for spec in parse(user)? {
        match specs.iter_mut().find(|s| s.id == spec.id) {
            Some(existing) => *existing = spec,
            None => specs.push(spec),
        }
    }
    Ok(specs)
}

/// A cache present on this machine.
#[derive(Debug, Clone, Serialize)]
pub struct GlobalCache {
    /// `spec-id`, or `spec-id/child` for per-child entries.
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub ecosystem: Option<String>,
    /// Removing the directory is safe.
    pub deletable: bool,
    /// The tool's own cleanup command.
    pub prune: Option<Vec<String>>,
    pub note: Option<String>,
    pub size: Option<Size>,
}

impl GlobalCache {
    /// The prune command as a task, when its program is installed.
    pub fn prune_task(&self) -> Option<Task> {
        let argv = self.prune.as_ref()?;
        let program = argv.first()?;
        find_program(program)?;
        let cwd = crate::paths::home().unwrap_or_else(|| PathBuf::from("/"));
        Some(Task {
            name: format!("prune {}", self.id),
            source: "cache".into(),
            program: program.clone(),
            args: argv[1..].to_vec(),
            description: Some(self.name.clone()),
            cwd,
            standard: true,
        })
    }
}

/// Looks `program` up on `PATH`.
pub fn find_program(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| {
            let plain = dir.join(program);
            let exe = dir.join(format!("{program}.exe"));
            [plain, exe]
        })
        .find(|candidate| candidate.is_file())
}

fn platform_ok(prefix: &str) -> bool {
    match prefix {
        "macos" => cfg!(target_os = "macos"),
        "linux" => cfg!(target_os = "linux"),
        "windows" => cfg!(windows),
        _ => false,
    }
}

/// Resolves one path candidate; `None` when its platform or variable does not apply.
fn resolve(candidate: &str, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let candidate = match candidate.split_once(':') {
        Some((prefix, rest)) if matches!(prefix, "macos" | "linux" | "windows") => {
            if !platform_ok(prefix) {
                return None;
            }
            rest
        }
        _ => candidate,
    };
    if let Some(rest) = candidate.strip_prefix("~/") {
        return Some(home.join(rest));
    }
    if let Some(rest) = candidate.strip_prefix('$') {
        let (var, tail) = rest.split_once('/').unwrap_or((rest, ""));
        let value = env(var).filter(|v| !v.is_empty())?;
        let base = PathBuf::from(value);
        return Some(if tail.is_empty() {
            base
        } else {
            base.join(tail)
        });
    }
    Some(PathBuf::from(candidate))
}

/// The default rustup toolchain, from `settings.toml` next to `toolchains/`.
fn rustup_default(toolchains: &Path) -> Option<String> {
    let settings = std::fs::read_to_string(toolchains.parent()?.join("settings.toml")).ok()?;
    let table: toml::Table = settings.parse().ok()?;
    table
        .get("default_toolchain")
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
}

/// Caches present under `home`, children expanded, in spec order. Sizes are not measured.
pub fn discover(specs: &[CacheSpec], home: &Path) -> Vec<GlobalCache> {
    discover_with(specs, home, &|var| std::env::var(var).ok())
}

fn discover_with(
    specs: &[CacheSpec],
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<GlobalCache> {
    let mut out = Vec::new();
    for spec in specs {
        let Some(path) = spec
            .paths
            .iter()
            .filter_map(|c| resolve(c, home, env))
            .find(|p| p.is_dir())
        else {
            continue;
        };
        if !spec.children {
            out.push(GlobalCache {
                id: spec.id.clone(),
                name: spec.name.clone(),
                path,
                ecosystem: spec.ecosystem.clone(),
                deletable: spec.delete,
                prune: spec.prune.clone(),
                note: spec.note.clone(),
                size: None,
            });
            continue;
        }
        let keep = match spec.keep.as_deref() {
            Some("rustup-default") => rustup_default(&path),
            _ => None,
        };
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        let mut children: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok().map(|n| (n, e.path())))
            .filter(|(name, _)| !name.starts_with('.'))
            .filter(|(name, _)| {
                keep.as_ref()
                    .is_none_or(|k| name != k && !name.starts_with(&format!("{k}-")))
            })
            .collect();
        children.sort();
        for (child, child_path) in children {
            out.push(GlobalCache {
                id: format!("{}/{child}", spec.id),
                name: format!("{}: {child}", spec.name),
                path: child_path,
                ecosystem: spec.ecosystem.clone(),
                deletable: spec.delete,
                prune: spec
                    .child_prune
                    .as_ref()
                    .map(|argv| argv.iter().map(|a| a.replace("{name}", &child)).collect()),
                note: spec.note.clone(),
                size: None,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn builtin_caches_parse_and_are_unique() {
        let specs = builtin_specs();
        assert!(specs.len() >= 30);
        let mut ids: Vec<&str> = specs.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), specs.len());
        for spec in &specs {
            assert!(
                spec.delete || spec.prune.is_some() || spec.child_prune.is_some(),
                "{} has no action",
                spec.id
            );
        }
    }

    #[test]
    fn resolves_home_env_and_platform_candidates() {
        let home = Path::new("/home/me");
        let vars = env(&[("CARGO_HOME", "/opt/cargo")]);
        assert_eq!(
            resolve("~/.npm", home, &vars),
            Some(PathBuf::from("/home/me/.npm"))
        );
        assert_eq!(
            resolve("$CARGO_HOME/registry", home, &vars),
            Some(PathBuf::from("/opt/cargo/registry"))
        );
        assert_eq!(resolve("$UNSET/x", home, &vars), None);
        let other = if cfg!(target_os = "macos") {
            "linux:~/x"
        } else {
            "macos:~/x"
        };
        assert_eq!(resolve(other, home, &vars), None);
    }

    #[test]
    fn discovers_existing_caches_and_expands_children() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path();
        fs::create_dir_all(root.join(".npm/_cacache")).unwrap();
        for toolchain in [
            "stable-aarch64-apple-darwin",
            "nightly-aarch64-apple-darwin",
            "1.80-aarch64-apple-darwin",
        ] {
            fs::create_dir_all(root.join(".rustup/toolchains").join(toolchain)).unwrap();
        }
        fs::write(
            root.join(".rustup/settings.toml"),
            "default_toolchain = \"stable\"\n",
        )
        .unwrap();

        let caches = discover_with(&builtin_specs(), root, &env(&[]));
        let ids: Vec<&str> = caches.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"npm"));
        assert!(ids.contains(&"rustup-toolchains/nightly-aarch64-apple-darwin"));
        assert!(ids.contains(&"rustup-toolchains/1.80-aarch64-apple-darwin"));
        assert!(
            !ids.iter().any(|id| id.contains("stable")),
            "default toolchain kept: {ids:?}"
        );
        let nightly = caches
            .iter()
            .find(|c| c.id.ends_with("nightly-aarch64-apple-darwin"))
            .unwrap();
        assert_eq!(
            nightly.prune.as_deref().unwrap(),
            [
                "rustup",
                "toolchain",
                "uninstall",
                "nightly-aarch64-apple-darwin"
            ]
        );
        assert!(
            !ids.contains(&"cargo-registry"),
            "absent caches are skipped"
        );
    }

    #[test]
    fn user_overrides_replace_and_add() {
        let specs = specs_with_overrides(
            "schema = 1\n[[cache]]\nid = \"npm\"\nname = \"npm (custom)\"\npaths = [\"~/.npm\"]\ndelete = true\n[[cache]]\nid = \"acme\"\nname = \"Acme\"\npaths = [\"~/.acme\"]\ndelete = true\n",
        )
        .unwrap();
        assert_eq!(
            specs.iter().find(|s| s.id == "npm").unwrap().name,
            "npm (custom)"
        );
        assert!(specs.iter().any(|s| s.id == "acme"));
        assert!(specs_with_overrides("schema = 9").is_err());
    }
}
