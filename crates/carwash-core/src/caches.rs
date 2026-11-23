//! Per-user caches outside any project: package stores, toolchains, IDE and SDK caches.
//!
//! Entries are data (`caches.toml`), resolved against the environment and platform. Each says
//! whether deleting the directory is safe and which command the tool offers to prune it.
//! Where a tool can describe its own entries (`simctl` for simulators), an adapter adds what
//! it reports: names, when an entry was last used, whether it is in use, and whether
//! anything still needs it.

use crate::Cancel;
use crate::cache::Fingerprint;
use crate::device_support::{self, Support};
use crate::model::{Size, unix_secs_opt};
use crate::scan::Engine;
use crate::simulators::{self, Simulator};
use crate::system::RunningApps;
use crate::tasks::Task;
use crate::toolchains::{self, Installed, Pin};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

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
    /// Command that empties one child and keeps it; `{name}` is the child's directory name.
    #[serde(default)]
    pub child_erase: Option<Vec<String>>,
    #[serde(default)]
    pub keep: Option<String>,
    /// Programs that must not be running while the cache is cleaned (process names, globs).
    #[serde(default)]
    pub apps: Vec<String>,
    /// Built-in knowledge about the cache's entries: `simulators`, `test-devices`,
    /// `device-support` or `rust-toolchains`.
    #[serde(default)]
    pub adapter: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Built-in knowledge about a cache's entries, selected by a spec's `adapter` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Adapter {
    /// Children are the simulators `simctl` lists, with their runtime, state and last use.
    Simulators,
    /// The clones Xcode keeps for parallel testing: left alone while one is booted.
    TestDevices,
    /// Children are device support folders, advised on by what `devicectl` says the
    /// paired devices run.
    DeviceSupport,
    /// Children are the toolchains rustup has installed. The default is never cleaned;
    /// `stable`, `nightly` and what projects pin are kept.
    RustToolchains,
}

impl Adapter {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "simulators" => Some(Self::Simulators),
            "test-devices" => Some(Self::TestDevices),
            "device-support" => Some(Self::DeviceSupport),
            "rust-toolchains" => Some(Self::RustToolchains),
            _ => None,
        }
    }
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
    for spec in &file.cache {
        if let Some(adapter) = &spec.adapter
            && Adapter::parse(adapter).is_none()
        {
            return Err(format!("cache {}: unknown adapter `{adapter}`", spec.id));
        }
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

/// Whether carwash cleans an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Recommended for removal: nothing needs it any more.
    Remove,
    /// Still needed. Left out when its parent is cleaned; cleaned only when named itself.
    Keep,
    /// carwash could not find out whether it is still needed, and says why. Treated like
    /// [`Verdict::Keep`]: what cannot be known is kept.
    Unknown,
    /// Never cleaned, even when named: it is in use.
    Protected,
}

/// What carwash makes of an entry, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Advice {
    pub verdict: Verdict,
    pub reason: String,
}

impl Advice {
    pub(crate) fn remove(reason: impl Into<String>) -> Self {
        Self {
            verdict: Verdict::Remove,
            reason: reason.into(),
        }
    }

    pub(crate) fn keep(reason: impl Into<String>) -> Self {
        Self {
            verdict: Verdict::Keep,
            reason: reason.into(),
        }
    }

    pub(crate) fn unknown(reason: impl Into<String>) -> Self {
        Self {
            verdict: Verdict::Unknown,
            reason: reason.into(),
        }
    }

    pub(crate) fn protected(reason: impl Into<String>) -> Self {
        Self {
            verdict: Verdict::Protected,
            reason: reason.into(),
        }
    }
}

/// What an entry is, where its tool describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Detail {
    Simulator {
        udid: String,
        /// The device's name, e.g. `iPhone 17 Pro`.
        device: String,
        /// The runtime it runs, e.g. `iOS 27.0`.
        runtime: String,
        /// `simctl`'s state in lower case: `shutdown`, `booted`...
        state: String,
    },
    /// Symbols Xcode copied from a device running one OS build.
    DeviceSupport {
        /// The device model, e.g. `iPad16,3`; folders from old versions of Xcode have none.
        model: Option<String>,
        os_version: String,
        build: String,
    },
    /// A Rust toolchain rustup installed.
    Toolchain {
        /// rustup's default toolchain.
        default: bool,
        /// The projects that pin it or declare it as their minimum version, once projects
        /// have been scanned. Not looked up for toolchains rustup itself keeps.
        needed_by: Option<Vec<PathBuf>>,
    },
    /// The simulator clones Xcode keeps for parallel testing.
    TestDevices {
        /// Clones in the set.
        devices: usize,
        /// Clones that are not shut down.
        booted: usize,
    },
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
    /// The tool's command that empties the entry and keeps it (a simulator's contents and
    /// settings).
    pub erase: Option<Vec<String>>,
    /// Programs that must not be running while it is cleaned (process names, globs).
    pub apps: Vec<String>,
    pub note: Option<String>,
    pub size: Option<Size>,
    /// When the entry was last used, where that is known.
    #[serde(with = "unix_secs_opt")]
    pub last_used: Option<SystemTime>,
    /// What carwash makes of the entry, where it can tell: recommended for removal, still
    /// needed, or in use.
    pub advice: Option<Advice>,
    pub detail: Option<Detail>,
    /// The directories whose sizes make up this entry's, when that is not all of `path`.
    #[serde(skip)]
    pub parts: Option<Vec<PathBuf>>,
    /// Taken at discovery, to tell whether a remembered size is still current.
    #[serde(skip)]
    pub fingerprint: Option<Fingerprint>,
}

/// How a cache is to be cleaned.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// With the tool's own command when it is installed, otherwise by deleting the directory
    /// when that is safe.
    #[default]
    Clean,
    /// By deleting the directory even when the tool's command is available.
    Delete,
    /// By emptying the entry and keeping it.
    Erase,
}

/// What cleaning a cache takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Run(Task),
    DeleteDirectory,
}

/// Why a cache is not cleaned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The entry is in use; see [`Advice`].
    #[error("{0}")]
    Protected(String),
    /// Programs that must be closed first are running.
    #[error("{}", running(.0))]
    Busy(Vec<String>),
    #[error("can only be cleaned with `{0}`, which is not installed")]
    ToolMissing(String),
    #[error("cannot be erased, only cleaned")]
    NotErasable,
    #[error("is cleaned with `{0}`; deleting its directory is not safe")]
    NotDeletable(String),
}

impl Refusal {
    /// The entry is left for now, as opposed to a request that can never work.
    pub fn is_temporary(&self) -> bool {
        matches!(self, Self::Protected(_) | Self::Busy(_))
    }
}

