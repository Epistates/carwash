//! `carwash os`: the operating system's own caches, logs, trash and temporary files.

use super::size_style;
use crate::cli::{OsAction, OsArgs};
use crate::context::Context;
use crate::table::{Align, Cell, Table};
use anstyle::{AnsiColor, Style};
use anyhow::{Context as _, Result, bail};
use carwash_core::clean::DeleteMode;
use carwash_core::history::{self, Record};
use carwash_core::paths::Dirs;
use carwash_core::sweep::{Skip, SweepReport};
use carwash_core::system::{self, Action, Area, Need, RunningApps, Tier};
use carwash_core::{ArtifactKind, Cancel, fmt};
use indicatif::{ProgressBar, ProgressStyle};
use serde::Serialize;
use std::io::{BufRead, IsTerminal, Write};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Areas present on this machine, with user overrides from `system.toml`.
pub fn present(dirs: Option<&Dirs>) -> Result<Vec<Area>> {
    let specs = match dirs.map(Dirs::system_file) {
        Some(path) if path.exists() => {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            system::specs_with_overrides(&text)
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("invalid areas in {}", path.display()))?
        }
        _ => system::builtin_specs(),
    };
    let home = carwash_core::paths::home().context("cannot find the home directory")?;
    Ok(system::discover(&specs, &home))
}

/// Dry-runs every area, a few at a time, reporting progress on stderr.
fn measure(areas: &[&Area], show_progress: bool) -> Vec<SweepReport> {
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
    let total = areas.len();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let reports: Vec<Mutex<SweepReport>> = areas.iter().map(|_| Mutex::default()).collect();
    std::thread::scope(|scope| {
        for _ in 0..4.min(total) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(area) = areas.get(i) else {
                        break;
                    };
                    *reports[i].lock().expect("report slot") = area.sweep(true, &Cancel::new());
                    done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        while done.load(Ordering::Relaxed) < total {
            bar.set_message(format!(
                "Measuring ({}/{total})",
                done.load(Ordering::Relaxed)
            ));
            std::thread::sleep(Duration::from_millis(80));
        }
    });
    bar.finish_and_clear();
    reports
        .into_iter()
        .map(|r| r.into_inner().expect("report slot"))
        .collect()
}

pub fn run(ctx: &Context, args: &OsArgs) -> Result<ExitCode> {
    let areas = present(ctx.dirs.as_ref())?;
    match &args.action {
        None => list(&areas, args.json),
        Some(OsAction::Clean {
            areas: picks,
            yes,
            dry_run,
            escalate,
            json,
        }) => clean(
            ctx,
            &areas,
            picks,
            &Flags {
                yes: *yes,
                dry_run: *dry_run,
                escalate: *escalate,
                json: *json,
            },
        ),
    }
}

