//! Rust toolchains rustup has installed, and the ones projects still need.
//!
//! A toolchain is needed when a project pins it (`rust-toolchain.toml` or the older
//! `rust-toolchain` file) or declares it as its minimum supported version (`rust-version`
//! in `Cargo.toml`, which is what `cargo +1.88 check` is run against). Matching is
//! deliberately generous: a pin of `1.90` keeps both `1.90` and `1.90.0`, since either may
//! be the one a project's commands name.
//!
//! Pins are resolved the way rustup resolves them (`find_override_from_dir_walk` in
//! rustup's `src/config.rs`): from the project's directory upwards to the filesystem
//! root, taking in each directory first a directory override (`rustup override set`), then
//! `rust-toolchain`, then `rust-toolchain.toml`, and stopping at the first one found.
//! `RUSTUP_TOOLCHAIN` in the environment comes before all of them and is kept as standing
//! advice. What no file records cannot be seen here: `cargo +toolchain` in scripts, CI
//! files and habits.

use crate::Cancel;
use crate::caches::Advice;
use crate::discover::{Counters, DiscoverOptions, Hidden};
use crate::scan::{Engine, ScanEvent, ScanOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Lists installed toolchains, one per line, marking the default.
pub const LIST: &[&str] = &["rustup", "toolchain", "list"];

const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// A toolchain `rustup toolchain list` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Installed {
    /// The name rustup knows it by, which is also its directory under `toolchains/`.
    pub name: String,
    pub default: bool,
}

/// Whether `name` can be a toolchain's name: one path component (it is also the toolchain's
/// directory), and nothing `rustup toolchain uninstall` would read as an option.
pub fn is_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '.'])
        && !name.contains(|c: char| c.is_whitespace() || matches!(c, '/' | '\\'))
}

/// Parses `rustup toolchain list`: a name per line, followed by `(active, default)`,
/// `(default)` or `(active)` where they apply.
pub fn parse_list(output: &str) -> Vec<Installed> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (name, markers) = match line.split_once(" (") {
                Some((name, markers)) => (name, markers.trim_end_matches(')')),
                None => (line, ""),
            };
            // "no installed toolchains", and anything else that is not a toolchain's name.
            if !is_name(name) {
                return None;
            }
            Some(Installed {
                name: name.to_owned(),
                default: markers.split(", ").any(|marker| marker == "default"),
            })
        })
        .collect()
}

/// The release channel a toolchain name or a pin stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channel {
    /// `stable`, `beta` or `nightly`, with the date of a dated one (`nightly-2026-01-15`).
    Named {
        name: &'static str,
        date: Option<String>,
    },
    /// A release: `1.90` is `[1, 90]`, `1.90.1` is `[1, 90, 1]`.
    Version(Vec<u32>),
}

impl Channel {
    /// Reads the channel at the start of a toolchain name (`1.90.0-aarch64-apple-darwin`)
    /// or of a pin (`nightly-2026-01-15`, `1.90`). `None` for custom toolchains.
    pub fn parse(spec: &str) -> Option<Self> {
        let mut parts = spec.trim().split('-');
        let first = parts.next()?;
        for name in ["stable", "beta", "nightly"] {
            if first == name {
                let rest: Vec<&str> = parts.collect();
                let date = match rest.as_slice() {
                    [year, month, day, ..]
                        if year.len() == 4
                            && month.len() == 2
                            && day.len() == 2
                            && [year, month, day]
                                .iter()
                                .all(|part| part.bytes().all(|b| b.is_ascii_digit())) =>
                    {
                        Some(format!("{year}-{month}-{day}"))
                    }
                    _ => None,
                };
                return Some(Self::Named { name, date });
            }
        }
        let numbers: Vec<u32> = first
            .split('.')
            .map(|part| part.parse().ok())
            .collect::<Option<_>>()?;
        matches!(numbers.len(), 2 | 3).then_some(Self::Version(numbers))
    }

    /// `stable` and `nightly` as rustup keeps them up to date, not pinned to a date.
    fn is_rolling(&self) -> Option<&'static str> {
        match self {
            Self::Named { name, date: None } if *name != "beta" => Some(name),
            _ => None,
        }
    }

    /// Whether a toolchain on this channel serves a pin of `wanted`. Releases match when
    /// one is the other's prefix: `1.90` and `1.90.0` serve each other.
    fn serves(&self, wanted: &Self) -> bool {
        match (self, wanted) {
            (Self::Version(have), Self::Version(want)) => {
                let shared = have.len().min(want.len());
                have[..shared] == want[..shared]
            }
            _ => self == wanted,
        }
    }
}