fn running(programs: &[String]) -> String {
    match programs {
        [one] => format!("{one} is running"),
        many => format!("{} are running", many.join(", ")),
    }
}

impl GlobalCache {
    fn task(&self, name: &str, argv: Option<&Vec<String>>) -> Option<Task> {
        let argv = argv?;
        let program = argv.first()?;
        find_program(program)?;
        let cwd = crate::paths::home().unwrap_or_else(|| PathBuf::from("/"));
        Some(Task {
            name: format!("{name} {}", self.id),
            source: "cache".into(),
            program: program.clone(),
            args: argv[1..].to_vec(),
            description: Some(self.name.clone()),
            cwd,
            standard: true,
        })
    }

    /// The prune command as a task, when its program is installed.
    pub fn prune_task(&self) -> Option<Task> {
        self.task("prune", self.prune.as_ref())
    }

    /// The erase command as a task, when the entry has one and its program is installed.
    pub fn erase_task(&self) -> Option<Task> {
        self.task("erase", self.erase.as_ref())
    }

    /// The verdict of the entry's [`Advice`], if it has one.
    pub fn verdict(&self) -> Option<Verdict> {
        self.advice.as_ref().map(|advice| advice.verdict)
    }

    /// The running programs among [`GlobalCache::apps`].
    pub fn blocked_by(&self, running: &RunningApps) -> Vec<String> {
        running.matching(&self.apps)
    }

    /// What cleaning this cache in `mode` takes, or why it is refused. Protection and
    /// running programs are checked first, so nothing is ever planned for an entry in use.
    pub fn plan(&self, mode: Mode, running: &RunningApps) -> Result<Step, Refusal> {
        if let Some(advice) = &self.advice
            && advice.verdict == Verdict::Protected
        {
            return Err(Refusal::Protected(advice.reason.clone()));
        }
        let blocked = self.blocked_by(running);
        if !blocked.is_empty() {
            return Err(Refusal::Busy(blocked));
        }
        let program = |argv: &Option<Vec<String>>| {
            argv.as_ref()
                .and_then(|argv| argv.first().cloned())
                .unwrap_or_default()
        };
        match mode {
            Mode::Erase => match (&self.erase, self.erase_task()) {
                (_, Some(task)) => Ok(Step::Run(task)),
                (Some(_), None) => Err(Refusal::ToolMissing(program(&self.erase))),
                (None, None) => Err(Refusal::NotErasable),
            },
            Mode::Delete if self.deletable => Ok(Step::DeleteDirectory),
            Mode::Delete => Err(Refusal::NotDeletable(program(&self.prune))),
            Mode::Clean => match self.prune_task() {
                Some(task) => Ok(Step::Run(task)),
                None if self.deletable => Ok(Step::DeleteDirectory),
                None => Err(Refusal::ToolMissing(program(&self.prune))),
            },
        }
    }

    /// The entry as it is now: what a tool reported at discovery (a simulator being shut
    /// down) is asked again, so a clean planned earlier cannot touch what came into use
    /// since. Entries no tool describes are returned unchanged.
    pub fn refreshed(&self, host: &dyn Host) -> Self {
        let mut cache = self.clone();
        match &self.detail {
            Some(Detail::Simulator { udid, .. }) => {
                let listed = host
                    .output(simulators::LIST)
                    .and_then(|json| simulators::parse(&json).ok());
                cache.advice = match listed {
                    // Not knowing is not the same as knowing it is shut down.
                    None => Some(Advice::protected(
                        "simctl could not be asked whether it is booted",
                    )),
                    Some(listed) => match listed.iter().find(|s| &s.udid == udid) {
                        Some(simulator) => simulator_advice(simulator),
                        None => Some(Advice::protected("no longer listed by simctl")),
                    },
                };
            }
            Some(Detail::TestDevices { .. }) => {
                let (detail, advice) = test_devices(host);
                cache.advice = advice;
                // Without an answer, what was counted before still describes the entry.
                if detail.is_some() {
                    cache.detail = detail;
                }
            }
            // Advice on what is still needed does not stand in the way of a clean.
            Some(Detail::DeviceSupport { .. } | Detail::Toolchain { .. }) | None => {}
        }
        cache
    }

