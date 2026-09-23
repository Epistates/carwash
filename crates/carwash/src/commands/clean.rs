//! `carwash clean`: delete selected artifacts.

use super::{artifact_table, count};
use crate::cli::{CleanArgs, SortKey};
use crate::context::Context;
use anstyle::{AnsiColor, Style};
use anyhow::{Result, bail};
use carwash_core::clean::{CleanEvent, CleanItem, CleanOptions, DeleteMode};
use carwash_core::history::{self, Record};
use carwash_core::select::Hold;
use carwash_core::{Artifact, ArtifactId, Cancel, fmt};
use indicatif::{ProgressBar, ProgressStyle};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::SystemTime;

const PLAN_ROWS: usize = 30;

#[derive(Serialize)]
struct CleanOutput<'a> {
    version: u32,
    root: &'a Path,
    dry_run: bool,
    mode: DeleteMode,
    selected: Vec<Planned<'a>>,
    held: BTreeMap<&'static str, usize>,
    removed: usize,
    failed: Vec<Failure>,
    bytes: u64,
    free_before: Option<u64>,
    free_after: Option<u64>,
}

#[derive(Serialize)]
struct Planned<'a> {
    id: u32,
    path: &'a Path,
    bytes: u64,
}

#[derive(Serialize)]
struct Failure {
    path: PathBuf,
    error: String,
}