/// A toolchain a project asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Pin {
    pub channel: Channel,
    /// The project directory the pin applies to.
    pub project: PathBuf,
}

impl Pin {
    pub fn new(channel: Channel, project: impl Into<PathBuf>) -> Self {
        Self {
            channel,
            project: project.into(),
        }
    }
}

fn read(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    (meta.is_file() && meta.len() <= MAX_FILE_BYTES)
        .then(|| std::fs::read_to_string(path).ok())
        .flatten()
}

/// The channel a pin file names. `rust-toolchain.toml` is TOML with a `[toolchain]` table;
/// the older `rust-toolchain` may be the same TOML or just the channel on a line.
fn pinned_channel(text: &str, legacy: bool) -> Option<Channel> {
    let channel = match text.parse::<toml::Table>() {
        Ok(table) => table
            .get("toolchain")?
            .get("channel")?
            .as_str()
            .map(str::to_owned),
        Err(_) if legacy => text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned),
        Err(_) => None,
    };
    Channel::parse(&channel?)
}

/// What one directory says about the toolchain to use in it and below.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stop {
    /// Nothing here: rustup looks in the parent.
    No,
    /// A directory override or a pin file. rustup looks no further, whether or not the
    /// channel could be read.
    Here(Option<Channel>),
}

/// What rustup finds in `dir` itself: a directory override first, then `rust-toolchain`,
/// then `rust-toolchain.toml`. When both files exist rustup uses `rust-toolchain`.
fn stop_at(dir: &Path, overrides: &[(String, String)]) -> Stop {
    if let Some((_, toolchain)) = overrides.iter().find(|(d, _)| Path::new(d) == dir) {
        return Stop::Here(Channel::parse(toolchain));
    }
    for (file, legacy) in [("rust-toolchain", true), ("rust-toolchain.toml", false)] {
        if let Some(text) = read(&dir.join(file)) {
            return Stop::Here(pinned_channel(&text, legacy));
        }
    }
    Stop::No
}

/// The `rust-version` a manifest declares for its package or for its workspace's packages.
fn minimum_versions(manifest: &str) -> Vec<Channel> {
    let Ok(table) = manifest.parse::<toml::Table>() else {
        return Vec::new();
    };
    let package = table.get("package").and_then(|p| p.get("rust-version"));
    let workspace = table
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("rust-version"));
    [package, workspace]
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .filter_map(Channel::parse)
        .collect()
}

/// The toolchains the projects at `projects` ask for: each one's `rust-version`, and the
/// pin rustup would resolve in its directory, looked for from there up to the filesystem
/// root. `overrides` are rustup's directory overrides.
pub fn pins(projects: &[PathBuf], overrides: &[(String, String)]) -> Vec<Pin> {
    let mut found = Vec::new();
    // Directories above several projects are read once.
    let mut stops: BTreeMap<PathBuf, Stop> = BTreeMap::new();
    for project in projects {
        if let Some(manifest) = read(&project.join("Cargo.toml")) {
            found.extend(minimum_versions(&manifest).into_iter().map(|channel| Pin {
                channel,
                project: project.clone(),
            }));
        }
        for dir in project.ancestors() {
            let stop = stops
                .entry(dir.to_path_buf())
                .or_insert_with(|| stop_at(dir, overrides));
            if let Stop::Here(channel) = stop {
                found.extend(channel.clone().map(|channel| Pin {
                    channel,
                    project: project.clone(),
                }));
                break;
            }
        }
    }
    found
}

/// What the Rust projects under some directories need of the installed toolchains.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Needs {
    /// The directories that were searched.
    pub roots: Vec<PathBuf>,
    /// How many Rust projects (directories with a `Cargo.toml`) were found. With none,
    /// nothing can be said about which toolchains are unused.
    pub projects: usize,
    pub pins: Vec<Pin>,
}

/// How many levels below a searched directory a symlink to another directory is still
/// followed.
pub const LINK_DEPTH: usize = 3;
/// How many such links are followed in all.
const LINKS: usize = 32;

