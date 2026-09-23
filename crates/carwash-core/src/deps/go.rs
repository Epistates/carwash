//! Go: direct requirements in go.mod, latest versions from the module proxy.

use super::{
    Adapter, Bump, Declared, DepKind, Dependency, Releases, Source, read_capped, semver_bump, task,
};
use crate::tasks::Task;
use semver::Version;
use std::path::Path;

pub struct Go;

/// Direct, non-replaced requirements of a go.mod: (module path, version).
pub(crate) fn requirements(gomod: &str) -> Vec<(String, String)> {
    let mut replaced = Vec::new();
    let mut required = Vec::new();
    let mut block: Option<&str> = None;
    for raw in gomod.lines() {
        let indirect = raw.contains("// indirect");
        let line = raw.split("//").next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(open) = block {
            if line == ")" {
                block = None;
                continue;
            }
            match open {
                "require" if !indirect => {
                    let mut parts = line.split_whitespace();
                    if let (Some(module), Some(version)) = (parts.next(), parts.next()) {
                        required.push((module.to_owned(), version.to_owned()));
                    }
                }
                "replace" => {
                    if let Some(module) = line.split_whitespace().next() {
                        replaced.push(module.to_owned());
                    }
                }
                _ => {}
            }
            continue;
        }
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next(), parts.next()) {
            (Some(keyword @ ("require" | "replace")), Some("("), _) => block = Some(keyword),
            (Some("require"), Some(module), Some(version)) if !indirect => {
                required.push((module.to_owned(), version.to_owned()));
            }
            (Some("replace"), Some(module), _) => replaced.push(module.to_owned()),
            _ => {}
        }
    }
    required.retain(|(module, _)| !replaced.contains(module));
    required
}

/// Module path escaping for the proxy protocol: uppercase letters become `!` + lowercase.
pub(crate) fn escape(module: &str) -> String {
    let mut out = String::with_capacity(module.len() + 4);
    for c in module.chars() {
        if c.is_ascii_uppercase() {
            out.push('!');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn parse(version: &str) -> Option<Version> {
    Version::parse(
        version
            .trim_start_matches('v')
            .split("+incompatible")
            .next()?,
    )
    .ok()
}

impl Adapter for Go {
    fn declared(&self, dir: &Path) -> Result<Vec<Declared>, String> {
        let manifest = dir.join("go.mod");
        let Some(gomod) = read_capped(&manifest) else {
            // go.work-only directories have nothing of their own to update.
            return Ok(Vec::new());
        };
        Ok(requirements(&gomod)
            .into_iter()
            .map(|(module, version)| Declared {
                name: module,
                requirement: version.clone(),
                current: Some(version),
                kind: DepKind::Normal,
                source: Source::Go,
                manifest: manifest.clone(),
            })
            .collect())
    }

    fn url(&self, name: &str) -> String {
        format!("https://proxy.golang.org/{}/@latest", escape(name))
    }

    fn parse(&self, body: &str) -> Result<Releases, String> {
        let json: serde_json::Value =
            serde_json::from_str(body).map_err(|e| format!("bad proxy response: {e}"))?;
        let latest = json["Version"].as_str().map(str::to_owned);
        Ok(Releases {
            versions: latest.clone().into_iter().collect(),
            latest,
        })
    }

    fn resolve(&self, dep: &Declared, releases: &Releases) -> (Option<String>, Option<String>) {
        // Module paths carry the major version, so the proxy's latest is always compatible.
        let latest = releases.latest.clone().filter(|latest| {
            match (dep.current.as_deref().and_then(parse), parse(latest)) {
                (Some(current), Some(latest)) => latest >= current,
                _ => true,
            }
        });
        (latest.clone(), latest)
    }

    fn bump(&self, from: &str, to: &str) -> Bump {
        match (parse(from), parse(to)) {
            (Some(a), Some(b)) => {
                semver_bump((a.major, a.minor, a.patch), (b.major, b.minor, b.patch))
            }
            _ => Bump::None,
        }
    }

    fn update(&self, dir: &Path, deps: &[&Dependency], _latest: bool) -> Result<Vec<Task>, String> {
        let mut argv = vec!["go".to_owned(), "get".to_owned()];
        argv.extend(deps.iter().map(|d| format!("{}@latest", d.declared.name)));
        let names: Vec<&str> = deps.iter().map(|d| d.declared.name.as_str()).collect();
        Ok(vec![task(
            dir,
            "update",
            argv,
            format!("update {}", names.join(", ")),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requirements_and_skips_indirect_and_replaced() {
        let gomod = "module example.com/app\n\ngo 1.22\n\nrequire github.com/single/dep v1.0.0\n\nrequire (\n\tgithub.com/spf13/cobra v1.8.0\n\tgolang.org/x/sys v0.15.0 // indirect\n\tgithub.com/Masterminds/semver/v3 v3.2.1\n\tgithub.com/local/fork v0.1.0\n)\n\nreplace github.com/local/fork => ../fork\n";
        assert_eq!(
            requirements(gomod),
            vec![
                ("github.com/single/dep".to_string(), "v1.0.0".to_string()),
                ("github.com/spf13/cobra".to_string(), "v1.8.0".to_string()),
                (
                    "github.com/Masterminds/semver/v3".to_string(),
                    "v3.2.1".to_string()
                ),
            ]
        );
    }

    #[test]
    fn escapes_uppercase_for_the_proxy() {
        assert_eq!(
            Go.url("github.com/Masterminds/semver/v3"),
            "https://proxy.golang.org/github.com/!masterminds/semver/v3/@latest"
        );
    }

    #[test]
    fn bumps_and_pseudo_versions() {
        assert_eq!(Go.bump("v1.8.0", "v1.9.1"), Bump::Minor);
        assert_eq!(Go.bump("v0.15.0", "v0.16.0"), Bump::Major);
        assert_eq!(
            Go.bump("v2.0.0+incompatible", "v2.0.1+incompatible"),
            Bump::Patch
        );
    }
}
