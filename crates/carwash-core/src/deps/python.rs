//! Python: pyproject/requirements specifiers, uv/poetry/pdm locks, PyPI.

use super::{
    Adapter, Bump, Declared, DepKind, Dependency, Releases, Source, find_upwards, read_capped,
    semver_bump, task,
};
use crate::tasks::Task;
use pep440_rs::{Version, VersionSpecifiers};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

pub struct Python;

/// PEP 503 normalization: lowercase, runs of `-_.` become `-`.
pub(crate) fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut separator = false;
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            separator = true;
        } else {
            if separator && !out.is_empty() {
                out.push('-');
            }
            separator = false;
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// Splits a PEP 508 requirement into (name, specifier); `None` for URL requirements.
pub(crate) fn split_requirement(line: &str) -> Option<(String, String)> {
    let line = line.split(';').next()?.split('#').next()?.trim();
    if line.is_empty() || line.starts_with('-') || line.contains(" @ ") || line.contains("://") {
        return None;
    }
    let end = line
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .unwrap_or(line.len());
    let name = &line[..end];
    if name.is_empty() {
        return None;
    }
    let mut rest = line[end..].trim();
    if let Some(stripped) = rest.strip_prefix('[') {
        rest = stripped.split_once(']').map_or("", |(_, r)| r).trim();
    }
    let spec = rest.trim_start_matches('(').trim_end_matches(')').trim();
    Some((name.to_owned(), spec.to_owned()))
}

/// Locked versions by normalized name, from uv.lock, poetry.lock or pdm.lock.
fn locked(dir: &Path) -> HashMap<String, String> {
    let mut versions = HashMap::new();
    for lock in ["uv.lock", "poetry.lock", "pdm.lock"] {
        let Some(table) = find_upwards(dir, lock, 6)
            .and_then(|p| read_capped(&p))
            .and_then(|s| s.parse::<toml::Table>().ok())
        else {
            continue;
        };
        for package in table
            .get("package")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let (Some(name), Some(version)) = (
                package.get("name").and_then(toml::Value::as_str),
                package.get("version").and_then(toml::Value::as_str),
            ) {
                versions.insert(normalize(name), version.to_owned());
            }
        }
        if !versions.is_empty() {
            break;
        }
    }
    versions
}

fn tool(dir: &Path) -> Option<&'static str> {
    [
        ("uv.lock", "uv"),
        ("poetry.lock", "poetry"),
        ("pdm.lock", "pdm"),
    ]
    .into_iter()
    .find(|(lock, _)| find_upwards(dir, lock, 6).is_some())
    .map(|(_, tool)| tool)
}