    /// Measures the cache: all of `path`, or its `parts` when only some of it counts.
    pub fn measure(&self, engine: &Engine, cancel: &Cancel) -> Size {
        match &self.parts {
            None if self.path.exists() => engine.measure_path(&self.path, cancel),
            None => Size::default(),
            Some(parts) => {
                let mut total = Size::default();
                for part in parts {
                    total.merge(engine.measure_path(part, cancel));
                }
                total
            }
        }
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

/// What the machine's tools report about its caches. Discovery only ever reads through
/// this, and tests replace it with captured output.
pub trait Host: Sync {
    /// The standard output of a read-only command; `None` when its program is missing, it
    /// fails, or it does not finish in time.
    fn output(&self, argv: &[&str]) -> Option<Vec<u8>>;

    /// The current time, against which "connected recently" is judged.
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Remembers each answer for the length of one discovery, so a tool that several caches
/// ask about (`devicectl`, for iOS and watchOS device support) runs once.
struct Once<'a> {
    host: &'a dyn Host,
    answers: Mutex<HashMap<Vec<String>, Option<Vec<u8>>>>,
}

impl Host for Once<'_> {
    fn output(&self, argv: &[&str]) -> Option<Vec<u8>> {
        let key: Vec<String> = argv.iter().map(|arg| (*arg).to_owned()).collect();
        if let Some(answer) = self.answers.lock().expect("answers").get(&key) {
            return answer.clone();
        }
        let answer = self.host.output(argv);
        self.answers
            .lock()
            .expect("answers")
            .insert(key, answer.clone());
        answer
    }

    fn now(&self) -> SystemTime {
        self.host.now()
    }
}

/// The machine carwash runs on.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalHost;

impl LocalHost {
    /// Long enough for a tool that has to start its service first.
    const TIMEOUT: Duration = Duration::from_secs(20);
}

impl Host for LocalHost {
    fn output(&self, argv: &[&str]) -> Option<Vec<u8>> {
        let (program, args) = argv.split_first()?;
        find_program(program)?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let deadline = Instant::now() + Self::TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let bytes = reader.join().ok()?.ok()?;
        status?.success().then_some(bytes)
    }
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
pub(crate) fn resolve(
    candidate: &str,
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<PathBuf> {
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
    discover_on(specs, home, &LocalHost)
}

/// [`discover`], asking `host` what the machine's tools report.
pub fn discover_on(specs: &[CacheSpec], home: &Path, host: &dyn Host) -> Vec<GlobalCache> {
    discover_for(specs, home, host, None)
}

/// [`discover_on`] for the caches `ids` name (an id, or the parent of one), when only
/// those are wanted. The tools behind the other caches are not asked anything, so cleaning
/// one cache never waits on a tool that describes another.
pub fn discover_for(
    specs: &[CacheSpec],
    home: &Path,
    host: &dyn Host,
    ids: Option<&[String]>,
) -> Vec<GlobalCache> {
    discover_with(
        &wanted(specs, ids),
        home,
        &|var| std::env::var(var).ok(),
        host,
    )
}

/// The specs `ids` name, each by its own id or by the id of one of its children; all of
/// them without `ids`.
fn wanted(specs: &[CacheSpec], ids: Option<&[String]>) -> Vec<CacheSpec> {
    specs
        .iter()
        .filter(|spec| {
            ids.is_none_or(|ids| {
                ids.iter()
                    .any(|id| id == &spec.id || id.starts_with(&format!("{}/", spec.id)))
            })
        })
        .cloned()
        .collect()
}

/// `argv` with `{name}` replaced by a child's name.
fn for_child(argv: Option<&Vec<String>>, child: &str) -> Option<Vec<String>> {
    argv.map(|argv| argv.iter().map(|a| a.replace("{name}", child)).collect())
}

/// The entry for a cache as a whole.
fn whole(spec: &CacheSpec, path: PathBuf) -> GlobalCache {
    GlobalCache {
        id: spec.id.clone(),
        name: spec.name.clone(),
        fingerprint: Fingerprint::of(&path),
        path,
        ecosystem: spec.ecosystem.clone(),
        deletable: spec.delete,
        prune: spec.prune.clone(),
        erase: None,
        apps: spec.apps.clone(),
        note: spec.note.clone(),
        size: None,
        last_used: None,
        advice: None,
        detail: None,
        parts: None,
    }
}

/// The entry for one subdirectory of a cache.
fn child(spec: &CacheSpec, name: &str, path: PathBuf) -> GlobalCache {
    GlobalCache {
        id: format!("{}/{name}", spec.id),
        name: format!("{}: {name}", spec.name),
        prune: for_child(spec.child_prune.as_ref(), name),
        erase: for_child(spec.child_erase.as_ref(), name),
        ..whole(spec, path)
    }
}

fn discover_with(
    specs: &[CacheSpec],
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    host: &dyn Host,
) -> Vec<GlobalCache> {
    let host: &dyn Host = &Once {
        host,
        answers: Mutex::default(),
    };
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
        match spec.adapter.as_deref().and_then(Adapter::parse) {
            Some(Adapter::Simulators) => out.extend(simulator_entries(spec, path, host)),
            Some(Adapter::DeviceSupport) => {
                out.extend(device_support_entries(spec, &path, host));
            }
            Some(Adapter::RustToolchains) => out.extend(toolchain_entries(spec, &path, host)),
            Some(Adapter::TestDevices) => {
                let (detail, advice) = test_devices(host);
                out.push(GlobalCache {
                    detail,
                    advice,
                    ..whole(spec, path)
                });
            }
            None if spec.children => {
                let keep = match spec.keep.as_deref() {
                    Some("rustup-default") => rustup_default(&path),
                    _ => None,
                };
                for (name, child_path) in subdirectories(&path) {
                    if keep
                        .as_ref()
                        .is_none_or(|k| &name != k && !name.starts_with(&format!("{k}-")))
                    {
                        out.push(child(spec, &name, child_path));
                    }
                }
            }
            None => out.push(whole(spec, path)),
        }
    }
    out
}

/// The directories directly inside `path`, by name. Hidden ones are left out.
fn subdirectories(path: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok().map(|n| (n, e.path())))
        .filter(|(name, _)| !name.starts_with('.'))
        .collect();
    found.sort();
    found
}

/// One entry per toolchain rustup has installed, with what rustup itself says must stay:
/// the default (never cleaned), `stable` and `nightly`, and directory overrides. What
/// projects need is added by [`apply_pins`] once they have been scanned.
fn toolchain_entries(spec: &CacheSpec, path: &Path, host: &dyn Host) -> Vec<GlobalCache> {
    let settings = path
        .parent()
        .and_then(|rustup| std::fs::read_to_string(rustup.join("settings.toml")).ok())
        .unwrap_or_default();
    let overrides = toolchains::overrides(&settings);
    let listed = host
        .output(toolchains::LIST)
        .and_then(|output| String::from_utf8(output).ok())
        .map(|output| toolchains::parse_list(&output))
        .filter(|listed| !listed.is_empty());
    // Without rustup's own list, the directories and its settings say the same.
    let installed = listed.unwrap_or_else(|| {
        let default = rustup_default(path);
        subdirectories(path)
            .into_iter()
            .filter(|(name, _)| toolchains::is_name(name))
            .map(|(name, _)| Installed {
                default: default
                    .as_ref()
                    .is_some_and(|d| &name == d || name.starts_with(&format!("{d}-"))),
                name,
            })
            .collect()
    });
    installed
        .iter()
        // Linked toolchains are symlinks to someone's build directory: not rustup's to size
        // or remove with the rest.
        .filter(|toolchain| {
            std::fs::symlink_metadata(path.join(&toolchain.name)).is_ok_and(|m| m.is_dir())
        })
        .map(|toolchain| GlobalCache {
            advice: toolchains::standing_advice(toolchain, &overrides),
            detail: Some(Detail::Toolchain {
                default: toolchain.default,
                needed_by: None,
            }),
            ..child(spec, &toolchain.name, path.join(&toolchain.name))
        })
        .collect()
}

/// Applies what scanned projects need to the Rust toolchains among `caches`: a toolchain a
/// project pins or declares as its minimum version is kept, and one no project needs is
/// recommended for removal. Toolchains rustup itself keeps (the default, `stable`,
/// `nightly`, directory overrides) are left as they are.
pub fn apply_pins(caches: &mut [GlobalCache], pins: &[Pin]) {
    for cache in caches {
        let Some(Detail::Toolchain { needed_by, .. }) = &mut cache.detail else {
            continue;
        };
        // Standing advice was given without looking at projects.
        if cache.advice.is_some() && needed_by.is_none() {
            continue;
        }
        let name = cache.id.rsplit('/').next().unwrap_or_default();
        *needed_by = Some(toolchains::needed_by(name, pins).unwrap_or_default());
        cache.advice = Some(toolchains::advice_from_pins(name, pins));
    }
}

/// One entry per device support folder, with what the paired devices make of it: kept
/// when a device runs that build or it is the newest for its model, recommended for
/// removal when a newer folder for the same model has replaced it and the devices could
/// be checked.
fn device_support_entries(spec: &CacheSpec, path: &Path, host: &dyn Host) -> Vec<GlobalCache> {
    let folders = subdirectories(path);
    // `None` when devicectl is missing, fails, times out or prints something else: then
    // nobody knows which folders devices still use, and none is recommended for removal.
    let devices = host
        .output(device_support::LIST)
        .and_then(|json| device_support::parse(&json).ok());
    let names: Vec<&str> = folders.iter().map(|(name, _)| name.as_str()).collect();
    let advice = device_support::advise(&names, devices.as_deref(), host.now());
    folders
        .iter()
        .zip(advice)
        .map(|((name, folder), advice)| GlobalCache {
            advice,
            detail: Support::parse(name).map(|support| Detail::DeviceSupport {
                os_version: support.version_text(),
                model: support.model,
                build: support.build,
            }),
            ..child(spec, name, folder.clone())
        })
        .collect()
}

/// One entry per simulator `simctl` lists in the device set at `devices`, and one for the
/// devices whose runtime is gone, which the set's own command removes together.
fn simulator_entries(spec: &CacheSpec, devices: PathBuf, host: &dyn Host) -> Vec<GlobalCache> {
    let listed = host
        .output(simulators::LIST)
        .and_then(|json| simulators::parse(&json).ok());
    let Some(mut listed) = listed else {
        // Without simctl's view, the device set is one entry.
        return vec![whole(spec, devices)];
    };
    listed.sort_by(|a, b| (&a.name, &a.runtime, &a.udid).cmp(&(&b.name, &b.runtime, &b.udid)));
    let mut out = Vec::new();
    let mut unavailable = Vec::new();
    for simulator in listed {
        let device = devices.join(&simulator.udid);
        if !device.is_dir() {
            continue;
        }
        if !simulator.available {
            unavailable.push(device);
            continue;
        }
        out.push(GlobalCache {
            name: format!("{}: {}, {}", spec.name, simulator.name, simulator.runtime),
            last_used: simulator
                .last_used
                .or_else(|| modified(&device.join("data"))),
            advice: simulator_advice(&simulator),
            detail: Some(Detail::Simulator {
                udid: simulator.udid.clone(),
                device: simulator.name.clone(),
                runtime: simulator.runtime.clone(),
                state: simulator.state.to_lowercase(),
            }),
            ..child(spec, &simulator.udid, device)
        });
    }
    out.push(GlobalCache {
        name: format!("{}: unavailable devices", spec.name),
        parts: Some(unavailable),
        ..whole(spec, devices)
    });
    out
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .ok()
}

/// A simulator that is not shut down is in use and never touched.
fn simulator_advice(simulator: &Simulator) -> Option<Advice> {
    (!simulator.is_shut_down()).then(|| {
        Advice::protected(format!(
            "{}; shut it down first",
            simulator.state.to_lowercase()
        ))
    })
}

/// The test clones, which are left alone while any of them is not shut down, and also
/// when `simctl` gives no answer (it is missing, fails, times out or prints something
/// else): not knowing is not the same as knowing that none is booted.
fn test_devices(host: &dyn Host) -> (Option<Detail>, Option<Advice>) {
    let Some(listed) = host
        .output(simulators::LIST_TESTING)
        .and_then(|json| simulators::parse(&json).ok())
    else {
        let unknown = Advice::protected("simctl could not be asked whether a test clone is booted");
        return (None, Some(unknown));
    };
    let booted = listed.iter().filter(|s| !s.is_shut_down()).count();
    let advice = (booted > 0).then(|| {
        Advice::protected(match booted {
            1 => "a test clone is booted".to_owned(),
            n => format!("{n} test clones are booted"),
        })
    });
    let detail = Detail::TestDevices {
        devices: listed.len(),
        booted,
    };
    (Some(detail), advice)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::simulators::tests::{LISTING, TESTING_LISTING};
    use std::fs;

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    /// Answers tool invocations with captured output; anything else is "not installed".
    pub(crate) struct Tools(pub Vec<(&'static [&'static str], String)>);

