//! OS mode: the operating system's own space hogs, outside any project and beyond the
//! developer caches in [`crate::caches`]: app and browser caches, logs, crash reports,
//! trash, temporary files, package-manager caches.
//!
//! Areas are data (`system.toml`), resolved against the platform and environment. Each is
//! cleaned in the way that is safe for it: sweeping its directories ([`crate::sweep`]),
//! running the operating system's own tool, or only reporting its size and how to clean
//! it by hand when carwash should not touch it at all.

use crate::Cancel;
use crate::caches::{find_program, resolve};
use crate::sweep::{self, SweepOptions, SweepReport};
use globset::GlobBuilder;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const BUILTIN: &str = include_str!("system.toml");
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Macos,
    Linux,
    Windows,
}

impl Platform {
    pub fn current() -> Option<Self> {
        if cfg!(target_os = "macos") {
            Some(Self::Macos)
        } else if cfg!(target_os = "linux") {
            Some(Self::Linux)
        } else if cfg!(windows) {
            Some(Self::Windows)
        } else {
            None
        }
    }
}

/// How readily an area is offered.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    /// Safe to clean whenever: it regenerates and holds nothing of the user's.
    #[default]
    Default,
    /// Fine to clean, at a cost the user should choose (permanence, re-downloads, a
    /// privileged command).
    OptIn,
    /// Never cleaned by carwash; shown with its size and how to clean it by hand.
    Report,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::OptIn => "opt-in",
            Self::Report => "report",
        }
    }
}

/// What cleaning an area takes beyond the user's own permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Need {
    /// Unix superuser: the command runs under `sudo`.
    Root,
    /// Windows administrator: the command runs elevated.
    Admin,
    /// macOS Full Disk Access for the terminal (privacy-protected locations).
    FullDiskAccess,
}

