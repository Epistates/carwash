//! Rust toolchains rustup has installed, and the ones projects still need.
//!
//! A toolchain is needed when a project pins it (`rust-toolchain.toml` or the older
//! `rust-toolchain` file) or declares it as its minimum supported version (`rust-version`
//! in `Cargo.toml`, which is what `cargo +1.88 check` is run against). Matching is
//! deliberately generous: a pin of `1.90` keeps both `1.90` and `1.90.0`, since either may
//! be the one a project's commands name.

use crate::caches::Advice;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Lists installed toolchains, one per line, marking the default.
pub const LIST: &[&str] = &["rustup", "toolchain", "list"];

const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// A toolchain `rustup toolchain list` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
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
pub struct Pin {
    pub channel: Channel,
    /// The project directory the pin applies to.
    pub project: PathBuf,
}

fn read(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    (meta.is_file() && meta.len() <= MAX_FILE_BYTES)
        .then(|| std::fs::read_to_string(path).ok())
        .flatten()
}

/// The channel a `rust-toolchain.toml` names, or a `rust-toolchain` file in either of its
/// forms: the same TOML, or just the channel on a line.
fn pinned_channel(text: &str) -> Option<Channel> {
    let channel = match text.parse::<toml::Table>() {
        Ok(table) => table
            .get("toolchain")?
            .get("channel")?
            .as_str()
            .map(str::to_owned),
        Err(_) => text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned),
    };
    Channel::parse(&channel?)
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

/// The toolchains the projects at `projects` ask for. Pin files are looked for in each
/// project and in the directories above it up to `root`, as rustup does.
pub fn pins(projects: &[PathBuf], root: &Path) -> Vec<Pin> {
    let mut found = Vec::new();
    // Directories above several projects are read once.
    let mut pin_files: BTreeMap<PathBuf, Option<Channel>> = BTreeMap::new();
    for project in projects {
        if let Some(manifest) = read(&project.join("Cargo.toml")) {
            found.extend(minimum_versions(&manifest).into_iter().map(|channel| Pin {
                channel,
                project: project.clone(),
            }));
        }
        for dir in project.ancestors() {
            let pinned = pin_files.entry(dir.to_path_buf()).or_insert_with(|| {
                ["rust-toolchain.toml", "rust-toolchain"]
                    .iter()
                    .find_map(|file| read(&dir.join(file)))
                    .and_then(|text| pinned_channel(&text))
            });
            if let Some(channel) = pinned {
                found.push(Pin {
                    channel: channel.clone(),
                    project: project.clone(),
                });
                // The nearest file wins.
                break;
            }
            if dir == root {
                break;
            }
        }
    }
    found
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

/// What rustup itself says must stay: the default toolchain, `stable` and `nightly`, and
/// any toolchain a directory override names. `None` leaves the question to the projects.
pub fn standing_advice(toolchain: &Installed, overrides: &[(String, String)]) -> Option<Advice> {
    if toolchain.default {
        return Some(Advice::protected("the default toolchain"));
    }
    if let Some(name) = Channel::parse(&toolchain.name).and_then(|c| c.is_rolling()) {
        return Some(Advice::keep(format!("the {name} channel")));
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

/// What the projects' pins make of a toolchain rustup does not itself keep: kept when a
/// project needs it, recommended for removal when none does.
pub fn advice_from_pins(name: &str, pins: &[Pin]) -> Advice {
    let Some(needed_by) = needed_by(name, pins) else {
        return Advice::keep("a custom toolchain");
    };
    let name_of = |project: &Path| {
        project.file_name().map_or_else(
            || project.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    };
    match needed_by.len() {
        0 => Advice::remove("no project needs it"),
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
        let found = pins(&projects(root), root);
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

    #[test]
    fn stable_nightly_and_the_default_always_stay() {
        let toolchains = parse_list(LISTING);
        let advice = |name: &str| {
            let toolchain = toolchains.iter().find(|t| t.name == name).unwrap();
            standing_advice(toolchain, &[])
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
            standing_advice(&moved[0], &[]).unwrap().verdict,
            Verdict::Keep
        );
        assert_eq!(
            standing_advice(&moved[1], &[]).unwrap().verdict,
            Verdict::Protected
        );
        // A dated nightly is a pin like any other.
        let dated = Installed {
            name: "nightly-2026-01-15-aarch64-apple-darwin".into(),
            default: false,
        };
        assert_eq!(standing_advice(&dated, &[]), None);
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
        let advice = standing_advice(&toolchain, &overrides).unwrap();
        assert_eq!(advice.verdict, Verdict::Keep);
        assert_eq!(advice.reason, "override for /work/old");
        assert!(super::overrides("version = \"12\"\n[overrides]\n").is_empty());
    }

    #[test]
    fn toolchains_no_project_needs_are_recommended_for_removal() {
        let dir = tempfile::tempdir().unwrap();
        let found = pins(&projects(dir.path()), dir.path());
        let verdict = |name: &str| {
            let advice = advice_from_pins(name, &found);
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
                (Verdict::Remove, "no project needs it".into()),
                "{unused}"
            );
        }
        assert_eq!(verdict("mine").0, Verdict::Keep, "custom toolchains stay");
        assert_eq!(
            advice_from_pins("1.91-aarch64-apple-darwin", &[]).verdict,
            Verdict::Remove
        );
    }
}
