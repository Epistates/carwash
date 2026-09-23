//! `carwash caches`: per-user caches outside projects.

use super::size_style;
use crate::cli::{CachesAction, CachesArgs};
use crate::context::Context;
use crate::table::{Align, Cell, Table};
use anstyle::{AnsiColor, Style};
use anyhow::{Context as _, Result, bail};
use carwash_core::caches::{self, GlobalCache};
use carwash_core::clean::{CleanItem, CleanOptions, DeleteMode};
use carwash_core::history::{self, Record};
use carwash_core::{ArtifactId, ArtifactKind, Cancel, fmt};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::{BufRead, IsTerminal, Write};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Caches present on this machine, with user overrides from `caches.toml`.
pub fn present(ctx: &Context) -> Result<Vec<GlobalCache>> {
    let specs = match ctx.dirs.as_ref().map(|d| d.caches_file()) {
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
                    cache.size = Some(ctx.engine.measure_path(&cache.path, &Cancel::new()));
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
}

fn action(cache: &GlobalCache) -> (String, Style) {
    let dim = Style::new().dimmed();
    match (&cache.prune, cache.prune_task(), cache.deletable) {
        (Some(_), Some(task), _) => (task.command_line(), Style::new()),
        (Some(argv), None, true) => (format!("delete ({} not installed)", argv[0]), dim),
        (Some(argv), None, false) => (
            format!("needs `{}`", argv[0]),
            AnsiColor::Yellow.on_default(),
        ),
        (None, _, true) => ("delete".into(), Style::new()),
        (None, _, false) => ("none".into(), dim),
    }
}

pub fn run(ctx: &Context, args: &CachesArgs) -> Result<ExitCode> {
    let mut caches = present(ctx)?;
    match &args.action {
        None => list(ctx, &mut caches, args.json),
        Some(CachesAction::Clean {
            ids,
            yes,
            dry_run,
            delete,
        }) => clean(ctx, &mut caches, ids, *yes, *dry_run, *delete),
    }
}

fn list(ctx: &Context, caches: &mut [GlobalCache], json: bool) -> Result<ExitCode> {
    measure(ctx, caches, !json);
    caches.sort_by_key(|c| std::cmp::Reverse(c.size.map_or(0, |s| s.on_disk)));
    let mut out = anstream::stdout().lock();
    if json {
        serde_json::to_writer_pretty(&mut out, &caches)?;
        writeln!(out)?;
        return Ok(ExitCode::SUCCESS);
    }
    if caches.is_empty() {
        writeln!(out, "No known caches found.")?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = Table::new(&[
        ("SIZE", Align::Right),
        ("ID", Align::Left),
        ("CACHE", Align::Left),
        ("CLEAN WITH", Align::Left),
    ]);
    let mut total = 0;
    for cache in caches.iter() {
        let bytes = cache.size.map_or(0, |s| s.on_disk);
        total += bytes;
        let (action, style) = action(cache);
        table.push(vec![
            Cell::styled(fmt::bytes(bytes), size_style(bytes)),
            Cell::styled(cache.id.clone(), Style::new().dimmed()),
            Cell::new(cache.name.clone()),
            Cell::styled(action, style),
        ]);
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
    Ok(ExitCode::SUCCESS)
}

fn clean(
    ctx: &Context,
    caches: &mut [GlobalCache],
    ids: &[String],
    yes: bool,
    dry_run: bool,
    force_delete: bool,
) -> Result<ExitCode> {
    let mut selected: Vec<GlobalCache> = Vec::new();
    for id in ids {
        let matching: Vec<&GlobalCache> = caches
            .iter()
            .filter(|c| &c.id == id || c.id.starts_with(&format!("{id}/")))
            .collect();
        if matching.is_empty() {
            bail!("no cache `{id}` on this machine (see `carwash caches`)");
        }
        selected.extend(matching.into_iter().cloned());
    }
    measure(ctx, &mut selected, true);

    let mut out = anstream::stdout().lock();
    let mut plan: Vec<(GlobalCache, Option<carwash_core::tasks::Task>)> = Vec::new();
    for cache in selected {
        let task = if force_delete {
            None
        } else {
            cache.prune_task()
        };
        if task.is_none() && !cache.deletable {
            let program = cache.prune.as_ref().map_or("its tool", |p| p[0].as_str());
            bail!(
                "{} can only be cleaned with `{program}`, which is not installed",
                cache.name
            );
        }
        let how = task
            .as_ref()
            .map_or_else(|| "delete".to_owned(), |t| t.command_line());
        writeln!(
            out,
            "{:>9}  {}  {}{how}{}",
            fmt::bytes(cache.size.map_or(0, |s| s.on_disk)),
            cache.name,
            Style::new().dimmed().render(),
            Style::new().dimmed().render_reset()
        )?;
        plan.push((cache, task));
    }
    if dry_run {
        writeln!(out, "Dry run: nothing was changed.")?;
        return Ok(ExitCode::SUCCESS);
    }
    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!(
                "refusing to clean without confirmation in a non-interactive session; pass --yes"
            );
        }
        write!(out, "Clean {} caches? [y/N] ", plan.len())?;
        out.flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "Yes") {
            writeln!(out, "Nothing changed.")?;
            return Ok(ExitCode::SUCCESS);
        }
    }
    drop(out);

    let mut failures = 0;
    let mut records = Vec::new();
    for (cache, task) in plan {
        let before = cache.size.map_or(0, |s| s.on_disk);
        let ok = match task {
            Some(task) => {
                anstream::println!("▶ {}", task.command_line());
                let status = Command::new(&task.program)
                    .args(&task.args)
                    .current_dir(&task.cwd)
                    .stdin(Stdio::inherit())
                    .status();
                matches!(status, Ok(s) if s.success())
            }
            None => {
                let Some(parent) = cache.path.parent() else {
                    continue;
                };
                let report = ctx.engine.clean(
                    &[CleanItem {
                        id: ArtifactId(0),
                        path: cache.path.clone(),
                        expected_bytes: before,
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
        };
        if ok {
            let after = if cache.path.exists() {
                ctx.engine.measure_path(&cache.path, &Cancel::new()).on_disk
            } else {
                0
            };
            let freed = before.saturating_sub(after);
            anstream::println!("  freed {} from {}", fmt::bytes(freed), cache.name);
            records.push(Record::now(
                cache.path.clone(),
                freed,
                ArtifactKind::Cache,
                cache.ecosystem.clone(),
                DeleteMode::Permanent,
            ));
        } else {
            failures += 1;
            anstream::eprintln!("  failed to clean {}", cache.name);
        }
    }
    if let Some(dirs) = &ctx.dirs
        && let Err(error) = history::append(&dirs.history_file(), &records)
    {
        tracing::warn!(%error, "cannot write history");
    }
    let total: u64 = records.iter().map(|r| r.bytes).sum();
    anstream::println!("Freed {} in total.", fmt::bytes(total));
    Ok(if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
