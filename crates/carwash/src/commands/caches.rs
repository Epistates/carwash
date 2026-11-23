//! `carwash caches`: per-user caches outside projects.

use super::size_style;
use crate::cli::{CachesAction, CachesArgs};
use crate::context::Context;
use crate::table::{Align, Cell, Table};
use anstyle::{AnsiColor, Style};
use anyhow::{Context as _, Result, bail};
use carwash_core::cache::Fingerprint;
use carwash_core::caches::{self, GlobalCache, Host, LocalHost, Mode, Refusal, Step};
use carwash_core::clean::{CleanItem, CleanOptions, DeleteMode};
use carwash_core::history::{self, Record};
use carwash_core::paths::Dirs;
use carwash_core::system::RunningApps;
use carwash_core::tasks::Task;
use carwash_core::{ArtifactId, ArtifactKind, Cancel, fmt};
use indicatif::{ProgressBar, ProgressStyle};
use serde::Serialize;
use std::io::{BufRead, IsTerminal, Write};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

/// Caches present on this machine, with user overrides from `caches.toml`.
pub fn present(dirs: Option<&Dirs>) -> Result<Vec<GlobalCache>> {
    let specs = match dirs.map(Dirs::caches_file) {
        Some(path) if path.exists() => {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            caches::specs_with_overrides(&text)
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("invalid caches in {}", path.display()))?
        }
        _ => caches::builtin_specs(),
    };
    let home = carwash_core::paths::home().context("cannot find the home directory")?;
    Ok(caches::discover(&specs, &home))
}

/// Measures every cache, a few at a time, reporting progress on stderr.
pub fn measure(ctx: &Context, caches: &mut [GlobalCache], show_progress: bool) {
    let bar = if show_progress && std::io::stderr().is_terminal() {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}").expect("valid template"),
        );
        bar.enable_steady_tick(Duration::from_millis(80));
        bar
    } else {
        ProgressBar::hidden()
    };
    let done = AtomicUsize::new(0);
    let total = caches.len();
    let slots: Vec<Mutex<&mut GlobalCache>> = caches.iter_mut().map(Mutex::new).collect();
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..4.min(total) {
            scope.spawn(|| {
                while let Some(slot) = slots.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let mut cache = slot.lock().expect("cache slot");
                    cache.fingerprint = Fingerprint::of(&cache.path);
                    cache.size = Some(cache.measure(&ctx.engine, &Cancel::new()));
                    done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        while done.load(Ordering::Relaxed) < total {
            bar.set_message(format!(
                "Measuring caches ({}/{total})",
                done.load(Ordering::Relaxed)
            ));
            std::thread::sleep(Duration::from_millis(80));
        }
    });
    bar.finish_and_clear();
    remember(ctx, caches);
}

/// Stores measured sizes in the size cache, where the TUI's Caches tab finds them.
fn remember(ctx: &Context, caches: &[GlobalCache]) {
    ctx.update_size_cache(|sizes| {
        for cache in caches {
            if let Some(size) = cache.size {
                sizes.insert_with(cache.path.clone(), size, cache.fingerprint);
            }
        }
    });
}

/// Why a cache is left for now, as the user reads it.
pub(crate) fn left_alone(refusal: &Refusal) -> String {
    match refusal {
        Refusal::Busy(_) => format!("{refusal}; try again when it has finished"),
        _ => refusal.to_string(),
    }
}

fn action(cache: &GlobalCache, running: &RunningApps) -> (String, Style) {
    let dim = Style::new().dimmed();
    let yellow = AnsiColor::Yellow.on_default();
    if let Err(refusal) = cache.plan(Mode::Clean, running)
        && refusal.is_temporary()
    {
        return (format!("left alone: {refusal}"), yellow);
    }
    match (&cache.prune, cache.prune_task(), cache.deletable) {
        (Some(_), Some(task), _) => (task.command_line(), Style::new()),
        (Some(argv), None, true) => (format!("delete ({} not installed)", argv[0]), dim),
        (Some(argv), None, false) => (format!("needs `{}`", argv[0]), yellow),
        (None, _, true) => ("delete".into(), Style::new()),
        (None, _, false) => ("none".into(), dim),
    }
}