impl Adapter for Python {
    fn declared(&self, dir: &Path) -> Result<Vec<Declared>, String> {
        let locked = locked(dir);
        let mut requirements: Vec<(String, DepKind, std::path::PathBuf)> = Vec::new();
        let pyproject = dir.join("pyproject.toml");
        if let Some(table) = read_capped(&pyproject).and_then(|s| s.parse::<toml::Table>().ok()) {
            let strings = |value: Option<&toml::Value>| -> Vec<String> {
                value
                    .and_then(toml::Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let project = table.get("project");
            for line in strings(project.and_then(|p| p.get("dependencies"))) {
                requirements.push((line, DepKind::Normal, pyproject.clone()));
            }
            if let Some(extras) = project
                .and_then(|p| p.get("optional-dependencies"))
                .and_then(toml::Value::as_table)
            {
                for lines in extras.values() {
                    for line in strings(Some(lines)) {
                        requirements.push((line, DepKind::Optional, pyproject.clone()));
                    }
                }
            }
            if let Some(groups) = table
                .get("dependency-groups")
                .and_then(toml::Value::as_table)
            {
                for lines in groups.values() {
                    for line in strings(Some(lines)) {
                        requirements.push((line, DepKind::Dev, pyproject.clone()));
                    }
                }
            }
            let uv_dev = table
                .get("tool")
                .and_then(|t| t.get("uv"))
                .and_then(|u| u.get("dev-dependencies"));
            for line in strings(uv_dev) {
                requirements.push((line, DepKind::Dev, pyproject.clone()));
            }
        }
        if requirements.is_empty() {
            for file in [
                "requirements.txt",
                "requirements-dev.txt",
                "requirements/base.txt",
            ] {
                let path = dir.join(file);
                if let Some(text) = read_capped(&path) {
                    let kind = if file.contains("dev") {
                        DepKind::Dev
                    } else {
                        DepKind::Normal
                    };
                    for line in text.lines() {
                        requirements.push((line.to_owned(), kind, path.clone()));
                    }
                }
            }
        }
        let mut out: Vec<Declared> = Vec::new();
        for (line, kind, manifest) in requirements {
            let Some((name, spec)) = split_requirement(&line) else {
                continue;
            };
            let normalized = normalize(&name);
            if out.iter().any(|d| d.name == normalized) {
                continue;
            }
            // A pinned requirement is its own current version when nothing is locked.
            let pinned = spec
                .strip_prefix("==")
                .filter(|v| !v.contains([',', '*']))
                .map(|v| v.trim().to_owned());
            out.push(Declared {
                current: locked.get(&normalized).cloned().or(pinned),
                name: normalized,
                requirement: spec,
                kind,
                source: Source::Pypi,
                manifest,
            });
        }
        Ok(out)
    }

    fn url(&self, name: &str) -> String {
        format!("https://pypi.org/pypi/{name}/json")
    }

    fn parse(&self, body: &str) -> Result<Releases, String> {
        let json: serde_json::Value =
            serde_json::from_str(body).map_err(|e| format!("bad PyPI response: {e}"))?;
        let versions = json["releases"]
            .as_object()
            .map(|releases| {
                releases
                    .iter()
                    .filter(|(_, files)| {
                        files
                            .as_array()
                            .is_some_and(|f| !f.is_empty() && f.iter().any(|x| x["yanked"] != true))
                    })
                    .map(|(v, _)| v.clone())
                    .collect()
            })
            .unwrap_or_default();
        Ok(Releases {
            versions,
            latest: json["info"]["version"].as_str().map(str::to_owned),
        })
    }

    fn resolve(&self, dep: &Declared, releases: &Releases) -> (Option<String>, Option<String>) {
        let specifiers = if dep.requirement.is_empty() {
            VersionSpecifiers::from_str("").ok()
        } else {
            VersionSpecifiers::from_str(&dep.requirement).ok()
        };
        let allow_pre = dep.requirement.contains(|c: char| c.is_ascii_alphabetic());
        let mut versions: Vec<Version> = releases
            .versions
            .iter()
            .filter_map(|v| Version::from_str(v).ok())
            .filter(|v| allow_pre || !v.any_prerelease())
            .collect();
        versions.sort();
        let wanted = specifiers.and_then(|spec| {
            versions
                .iter()
                .rev()
                .find(|v| spec.contains(v))
                .map(ToString::to_string)
        });
        let latest = releases
            .latest
            .clone()
            .or_else(|| versions.last().map(ToString::to_string));
        (wanted, latest)
    }

    fn bump(&self, from: &str, to: &str) -> Bump {
        match (Version::from_str(from), Version::from_str(to)) {
            (Ok(a), Ok(b)) if b > a => {
                let part = |v: &Version, i: usize| v.release().get(i).copied().unwrap_or(0);
                let bump = semver_bump(
                    (part(&a, 0), part(&a, 1), part(&a, 2)),
                    (part(&b, 0), part(&b, 1), part(&b, 2)),
                );
                // Post-releases and dev builds change nothing in the first three parts.
                if bump == Bump::None {
                    Bump::Patch
                } else {
                    bump
                }
            }
            _ => Bump::None,
        }
    }

    fn update(&self, dir: &Path, deps: &[&Dependency], latest: bool) -> Result<Vec<Task>, String> {
        let tool = tool(dir).ok_or_else(|| {
            "no uv, poetry or pdm lockfile: edit the requirements file to update".to_owned()
        })?;
        let names: Vec<String> = deps.iter().map(|d| d.declared.name.clone()).collect();
        let argv: Vec<String> = match (tool, latest) {
            ("uv", false) => ["uv", "lock"]
                .into_iter()
                .map(str::to_owned)
                .chain(
                    names
                        .iter()
                        .flat_map(|n| ["--upgrade-package".to_owned(), n.clone()]),
                )
                .collect(),
            ("uv", true) => std::iter::once("uv".to_owned())
                .chain(std::iter::once("add".to_owned()))
                .chain(deps.iter().filter_map(|d| {
                    d.latest
                        .as_ref()
                        .map(|v| format!("{}>={v}", d.declared.name))
                }))
                .collect(),
            ("poetry", false) => std::iter::once("poetry".to_owned())
                .chain(std::iter::once("update".to_owned()))
                .chain(names.iter().cloned())
                .collect(),
            ("poetry", true) => std::iter::once("poetry".to_owned())
                .chain(std::iter::once("add".to_owned()))
                .chain(names.iter().map(|n| format!("{n}@latest")))
                .collect(),
            ("pdm", false) => std::iter::once("pdm".to_owned())
                .chain(std::iter::once("update".to_owned()))
                .chain(names.iter().cloned())
                .collect(),
            (_, _) => std::iter::once("pdm".to_owned())
                .chain(["update".to_owned(), "--unconstrained".to_owned()])
                .chain(names.iter().cloned())
                .collect(),
        };
        let what = if latest { "upgrade" } else { "update" };
        Ok(vec![task(
            dir,
            what,
            argv,
            format!("{what} {}", names.join(", ")),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn normalizes_names() {
        assert_eq!(normalize("Django_REST.framework"), "django-rest-framework");
        assert_eq!(normalize("ruamel.yaml"), "ruamel-yaml");
    }

    #[test]
    fn splits_requirements() {
        assert_eq!(
            split_requirement("requests>=2.31"),
            Some(("requests".into(), ">=2.31".into()))
        );
        assert_eq!(
            split_requirement("uvicorn[standard] (>=0.20,<1) ; python_version >= '3.9'"),
            Some(("uvicorn".into(), ">=0.20,<1".into()))
        );
        assert_eq!(
            split_requirement("rich"),
            Some(("rich".into(), String::new()))
        );
        assert_eq!(split_requirement("pkg @ https://x/y.whl"), None);
        assert_eq!(split_requirement("-r base.txt"), None);
        assert_eq!(split_requirement("# comment"), None);
    }

    #[test]
    fn declared_from_pyproject_with_uv_lock() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"svc\"\ndependencies = [\"FastAPI>=0.100\", \"httpx\"]\n[dependency-groups]\ndev = [\"pytest>=8\"]\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("uv.lock"),
            "version = 1\n[[package]]\nname = \"fastapi\"\nversion = \"0.110.0\"\n[[package]]\nname = \"pytest\"\nversion = \"8.1.1\"\n",
        )
        .unwrap();
        let deps = Python.declared(dir.path()).unwrap();
        let names: Vec<(&str, Option<&str>, DepKind)> = deps
            .iter()
            .map(|d| (d.name.as_str(), d.current.as_deref(), d.kind))
            .collect();
        assert_eq!(
            names,
            vec![
                ("fastapi", Some("0.110.0"), DepKind::Normal),
                ("httpx", None, DepKind::Normal),
                ("pytest", Some("8.1.1"), DepKind::Dev),
            ]
        );
        let task = Python.update(dir.path(), &[], false).unwrap();
        assert_eq!(task[0].program, "uv");
    }

    #[test]
    fn requirements_pins_are_current_versions() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("requirements.txt"),
            "flask==2.3.2\nclick>=8\n",
        )
        .unwrap();
        let deps = Python.declared(dir.path()).unwrap();
        assert_eq!(deps[0].current.as_deref(), Some("2.3.2"));
        assert_eq!(deps[1].current, None);
        assert!(
            Python.update(dir.path(), &[], false).is_err(),
            "no lock tool to run"
        );
    }

    #[test]
    fn resolves_specifiers_and_skips_prereleases() {
        let body = r#"{"info":{"version":"2.2.0"},"releases":{
            "1.9.0":[{"yanked":false}],"2.0.0":[{"yanked":false}],"2.1.0":[{"yanked":true}],
            "2.2.0":[{"yanked":false}],"3.0.0a1":[{"yanked":false}],"0.1":[]}}"#;
        let releases = Python.parse(body).unwrap();
        assert!(!releases.versions.contains(&"2.1.0".to_string()), "yanked");
        assert!(!releases.versions.contains(&"0.1".to_string()), "no files");
        let dep = Declared {
            name: "x".into(),
            requirement: ">=1.9,<2.1".into(),
            current: Some("1.9.0".into()),
            kind: DepKind::Normal,
            source: Source::Pypi,
            manifest: PathBuf::new(),
        };
        assert_eq!(
            Python.resolve(&dep, &releases),
            (Some("2.0.0".into()), Some("2.2.0".into()))
        );
        assert_eq!(Python.bump("1.9.0", "2.2.0"), Bump::Major);
        assert_eq!(Python.bump("2.2.0", "2.2.0.post1"), Bump::Patch);
    }
}