/// The directories that symlinks within [`LINK_DEPTH`] levels of `root` lead to, resolved,
/// each once. Links back into `root`, to a directory above it, or to something already
/// listed are left out, so nothing is searched twice and no loop is followed.
fn linked_directories(root: &Path, seen: &mut Vec<PathBuf>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut level = vec![root.to_path_buf()];
    for _ in 0..LINK_DEPTH {
        let mut next = Vec::new();
        for dir in level {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    // Nothing a project is linked from lives in these.
                    let name = entry.file_name();
                    if !["target", "node_modules", ".git"]
                        .iter()
                        .any(|skip| name == *skip)
                    {
                        next.push(entry.path());
                    }
                } else if kind.is_symlink()
                    && found.len() < LINKS
                    && let Ok(target) = std::fs::canonicalize(entry.path())
                    && target.is_dir()
                    && !seen
                        .iter()
                        .any(|known| target.starts_with(known) || known.starts_with(&target))
                {
                    seen.push(target.clone());
                    found.push(target);
                }
            }
        }
        level = next;
    }
    found
}

/// Finds the Rust projects under `roots` and what they need. Hidden folders are searched
/// too, and symlinks to other directories are followed once when they sit within
/// [`LINK_DEPTH`] levels of a root. Only names are read while searching; then each
/// project's `Cargo.toml` and the pin files above it.
pub fn needs(
    engine: &Engine,
    roots: &[PathBuf],
    options: &DiscoverOptions,
    overrides: &[(String, String)],
    cancel: &Cancel,
) -> Needs {
    // Crates that cargo and rustup downloaded are dependencies, not projects: what they
    // declare as their minimum version says nothing about the toolchains in use here.
    let mut exclude = options.exclude.clone();
    let home = crate::paths::home();
    for (variable, default) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
        let dir = std::env::var_os(variable)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(default)));
        exclude.extend(dir.map(|dir| std::fs::canonicalize(&dir).unwrap_or(dir)));
    }
    let options = ScanOptions {
        discover: DiscoverOptions {
            hidden: Hidden::Always,
            exclude,
            ..options.clone()
        },
        git: false,
        measure: false,
    };
    let roots: Vec<PathBuf> = roots
        .iter()
        .map(|root| std::fs::canonicalize(root).unwrap_or_else(|_| root.clone()))
        .collect();
    let mut seen = roots.clone();
    let mut search = roots.clone();
    for root in &roots {
        search.extend(linked_directories(root, &mut seen));
    }
    let projects = Mutex::new(BTreeSet::new());
    for dir in &search {
        engine.scan(dir, &options, &Counters::default(), cancel, &|event| {
            if let ScanEvent::Project(project) = event
                && project.path.join("Cargo.toml").is_file()
            {
                projects.lock().expect("projects").insert(project.path);
            }
        });
    }
    let projects: Vec<PathBuf> = projects
        .into_inner()
        .expect("projects")
        .into_iter()
        .collect();
    Needs {
        pins: pins(&projects, overrides),
        projects: projects.len(),
        roots,
    }
}

/// The directory overrides in rustup's `settings.toml` (`rustup override set`): directory
/// and toolchain name.
pub fn overrides(settings: &str) -> Vec<(String, String)> {
    let Ok(table) = settings.parse::<toml::Table>() else {
        return Vec::new();
    };
    table
        .get("overrides")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flatten()
        .filter_map(|(dir, toolchain)| Some((dir.clone(), toolchain.as_str()?.to_owned())))
        .collect()
}

/// Whether `toolchain` is what a `RUSTUP_TOOLCHAIN` value or an override names: its full
/// name, its name without the host, or its channel.
fn named_by(toolchain: &str, value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && (toolchain == value
            || toolchain.starts_with(&format!("{value}-"))
            || Channel::parse(toolchain)
                .zip(Channel::parse(value))
                .is_some_and(|(have, want)| have.serves(&want)))
}