/// `3d ago`, or `-` when nothing records it.
fn last_used(cache: &GlobalCache, now: SystemTime) -> String {
    match cache
        .last_used
        .map(|at| now.duration_since(at).unwrap_or_default())
    {
        Some(age) if age < Duration::from_secs(60) => "just now".into(),
        Some(age) => format!("{} ago", fmt::age(age)),
        None => "-".into(),
    }
}

pub fn run(ctx: &Context, args: &CachesArgs) -> Result<ExitCode> {
    let mut caches = present(ctx.dirs.as_ref())?;
    match &args.action {
        None => list(ctx, &mut caches, args.json),
        Some(CachesAction::Clean {
            ids,
            yes,
            dry_run,
            delete,
            erase,
        }) => {
            let flags = Flags {
                yes: *yes,
                dry_run: *dry_run,
                mode: match (*erase, *delete) {
                    (true, _) => Mode::Erase,
                    (false, true) => Mode::Delete,
                    (false, false) => Mode::Clean,
                },
            };
            let mut out = anstream::stdout().lock();
            clean(ctx, &Machine(ctx), &mut out, &caches, ids, &flags)
        }
    }
}

#[derive(Serialize)]
struct CacheOut<'a> {
    #[serde(flatten)]
    cache: &'a GlobalCache,
    /// Running programs that must finish before it can be cleaned.
    blocked_by: Vec<String>,
}

fn list(ctx: &Context, caches: &mut [GlobalCache], json: bool) -> Result<ExitCode> {
    measure(ctx, caches, !json);
    caches.sort_by_key(|c| std::cmp::Reverse(c.size.map_or(0, |s| s.on_disk)));
    let running = RunningApps::snapshot();
    let mut out = anstream::stdout().lock();
    if json {
        let rows: Vec<CacheOut> = caches
            .iter()
            .map(|cache| CacheOut {
                cache,
                blocked_by: cache.blocked_by(&running),
            })
            .collect();
        serde_json::to_writer_pretty(&mut out, &rows)?;
        writeln!(out)?;
        return Ok(ExitCode::SUCCESS);
    }
    if caches.is_empty() {
        writeln!(out, "No known caches found.")?;
        return Ok(ExitCode::SUCCESS);
    }
    // Only some caches record when they were last used; without any, the column is noise.
    let dated = caches.iter().any(|c| c.last_used.is_some());
    let mut columns = vec![
        ("SIZE", Align::Right),
        ("ID", Align::Left),
        ("CACHE", Align::Left),
    ];
    if dated {
        columns.push(("LAST USED", Align::Right));
    }
    columns.push(("CLEAN WITH", Align::Left));
    let mut table = Table::new(&columns);
    let now = SystemTime::now();
    let mut total = 0;
    for cache in caches.iter() {
        let bytes = cache.size.map_or(0, |s| s.on_disk);
        total += bytes;
        let (action, style) = action(cache, &running);
        let mut cells = vec![
            Cell::styled(fmt::bytes(bytes), size_style(bytes)),
            Cell::styled(cache.id.clone(), Style::new().dimmed()),
            Cell::new(cache.name.clone()),
        ];
        if dated {
            cells.push(Cell::styled(last_used(cache, now), Style::new().dimmed()));
        }
        cells.push(Cell::styled(action, style));
        table.push(cells);
    }
    table.write(&mut out)?;
    let bold = Style::new().bold();
    writeln!(
        out,
        "\n{}{}{} in {} caches. Clean some with `carwash caches clean <ID>...`.",
        bold.render(),
        fmt::bytes(total),
        bold.render_reset(),
        caches.len()
    )?;
    if caches.iter().any(|c| c.erase.is_some()) {
        writeln!(
            out,
            "Simulators: cleaning one deletes the device; add --erase to clear its contents and settings and keep it."
        )?;
    }
    Ok(ExitCode::SUCCESS)
}

struct Flags {
    yes: bool,
    dry_run: bool,
    mode: Mode,
}

/// Carries out what a clean decided, and reports the machine's state just before each
/// step. Tests replace it, so none of them runs a tool or removes a directory.
pub(crate) trait Cleaner {
    /// What the machine's tools report now.
    fn host(&self) -> &dyn Host;
    /// The programs running now.
    fn running(&self) -> RunningApps;
    /// Runs a tool's command with the terminal attached; true when it succeeded.
    fn run(&self, task: &Task) -> bool;
    /// Deletes a cache's directory; true when it is gone.
    fn delete(&self, cache: &GlobalCache) -> bool;
}