    impl Tools {
        pub(crate) fn none() -> Self {
            Self(Vec::new())
        }
    }

    impl Host for Tools {
        fn output(&self, argv: &[&str]) -> Option<Vec<u8>> {
            self.0
                .iter()
                .find(|(command, _)| *command == argv)
                .map(|(_, output)| output.clone().into_bytes())
        }

        /// The day after the listings were captured.
        fn now(&self) -> SystemTime {
            device_support::tests::now()
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

        let caches = discover_with(&builtin_specs(), root, &env(&[]), &Tools::none());
        let ids: Vec<&str> = caches.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"npm"));
        assert!(ids.contains(&"rustup-toolchains/nightly-aarch64-apple-darwin"));
        assert!(ids.contains(&"rustup-toolchains/1.80-aarch64-apple-darwin"));
        let default = caches
            .iter()
            .find(|c| c.id == "rustup-toolchains/stable-aarch64-apple-darwin")
            .expect("the default toolchain is listed");
        assert_eq!(default.verdict(), Some(Verdict::Protected));
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
        assert!(
            specs_with_overrides(
                "schema = 1\n[[cache]]\nid = \"x\"\nname = \"X\"\npaths = [\"~/x\"]\ndelete = true\nadapter = \"nope\"\n"
            )
            .is_err(),
            "unknown adapter"
        );
    }

    const DEVICES: &str = "Library/Developer/CoreSimulator/Devices";
    const TEST_DEVICES: &str = "Library/Developer/XCTestDevices";
    const SHUT_DOWN: &str = "00000001-0000-4000-8000-000000000001";
    const BOOTED: &str = "0000000A-0000-4000-8000-00000000000A";
    const UNAVAILABLE: &str = "00000384-0000-4000-8000-000000000384";

    /// The built-in simulator caches, on every platform.
    pub(crate) fn simulator_specs() -> Vec<CacheSpec> {
        builtin_specs()
            .into_iter()
            .filter(|s| matches!(s.id.as_str(), "simulators" | "xctest-devices"))
            .map(|mut spec| {
                for path in &mut spec.paths {
                    *path = path.trim_start_matches("macos:").to_owned();
                }
                spec
            })
            .collect()
    }

    /// A home directory holding a folder for every simulator in the captured listing.
    pub(crate) fn simulator_home() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        for simulator in simulators::parse(LISTING.as_bytes()).unwrap() {
            fs::create_dir_all(home.path().join(DEVICES).join(simulator.udid).join("data"))
                .unwrap();
        }
        fs::create_dir_all(home.path().join(TEST_DEVICES)).unwrap();
        home
    }

    pub(crate) fn simctl(listing: &str, testing: &str) -> Tools {
        Tools(vec![
            (simulators::LIST, listing.to_owned()),
            (simulators::LIST_TESTING, testing.to_owned()),
        ])
    }

    fn simulator_caches(home: &Path, host: &Tools) -> Vec<GlobalCache> {
        discover_with(&simulator_specs(), home, &env(&[]), host)
    }

    fn find<'a>(caches: &'a [GlobalCache], id: &str) -> &'a GlobalCache {
        caches
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("no cache {id}"))
    }

    fn idle() -> RunningApps {
        RunningApps::from_names(&["Finder", "zsh"])
    }

    const MODES: [Mode; 3] = [Mode::Clean, Mode::Delete, Mode::Erase];

    #[test]
    fn lists_each_simulator_with_its_runtime_state_and_commands() {
        let home = simulator_home();
        let caches = simulator_caches(home.path(), &simctl(LISTING, TESTING_LISTING));
        let devices: Vec<&GlobalCache> = caches
            .iter()
            .filter(|c| c.id.starts_with("simulators/"))
            .collect();
        assert_eq!(devices.len(), 63, "every available device");

        let device = find(&caches, &format!("simulators/{SHUT_DOWN}"));
        assert_eq!(device.name, "Simulators: iPhone 18 Pro, iOS 27.0");
        assert_eq!(device.path, home.path().join(DEVICES).join(SHUT_DOWN));
        assert_eq!(
            device.prune.as_deref().unwrap(),
            ["xcrun", "simctl", "delete", SHUT_DOWN]
        );
        assert_eq!(
            device.erase.as_deref().unwrap(),
            ["xcrun", "simctl", "erase", SHUT_DOWN]
        );
        assert!(!device.deletable, "device folders belong to CoreSimulator");
        assert_eq!(device.apps, ["xcodebuild", "xctest"]);
        assert!(device.advice.is_none());
        assert_eq!(
            device.detail,
            Some(Detail::Simulator {
                udid: SHUT_DOWN.into(),
                device: "iPhone 18 Pro".into(),
                runtime: "iOS 27.0".into(),
                state: "shutdown".into(),
            })
        );
        assert!(
            devices.iter().all(|c| c.last_used.is_some()),
            "simctl's record, or the data folder's date for a device never booted"
        );
    }

    #[test]
    fn unavailable_simulators_are_one_entry_cleaned_by_simctl() {
        let home = simulator_home();
        let caches = simulator_caches(home.path(), &simctl(LISTING, TESTING_LISTING));
        assert!(
            !caches.iter().any(|c| c.id.ends_with(UNAVAILABLE)),
            "not listed on its own"
        );
        let set = find(&caches, "simulators");
        assert_eq!(set.name, "Simulators: unavailable devices");
        assert_eq!(
            set.prune.as_deref().unwrap(),
            ["xcrun", "simctl", "delete", "unavailable"]
        );
        assert_eq!(
            set.parts.as_deref().unwrap(),
            [home.path().join(DEVICES).join(UNAVAILABLE)]
        );
        assert!(set.erase.is_none());
    }

    #[test]
    fn a_booted_simulator_is_never_cleaned() {
        let home = simulator_home();
        let caches = simulator_caches(home.path(), &simctl(LISTING, TESTING_LISTING));
        let booted: Vec<&GlobalCache> = caches.iter().filter(|c| c.advice.is_some()).collect();
        assert_eq!(booted.len(), 3);
        let device = find(&caches, &format!("simulators/{BOOTED}"));
        for mode in MODES {
            assert_eq!(
                device.plan(mode, &idle()),
                Err(Refusal::Protected("booted; shut it down first".into())),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn a_running_build_or_test_blocks_every_simulator_action() {
        let home = simulator_home();
        let caches = simulator_caches(home.path(), &simctl(LISTING, TESTING_LISTING));
        assert!(caches.len() > 60);
        for (names, blocked) in [
            (&["xcodebuild", "zsh"][..], vec!["xcodebuild".to_owned()]),
            (&["xctest"][..], vec!["xctest".to_owned()]),
            (
                &["XCTest", "xcodebuild"][..],
                vec!["xcodebuild".to_owned(), "xctest".to_owned()],
            ),
        ] {
            let running = RunningApps::from_names(names);
            for cache in caches.iter().filter(|c| c.advice.is_none()) {
                for mode in MODES {
                    assert_eq!(
                        cache.plan(mode, &running),
                        Err(Refusal::Busy(blocked.clone())),
                        "{} {mode:?}",
                        cache.id
                    );
                }
            }
        }
        assert_eq!(
            Refusal::Busy(vec!["xcodebuild".into()]).to_string(),
            "xcodebuild is running"
        );
        assert_eq!(
            Refusal::Busy(vec!["xcodebuild".into(), "xctest".into()]).to_string(),
            "xcodebuild, xctest are running"
        );
    }

    #[test]
    fn a_simulator_booted_after_discovery_is_refused_at_cleaning_time() {
        let home = simulator_home();
        let caches = simulator_caches(home.path(), &simctl(LISTING, TESTING_LISTING));
        let device = find(&caches, &format!("simulators/{SHUT_DOWN}"));
        assert!(device.advice.is_none());

        let now_booted = LISTING.replacen("\"state\": \"Shutdown\"", "\"state\": \"Booted\"", 64);
        let fresh = device.refreshed(&simctl(&now_booted, TESTING_LISTING));
        assert!(matches!(
            fresh.plan(Mode::Erase, &idle()),
            Err(Refusal::Protected(_))
        ));
        // A device simctl no longer lists, or a simctl that cannot be asked, is left alone.
        let gone = device.refreshed(&simctl("{\"devices\":{}}", TESTING_LISTING));
        assert_eq!(
            gone.plan(Mode::Clean, &idle()),
            Err(Refusal::Protected("no longer listed by simctl".into()))
        );
        for silent in [
            Tools::none(),
            simctl(
                "CoreSimulatorService connection became invalid",
                TESTING_LISTING,
            ),
        ] {
            let unknown = device.refreshed(&silent);
            assert_eq!(
                unknown.plan(Mode::Clean, &idle()),
                Err(Refusal::Protected(
                    "simctl could not be asked whether it is booted".into()
                ))
            );
        }
        // Unchanged: still free to clean.
        let same = device.refreshed(&simctl(LISTING, TESTING_LISTING));
        assert!(same.advice.is_none());
    }

    #[test]
    fn without_simctl_the_device_set_is_one_entry() {
        let home = simulator_home();
        let caches = simulator_caches(home.path(), &Tools::none());
        let ids: Vec<&str> = caches.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["simulators", "xctest-devices"]);
        let set = find(&caches, "simulators");
        assert_eq!(set.name, "Simulators");
        assert!(set.parts.is_none(), "measured whole");
    }

    /// Not knowing whether a clone is booted must protect the clones like knowing one is.
    #[test]
    fn test_clones_stay_protected_when_simctl_cannot_answer() {
        const UNKNOWN: &str = "simctl could not be asked whether a test clone is booted";
        let home = simulator_home();
        // Discovery sees three booted clones: protected.
        let busy = simulator_caches(home.path(), &simctl(LISTING, LISTING));
        let clones = find(&busy, "xctest-devices");
        assert_eq!(
            clones.plan(Mode::Clean, &idle()),
            Err(Refusal::Protected("3 test clones are booted".into()))
        );
        // Just before cleaning, simctl is missing, fails or times out; or it answers with
        // something that is not a device list.
        for silent in [
            Tools::none(),
            simctl(LISTING, "CoreSimulatorService connection became invalid"),
            simctl(LISTING, "{\"devices\": 3}"),
        ] {
            let later = clones.refreshed(&silent);
            assert_eq!(
                later.plan(Mode::Clean, &idle()),
                Err(Refusal::Protected(UNKNOWN.into())),
            );
            assert!(later.detail.is_some(), "what was counted is kept");
        }
        // The same when simctl was already silent at discovery, and it stays that way
        // until simctl answers.
        let unknown = simulator_caches(home.path(), &Tools::none());
        let clones = find(&unknown, "xctest-devices");
        assert_eq!(
            clones.plan(Mode::Clean, &idle()),
            Err(Refusal::Protected(UNKNOWN.into()))
        );
        assert_eq!(
            clones.refreshed(&Tools::none()).plan(Mode::Clean, &idle()),
            Err(Refusal::Protected(UNKNOWN.into()))
        );
    }

    #[test]
    fn test_clones_are_cleaned_through_simctl_unless_one_is_booted() {
        let home = simulator_home();
        let caches = simulator_caches(home.path(), &simctl(LISTING, TESTING_LISTING));
        let clones = find(&caches, "xctest-devices");
        assert_eq!(clones.path, home.path().join(TEST_DEVICES));
        assert_eq!(
            clones.prune.as_deref().unwrap(),
            ["xcrun", "simctl", "--set", "testing", "delete", "all"]
        );
        assert!(!clones.deletable);
        assert_eq!(clones.apps, ["xcodebuild", "xctest"]);
        assert_eq!(
            clones.detail,
            Some(Detail::TestDevices {
                devices: 0,
                booted: 0
            })
        );
        assert!(clones.advice.is_none());
        assert_eq!(
            clones.plan(Mode::Clean, &RunningApps::from_names(&["xcodebuild"])),
            Err(Refusal::Busy(vec!["xcodebuild".into()]))
        );

        // The default set's listing has the shape of a test set with booted clones.
        let busy = simulator_caches(home.path(), &simctl(LISTING, LISTING));
        let clones = find(&busy, "xctest-devices");
        assert_eq!(
            clones.plan(Mode::Clean, &idle()),
            Err(Refusal::Protected("3 test clones are booted".into()))
        );
        let later = clones.refreshed(&simctl(LISTING, TESTING_LISTING));
        assert!(later.advice.is_none(), "asked again before cleaning");
    }

    const SUPPORT: &str = "Library/Developer/Xcode/iOS DeviceSupport";

    fn device_support_caches(host: &Tools) -> (tempfile::TempDir, Vec<GlobalCache>) {
        let home = tempfile::tempdir().unwrap();
        for folder in device_support::tests::FOLDERS {
            fs::create_dir_all(home.path().join(SUPPORT).join(folder)).unwrap();
        }
        // A half-removed folder from an interrupted clean is not a device support version.
        fs::create_dir_all(home.path().join(SUPPORT).join(".carwash-trash-1-0-old")).unwrap();
        let specs: Vec<CacheSpec> = builtin_specs()
            .into_iter()
            .filter(|s| s.id == "xcode-device-support")
            .map(|mut spec| {
                for path in &mut spec.paths {
                    *path = path.trim_start_matches("macos:").to_owned();
                }
                spec
            })
            .collect();
        let caches = discover_with(&specs, home.path(), &env(&[]), host);
        (home, caches)
    }

    fn verdicts(caches: &[GlobalCache]) -> Vec<(&str, Option<Verdict>)> {
        caches
            .iter()
            .map(|c| (c.id.rsplit('/').next().unwrap(), c.verdict()))
            .collect()
    }

    #[test]
    fn device_support_is_kept_for_connected_devices_and_recommended_when_superseded() {
        let devicectl = Tools(vec![(
            device_support::LIST,
            device_support::tests::LISTING.to_owned(),
        )]);
        let (_home, caches) = device_support_caches(&devicectl);
        assert_eq!(
            verdicts(&caches),
            [
                ("iPad16,3 26.7 (23H24)", Some(Verdict::Remove)),
                ("iPad16,3 27.2 (24B119)", Some(Verdict::Remove)),
                ("iPad16,3 27.3 (24C5037i)", Some(Verdict::Keep)),
                ("iPhone14,3 27.0.1 (24A446)", Some(Verdict::Remove)),
                ("iPhone14,3 27.2.1 (24B156)", Some(Verdict::Keep)),
            ]
        );
        let newest = find(&caches, "xcode-device-support/iPad16,3 27.3 (24C5037i)");
        assert_eq!(newest.advice.as_ref().unwrap().reason, "Test iPad runs it");
        assert_eq!(
            newest.detail,
            Some(Detail::DeviceSupport {
                model: Some("iPad16,3".into()),
                os_version: "27.3".into(),
                build: "24C5037i".into(),
            })
        );
        assert!(newest.deletable);
        // Advice never stands in the way of an explicit clean; only use does.
        for cache in &caches {
            assert_eq!(cache.plan(Mode::Clean, &idle()), Ok(Step::DeleteDirectory));
        }
    }

    #[test]
    fn without_devicectl_no_device_support_is_recommended_for_removal() {
        for silent in [
            Tools::none(),
            Tools(vec![(device_support::LIST, "error: timed out".to_owned())]),
        ] {
            let (_home, caches) = device_support_caches(&silent);
            assert_eq!(
                verdicts(&caches),
                [
                    ("iPad16,3 26.7 (23H24)", Some(Verdict::Unknown)),
                    ("iPad16,3 27.2 (24B119)", Some(Verdict::Unknown)),
                    ("iPad16,3 27.3 (24C5037i)", Some(Verdict::Keep)),
                    ("iPhone14,3 27.0.1 (24A446)", Some(Verdict::Unknown)),
                    ("iPhone14,3 27.2.1 (24B156)", Some(Verdict::Keep)),
                ]
            );
            assert_eq!(
                find(&caches, "xcode-device-support/iPad16,3 27.2 (24B119)")
                    .advice
                    .as_ref()
                    .unwrap()
                    .reason,
                "connected devices could not be checked"
            );
        }
    }

    /// Counts how often each tool is asked.
    struct Counting<'a> {
        tools: &'a Tools,
        asked: Mutex<Vec<String>>,
    }

    impl Host for Counting<'_> {
        fn output(&self, argv: &[&str]) -> Option<Vec<u8>> {
            self.asked.lock().unwrap().push(argv[..2].join(" "));
            self.tools.output(argv)
        }

        fn now(&self) -> SystemTime {
            self.tools.now()
        }
    }

    #[test]
    fn tools_are_asked_once_and_only_for_the_caches_wanted() {
        let home = simulator_home();
        for dir in [
            "Library/Developer/Xcode/iOS DeviceSupport",
            "Library/Developer/Xcode/watchOS DeviceSupport",
            ".npm/_cacache",
        ] {
            fs::create_dir_all(home.path().join(dir).join("x")).unwrap();
        }
        let specs: Vec<CacheSpec> = builtin_specs()
            .into_iter()
            .map(|mut spec| {
                for path in &mut spec.paths {
                    *path = path.trim_start_matches("macos:").to_owned();
                }
                spec
            })
            .collect();
        let tools = simctl(LISTING, TESTING_LISTING);
        let asked = |ids: Option<&[String]>| {
            let host = Counting {
                tools: &tools,
                asked: Mutex::default(),
            };
            let caches = discover_with(&wanted(&specs, ids), home.path(), &env(&[]), &host);
            let mut asked = host.asked.into_inner().unwrap();
            asked.sort();
            (caches, asked)
        };
        // Listing everything asks devicectl once for both device support caches.
        let (_, everything) = asked(None);
        assert_eq!(
            everything
                .iter()
                .filter(|a| *a == "xcrun devicectl")
                .count(),
            1,
            "{everything:?}"
        );
        // Cleaning npm asks nothing of anyone.
        let npm = ["npm".to_owned()];
        let (caches, nothing) = asked(Some(&npm));
        assert_eq!(caches.len(), 1);
        assert_eq!(caches[0].id, "npm");
        assert!(nothing.is_empty(), "{nothing:?}");
        // A child id is enough to want its parent's listing, and nothing else.
        let one = [format!("simulators/{SHUT_DOWN}")];
        let (caches, simctl_only) = asked(Some(&one));
        assert!(caches.iter().any(|c| c.id == one[0]));
        assert_eq!(simctl_only, ["xcrun simctl"]);
    }

    const TOOLCHAINS: &str = ".rustup/toolchains";

    /// A rustup home holding every toolchain in the captured list, and one linked build.
    fn rustup_home(settings: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let toolchains = home.path().join(TOOLCHAINS);
        for toolchain in toolchains::parse_list(toolchains::tests::LISTING) {
            fs::create_dir_all(toolchains.join(toolchain.name)).unwrap();
        }
        fs::write(home.path().join(".rustup/settings.toml"), settings).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(home.path(), toolchains.join("stage1")).unwrap();
        home
    }

    fn rustup(listing: &str) -> Tools {
        Tools(vec![(toolchains::LIST, listing.to_owned())])
    }

    fn toolchain_caches(home: &Path, host: &Tools) -> Vec<GlobalCache> {
        let specs: Vec<CacheSpec> = builtin_specs()
            .into_iter()
            .filter(|s| s.id == "rustup-toolchains")
            .collect();
        discover_with(&specs, home, &env(&[]), host)
    }

    const SETTINGS: &str = "version = \"12\"\ndefault_toolchain = \"stable-aarch64-apple-darwin\"\nprofile = \"default\"\n\n[overrides]\n";

    #[test]
    fn lists_installed_toolchains_and_always_keeps_stable_nightly_and_the_default() {
        let home = rustup_home(SETTINGS);
        let listing = format!("{}stage1\n", toolchains::tests::LISTING);
        let caches = toolchain_caches(home.path(), &rustup(&listing));
        assert_eq!(
            caches.len(),
            18,
            "the linked toolchain is not rustup's to remove"
        );

        let default = find(&caches, "rustup-toolchains/stable-aarch64-apple-darwin");
        assert_eq!(default.verdict(), Some(Verdict::Protected));
        for mode in MODES {
            assert_eq!(
                default.plan(mode, &idle()),
                Err(Refusal::Protected("the default toolchain".into())),
                "{mode:?}"
            );
        }
        let nightly = find(&caches, "rustup-toolchains/nightly-aarch64-apple-darwin");
        assert_eq!(nightly.verdict(), Some(Verdict::Keep));
        assert_eq!(
            nightly.advice.as_ref().unwrap().reason,
            "the nightly channel"
        );

        let release = find(&caches, "rustup-toolchains/1.88-aarch64-apple-darwin");
        assert_eq!(release.advice, None, "nothing is known about projects yet");
        assert_eq!(
            release.prune.as_deref().unwrap(),
            [
                "rustup",
                "toolchain",
                "uninstall",
                "1.88-aarch64-apple-darwin"
            ]
        );
        assert!(!release.deletable, "only rustup removes a toolchain");
        assert_eq!(
            release.detail,
            Some(Detail::Toolchain {
                default: false,
                needed_by: None
            })
        );
    }

    #[test]
    fn toolchains_no_project_needs_are_recommended_once_projects_are_known() {
        let home = rustup_home(SETTINGS);
        let mut caches = toolchain_caches(home.path(), &rustup(toolchains::tests::LISTING));
        let work = tempfile::tempdir().unwrap();
        let pins = toolchains::pins(&toolchains::tests::projects(work.path()), work.path());
        // Applying twice changes nothing more.
        apply_pins(&mut caches, &pins);
        apply_pins(&mut caches, &pins);

        let verdict = |name: &str| find(&caches, &format!("rustup-toolchains/{name}")).verdict();
        // What the projects pin (1.98.1, 1.90.0, 1.99) or declare as rust-version (1.95, 1.88).
        for needed in [
            "1.88-aarch64-apple-darwin",
            "1.90-aarch64-apple-darwin",
            "1.90.0-aarch64-apple-darwin",
            "1.95-aarch64-apple-darwin",
            "1.95.0-aarch64-apple-darwin",
            "1.98.1-aarch64-apple-darwin",
            "1.99-aarch64-apple-darwin",
            "1.99.0-aarch64-apple-darwin",
            "1.99.1-aarch64-apple-darwin",
        ] {
            assert_eq!(verdict(needed), Some(Verdict::Keep), "{needed}");
        }
        for unused in [
            "1.89-aarch64-apple-darwin",
            "1.91-aarch64-apple-darwin",
            "1.91.0-aarch64-apple-darwin",
            "1.94-aarch64-apple-darwin",
            "1.97-aarch64-apple-darwin",
            "1.98.0-aarch64-apple-darwin",
            "1.100.0-aarch64-apple-darwin",
        ] {
            assert_eq!(verdict(unused), Some(Verdict::Remove), "{unused}");
        }
        // Whatever projects say, these stay as they were.
        assert_eq!(
            verdict("stable-aarch64-apple-darwin"),
            Some(Verdict::Protected)
        );
        assert_eq!(verdict("nightly-aarch64-apple-darwin"), Some(Verdict::Keep));
        let pinned = find(&caches, "rustup-toolchains/1.98.1-aarch64-apple-darwin");
        assert_eq!(
            pinned.detail,
            Some(Detail::Toolchain {
                default: false,
                needed_by: Some(vec![work.path().join("pinned")]),
            })
        );
    }

    #[test]
    fn a_directory_override_keeps_its_toolchain() {
        let settings = format!("{SETTINGS}\"/work/old\" = \"1.89-aarch64-apple-darwin\"\n");
        let home = rustup_home(&settings);
        let mut caches = toolchain_caches(home.path(), &rustup(toolchains::tests::LISTING));
        apply_pins(&mut caches, &[]);
        let overridden = find(&caches, "rustup-toolchains/1.89-aarch64-apple-darwin");
        assert_eq!(overridden.verdict(), Some(Verdict::Keep));
        assert_eq!(
            overridden.advice.as_ref().unwrap().reason,
            "override for /work/old"
        );
        assert_eq!(
            find(&caches, "rustup-toolchains/1.91-aarch64-apple-darwin").verdict(),
            Some(Verdict::Remove)
        );
    }

    #[test]
    fn without_rustup_the_toolchain_directories_and_settings_are_read() {
        let home = rustup_home(SETTINGS);
        let caches = toolchain_caches(home.path(), &Tools::none());
        assert_eq!(caches.len(), 18);
        assert_eq!(
            find(&caches, "rustup-toolchains/stable-aarch64-apple-darwin").verdict(),
            Some(Verdict::Protected)
        );
        assert_eq!(
            find(&caches, "rustup-toolchains/nightly-aarch64-apple-darwin").verdict(),
            Some(Verdict::Keep)
        );
    }

    #[test]
    fn user_specs_can_still_hide_the_default_toolchain() {
        let home = rustup_home("default_toolchain = \"stable\"\n");
        let spec: CacheSpec = toml::from_str(
            "id = \"mine\"\nname = \"Mine\"\npaths = [\"~/.rustup/toolchains\"]\nchildren = true\nkeep = \"rustup-default\"\nchild_prune = [\"rustup\", \"toolchain\", \"uninstall\", \"{name}\"]\n",
        )
        .unwrap();
        let caches = discover_with(&[spec], home.path(), &env(&[]), &Tools::none());
        assert!(!caches.iter().any(|c| c.id.contains("stable")));
        assert!(caches.iter().any(|c| c.id.contains("nightly")));
    }

    fn cache(extra: &str) -> GlobalCache {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("store")).unwrap();
        let spec: CacheSpec = toml::from_str(&format!(
            "id = \"store\"\nname = \"Store\"\npaths = [\"~/store\"]\n{extra}"
        ))
        .unwrap();
        discover_with(&[spec], home.path(), &env(&[]), &Tools::none()).remove(0)
    }

    #[test]
    fn plans_the_tool_then_deletion_and_refuses_what_cannot_work() {
        // `cargo` is on PATH while tests run; nothing here is executed.
        let pruned = cache("delete = true\nprune = [\"cargo\", \"clean\"]");
        let Ok(Step::Run(task)) = pruned.plan(Mode::Clean, &idle()) else {
            panic!("expected the tool's command");
        };
        assert_eq!(task.command_line(), "cargo clean");
        assert_eq!(
            pruned.plan(Mode::Delete, &idle()),
            Ok(Step::DeleteDirectory)
        );
        assert_eq!(pruned.plan(Mode::Erase, &idle()), Err(Refusal::NotErasable));

        let missing = cache("delete = true\nprune = [\"carwash-no-such-tool\", \"gc\"]");
        assert_eq!(
            missing.plan(Mode::Clean, &idle()),
            Ok(Step::DeleteDirectory)
        );

        let tool_only = cache("prune = [\"carwash-no-such-tool\", \"gc\"]");
        assert_eq!(
            tool_only.plan(Mode::Clean, &idle()),
            Err(Refusal::ToolMissing("carwash-no-such-tool".into()))
        );
        assert_eq!(
            tool_only.plan(Mode::Delete, &idle()),
            Err(Refusal::NotDeletable("carwash-no-such-tool".into()))
        );
        assert!(!Refusal::NotErasable.is_temporary());
        assert!(Refusal::Busy(Vec::new()).is_temporary());

        let guarded = cache("delete = true\napps = [\"acme-daemon\"]");
        assert_eq!(
            guarded.plan(Mode::Clean, &RunningApps::from_names(&["acme-daemon"])),
            Err(Refusal::Busy(vec!["acme-daemon".into()]))
        );
        assert_eq!(
            guarded.plan(Mode::Clean, &idle()),
            Ok(Step::DeleteDirectory)
        );
    }

    #[test]
    fn measures_only_its_parts_when_it_has_them() {
        let home = tempfile::tempdir().unwrap();
        for (dir, bytes) in [("set/a", 8_000), ("set/b", 64_000)] {
            fs::create_dir_all(home.path().join(dir)).unwrap();
            fs::write(home.path().join(dir).join("blob"), vec![1u8; bytes]).unwrap();
        }
        let engine = Engine::new(crate::Registry::builtin());
        let mut set = cache("delete = true");
        set.path = home.path().join("set");
        let everything = set.measure(&engine, &Cancel::new());
        assert_eq!(everything.files, 2);
        set.parts = Some(vec![home.path().join("set/a")]);
        let part = set.measure(&engine, &Cancel::new());
        assert_eq!(part.files, 1);
        assert!(part.on_disk < everything.on_disk);
        set.parts = Some(Vec::new());
        assert_eq!(set.measure(&engine, &Cancel::new()), Size::default());
    }
}