/// rustup's directory overrides on this machine, from `settings.toml` in `RUSTUP_HOME` or
/// `~/.rustup`.
pub fn local_overrides() -> Vec<(String, String)> {
    let home = std::env::var_os("RUSTUP_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::paths::home().map(|home| home.join(".rustup")));
    home.and_then(|home| std::fs::read_to_string(home.join("settings.toml")).ok())
        .map(|settings| overrides(&settings))
        .unwrap_or_default()
}

/// What rustup itself says must stay: the default toolchain, `stable` and `nightly`, the
/// toolchain `RUSTUP_TOOLCHAIN` names (`selected`), and any toolchain a directory override
/// names. `None` leaves the question to the projects.
pub fn standing_advice(
    toolchain: &Installed,
    overrides: &[(String, String)],
    selected: Option<&str>,
) -> Option<Advice> {
    if toolchain.default {
        return Some(Advice::protected("the default toolchain"));
    }
    if let Some(name) = Channel::parse(&toolchain.name).and_then(|c| c.is_rolling()) {
        return Some(Advice::keep(format!("the {name} channel")));
    }
    if selected.is_some_and(|value| named_by(&toolchain.name, value)) {
        return Some(Advice::keep("named by RUSTUP_TOOLCHAIN"));
    }
    overrides
        .iter()
        .find(|(_, name)| *name == toolchain.name)
        .map(|(dir, _)| Advice::keep(format!("override for {dir}")))
}

/// The projects whose pins a toolchain serves, in path order; `None` for a custom toolchain,
/// whose users carwash cannot know.
pub fn needed_by(name: &str, pins: &[Pin]) -> Option<Vec<PathBuf>> {
    let channel = Channel::parse(name)?;
    let projects: BTreeSet<&Path> = pins
        .iter()
        .filter(|pin| channel.serves(&pin.channel))
        .map(|pin| pin.project.as_path())
        .collect();
    Some(projects.into_iter().map(Path::to_path_buf).collect())
}

/// The directories of `needs`, for a reason that names its scope.
fn scope(needs: &Needs) -> String {
    let roots: Vec<String> = needs
        .roots
        .iter()
        .map(|root| root.display().to_string())
        .collect();
    roots.join(" or ")
}

/// What projects make of a toolchain rustup does not itself keep. `None` for `needs` means
/// no projects were looked at; so does a search that found no Rust project. Either way
/// nothing is known, and the verdict says so. Otherwise the toolchain is kept when a
/// project needs it and recommended for removal when none under the searched directories
/// does, which the reason names.
pub fn advice_from_needs(name: &str, needs: Option<&Needs>) -> Advice {
    let Some(needs) = needs else {
        return Advice::unknown("not checked against any projects");
    };
    if needs.projects == 0 {
        return Advice::unknown(format!("no Rust project found under {}", scope(needs)));
    }
    let Some(needed_by) = needed_by(name, &needs.pins) else {
        return Advice::unknown("a custom toolchain, which carwash cannot match to projects");
    };
    let name_of = |project: &Path| {
        project.file_name().map_or_else(
            || project.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    };
    match needed_by.len() {
        0 => Advice::remove(format!("no project under {} needs it", scope(needs))),
        1 => Advice::keep(format!(
            "{} needs it",
            needed_by.first().map(|p| name_of(p)).unwrap_or_default()
        )),
        n => Advice::keep(format!("{n} projects need it")),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::caches::Verdict;
    use std::fs;

    /// `rustup toolchain list` as captured.
    pub(crate) const LISTING: &str = include_str!("../tests/data/rustup-toolchain-list.txt");

    #[test]
    fn parses_the_toolchain_list() {
        let toolchains = parse_list(LISTING);
        assert_eq!(toolchains.len(), 18);
        assert_eq!(
            toolchains[0],
            Installed {
                name: "stable-aarch64-apple-darwin".into(),
                default: true,
            }
        );
        assert_eq!(toolchains.iter().filter(|t| t.default).count(), 1);
        assert!(
            toolchains
                .iter()
                .any(|t| t.name == "1.90.0-aarch64-apple-darwin")
        );
        assert!(parse_list("no installed toolchains\n").is_empty());
        assert!(
            parse_list("--help\n-v (default)\n../outside\n/etc\n.hidden\nc:\\x\n").is_empty(),
            "nothing rustup would read as an option, and nothing outside its folder"
        );
        assert_eq!(
            parse_list("nightly-x86_64-unknown-linux-gnu (default)\nmine (active)\n"),
            [
                Installed {
                    name: "nightly-x86_64-unknown-linux-gnu".into(),
                    default: true,
                },
                Installed {
                    name: "mine".into(),
                    default: false,
                },
            ]
        );
    }

    fn version(numbers: &[u32]) -> Channel {
        Channel::Version(numbers.to_vec())
    }

    #[test]
    fn reads_channels_from_names_and_pins() {
        assert_eq!(
            Channel::parse("1.90.0-aarch64-apple-darwin"),
            Some(version(&[1, 90, 0]))
        );
        assert_eq!(Channel::parse("1.90"), Some(version(&[1, 90])));
        assert_eq!(
            Channel::parse("stable-x86_64-pc-windows-msvc"),
            Some(Channel::Named {
                name: "stable",
                date: None
            })
        );
        assert_eq!(
            Channel::parse("nightly-2026-01-15-aarch64-apple-darwin"),
            Some(Channel::Named {
                name: "nightly",
                date: Some("2026-01-15".into())
            })
        );
        for custom in ["mine", "stage1", "", "1", "1.x"] {
            assert_eq!(Channel::parse(custom), None, "{custom}");
        }
    }

    #[test]
    fn a_release_serves_pins_of_the_same_minor() {
        let serves = |have: &str, want: &str| {
            Channel::parse(have)
                .unwrap()
                .serves(&Channel::parse(want).unwrap())
        };
        assert!(serves("1.90-aarch64-apple-darwin", "1.90"));
        assert!(serves("1.90.0-aarch64-apple-darwin", "1.90"));
        assert!(serves("1.90-aarch64-apple-darwin", "1.90.0"));
        assert!(!serves("1.90.1-aarch64-apple-darwin", "1.90.0"));
        assert!(!serves("1.91-aarch64-apple-darwin", "1.90"));
        assert!(serves(
            "nightly-2026-01-15-aarch64-apple-darwin",
            "nightly-2026-01-15"
        ));
        assert!(!serves(
            "nightly-aarch64-apple-darwin",
            "nightly-2026-01-15"
        ));
        assert!(!serves("stable-aarch64-apple-darwin", "1.90"));
    }

    /// Projects pinning toolchains in each of the ways there are.
    pub(crate) fn projects(root: &Path) -> Vec<PathBuf> {
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            "pinned/rust-toolchain.toml",
            "# Reproducible releases.\n[toolchain]\nchannel = \"1.98.1\"\ncomponents = [\"rustfmt\", \"clippy\"]\n",
        );
        write(
            "pinned/Cargo.toml",
            "[package]\nname = \"pinned\"\nrust-version = \"1.95\"\n",
        );
        write("legacy/rust-toolchain", "1.90.0\n");
        write("legacy/Cargo.toml", "[package]\nname = \"legacy\"\n");
        write(
            "space/Cargo.toml",
            "[workspace]\nmembers = [\"member\"]\n[workspace.package]\nrust-version = \"1.88\"\n",
        );
        write(
            "space/rust-toolchain.toml",
            "[toolchain]\nchannel = \"1.99\"\n",
        );
        write(
            "space/member/Cargo.toml",
            "[package]\nname = \"member\"\nrust-version.workspace = true\n",
        );
        write("plain/Cargo.toml", "[package]\nname = \"plain\"\n");
        ["pinned", "legacy", "space", "space/member", "plain"]
            .iter()
            .map(|rel| root.join(rel))
            .collect()
    }

    #[test]
    fn finds_what_projects_pin_and_their_minimum_versions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let found = pins(&projects(root), &[]);
        let of = |project: &str| -> Vec<Channel> {
            found
                .iter()
                .filter(|pin| pin.project == root.join(project))
                .map(|pin| pin.channel.clone())
                .collect()
        };
        assert_eq!(of("pinned"), [version(&[1, 95]), version(&[1, 98, 1])]);
        assert_eq!(of("legacy"), [version(&[1, 90, 0])]);
        assert_eq!(of("space"), [version(&[1, 88]), version(&[1, 99])]);
        assert_eq!(
            of("space/member"),
            [version(&[1, 99])],
            "the workspace's pin file applies to its members"
        );
        assert!(of("plain").is_empty());
    }

    /// Writes files under `root`, creating their folders.
    fn tree(root: &Path, files: &[(&str, &str)]) {
        for (rel, text) in files {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    fn channels(found: &[Pin]) -> Vec<Channel> {
        found.iter().map(|pin| pin.channel.clone()).collect()
    }

    #[test]
    fn rust_toolchain_wins_over_rust_toolchain_toml_as_in_rustup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        tree(
            root,
            &[
                ("both/rust-toolchain", "1.89\n"),
                (
                    "both/rust-toolchain.toml",
                    "[toolchain]\nchannel = \"1.91\"\n",
                ),
                ("both/Cargo.toml", "[package]\nname = \"both\"\n"),
                // The legacy file may hold the TOML form too.
                (
                    "toml-in-legacy/rust-toolchain",
                    "[toolchain]\nchannel = \"1.94\"\n",
                ),
                // Only the legacy file may hold a bare channel.
                ("bare-in-toml/rust-toolchain.toml", "1.97\n"),
            ],
        );
        assert_eq!(
            channels(&pins(&[root.join("both")], &[])),
            [version(&[1, 89])],
            "the file rustup reads"
        );
        assert_eq!(
            channels(&pins(&[root.join("toml-in-legacy")], &[])),
            [version(&[1, 94])]
        );
        assert!(pins(&[root.join("bare-in-toml")], &[]).is_empty());
    }

    #[test]
    fn a_pin_above_the_searched_folder_counts_for_the_projects_below() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        tree(
            root,
            &[
                (
                    "mono/rust-toolchain.toml",
                    "[toolchain]\nchannel = \"1.94\"\n",
                ),
                ("mono/crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
                (
                    "mono/crates/tool/Cargo.toml",
                    "[package]\nname = \"tool\"\n",
                ),
                // The nearest file is the one rustup stops at.
                ("mono/crates/tool/rust-toolchain", "1.90\n"),
            ],
        );
        // The projects are given as a search of `mono/crates` would find them; the pin
        // file sits one level above that.
        let crates = root.join("mono/crates");
        let found = pins(&[crates.join("app"), crates.join("tool")], &[]);
        assert_eq!(
            found,
            [
                Pin {
                    channel: version(&[1, 94]),
                    project: crates.join("app"),
                },
                Pin {
                    channel: version(&[1, 90]),
                    project: crates.join("tool"),
                },
            ]
        );
    }

    #[test]
    fn a_directory_override_comes_before_a_pin_file_and_stops_the_walk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        tree(
            root,
            &[
                (
                    "work/rust-toolchain.toml",
                    "[toolchain]\nchannel = \"1.94\"\n",
                ),
                (
                    "work/app/rust-toolchain.toml",
                    "[toolchain]\nchannel = \"1.91\"\n",
                ),
                ("work/app/Cargo.toml", "[package]\nname = \"app\"\n"),
                ("work/app/sub/Cargo.toml", "[package]\nname = \"sub\"\n"),
            ],
        );
        let app = root.join("work/app");
        let overrides = vec![(
            app.display().to_string(),
            "1.89-aarch64-apple-darwin".to_owned(),
        )];
        let found = pins(&[app.clone(), app.join("sub")], &overrides);
        assert_eq!(
            channels(&found),
            [version(&[1, 89]), version(&[1, 89])],
            "the override, for the directory and below it; not the files"
        );
    }

    #[test]
    fn a_pin_file_that_cannot_be_read_still_stops_the_walk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        tree(
            root,
            &[
                (
                    "work/rust-toolchain.toml",
                    "[toolchain]\nchannel = \"1.94\"\n",
                ),
                // rustup fails here; it does not go on to the file above.
                (
                    "work/broken/rust-toolchain.toml",
                    "[toolchain]\nchannel = \n",
                ),
                (
                    "work/broken/Cargo.toml",
                    "[package]\nname = \"broken\"\nrust-version = \"1.88\"\n[package]\n",
                ),
                // Components only: the default toolchain, and again no further.
                (
                    "work/parts/rust-toolchain.toml",
                    "[toolchain]\ncomponents = [\"clippy\"]\n",
                ),
            ],
        );
        assert!(pins(&[root.join("work/broken")], &[]).is_empty());
        assert!(pins(&[root.join("work/parts")], &[]).is_empty());
    }

    fn search(roots: &[&Path]) -> Needs {
        let engine = Engine::new(crate::Registry::builtin());
        let roots: Vec<PathBuf> = roots.iter().map(|root| root.to_path_buf()).collect();
        needs(
            &engine,
            &roots,
            &DiscoverOptions::default(),
            &[],
            &Cancel::new(),
        )
    }

    #[test]
    fn a_search_finds_projects_in_hidden_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        tree(
            &root,
            &[
                (
                    "app/Cargo.toml",
                    "[package]\nname = \"app\"\nrust-version = \"1.88\"\n",
                ),
                (".tools/gen/Cargo.toml", "[package]\nname = \"gen\"\n"),
                (
                    ".tools/gen/rust-toolchain.toml",
                    "[toolchain]\nchannel = \"1.91\"\n",
                ),
                ("notes/readme.txt", "no manifest here"),
            ],
        );
        let found = search(&[&root]);
        assert_eq!(found.roots.as_slice(), std::slice::from_ref(&root));
        assert_eq!(found.projects, 2);
        assert_eq!(
            channels(&found.pins),
            [version(&[1, 91]), version(&[1, 88])]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_search_follows_a_symlinked_project_once_and_no_loops() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let root = base.join("work");
        tree(
            &base,
            &[
                ("work/app/Cargo.toml", "[package]\nname = \"app\"\n"),
                // Lives elsewhere, under a pin of its own.
                ("elsewhere/rust-toolchain", "1.90.0\n"),
                (
                    "elsewhere/linked/Cargo.toml",
                    "[package]\nname = \"linked\"\n",
                ),
            ],
        );
        symlink(base.join("elsewhere/linked"), root.join("linked")).unwrap();
        // The same project again, a link back into the folder, a link to the folder above
        // it, and a link that leads nowhere.
        symlink(base.join("elsewhere/linked"), root.join("again")).unwrap();
        symlink(root.join("app"), root.join("app-link")).unwrap();
        symlink(&base, root.join("up")).unwrap();
        symlink(base.join("missing"), root.join("dangling")).unwrap();

        let found = search(&[&root]);
        assert_eq!(found.projects, 2, "{found:?}");
        assert_eq!(
            found.pins,
            [Pin {
                channel: version(&[1, 90, 0]),
                project: base.join("elsewhere/linked"),
            }],
            "resolved where the link leads, with the pin rustup finds there"
        );
    }

    #[test]
    fn stable_nightly_and_the_default_always_stay() {
        let toolchains = parse_list(LISTING);
        let advice = |name: &str| {
            let toolchain = toolchains.iter().find(|t| t.name == name).unwrap();
            standing_advice(toolchain, &[], None)
        };
        let default = advice("stable-aarch64-apple-darwin").unwrap();
        assert_eq!(default.verdict, Verdict::Protected);
        assert_eq!(default.reason, "the default toolchain");
        let nightly = advice("nightly-aarch64-apple-darwin").unwrap();
        assert_eq!(nightly.verdict, Verdict::Keep);
        assert_eq!(nightly.reason, "the nightly channel");
        assert_eq!(advice("1.88-aarch64-apple-darwin"), None);

        // Wherever the default is, stable stays.
        let moved = parse_list(
            "stable-aarch64-apple-darwin\n1.90-aarch64-apple-darwin (active, default)\n",
        );
        assert_eq!(
            standing_advice(&moved[0], &[], None).unwrap().verdict,
            Verdict::Keep
        );
        assert_eq!(
            standing_advice(&moved[1], &[], None).unwrap().verdict,
            Verdict::Protected
        );
        // A dated nightly is a pin like any other.
        let dated = Installed {
            name: "nightly-2026-01-15-aarch64-apple-darwin".into(),
            default: false,
        };
        assert_eq!(standing_advice(&dated, &[], None), None);
    }

    #[test]
    fn the_toolchain_rustup_toolchain_names_is_kept() {
        let toolchain = |name: &str| Installed {
            name: name.into(),
            default: false,
        };
        for value in ["1.90", "1.90.0", "1.90.0-aarch64-apple-darwin", " 1.90 "] {
            let advice =
                standing_advice(&toolchain("1.90.0-aarch64-apple-darwin"), &[], Some(value))
                    .unwrap_or_else(|| panic!("{value}"));
            assert_eq!(advice.verdict, Verdict::Keep);
            assert_eq!(advice.reason, "named by RUSTUP_TOOLCHAIN");
        }
        for other in ["1.91", "stable", "", "1.9"] {
            assert_eq!(
                standing_advice(&toolchain("1.90.0-aarch64-apple-darwin"), &[], Some(other)),
                None,
                "{other}"
            );
        }
        assert!(standing_advice(&toolchain("mine"), &[], Some("mine")).is_some());
    }

    #[test]
    fn directory_overrides_keep_their_toolchain() {
        let settings = "version = \"12\"\ndefault_toolchain = \"stable-aarch64-apple-darwin\"\n\n[overrides]\n\"/work/old\" = \"1.89-aarch64-apple-darwin\"\n";
        let overrides = overrides(settings);
        assert_eq!(
            overrides,
            [(
                "/work/old".to_owned(),
                "1.89-aarch64-apple-darwin".to_owned()
            )]
        );
        let toolchain = Installed {
            name: "1.89-aarch64-apple-darwin".into(),
            default: false,
        };
        let advice = standing_advice(&toolchain, &overrides, None).unwrap();
        assert_eq!(advice.verdict, Verdict::Keep);
        assert_eq!(advice.reason, "override for /work/old");
        assert!(super::overrides("version = \"12\"\n[overrides]\n").is_empty());
    }

    #[test]
    fn toolchains_no_project_needs_are_recommended_with_the_scope_named() {
        let dir = tempfile::tempdir().unwrap();
        let projects = projects(dir.path());
        let needs = Needs {
            roots: vec![PathBuf::from("/work")],
            projects: projects.len(),
            pins: pins(&projects, &[]),
        };
        let verdict = |name: &str| {
            let advice = advice_from_needs(name, Some(&needs));
            (advice.verdict, advice.reason)
        };
        assert_eq!(
            verdict("1.98.1-aarch64-apple-darwin"),
            (Verdict::Keep, "pinned needs it".into())
        );
        assert_eq!(
            verdict("1.95.0-aarch64-apple-darwin"),
            (Verdict::Keep, "pinned needs it".into()),
            "its minimum supported version"
        );
        assert_eq!(
            verdict("1.99-aarch64-apple-darwin"),
            (Verdict::Keep, "2 projects need it".into())
        );
        assert_eq!(
            verdict("1.88-aarch64-apple-darwin"),
            (Verdict::Keep, "space needs it".into())
        );
        assert_eq!(
            verdict("1.90-aarch64-apple-darwin"),
            (Verdict::Keep, "legacy needs it".into())
        );
        for unused in [
            "1.91-aarch64-apple-darwin",
            "1.98.0-aarch64-apple-darwin",
            "1.100.0-aarch64-apple-darwin",
        ] {
            assert_eq!(
                verdict(unused),
                (Verdict::Remove, "no project under /work needs it".into()),
                "{unused}"
            );
        }
        assert_eq!(
            verdict("mine").0,
            Verdict::Unknown,
            "custom toolchains stay"
        );
        let two = Needs {
            roots: vec![PathBuf::from("/work"), PathBuf::from("/src")],
            ..needs.clone()
        };
        assert_eq!(
            advice_from_needs("1.91-aarch64-apple-darwin", Some(&two)).reason,
            "no project under /work or /src needs it"
        );
    }

    #[test]
    fn nothing_is_recommended_without_rust_projects_to_go_by() {
        let name = "1.91-aarch64-apple-darwin";
        let unchecked = advice_from_needs(name, None);
        assert_eq!(unchecked.verdict, Verdict::Unknown);
        assert_eq!(unchecked.reason, "not checked against any projects");

        // A folder was searched, and holds no Rust project at all.
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        tree(&root, &[("web/package.json", "{}"), ("notes/todo.txt", "")]);
        let found = search(&[&root]);
        assert_eq!((found.projects, found.pins.len()), (0, 0));
        let empty = advice_from_needs(name, Some(&found));
        assert_eq!(empty.verdict, Verdict::Unknown);
        assert_eq!(
            empty.reason,
            format!("no Rust project found under {}", root.display())
        );
        // One Rust project is enough to go by, even one that pins nothing.
        tree(&root, &[("app/Cargo.toml", "[package]\nname = \"app\"\n")]);
        let found = search(&[&root]);
        assert_eq!(found.projects, 1);
        assert_eq!(
            advice_from_needs(name, Some(&found)).verdict,
            Verdict::Remove
        );
    }
}