/// A command as it would be typed.
fn command_line(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            if arg.is_empty() || arg.contains([' ', '\'', '"']) {
                format!("'{}'", arg.replace('\'', "'\\''"))
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The command as the user would run it with the rights it needs.
fn privileged_line(argv: &[String], needs: Option<Need>) -> String {
    match needs {
        Some(Need::Root) => format!("sudo {}", command_line(argv)),
        Some(Need::Admin) => format!("{} (in an administrator terminal)", command_line(argv)),
        _ => command_line(argv),
    }
}

fn needs_elevation(area: &Area) -> bool {
    matches!(area.action, Action::Command { .. }) && area.needs.is_some_and(Need::elevates)
}

/// Whether the walk was refused somewhere for want of macOS Full Disk Access.
fn lacks_disk_access(area: &Area, report: &SweepReport) -> bool {
    area.needs == Some(Need::FullDiskAccess) && report.skipped.contains_key(&Skip::Denied)
}

const DISK_ACCESS_HINT: &str =
    "grant Full Disk Access to your terminal in System Settings > Privacy & Security";

fn notes(area: &Area, report: &SweepReport, blocked: &[String]) -> String {
    let mut notes = Vec::new();
    if !blocked.is_empty() {
        notes.push(format!("close {} first", blocked.join(", ")));
    }
    match &area.action {
        Action::Report => notes.push("report only".into()),
        Action::Command { argv } => notes.push(privileged_line(argv, area.needs)),
        Action::Sweep => {
            if let Some(age) = area.older_than {
                notes.push(format!("keeps the last {}", fmt::age(age)));
            }
        }
    }
    if lacks_disk_access(area, report) {
        notes.push("needs Full Disk Access".into());
    }
    notes.join("; ")
}

#[derive(Serialize)]
struct AreaOut<'a> {
    index: usize,
    #[serde(flatten)]
    area: &'a Area,
    /// What cleaning would free (for commands and reports: everything in their roots).
    frees: u64,
    skipped: &'a std::collections::BTreeMap<Skip, carwash_core::sweep::Tally>,
    blocked_by: Vec<String>,
}

fn list(areas: &[Area], json: bool) -> Result<ExitCode> {
    let refs: Vec<&Area> = areas.iter().collect();
    let reports = measure(&refs, !json);
    let running = RunningApps::snapshot();
    let mut out = anstream::stdout().lock();
    if json {
        let rows: Vec<AreaOut> = areas
            .iter()
            .zip(&reports)
            .enumerate()
            .map(|(i, (area, report))| AreaOut {
                index: i + 1,
                area,
                frees: report.removed.bytes,
                skipped: &report.skipped,
                blocked_by: area.blocked_by(&running),
            })
            .collect();
        serde_json::to_writer_pretty(&mut out, &rows)?;
        writeln!(out)?;
        return Ok(ExitCode::SUCCESS);
    }
    if areas.is_empty() {
        writeln!(out, "No system areas found on this machine.")?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = Table::new(&[
        ("#", Align::Right),
        ("TIER", Align::Left),
        ("FREES", Align::Right),
        ("AREA", Align::Left),
        ("ID", Align::Left),
        ("NOTES", Align::Left),
    ]);
    let mut ready = 0u64;
    for (i, (area, report)) in areas.iter().zip(&reports).enumerate() {
        let blocked = area.blocked_by(&running);
        let bytes = report.removed.bytes;
        let frees = match area.action {
            Action::Command { .. } if area.roots.is_empty() => {
                Cell::styled("-", Style::new().dimmed())
            }
            _ => Cell::styled(fmt::bytes(bytes), size_style(bytes)),
        };
        let tier = match area.tier {
            Tier::Default => Cell::styled("default", AnsiColor::Green.on_default()),
            Tier::OptIn => Cell::styled("opt-in", AnsiColor::Yellow.on_default()),
            Tier::Report => Cell::styled("report", Style::new().dimmed()),
        };
        if area.tier == Tier::Default && blocked.is_empty() && !needs_elevation(area) {
            ready += bytes;
        }
        table.push(vec![
            Cell::styled((i + 1).to_string(), Style::new().dimmed()),
            tier,
            frees,
            Cell::new(area.name.clone()),
            Cell::styled(area.id.clone(), Style::new().dimmed()),
            Cell::styled(notes(area, report, &blocked), Style::new().dimmed()),
        ]);
    }
    table.write(&mut out)?;
    writeln!(out)?;
    let bold = Style::new().bold();
    writeln!(
        out,
        "{}{}{} ready in the default tier: `carwash os clean`. Pick others by number or id: `carwash os clean 2,5-7`.",
        bold.render(),
        fmt::bytes(ready),
        bold.render_reset()
    )?;
    let reports_only: Vec<&Area> = areas.iter().filter(|a| a.tier == Tier::Report).collect();
    if !reports_only.is_empty() {
        writeln!(out, "\nReport-only areas, to clean by hand:")?;
        for area in reports_only {
            if let Some(how) = &area.how {
                writeln!(
                    out,
                    "  {}: {}{how}{}",
                    area.name,
                    Style::new().dimmed().render(),
                    Style::new().dimmed().render_reset()
                )?;
            }
        }
    }
    if areas
        .iter()
        .zip(&reports)
        .any(|(area, report)| lacks_disk_access(area, report))
    {
        writeln!(
            out,
            "\nSome areas need Full Disk Access: {DISK_ACCESS_HINT}."
        )?;
    }
    Ok(ExitCode::SUCCESS)
}

struct Flags {
    yes: bool,
    dry_run: bool,
    escalate: bool,
    json: bool,
}

/// What happens to a selected area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Plan {
    Sweep,
    Command,
    /// An elevated command the user runs themselves (no --escalate).
    Manual,
    /// A program that must be closed is running.
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Cleaned,
    WouldClean,
    Manual,
    Blocked,
    Failed,
}

#[derive(Serialize)]
struct CleanedOut {
    id: String,
    name: String,
    outcome: Outcome,
    freed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    blocked_by: Vec<String>,
    skipped: std::collections::BTreeMap<Skip, carwash_core::sweep::Tally>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct CleanOut {
    version: u32,
    dry_run: bool,
    areas: Vec<CleanedOut>,
    freed: u64,
    free_before: Option<u64>,
    free_after: Option<u64>,
}

/// Areas named by id or row number; none means the default tier (without elevated
/// commands unless escalating).
fn select<'a>(areas: &'a [Area], picks: &[String], escalate: bool) -> Result<Vec<&'a Area>> {
    let picks: Vec<&str> = picks
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    if picks.is_empty() {
        return Ok(areas
            .iter()
            .filter(|a| a.tier == Tier::Default && (escalate || !needs_elevation(a)))
            .collect());
    }
    let mut chosen: Vec<&Area> = Vec::new();
    for pick in picks {
        let rows = super::pick::rows(pick).map_err(anyhow::Error::msg)?;
        let found: Vec<&Area> = match rows {
            Some((first, last)) => {
                if last > areas.len() {
                    bail!(
                        "{last} is out of range: `carwash os` lists {} areas",
                        areas.len()
                    );
                }
                areas[first - 1..last].iter().collect()
            }
            None => match areas.iter().find(|a| a.id == pick) {
                Some(area) => vec![area],
                None => bail!("no area `{pick}` on this machine (see `carwash os`)"),
            },
        };
        for area in found {
            if area.tier == Tier::Report {
                bail!(
                    "{} is report-only; carwash does not clean it. {}",
                    area.name,
                    area.how.as_deref().unwrap_or_default()
                );
            }
            if !chosen.iter().any(|a| std::ptr::eq(*a, area)) {
                chosen.push(area);
            }
        }
    }
    Ok(chosen)
}