impl Need {
    pub fn label(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Admin => "administrator",
            Self::FullDiskAccess => "Full Disk Access",
        }
    }

    /// Whether the clean must be elevated (as opposed to merely permitted).
    pub fn elevates(self) -> bool {
        matches!(self, Self::Root | Self::Admin)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AreaSpec {
    pub id: String,
    pub name: String,
    pub platform: Platform,
    /// Grouping for display: caches, browsers, apps, logs, trash, temp, packages,
    /// containers, backups, system.
    pub category: String,
    #[serde(default)]
    pub tier: Tier,
    /// Path candidates (`~/`, `$VAR/`, `*`, `?`, `{a,b}`); the first that matches an existing
    /// directory is used, with all its matches. With `command`, they are only measured.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Only entries not modified for this long are removed, e.g. `7d`.
    #[serde(default)]
    pub older_than: Option<String>,
    /// Names directly under each path that are never touched (globs).
    #[serde(default)]
    pub keep: Vec<String>,
    /// Programs that must not be running while the area is cleaned (process names, globs).
    #[serde(default)]
    pub apps: Vec<String>,
    /// The operating system's own cleanup command, used instead of sweeping.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub needs: Option<Need>,
    /// What it is and what removing it costs.
    #[serde(default)]
    pub note: Option<String>,
    /// For `report` areas: how to clean it by hand.
    #[serde(default)]
    pub how: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemFile {
    schema: u32,
    #[serde(default)]
    area: Vec<AreaSpec>,
}

fn parse(source: &str) -> Result<Vec<AreaSpec>, String> {
    let file: SystemFile = toml::from_str(source).map_err(|e| e.to_string())?;
    if file.schema != SCHEMA_VERSION {
        return Err(format!("unsupported system schema {}", file.schema));
    }
    for spec in &file.area {
        validate(spec)?;
    }
    Ok(file.area)
}

fn validate(spec: &AreaSpec) -> Result<(), String> {
    let id = &spec.id;
    if spec.paths.is_empty() && spec.command.is_none() && spec.tier != Tier::Report {
        return Err(format!("area {id} has neither paths nor a command"));
    }
    if spec.command.as_ref().is_some_and(Vec::is_empty) {
        return Err(format!("area {id} has an empty command"));
    }
    if let Some(age) = &spec.older_than
        && crate::fmt::parse_age(age).is_none()
    {
        return Err(format!("area {id} has an invalid older_than `{age}`"));
    }
    if spec.needs.is_some_and(Need::elevates) && spec.command.is_none() && spec.tier != Tier::Report
    {
        // carwash never sweeps as root; elevated cleaning goes through the OS's own tool.
        return Err(format!("area {id} needs elevation but has no command"));
    }
    if spec.needs == Some(Need::FullDiskAccess)
        && spec.paths.iter().any(|p| p.contains(['*', '?', '{']))
    {
        // Expanding a pattern would read the protected location during discovery.
        return Err(format!(
            "area {id} needs Full Disk Access but uses a pattern"
        ));
    }
    Ok(())
}

pub fn builtin_specs() -> Vec<AreaSpec> {
    parse(BUILTIN).expect("built-in system areas parse")
}

/// Built-ins with user entries applied (same `id` and platform replaces, new ones are
/// added).
pub fn specs_with_overrides(user: &str) -> Result<Vec<AreaSpec>, String> {
    let mut specs = builtin_specs();
    for spec in parse(user)? {
        match specs
            .iter_mut()
            .find(|s| s.id == spec.id && s.platform == spec.platform)
        {
            Some(existing) => *existing = spec,
            None => specs.push(spec),
        }
    }
    Ok(specs)
}

/// How an area is cleaned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Action {
    /// Sweep the area's directories.
    Sweep,
    /// Run the operating system's tool.
    Command { argv: Vec<String> },
    /// Leave it to the user.
    Report,
}

/// An area present on this machine.
#[derive(Debug, Clone, Serialize)]
pub struct Area {
    pub id: String,
    pub name: String,
    pub category: String,
    pub tier: Tier,
    pub action: Action,
    /// Directories it covers: swept, or only measured for commands and reports.
    pub roots: Vec<PathBuf>,
    /// False when the roots were not checked during discovery because looking would need
    /// Full Disk Access; missing ones are skipped when the area is used.
    pub checked: bool,
    #[serde(serialize_with = "serialize_age")]
    pub older_than: Option<Duration>,
    pub keep: Vec<String>,
    pub apps: Vec<String>,
    pub needs: Option<Need>,
    pub note: Option<String>,
    pub how: Option<String>,
}

fn serialize_age<S: serde::Serializer>(age: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
    match age {
        Some(age) => s.serialize_some(&age.as_secs()),
        None => s.serialize_none(),
    }
}

impl Area {
    /// What sweeping would free (on a dry run) or freed. Commands and reports measure
    /// everything in their roots, since their tools decide what goes.
    pub fn sweep(&self, dry_run: bool, cancel: &Cancel) -> SweepReport {
        let options = match self.action {
            Action::Sweep => SweepOptions {
                older_than: self.older_than,
                keep: self.keep.clone(),
                dry_run,
            },
            // Never delete through a command or report area; only measure.
            Action::Command { .. } | Action::Report => SweepOptions {
                dry_run: true,
                ..SweepOptions::default()
            },
        };
        let mut report = SweepReport::default();
        for root in &self.roots {
            if cancel.is_cancelled() {
                report.cancelled = true;
                break;
            }
            match sweep::sweep(root, &options, cancel) {
                Ok(swept) => report.merge(swept),
                Err(error) => report.root_failed(root, &error),
            }
        }
        report
    }

    /// The running programs among [`Area::apps`].
    pub fn blocked_by(&self, running: &RunningApps) -> Vec<String> {
        running.matching(&self.apps)
    }
}

/// Areas present on this machine, in catalog order. Nothing is measured, and areas
/// needing Full Disk Access are listed without looking inside them.
pub fn discover(specs: &[AreaSpec], home: &Path) -> Vec<Area> {
    discover_with(
        specs,
        home,
        &|var| std::env::var(var).ok(),
        Platform::current(),
    )
}

fn discover_with(
    specs: &[AreaSpec],
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    platform: Option<Platform>,
) -> Vec<Area> {
    let mut out = Vec::new();
    for spec in specs.iter().filter(|s| Some(s.platform) == platform) {
        let checked = spec.needs != Some(Need::FullDiskAccess);
        let roots: Vec<PathBuf> = if checked {
            spec.paths
                .iter()
                .filter_map(|c| resolve(c, home, env))
                .map(|pattern| expand(&pattern))
                .find(|found| !found.is_empty())
                .unwrap_or_default()
        } else {
            spec.paths
                .iter()
                .filter_map(|c| resolve(c, home, env))
                .take(1)
                .collect()
        };
        let action = match (&spec.command, spec.tier) {
            (_, Tier::Report) => Action::Report,
            (Some(argv), _) => Action::Command { argv: argv.clone() },
            (None, _) => Action::Sweep,
        };
        let tool = || {
            spec.command
                .as_ref()
                .is_some_and(|argv| find_program(&argv[0]).is_some())
        };
        let present = match &action {
            Action::Command { .. } => tool(),
            Action::Sweep => !roots.is_empty(),
            // A report about a tool's data (snapshots, revisions) needs only the tool.
            Action::Report => !roots.is_empty() || tool(),
        };
        if !present {
            continue;
        }
        out.push(Area {
            id: spec.id.clone(),
            name: spec.name.clone(),
            category: spec.category.clone(),
            tier: spec.tier,
            action,
            roots,
            checked,
            older_than: spec.older_than.as_deref().and_then(crate::fmt::parse_age),
            keep: spec.keep.clone(),
            apps: spec.apps.clone(),
            needs: spec.needs,
            note: spec.note.clone(),
            how: spec.how.clone(),
        });
    }
    out
}

/// Existing directories matching `pattern`, whose components may hold `*`, `?` or `{a,b}`.
/// Matches are real directories, never symlinks; literal components are taken as given.
fn expand(pattern: &Path) -> Vec<PathBuf> {
    let mut current = vec![PathBuf::new()];
    for component in pattern.components() {
        let text = component.as_os_str().to_string_lossy();
        let is_glob = matches!(component, Component::Normal(_)) && text.contains(['*', '?', '{']);
        if !is_glob {
            for path in &mut current {
                path.push(component.as_os_str());
            }
            continue;
        }
        let Ok(glob) = GlobBuilder::new(&text)
            .case_insensitive(cfg!(windows))
            .build()
        else {
            return Vec::new();
        };
        let matcher = glob.compile_matcher();
        let mut next = Vec::new();
        for base in &current {
            let Ok(entries) = std::fs::read_dir(base) else {
                continue;
            };
            let mut found: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .filter(|e| matcher.is_match(Path::new(&e.file_name())))
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect();
            found.sort();
            next.extend(found);
        }
        current = next;
    }
    current
        .into_iter()
        .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
        .collect()
}

/// A snapshot of running process names, to keep from cleaning caches in use.
#[derive(Debug, Clone, Default)]
pub struct RunningApps(Vec<String>);

impl RunningApps {
    pub fn snapshot() -> Self {
        use sysinfo::{ProcessRefreshKind, RefreshKind, System};
        // Names only: no CPU, memory or command lines.
        let system = System::new_with_specifics(
            RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
        );
        let mut names: Vec<String> = system
            .processes()
            .values()
            .map(|p| {
                let name = p.name().to_string_lossy();
                name.strip_suffix(".exe").unwrap_or(&name).to_owned()
            })
            .collect();
        names.sort_unstable();
        names.dedup();
        Self(names)
    }

    #[cfg(test)]
    pub(crate) fn from_names(names: &[&str]) -> Self {
        Self(names.iter().map(|n| (*n).to_owned()).collect())
    }

    /// The patterns (process names, globs, case-insensitive) that match a running process.
    pub fn matching(&self, patterns: &[String]) -> Vec<String> {
        patterns
            .iter()
            .filter(|pattern| {
                GlobBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
                    .is_ok_and(|glob| {
                        let matcher = glob.compile_matcher();
                        self.0.iter().any(|name| matcher.is_match(name))
                    })
            })
            .cloned()
            .collect()
    }
}

/// Whether carwash runs as the Unix superuser. Sweeping as root is refused: its home
/// would be root's, and shared directories are where symlink attacks on cleaners happen.
pub fn running_as_root() -> bool {
    #[cfg(unix)]
    {
        rustix::process::geteuid().is_root()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn builtin_areas_parse_and_are_unique_per_platform() {
        let specs = builtin_specs();
        for platform in [Platform::Macos, Platform::Linux, Platform::Windows] {
            let mut ids: Vec<&str> = specs
                .iter()
                .filter(|s| s.platform == platform)
                .map(|s| s.id.as_str())
                .collect();
            assert!(ids.len() >= 10, "{platform:?} has {} areas", ids.len());
            let total = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), total, "{platform:?} has duplicate ids");
        }
        for spec in &specs {
            assert!(
                spec.tier != Tier::Report || spec.how.is_some(),
                "{} is report-only without instructions",
                spec.id
            );
            assert!(spec.note.is_some(), "{} has no note", spec.id);
        }
    }

    #[test]
    fn rejects_unsafe_specs() {
        let area = |extra: &str| {
            format!(
                "schema = 1\n[[area]]\nid = \"x\"\nname = \"X\"\nplatform = \"linux\"\ncategory = \"logs\"\n{extra}\n"
            )
        };
        assert!(parse(&area("paths = [\"/var/log\"]")).is_ok());
        assert!(parse(&area("")).is_err(), "nothing to do");
        assert!(
            parse(&area("paths = [\"/var/log\"]\nneeds = \"root\"")).is_err(),
            "root sweep"
        );
        assert!(parse(&area("paths = [\"/x\"]\nolder_than = \"soon\"")).is_err());
        assert!(
            parse(&area("paths = [\"~/a/*\"]\nneeds = \"full-disk-access\"")).is_err(),
            "pattern behind Full Disk Access"
        );
    }

    #[test]
    fn expands_patterns_without_following_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for profile in ["Default", "Profile 1"] {
            for cache in ["Cache", "Code Cache", "Other"] {
                fs::create_dir_all(root.join(profile).join(cache)).unwrap();
            }
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("Default"), root.join("Linked")).unwrap();
        let found = expand(&root.join("*").join("{Cache,Code Cache}"));
        let rel: Vec<String> = found
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(
            rel,
            [
                "Default/Cache",
                "Default/Code Cache",
                "Profile 1/Cache",
                "Profile 1/Code Cache"
            ]
        );
        assert!(expand(&root.join("missing")).is_empty());
        assert_eq!(expand(&root.join("Default")), [root.join("Default")]);
    }

    fn spec(id: &str, extra: &str) -> AreaSpec {
        toml::from_str(&format!(
            "id = \"{id}\"\nname = \"{id}\"\nplatform = \"linux\"\ncategory = \"caches\"\n{extra}"
        ))
        .unwrap()
    }

    #[test]
    fn discovers_present_areas_for_the_platform() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join(".cache/thumbnails")).unwrap();
        let specs = vec![
            spec(
                "thumbs",
                "paths = [\"$XDG_CACHE_HOME/thumbnails\", \"~/.cache/thumbnails\"]\nolder_than = \"30d\"",
            ),
            spec("absent", "paths = [\"~/.nothing\"]"),
            spec("tool", "command = [\"carwash-no-such-tool\"]"),
            spec(
                "private",
                "paths = [\"~/.private\"]\nneeds = \"full-disk-access\"",
            ),
        ];
        let areas = discover_with(&specs, home.path(), &|_| None, Some(Platform::Linux));
        let ids: Vec<&str> = areas.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["thumbs", "private"]);
        assert_eq!(areas[0].roots, [home.path().join(".cache/thumbnails")]);
        assert_eq!(areas[0].older_than, Some(Duration::from_secs(30 * 86_400)));
        assert!(!areas[1].checked, "not looked at without Full Disk Access");
        assert!(discover_with(&specs, home.path(), &|_| None, Some(Platform::Macos)).is_empty());
    }

    #[test]
    fn command_and_report_areas_are_only_measured() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("logs")).unwrap();
        fs::write(home.path().join("logs/a.log"), "x".repeat(4096)).unwrap();
        let specs = vec![spec(
            "logs",
            "tier = \"report\"\npaths = [\"~/logs\"]\nhow = \"by hand\"",
        )];
        let area = &discover_with(&specs, home.path(), &|_| None, Some(Platform::Linux))[0];
        assert_eq!(area.action, Action::Report);
        let report = area.sweep(false, &Cancel::new());
        assert!(report.removed.bytes >= 4096, "measured");
        assert!(home.path().join("logs/a.log").exists(), "never deleted");
    }

    #[test]
    fn matches_running_apps() {
        let running =
            RunningApps::from_names(&["Google Chrome", "slack", "Adobe Premiere Pro 2026"]);
        let apps = |names: &[&str]| -> Vec<String> { names.iter().map(|n| (*n).into()).collect() };
        assert_eq!(
            running.matching(&apps(&["Slack", "Discord"])),
            ["Slack"],
            "case-insensitive"
        );
        assert_eq!(
            running.matching(&apps(&["Adobe Premiere Pro*"])),
            ["Adobe Premiere Pro*"]
        );
        assert!(
            running.matching(&apps(&["Chrome"])).is_empty(),
            "whole names"
        );
    }
}