pub fn run(ctx: &Context, args: &CleanArgs) -> Result<ExitCode> {
    let (root, options) = ctx.scan_options(&args.walk, true, true)?;
    let snapshot = ctx.run_scan(&root, &options, !args.json);
    let filter = ctx.filter(&args.filter)?;
    // An explicit --older-than replaces the default recent-activity window.
    let policy = ctx.policy(
        args.include_review,
        args.include_recent || args.filter.older_than.is_some(),
    );
    let registry = ctx.registry();
    let now = SystemTime::now();
    let mode = if args.trash {
        DeleteMode::Trash
    } else {
        ctx.config.clean.mode
    };

    let mut selected: Vec<&Artifact> = Vec::new();
    let mut held: BTreeMap<&'static str, usize> = BTreeMap::new();
    for artifact in snapshot.artifacts.iter().filter(|a| filter.matches(a, now)) {
        match policy.hold(artifact, now) {
            None if artifact.size.is_some() => selected.push(artifact),
            None => *held.entry("unmeasured").or_default() += 1,
            Some(hold) => *held.entry(hold.describe()).or_default() += 1,
        }
    }
    super::scan::sort(&mut selected, SortKey::Size, now);
    let total: u64 = selected
        .iter()
        .filter_map(|a| a.size)
        .map(|s| s.reclaimable)
        .sum();

    let mut out = anstream::stdout().lock();
    if !args.json {
        print_plan(&mut out, &selected, &held, total, &root, ctx, now, &policy)?;
    }
    let mut output = CleanOutput {
        version: crate::report::VERSION,
        root: &root,
        dry_run: args.dry_run,
        mode,
        selected: selected
            .iter()
            .map(|a| Planned {
                id: a.id.0,
                path: &a.path,
                bytes: a.size.map_or(0, |s| s.reclaimable),
            })
            .collect(),
        held: held.clone(),
        removed: 0,
        failed: Vec::new(),
        bytes: 0,
        free_before: None,
        free_after: None,
    };

    if selected.is_empty() || args.dry_run {
        if args.json {
            serde_json::to_writer_pretty(&mut out, &output)?;
            writeln!(out)?;
        } else if args.dry_run && !selected.is_empty() {
            writeln!(out, "Dry run: nothing was deleted.")?;
        }
        return Ok(ExitCode::SUCCESS);
    }

    if !args.yes {
        if !std::io::stdin().is_terminal() {
            bail!(
                "refusing to delete without confirmation in a non-interactive session; pass --yes"
            );
        }
        let verb = match mode {
            DeleteMode::Permanent => "Delete",
            DeleteMode::Trash => "Move to trash",
        };
        write!(
            out,
            "{verb} {} directories, freeing {}? [y/N] ",
            count(u64::try_from(selected.len()).unwrap_or(u64::MAX)),
            fmt::bytes(total)
        )?;
        out.flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "Yes") {
            writeln!(out, "Nothing deleted.")?;
            return Ok(ExitCode::SUCCESS);
        }
    }

    // Enclosing projects (above the scan root) own artifacts the user was shown.
    let mut allowed_roots = vec![root.clone()];
    allowed_roots.extend(
        snapshot
            .projects
            .iter()
            .filter(|p| p.outside_root)
            .map(|p| p.path.clone()),
    );
    let items: Vec<CleanItem> = selected
        .iter()
        .map(|a| CleanItem {
            id: a.id,
            path: a.path.clone(),
            expected_bytes: a.size.map_or(0, |s| s.reclaimable),
        })
        .collect();
    let by_id: HashMap<ArtifactId, &Artifact> = selected.iter().map(|a| (a.id, *a)).collect();

    let bar = if !args.json && std::io::stderr().is_terminal() {
        let bar = ProgressBar::new(total);
        bar.set_style(
            ProgressStyle::with_template(
                "{bar:30.green/dim} {binary_bytes}/{binary_total_bytes} {msg}",
            )
            .expect("valid template")
            .progress_chars("━━─"),
        );
        bar
    } else {
        ProgressBar::hidden()
    };
    let failures = Mutex::new(Vec::new());
    let records = Mutex::new(Vec::new());
    let report = ctx.engine.clean(
        &items,
        &CleanOptions {
            mode,
            allowed_roots,
            dry_run: false,
        },
        &Cancel::new(),
        &|event| match event {
            CleanEvent::Started { id } => {
                if let Some(a) = by_id.get(&id) {
                    bar.set_message(super::display_path(&a.path, &root));
                }
            }
            CleanEvent::Removed { id, bytes } => {
                bar.inc(bytes);
                if let Some(a) = by_id.get(&id) {
                    records.lock().expect("records lock").push(Record::now(
                        a.path.clone(),
                        bytes,
                        a.kind,
                        a.ecosystem.map(|e| registry.ecosystem(e).key.clone()),
                        mode,
                    ));
                }
            }
            CleanEvent::Failed { id, error } => {
                if let Some(a) = by_id.get(&id) {
                    failures.lock().expect("failures lock").push(Failure {
                        path: a.path.clone(),
                        error,
                    });
                }
            }
        },
    );
    bar.finish_and_clear();

    let records = records.into_inner().expect("records lock");
    if let Some(dirs) = &ctx.dirs
        && let Err(error) = history::append(&dirs.history_file(), &records)
    {
        tracing::warn!(%error, "cannot write history");
    }
    ctx.update_size_cache(|cache| {
        for record in &records {
            cache.remove(&record.path);
        }
    });

    output.removed = report.removed;
    output.failed = failures.into_inner().expect("failures lock");
    output.bytes = report.bytes;
    output.free_before = report.free_before;
    output.free_after = report.free_after;

    if args.json {
        serde_json::to_writer_pretty(&mut out, &output)?;
        writeln!(out)?;
    } else {
        let green = AnsiColor::Green.on_default().bold();
        writeln!(
            out,
            "{}Freed {}{} from {} directories.",
            green.render(),
            fmt::bytes(report.bytes),
            green.render_reset(),
            count(u64::try_from(report.removed).unwrap_or(u64::MAX))
        )?;
        if let (Some(before), Some(after)) = (report.free_before, report.free_after) {
            writeln!(
                out,
                "Free space: {} → {}",
                fmt::bytes(before),
                fmt::bytes(after)
            )?;
        }
        if mode == DeleteMode::Trash {
            writeln!(
                out,
                "Items are in the trash; empty it to actually free the space."
            )?;
        }
        for failure in &output.failed {
            let red = AnsiColor::Red.on_default();
            writeln!(
                out,
                "{}failed{} {}: {}",
                red.render(),
                red.render_reset(),
                failure.path.display(),
                failure.error
            )?;
        }
    }
    Ok(if output.failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[allow(clippy::too_many_arguments)]
fn print_plan(
    out: &mut impl Write,
    selected: &[&Artifact],
    held: &BTreeMap<&'static str, usize>,
    total: u64,
    root: &Path,
    ctx: &Context,
    now: SystemTime,
    policy: &carwash_core::select::Policy,
) -> Result<()> {
    if selected.is_empty() {
        writeln!(out, "Nothing to clean under {}.", root.display())?;
    } else {
        let shown: Vec<&Artifact> = selected.iter().take(PLAN_ROWS).copied().collect();
        artifact_table(&shown, root, ctx.registry(), |a| policy.hold(a, now), now).write(out)?;
        if selected.len() > shown.len() {
            let rest: u64 = selected[shown.len()..]
                .iter()
                .filter_map(|a| a.size)
                .map(|s| s.reclaimable)
                .sum();
            writeln!(
                out,
                "{}… and {} more ({}){}",
                Style::new().dimmed().render(),
                selected.len() - shown.len(),
                fmt::bytes(rest),
                Style::new().dimmed().render_reset()
            )?;
        }
        writeln!(out)?;
        let bold = Style::new().bold();
        writeln!(
            out,
            "Selected {} directories, {}{}{} to free.",
            count(u64::try_from(selected.len()).unwrap_or(u64::MAX)),
            bold.render(),
            fmt::bytes(total),
            bold.render_reset()
        )?;
    }
    if !held.is_empty() {
        let parts: Vec<String> = held
            .iter()
            .map(|(label, n)| format!("{n} {label}"))
            .collect();
        let hint = if held.contains_key(Hold::Review.describe())
            || held.contains_key(Hold::Recent.describe())
        {
            " (see --include-review, --include-recent)"
        } else {
            ""
        };
        writeln!(
            out,
            "{}Held back: {}{hint}{}",
            Style::new().dimmed().render(),
            parts.join(", "),
            Style::new().dimmed().render_reset()
        )?;
    }
    Ok(())
}