fn clean(ctx: &Context, areas: &[Area], picks: &[String], flags: &Flags) -> Result<ExitCode> {
    if system::running_as_root() {
        bail!(
            "refusing to run as root: your home would be root's, and sweeping shared directories as root is unsafe. Run as yourself; commands that need root run with --escalate"
        );
    }
    let selected = select(areas, picks, flags.escalate)?;
    let named = !picks.is_empty();
    let held_back: Vec<&Area> = if !named && !flags.escalate {
        areas
            .iter()
            .filter(|a| a.tier == Tier::Default && needs_elevation(a))
            .collect()
    } else {
        Vec::new()
    };
    let running = RunningApps::snapshot();
    let before = measure(&selected, !flags.json);
    let plans: Vec<Plan> = selected
        .iter()
        .map(|area| {
            if !area.blocked_by(&running).is_empty() {
                Plan::Blocked
            } else if matches!(area.action, Action::Sweep) {
                Plan::Sweep
            } else if needs_elevation(area) && !flags.escalate {
                Plan::Manual
            } else {
                Plan::Command
            }
        })
        .collect();

    let mut out = anstream::stdout().lock();
    if !flags.json {
        if selected.is_empty() {
            writeln!(out, "Nothing to clean.")?;
        }
        let width = selected.iter().map(|a| a.name.len()).max().unwrap_or(0);
        for ((area, report), plan) in selected.iter().zip(&before).zip(&plans) {
            let how = match (plan, &area.action) {
                (Plan::Blocked, _) => format!(
                    "skipped: close {} first",
                    area.blocked_by(&running).join(", ")
                ),
                (Plan::Manual, Action::Command { argv }) => {
                    format!("run yourself: {}", privileged_line(argv, area.needs))
                }
                (Plan::Command, Action::Command { argv }) => {
                    let line = command_line(argv);
                    if needs_elevation(area) {
                        format!("runs elevated: {line}")
                    } else {
                        format!("runs {line}")
                    }
                }
                _ => match area.older_than {
                    Some(age) => format!("sweep, keeping the last {}", fmt::age(age)),
                    None => "sweep".into(),
                },
            };
            let size = match area.action {
                Action::Command { .. } if area.roots.is_empty() => "-".to_owned(),
                _ => fmt::bytes(report.removed.bytes),
            };
            writeln!(
                out,
                "{size:>9}  {:width$}  {}{how}{}",
                area.name,
                Style::new().dimmed().render(),
                Style::new().dimmed().render_reset()
            )?;
        }
        for area in &held_back {
            if let Action::Command { argv } = &area.action {
                writeln!(
                    out,
                    "{}also available: {}: {} (or pass --escalate){}",
                    Style::new().dimmed().render(),
                    area.name,
                    privileged_line(argv, area.needs),
                    Style::new().dimmed().render_reset()
                )?;
            }
        }
    }

    let actionable = plans
        .iter()
        .filter(|p| matches!(p, Plan::Sweep | Plan::Command))
        .count();
    let home = carwash_core::paths::home();
    let free_before = home.as_deref().and_then(|h| fs4::available_space(h).ok());
    let mut results: Vec<CleanedOut> = Vec::new();
    let finish =
        |results: Vec<CleanedOut>, out: &mut dyn Write, dry_run: bool| -> Result<ExitCode> {
            let freed = results.iter().map(|r| r.freed).sum();
            // Named areas left undone are a failure; in the default tier, an open app only
            // postpones its cache.
            let incomplete = results.iter().any(|r| match r.outcome {
                Outcome::Failed => true,
                Outcome::Manual | Outcome::Blocked => named,
                Outcome::Cleaned | Outcome::WouldClean => false,
            });
            if flags.json {
                let report = CleanOut {
                    version: crate::report::VERSION,
                    dry_run,
                    areas: results,
                    freed,
                    free_before,
                    free_after: home.as_deref().and_then(|h| fs4::available_space(h).ok()),
                };
                serde_json::to_writer_pretty(&mut *out, &report)?;
                writeln!(out)?;
            }
            Ok(if incomplete {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        };
    let outcome_for =
        |area: &Area, plan: Plan, report: &SweepReport, outcome: Outcome| CleanedOut {
            id: area.id.clone(),
            name: area.name.clone(),
            outcome,
            freed: if outcome == Outcome::WouldClean || outcome == Outcome::Cleaned {
                report.removed.bytes
            } else {
                0
            },
            command: match (&area.action, plan) {
                (Action::Command { argv }, _) => Some(privileged_line(argv, area.needs)),
                _ => None,
            },
            blocked_by: area.blocked_by(&running),
            skipped: report.skipped.clone(),
            error: None,
        };

    if flags.dry_run || actionable == 0 {
        for ((area, report), plan) in selected.iter().zip(&before).zip(&plans) {
            let outcome = match plan {
                Plan::Blocked => Outcome::Blocked,
                Plan::Manual => Outcome::Manual,
                Plan::Sweep | Plan::Command => Outcome::WouldClean,
            };
            results.push(outcome_for(area, *plan, report, outcome));
        }
        if !flags.json && flags.dry_run && actionable > 0 {
            writeln!(out, "Dry run: nothing was changed.")?;
        }
        return finish(results, &mut out, flags.dry_run);
    }

    if !flags.yes {
        if !std::io::stdin().is_terminal() {
            bail!(
                "refusing to clean without confirmation in a non-interactive session; pass --yes"
            );
        }
        let planned: u64 = selected
            .iter()
            .zip(&before)
            .zip(&plans)
            .filter(|(_, p)| **p == Plan::Sweep)
            .map(|((_, r), _)| r.removed.bytes)
            .sum();
        write!(
            out,
            "Clean {actionable} areas, freeing about {}? [y/N] ",
            fmt::bytes(planned)
        )?;
        out.flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "Yes") {
            writeln!(out, "Nothing changed.")?;
            return Ok(ExitCode::SUCCESS);
        }
    }
    drop(out);

    let mut records = Vec::new();
    for ((area, measured), plan) in selected.iter().zip(&before).zip(&plans) {
        let result = match plan {
            Plan::Blocked => outcome_for(area, *plan, measured, Outcome::Blocked),
            Plan::Manual => outcome_for(area, *plan, measured, Outcome::Manual),
            Plan::Sweep => {
                let report = area.sweep(false, &Cancel::new());
                if !flags.json {
                    print_swept(area, &report);
                }
                outcome_for(area, *plan, &report, Outcome::Cleaned)
            }
            Plan::Command => {
                let Action::Command { argv } = &area.action else {
                    unreachable!("command plan for a command area");
                };
                if !flags.json {
                    anstream::println!("▶ {}", privileged_line(argv, area.needs));
                }
                match run_command(argv, needs_elevation(area), flags.json) {
                    Ok(()) => {
                        let after = area.sweep(true, &Cancel::new());
                        let mut report = measured.clone();
                        report.removed.bytes =
                            measured.removed.bytes.saturating_sub(after.removed.bytes);
                        if !flags.json {
                            anstream::println!(
                                "  freed {} with {}",
                                fmt::bytes(report.removed.bytes),
                                area.name
                            );
                        }
                        outcome_for(area, *plan, &report, Outcome::Cleaned)
                    }
                    Err(error) => {
                        if !flags.json {
                            anstream::eprintln!("  {} failed: {error:#}", area.name);
                        }
                        let mut failed = outcome_for(area, *plan, measured, Outcome::Failed);
                        failed.error = Some(format!("{error:#}"));
                        failed
                    }
                }
            }
        };
        if result.outcome == Outcome::Cleaned && result.freed > 0 {
            for root in area.roots.iter().take(1) {
                records.push(Record::now(
                    root.clone(),
                    result.freed,
                    ArtifactKind::Cache,
                    None,
                    DeleteMode::Permanent,
                ));
            }
        }
        if !flags.json {
            match result.outcome {
                Outcome::Blocked => anstream::println!(
                    "  skipped {}: close {} first",
                    area.name,
                    result.blocked_by.join(", ")
                ),
                Outcome::Manual => anstream::println!(
                    "  {}: run yourself: {}",
                    area.name,
                    result.command.as_deref().unwrap_or_default()
                ),
                _ => {}
            }
        }
        results.push(result);
    }
    if let Some(dirs) = &ctx.dirs
        && let Err(error) = history::append(&dirs.history_file(), &records)
    {
        tracing::warn!(%error, "cannot write history");
    }
    let total: u64 = results.iter().map(|r| r.freed).sum();
    let mut out = anstream::stdout().lock();
    if !flags.json {
        let green = AnsiColor::Green.on_default().bold();
        writeln!(
            out,
            "{}Freed {}{} in total.",
            green.render(),
            fmt::bytes(total),
            green.render_reset()
        )?;
    }
    finish(results, &mut out, false)
}

fn print_swept(area: &Area, report: &SweepReport) {
    let mut kept = Vec::new();
    for (reason, tally) in &report.skipped {
        let label = match reason {
            Skip::Recent => "recent",
            Skip::Kept => continue,
            Skip::InUse => "in use",
            Skip::Denied => "not permitted",
            Skip::Cloud => "in the cloud",
            Skip::MountPoint => "on other disks",
            Skip::Special => "special files",
            Skip::Changed => "changed meanwhile",
            Skip::TooDeep => "too deep",
        };
        kept.push(format!("{} {label}", tally.entries));
    }
    let detail = if kept.is_empty() {
        String::new()
    } else {
        format!(" (left {})", kept.join(", "))
    };
    anstream::println!(
        "  freed {} from {}{detail}",
        fmt::bytes(report.removed.bytes),
        area.name
    );
    if lacks_disk_access(area, report) {
        anstream::println!("  {}: {DISK_ACCESS_HINT}", area.name);
    }
    for failure in &report.failures {
        anstream::eprintln!(
            "  could not remove {}: {}",
            failure.path.display(),
            failure.error
        );
    }
}

/// Runs an area's command with the terminal attached, elevated through sudo or a UAC
/// prompt when it needs root or administrator rights.
fn run_command(argv: &[String], elevate: bool, json: bool) -> Result<()> {
    let mut command = if elevate {
        elevated(argv)
    } else {
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        command
    };
    // With --json, stdout carries only the report.
    if json {
        command.stdout(Stdio::null());
    }
    let status = command
        .stdin(Stdio::inherit())
        .status()
        .with_context(|| format!("cannot run {}", argv[0]))?;
    if !status.success() {
        bail!("{} exited with {status}", argv[0]);
    }
    Ok(())
}

#[cfg(unix)]
fn elevated(argv: &[String]) -> Command {
    // sudo prompts for the password itself, in this terminal.
    let mut command = Command::new("sudo");
    command.arg("--").args(argv);
    command
}

#[cfg(windows)]
fn elevated(argv: &[String]) -> Command {
    // Start-Process -Verb RunAs shows the UAC prompt and waits for the elevated process.
    let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let mut script = format!(
        "$p = Start-Process -FilePath {} -Verb RunAs -Wait -PassThru",
        quote(&argv[0])
    );
    if argv.len() > 1 {
        let args: Vec<String> = argv[1..].iter().map(|a| quote(a)).collect();
        script.push_str(&format!(" -ArgumentList {}", args.join(",")));
    }
    script.push_str("; exit $p.ExitCode");
    let mut command = Command::new("powershell");
    command.args(["-NoProfile", "-Command", &script]);
    command
}

#[cfg(not(any(unix, windows)))]
fn elevated(argv: &[String]) -> Command {
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_commands_for_display() {
        let argv: Vec<String> = ["powershell", "-Command", "Clear-RecycleBin -Force"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            command_line(&argv),
            "powershell -Command 'Clear-RecycleBin -Force'"
        );
        let apt: Vec<String> = ["apt-get", "clean"].map(String::from).to_vec();
        assert_eq!(
            privileged_line(&apt, Some(Need::Root)),
            "sudo apt-get clean"
        );
    }
}