/// The machine carwash runs on.
struct Machine<'a>(&'a Context);

impl Cleaner for Machine<'_> {
    fn host(&self) -> &dyn Host {
        &LocalHost
    }

    fn running(&self) -> RunningApps {
        RunningApps::snapshot()
    }

    fn run(&self, task: &Task) -> bool {
        let status = Command::new(&task.program)
            .args(&task.args)
            .current_dir(&task.cwd)
            .stdin(Stdio::inherit())
            .status();
        matches!(status, Ok(s) if s.success())
    }

    fn delete(&self, cache: &GlobalCache) -> bool {
        let Some(parent) = cache.path.parent() else {
            return false;
        };
        let report = self.0.engine.clean(
            &[CleanItem {
                id: ArtifactId(0),
                path: cache.path.clone(),
                expected_bytes: cache.size.map_or(0, |s| s.on_disk),
            }],
            &CleanOptions {
                mode: DeleteMode::Permanent,
                allowed_roots: vec![parent.to_path_buf()],
                dry_run: false,
            },
            &Cancel::new(),
            &|_| {},
        );
        report.removed == 1
    }
}

/// The caches `ids` name: the cache with that id, or else every child of it.
fn select(caches: &[GlobalCache], ids: &[String]) -> Result<Vec<GlobalCache>> {
    let mut selected: Vec<GlobalCache> = Vec::new();
    for id in ids {
        let exact: Vec<&GlobalCache> = caches.iter().filter(|c| &c.id == id).collect();
        let matching = if exact.is_empty() {
            let prefix = format!("{id}/");
            caches
                .iter()
                .filter(|c| c.id.starts_with(&prefix))
                .collect()
        } else {
            exact
        };
        if matching.is_empty() {
            bail!("no cache `{id}` on this machine (see `carwash caches`)");
        }
        for cache in matching {
            if !selected.iter().any(|c| c.id == cache.id) {
                selected.push(cache.clone());
            }
        }
    }
    Ok(selected)
}

fn clean(
    ctx: &Context,
    cleaner: &dyn Cleaner,
    out: &mut dyn Write,
    caches: &[GlobalCache],
    ids: &[String],
    flags: &Flags,
) -> Result<ExitCode> {
    let mut selected = select(caches, ids)?;
    measure(ctx, &mut selected, true);

    let running = cleaner.running();
    let dim = Style::new().dimmed();
    let mut plan: Vec<(GlobalCache, Step)> = Vec::new();
    // Named caches that are in use, and so left alone.
    let mut skipped = 0;
    for cache in selected {
        let step = match cache.plan(flags.mode, &running) {
            Err(refusal) if !refusal.is_temporary() => bail!("{} {refusal}", cache.name),
            step => step,
        };
        let how = match &step {
            Ok(Step::Run(task)) => task.command_line(),
            Ok(Step::DeleteDirectory) => "delete".to_owned(),
            Err(refusal) => format!("skipped: {}", left_alone(refusal)),
        };
        writeln!(
            out,
            "{:>9}  {}  {}{how}{}",
            fmt::bytes(cache.size.map_or(0, |s| s.on_disk)),
            cache.name,
            dim.render(),
            dim.render_reset()
        )?;
        match step {
            Ok(step) => plan.push((cache, step)),
            Err(_) => skipped += 1,
        }
    }
    let exit = |incomplete: bool| {
        if incomplete {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        }
    };
    if flags.dry_run || plan.is_empty() {
        if flags.dry_run {
            writeln!(out, "Dry run: nothing was changed.")?;
        } else {
            writeln!(out, "Nothing changed.")?;
        }
        return Ok(exit(skipped > 0));
    }
    if !flags.yes {
        if !std::io::stdin().is_terminal() {
            bail!(
                "refusing to clean without confirmation in a non-interactive session; pass --yes"
            );
        }
        let question = match flags.mode {
            Mode::Erase => format!("Erase the contents and settings of {}?", plural(plan.len())),
            Mode::Clean | Mode::Delete => format!("Clean {} caches?", plan.len()),
        };
        write!(out, "{question} [y/N] ")?;
        out.flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "Yes") {
            writeln!(out, "Nothing changed.")?;
            return Ok(ExitCode::SUCCESS);
        }
    }

    let mut failures = 0;
    let mut records = Vec::new();
    let mut cleaned = Vec::new();
    for (cache, _) in plan {
        // The plan was made before the question was answered: ask again what is booted
        // and what is running, so nothing that came into use since is touched.
        let mut cache = cache.refreshed(cleaner.host());
        let running = if cache.apps.is_empty() {
            RunningApps::default()
        } else {
            cleaner.running()
        };
        let step = match cache.plan(flags.mode, &running) {
            Ok(step) => step,
            Err(refusal) => {
                skipped += 1;
                writeln!(out, "  skipped {}: {}", cache.name, left_alone(&refusal))?;
                continue;
            }
        };
        let before = cache.size.map_or(0, |s| s.on_disk);
        let ok = match step {
            Step::Run(task) => {
                writeln!(out, "▶ {}", task.command_line())?;
                out.flush()?;
                cleaner.run(&task)
            }
            Step::DeleteDirectory => cleaner.delete(&cache),
        };
        if ok {
            cache.fingerprint = Fingerprint::of(&cache.path);
            let after = cache.measure(&ctx.engine, &Cancel::new());
            let freed = before.saturating_sub(after.on_disk);
            writeln!(out, "  freed {} from {}", fmt::bytes(freed), cache.name)?;
            records.push(Record::now(
                cache.path.clone(),
                freed,
                ArtifactKind::Cache,
                cache.ecosystem.clone(),
                DeleteMode::Permanent,
            ));
            cache.size = Some(after);
            cleaned.push(cache);
        } else {
            failures += 1;
            writeln!(out, "  failed to clean {}", cache.name)?;
        }
    }
    remember(ctx, &cleaned);
    if let Some(dirs) = &ctx.dirs
        && let Err(error) = history::append(&dirs.history_file(), &records)
    {
        tracing::warn!(%error, "cannot write history");
    }
    let total: u64 = records.iter().map(|r| r.bytes).sum();
    writeln!(out, "Freed {} in total.", fmt::bytes(total))?;
    Ok(exit(failures > 0 || skipped > 0))
}

