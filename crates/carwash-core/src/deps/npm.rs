//! JavaScript: package.json ranges, installed versions, the npm registry.

use super::{
    Adapter, Bump, Declared, DepKind, Dependency, Releases, Source, find_upwards, read_capped,
    semver_bump, task,
};
use crate::tasks::{Task, node_package_manager};
use nodejs_semver::{Range, Version};
use std::path::Path;

pub struct Npm;

const SECTIONS: [(&str, DepKind); 3] = [
    ("dependencies", DepKind::Normal),
    ("devDependencies", DepKind::Dev),
    ("optionalDependencies", DepKind::Optional),
];

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&read_capped(path)?).ok()
}

/// (package name, range) for a registry dependency; `None` for workspace, file, git, URL specs.
fn registry_spec(key: &str, spec: &str) -> Option<(String, String)> {
    let spec = spec.trim();
    if let Some(alias) = spec.strip_prefix("npm:") {
        // `npm:real-name@range`; scoped names start with `@`.
        let split = alias[1..].find('@').map(|i| i + 1);
        return Some(match split {
            Some(i) => (alias[..i].to_owned(), alias[i + 1..].to_owned()),
            None => (alias.to_owned(), "*".to_owned()),
        });
    }
    let local = [
        "workspace:",
        "file:",
        "link:",
        "portal:",
        "catalog:",
        "patch:",
        "exec:",
    ];
    let remote = ["git", "http:", "https:", "github:", "gitlab:", "bitbucket:"];
    if local.iter().chain(&remote).any(|p| spec.starts_with(p))
        || spec.starts_with('.')
        || spec.starts_with('/')
        || spec.starts_with('~') && spec.contains('/')
        || (spec.contains('/') && !spec.starts_with('@') && !spec.contains(' '))
    {
        return None;
    }
    Some((
        key.to_owned(),
        if spec.is_empty() {
            "*".into()
        } else {
            spec.to_owned()
        },
    ))
}

/// `dir` relative to the directory holding `lock`, with `/` separators; `None` at the root.
fn relative_to_lock(dir: &Path, lock: &Path) -> Option<String> {
    let root = lock.parent()?;
    dir.strip_prefix(root)
        .ok()
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .filter(|r| !r.is_empty())
}

/// Lockfiles for one project, parsed once and queried per dependency.
struct Installed<'a> {
    dir: &'a Path,
    package_lock: Option<(serde_json::Value, Option<String>)>,
    pnpm_importer: Option<yaml_serde::Value>,
}

impl<'a> Installed<'a> {
    fn load(dir: &'a Path) -> Self {
        let package_lock = find_upwards(dir, "package-lock.json", 6)
            .and_then(|lock| read_json(&lock).map(|json| (json, relative_to_lock(dir, &lock))));
        let pnpm_importer = find_upwards(dir, "pnpm-lock.yaml", 6).and_then(|lock| {
            let doc = read_capped(&lock)
                .and_then(|s| yaml_serde::from_str::<yaml_serde::Value>(&s).ok())?;
            let importer = relative_to_lock(dir, &lock).unwrap_or_else(|| ".".into());
            doc.get("importers")?.get(importer.as_str()).cloned()
        });
        Self {
            dir,
            package_lock,
            pnpm_importer,
        }
    }

    /// Installed version: node_modules (hoisted up to the workspace root), then lockfiles.
    fn version(&self, name: &str) -> Option<String> {
        for ancestor in self.dir.ancestors().take(6) {
            let manifest = ancestor
                .join("node_modules")
                .join(name)
                .join("package.json");
            if let Some(version) = read_json(&manifest)
                .as_ref()
                .and_then(|j| j["version"].as_str())
            {
                return Some(version.to_owned());
            }
        }
        if let Some((json, prefix)) = &self.package_lock {
            let keys = [
                prefix.as_ref().map(|p| format!("{p}/node_modules/{name}")),
                Some(format!("node_modules/{name}")),
            ];
            for key in keys.into_iter().flatten() {
                if let Some(version) = json["packages"][key.as_str()]["version"].as_str() {
                    return Some(version.to_owned());
                }
            }
        }
        if let Some(importer) = &self.pnpm_importer {
            for section in ["dependencies", "devDependencies", "optionalDependencies"] {
                if let Some(version) = importer[section][name]["version"].as_str() {
                    // `1.2.3(react@18.2.0)` carries peer suffixes.
                    let version = version.split('(').next().unwrap_or(version);
                    if Version::parse(version).is_ok() {
                        return Some(version.to_owned());
                    }
                }
            }
        }
        None
    }
}

impl Adapter for Npm {
    fn declared(&self, dir: &Path) -> Result<Vec<Declared>, String> {
        let manifest = dir.join("package.json");
        let json = read_json(&manifest).ok_or_else(|| "cannot read package.json".to_owned())?;
        let installed = Installed::load(dir);
        let mut out: Vec<Declared> = Vec::new();
        for (section, kind) in SECTIONS {
            let Some(deps) = json[section].as_object() else {
                continue;
            };
            for (key, spec) in deps {
                let Some((name, requirement)) = spec.as_str().and_then(|s| registry_spec(key, s))
                else {
                    continue;
                };
                if out.iter().any(|d| d.name == name) {
                    continue;
                }
                out.push(Declared {
                    current: installed.version(key),
                    name,
                    requirement,
                    kind,
                    source: Source::Npm,
                    manifest: manifest.clone(),
                });
            }
        }
        Ok(out)
    }

    fn url(&self, name: &str) -> String {
        format!("https://registry.npmjs.org/{}", name.replace('/', "%2f"))
    }

    fn accept(&self) -> Option<&'static str> {
        // Abbreviated metadata: a fraction of the full document.
        Some("application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8")
    }

    fn parse(&self, body: &str) -> Result<Releases, String> {
        let json: serde_json::Value =
            serde_json::from_str(body).map_err(|e| format!("bad registry response: {e}"))?;
        let versions = json["versions"]
            .as_object()
            .map(|v| v.keys().cloned().collect())
            .unwrap_or_default();
        Ok(Releases {
            versions,
            latest: json["dist-tags"]["latest"].as_str().map(str::to_owned),
        })
    }

    fn resolve(&self, dep: &Declared, releases: &Releases) -> (Option<String>, Option<String>) {
        let versions: Vec<Version> = releases
            .versions
            .iter()
            .filter_map(|v| Version::parse(v).ok())
            .collect();
        let wanted = Range::parse(&dep.requirement)
            .ok()
            .and_then(|range| range.max_satisfying(&versions).map(Version::to_string));
        let latest = releases.latest.clone().or_else(|| {
            versions
                .iter()
                .filter(|v| !v.is_prerelease())
                .max()
                .map(Version::to_string)
        });
        (wanted, latest)
    }

    fn bump(&self, from: &str, to: &str) -> Bump {
        match (Version::parse(from), Version::parse(to)) {
            (Ok(a), Ok(b)) => semver_bump(
                (a.major(), a.minor(), a.patch()),
                (b.major(), b.minor(), b.patch()),
            ),
            _ => Bump::None,
        }
    }

    fn update(&self, dir: &Path, deps: &[&Dependency], latest: bool) -> Result<Vec<Task>, String> {
        let pm = node_package_manager(dir);
        let berry = find_upwards(dir, ".yarnrc.yml", 6).is_some();
        let names = |filter: &dyn Fn(&&&Dependency) -> bool, suffix: &str| -> Vec<String> {
            deps.iter()
                .filter(filter)
                .map(|d| format!("{}{suffix}", d.declared.name))
                .collect()
        };
        let mut tasks = Vec::new();
        if latest {
            for dev in [false, true] {
                let list = names(&|d| (d.declared.kind == DepKind::Dev) == dev, "@latest");
                if list.is_empty() {
                    continue;
                }
                let mut argv = vec![
                    pm.to_owned(),
                    if pm == "npm" { "install" } else { "add" }.to_owned(),
                ];
                if dev {
                    argv.push(if pm == "bun" { "-d" } else { "-D" }.to_owned());
                }
                argv.extend(list.clone());
                tasks.push(task(
                    dir,
                    "upgrade",
                    argv,
                    format!("upgrade {} to latest", list.join(", ")),
                ));
            }
        } else {
            let list = names(&|_| true, "");
            let verb = match (pm, berry) {
                ("yarn", true) => "up",
                ("yarn", false) => "upgrade",
                _ => "update",
            };
            let mut argv = vec![pm.to_owned(), verb.to_owned()];
            argv.extend(list.clone());
            tasks.push(task(
                dir,
                "update",
                argv,
                format!("update {} within their ranges", list.join(", ")),
            ));
        }
        Ok(tasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn write(dir: &Path, name: &str, contents: &str) {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn registry_specs() {
        assert_eq!(
            registry_spec("a", "^1.2.0"),
            Some(("a".into(), "^1.2.0".into()))
        );
        assert_eq!(registry_spec("a", ""), Some(("a".into(), "*".into())));
        assert_eq!(
            registry_spec("alias", "npm:@scope/real@^2"),
            Some(("@scope/real".into(), "^2".into()))
        );
        for local in [
            "workspace:*",
            "file:../x",
            "link:x",
            "git+https://x",
            "github:a/b",
            "user/repo",
            "catalog:",
        ] {
            assert_eq!(registry_spec("a", local), None, "{local}");
        }
        assert!(registry_spec("a", ">=1 <2").is_some());
    }

    #[test]
    fn installed_versions_from_node_modules_and_lockfiles() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "node_modules/react/package.json",
            r#"{"version":"18.2.0"}"#,
        );
        write(
            root,
            "package-lock.json",
            r#"{"packages":{"node_modules/zod":{"version":"3.22.4"},"apps/web/node_modules/next":{"version":"14.1.0"}}}"#,
        );
        write(
            root,
            "pnpm-lock.yaml",
            "importers:\n  apps/web:\n    dependencies:\n      swr:\n        version: 2.2.0(react@18.2.0)\n",
        );
        let web = root.join("apps/web");
        fs::create_dir_all(&web).unwrap();
        let installed = Installed::load(&web);
        assert_eq!(
            installed.version("react").as_deref(),
            Some("18.2.0"),
            "hoisted"
        );
        assert_eq!(installed.version("next").as_deref(), Some("14.1.0"));
        assert_eq!(installed.version("zod").as_deref(), Some("3.22.4"));
        assert_eq!(installed.version("swr").as_deref(), Some("2.2.0"));
        assert_eq!(installed.version("missing"), None);
    }

    #[test]
    fn resolves_ranges() {
        let releases = Npm
            .parse(r#"{"dist-tags":{"latest":"5.1.0"},"versions":{"4.1.0":{},"4.9.2":{},"5.0.0":{},"5.1.0":{},"6.0.0-rc.1":{}}}"#)
            .unwrap();
        let dep = |req: &str| Declared {
            name: "x".into(),
            requirement: req.into(),
            current: Some("4.1.0".into()),
            kind: DepKind::Normal,
            source: Source::Npm,
            manifest: PathBuf::new(),
        };
        assert_eq!(
            Npm.resolve(&dep("^4.1.0"), &releases),
            (Some("4.9.2".into()), Some("5.1.0".into()))
        );
        assert_eq!(
            Npm.resolve(&dep("~4.1.0"), &releases).0.as_deref(),
            Some("4.1.0")
        );
        assert_eq!(
            Npm.resolve(&dep(">=4 <6 || 7"), &releases).0.as_deref(),
            Some("5.1.0")
        );
        assert_eq!(Npm.bump("4.1.0", "5.1.0"), Bump::Major);
    }

    #[test]
    fn update_commands_match_the_package_manager() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "pnpm-lock.yaml", "");
        let dep = |name: &str, kind| Dependency {
            declared: Declared {
                name: name.into(),
                requirement: "^1".into(),
                current: Some("1.0.0".into()),
                kind,
                source: Source::Npm,
                manifest: PathBuf::new(),
            },
            wanted: Some("1.1.0".into()),
            latest: Some("2.0.0".into()),
            bump: Bump::Major,
            wanted_bump: Bump::Minor,
            vulnerabilities: Vec::new(),
            error: None,
        };
        let a = dep("a", DepKind::Normal);
        let b = dep("b", DepKind::Dev);
        let compatible = Npm.update(dir.path(), &[&a, &b], false).unwrap();
        assert_eq!(compatible[0].command_line(), "pnpm update a b");
        let latest = Npm.update(dir.path(), &[&a, &b], true).unwrap();
        let lines: Vec<String> = latest.iter().map(Task::command_line).collect();
        assert_eq!(lines, vec!["pnpm add a@latest", "pnpm add -D b@latest"]);
    }
}