fn plural(simulators: usize) -> String {
    match simulators {
        1 => "1 simulator".into(),
        n => format!("{n} simulators"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use carwash_core::caches::CacheSpec;
    use carwash_core::{Engine, Registry, simulators};
    use std::fs;
    use std::path::Path;

    /// `xcrun simctl list --json devices` as captured, with one unavailable device added.
    const LISTING: &str = include_str!("../../../carwash-core/tests/data/simctl-list-devices.json");
    const NO_CLONES: &str =
        include_str!("../../../carwash-core/tests/data/simctl-testing-list-devices.json");
    const DEVICES: &str = "Library/Developer/CoreSimulator/Devices";
    const SHUT_DOWN: &str = "00000001-0000-4000-8000-000000000001";
    const BOOTED: &str = "0000000A-0000-4000-8000-00000000000A";

    /// Stands in for the machine: answers `simctl` with listings, reports the programs it
    /// is told are running, and records what it is asked to run or delete instead of
    /// doing it.
    struct Fake {
        /// Default-set listings, one per call; the last repeats.
        listings: Mutex<Vec<String>>,
        running: Vec<&'static str>,
        ran: Mutex<Vec<String>>,
        deleted: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(listings: &[&str], running: &[&'static str]) -> Self {
            Self {
                listings: Mutex::new(listings.iter().map(|l| (*l).to_owned()).collect()),
                running: running.to_vec(),
                ran: Mutex::default(),
                deleted: Mutex::default(),
            }
        }

        fn ran(&self) -> Vec<String> {
            self.ran.lock().unwrap().clone()
        }

        fn touched_nothing(&self) -> bool {
            self.ran().is_empty() && self.deleted.lock().unwrap().is_empty()
        }
    }

    impl Host for Fake {
        fn output(&self, argv: &[&str]) -> Option<Vec<u8>> {
            if argv == simulators::LIST {
                let mut listings = self.listings.lock().unwrap();
                let listing = if listings.len() > 1 {
                    listings.remove(0)
                } else {
                    listings[0].clone()
                };
                Some(listing.into_bytes())
            } else if argv == simulators::LIST_TESTING {
                Some(NO_CLONES.as_bytes().to_vec())
            } else {
                None
            }
        }
    }

    impl Cleaner for Fake {
        fn host(&self) -> &dyn Host {
            self
        }

        fn running(&self) -> RunningApps {
            RunningApps::from_names(&self.running)
        }

        fn run(&self, task: &Task) -> bool {
            self.ran.lock().unwrap().push(task.command_line());
            true
        }

        fn delete(&self, cache: &GlobalCache) -> bool {
            self.deleted.lock().unwrap().push(cache.id.clone());
            true
        }
    }

    fn context() -> Context {
        Context {
            config: Config::default(),
            dirs: None,
            engine: Engine::new(Registry::builtin()),
        }
    }

    /// The built-in simulator caches found in a home that holds a folder for every device
    /// in the listing, plus one plain cache. The simulator commands are spelled with a
    /// program every test machine has, since only macOS has `xcrun`.
    fn caches(home: &Path, fake: &Fake) -> Vec<GlobalCache> {
        for simulator in simulators::parse(LISTING.as_bytes()).unwrap() {
            fs::create_dir_all(home.join(DEVICES).join(simulator.udid).join("data")).unwrap();
        }
        fs::create_dir_all(home.join("Library/Developer/XCTestDevices")).unwrap();
        fs::create_dir_all(home.join(".plain")).unwrap();
        let mut specs: Vec<CacheSpec> = caches::builtin_specs()
            .into_iter()
            .filter(|s| matches!(s.id.as_str(), "simulators" | "xctest-devices"))
            .collect();
        for spec in &mut specs {
            for path in &mut spec.paths {
                *path = path.trim_start_matches("macos:").to_owned();
            }
            for argv in [
                &mut spec.prune,
                &mut spec.child_prune,
                &mut spec.child_erase,
            ]
            .into_iter()
            .flatten()
            {
                assert_eq!(argv[0], "xcrun");
                argv[0] = "cargo".into();
            }
        }
        specs.extend(
            caches::specs_with_overrides(
                "schema = 1\n[[cache]]\nid = \"plain\"\nname = \"Plain\"\npaths = [\"~/.plain\"]\ndelete = true\n",
            )
            .unwrap()
            .into_iter()
            .filter(|s| s.id == "plain"),
        );
        caches::discover_on(&specs, home, fake)
    }

    fn run_clean(fake: &Fake, ids: &[&str], mode: Mode, dry_run: bool) -> (ExitCode, String) {
        let home = tempfile::tempdir().unwrap();
        let caches = caches(home.path(), fake);
        let ids: Vec<String> = ids.iter().map(|id| (*id).to_owned()).collect();
        let mut out = Vec::new();
        let flags = Flags {
            yes: true,
            dry_run,
            mode,
        };
        let exit = clean(&context(), fake, &mut out, &caches, &ids, &flags).unwrap();
        (exit, String::from_utf8(out).unwrap())
    }

    fn failed(exit: ExitCode) -> bool {
        // `ExitCode` cannot be compared.
        format!("{exit:?}") != format!("{:?}", ExitCode::SUCCESS)
    }

    #[test]
    fn erasing_and_deleting_go_through_simctl_for_the_named_device_only() {
        let fake = Fake::new(&[LISTING], &[]);
        let id = format!("simulators/{SHUT_DOWN}");
        let (exit, out) = run_clean(&fake, &[&id], Mode::Erase, false);
        assert!(!failed(exit), "{out}");
        assert_eq!(fake.ran(), [format!("cargo simctl erase {SHUT_DOWN}")]);

        let fake = Fake::new(&[LISTING], &[]);
        let (exit, out) = run_clean(&fake, &[&id], Mode::Clean, false);
        assert!(!failed(exit), "{out}");
        assert_eq!(fake.ran(), [format!("cargo simctl delete {SHUT_DOWN}")]);
        assert!(
            fake.deleted.lock().unwrap().is_empty(),
            "never by removing its folder"
        );
    }

    #[test]
    fn the_set_id_still_deletes_only_unavailable_devices() {
        let fake = Fake::new(&[LISTING], &[]);
        let (exit, out) = run_clean(&fake, &["simulators"], Mode::Clean, false);
        assert!(!failed(exit), "{out}");
        assert_eq!(fake.ran(), ["cargo simctl delete unavailable"]);
    }

    #[test]
    fn a_dry_run_changes_nothing() {
        let fake = Fake::new(&[LISTING], &[]);
        let id = format!("simulators/{SHUT_DOWN}");
        let (exit, out) = run_clean(&fake, &[&id, "plain", "xctest-devices"], Mode::Clean, true);
        assert!(!failed(exit), "{out}");
        assert!(fake.touched_nothing());
        assert!(out.contains("Dry run: nothing was changed."), "{out}");
        assert!(
            out.contains(&format!("cargo simctl delete {SHUT_DOWN}")),
            "{out}"
        );
        assert!(
            out.contains("cargo simctl --set testing delete all"),
            "{out}"
        );
    }

    #[test]
    fn a_booted_simulator_is_skipped_even_when_named() {
        for mode in [Mode::Clean, Mode::Erase] {
            let fake = Fake::new(&[LISTING], &[]);
            let booted = format!("simulators/{BOOTED}");
            let shut_down = format!("simulators/{SHUT_DOWN}");
            let (exit, out) = run_clean(&fake, &[&booted, &shut_down], mode, false);
            assert!(failed(exit), "a named cache was left: {out}");
            assert!(out.contains("skipped: booted; shut it down first"), "{out}");
            let ran = fake.ran();
            assert_eq!(ran.len(), 1, "the other one is still cleaned: {ran:?}");
            assert!(ran[0].ends_with(SHUT_DOWN));
            assert!(!ran.iter().any(|line| line.contains(BOOTED)));
        }
    }

    #[test]
    fn a_running_build_or_test_stops_every_simulator_action_and_is_named() {
        for program in ["xcodebuild", "xctest"] {
            let id = format!("simulators/{SHUT_DOWN}");
            for (ids, mode) in [
                (
                    vec![id.as_str(), "simulators", "xctest-devices"],
                    Mode::Clean,
                ),
                (vec![id.as_str()], Mode::Erase),
            ] {
                let fake = Fake::new(&[LISTING], &[program, "zsh"]);
                let (exit, out) = run_clean(&fake, &ids, mode, false);
                assert!(failed(exit), "{out}");
                assert!(fake.touched_nothing(), "{:?}", fake.ran());
                let message =
                    format!("skipped: {program} is running; try again when it has finished");
                assert_eq!(out.matches(&message).count(), ids.len(), "{out}");
            }
            // Caches that do not involve simulators are not held up.
            let fake = Fake::new(&[LISTING], &[program]);
            let (exit, out) = run_clean(&fake, &["plain"], Mode::Clean, false);
            assert!(!failed(exit), "{out}");
            assert_eq!(*fake.deleted.lock().unwrap(), ["plain"]);
        }
    }

    #[test]
    fn a_simulator_that_boots_after_the_plan_is_left_alone() {
        let booted_since = LISTING.replace("\"state\": \"Shutdown\"", "\"state\": \"Booted\"");
        // Discovery sees it shut down; the check just before erasing sees it booted.
        let fake = Fake::new(&[LISTING, &booted_since], &[]);
        let id = format!("simulators/{SHUT_DOWN}");
        let (exit, out) = run_clean(&fake, &[&id], Mode::Erase, false);
        assert!(failed(exit), "{out}");
        assert!(fake.touched_nothing(), "{:?}", fake.ran());
        assert!(
            out.contains("skipped Simulators: iPhone 18 Pro, iOS 27.0: booted"),
            "{out}"
        );
    }

    #[test]
    fn erase_is_refused_for_caches_that_cannot_be_erased() {
        let fake = Fake::new(&[LISTING], &[]);
        let home = tempfile::tempdir().unwrap();
        let caches = caches(home.path(), &fake);
        let flags = Flags {
            yes: true,
            dry_run: false,
            mode: Mode::Erase,
        };
        let error = clean(
            &context(),
            &fake,
            &mut Vec::new(),
            &caches,
            &["plain".to_owned()],
            &flags,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "Plain cannot be erased, only cleaned");
        assert!(fake.touched_nothing());
        assert!(
            clean(
                &context(),
                &fake,
                &mut Vec::new(),
                &caches,
                &["nope".to_owned()],
                &flags
            )
            .is_err()
        );
    }

    #[test]
    fn a_parent_id_selects_children_only_when_no_cache_has_that_id() {
        let fake = Fake::new(&[LISTING], &[]);
        let home = tempfile::tempdir().unwrap();
        let caches = caches(home.path(), &fake);
        let set = select(&caches, &["simulators".to_owned()]).unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].id, "simulators");
        let twice = select(&caches, &["plain".to_owned(), "plain".to_owned()]).unwrap();
        assert_eq!(twice.len(), 1);
    }
}
