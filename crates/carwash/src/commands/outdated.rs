//! `carwash outdated`: outdated and vulnerable dependencies across projects.

use super::{count, display_path};
use crate::cli::OutdatedArgs;
use crate::context::Context;
use crate::table::{Align, Cell, Table};
use anstyle::{AnsiColor, Style};
use anyhow::{Context as _, Result};
use carwash_core::deps::{Bump, CheckProgress, Checker, Dependency, ProjectSpec, Source};
use carwash_core::{EcoId, Project};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::time::Duration;

pub fn checker(ctx: &Context) -> Checker {
    Checker::new(
        ctx.dirs.as_ref().map(|d| d.cache.join("deps.json")),
        Duration::from_secs(ctx.config.updates.cache_hours * 3600),
    )
}

/// Projects whose ecosystems carwash can check.
pub fn checkable(ctx: &Context, project: &Project) -> bool {
    project
        .ecosystems
        .iter()
        .any(|&e| Source::for_ecosystem(&ctx.registry().ecosystem(e).key).is_some())
}

pub(crate) fn bump_cell(bump: Bump) -> Cell {
    match bump {
        Bump::Major => Cell::styled("major", AnsiColor::Red.on_default().bold()),
        Bump::Minor => Cell::styled("minor", AnsiColor::Yellow.on_default()),
        Bump::Patch => Cell::styled("patch", AnsiColor::Green.on_default()),
        Bump::None => Cell::styled("", Style::new()),
    }
}

pub fn run(ctx: &Context, args: &OutdatedArgs) -> Result<ExitCode> {
    let ecosystems: Vec<EcoId> = args
        .ecosystems
        .iter()
        .map(|key| {
            ctx.registry()
                .find(key)
                .with_context(|| format!("unknown ecosystem `{key}` (see `carwash ecosystems`)"))
        })
        .collect::<Result<_>>()?;
    let (root, options) = ctx.scan_options(&args.walk, false, false)?;
    let snapshot = ctx.run_scan(&root, &options, !args.json);
    let projects: Vec<&Project> = snapshot
        .projects
        .iter()
        .filter(|p| !p.outside_root && checkable(ctx, p))
        .filter(|p| ecosystems.is_empty() || p.ecosystems.iter().any(|e| ecosystems.contains(e)))
        .collect();
    let specs: Vec<ProjectSpec> = projects
        .iter()
        .map(|p| ProjectSpec {
            path: p.path.clone(),
            ecosystems: p.ecosystems.clone(),
        })
        .collect();

    let checker = checker(ctx);
    let progress = CheckProgress::default();
    let bar = if !args.json && std::io::stderr().is_terminal() {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}").expect("valid template"),
        );
        bar.enable_steady_tick(Duration::from_millis(80));
        bar
    } else {
        ProgressBar::hidden()
    };
    let results = std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            checker.check(
                specs.as_slice(),
                ctx.registry(),
                !args.no_vulns,
                args.refresh,
                &progress,
            )
        });
        while !worker.is_finished() {
            bar.set_message(format!(
                "Checking {} packages from {} projects ({} done)",
                progress.total.load(Ordering::Relaxed),
                specs.len(),
                progress.done.load(Ordering::Relaxed)
            ));
            std::thread::sleep(Duration::from_millis(80));
        }
        worker.join().expect("check thread")
    });
    bar.finish_and_clear();
    if let Err(error) = checker.save() {
        tracing::warn!(%error, "cannot save dependency cache");
    }

    let mut out = anstream::stdout().lock();
    if args.json {
        serde_json::to_writer_pretty(&mut out, &results)?;
        writeln!(out)?;
        return Ok(ExitCode::SUCCESS);
    }

    let dim = Style::new().dimmed();
    let bold = Style::new().bold();
    let (mut outdated, mut vulnerable, mut majors) = (0usize, 0usize, 0usize);
    let mut shown_projects = 0usize;
    let checked: Vec<std::path::PathBuf> = results.iter().map(|r| r.path.clone()).collect();
    for (project, result) in projects.iter().zip(&results) {
        let listed: Vec<&Dependency> = result.listed(&checked).collect();
        let rows: Vec<&Dependency> = listed
            .iter()
            .copied()
            .filter(|d| args.all || d.is_outdated() || !d.vulnerabilities.is_empty())
            .collect();
        outdated += listed.iter().filter(|d| d.is_outdated()).count();
        majors += listed.iter().filter(|d| d.bump == Bump::Major).count();
        vulnerable += listed
            .iter()
            .filter(|d| !d.vulnerabilities.is_empty())
            .count();
        if rows.is_empty() && result.errors.is_empty() {
            continue;
        }
        shown_projects += 1;
        let label = if project.path == root {
            project.name.clone()
        } else {
            display_path(&project.path, &root)
        };
        writeln!(out, "{}{label}{}", bold.render(), bold.render_reset())?;
        for error in &result.errors {
            writeln!(out, "  {}{error}{}", dim.render(), dim.render_reset())?;
        }
        if rows.is_empty() {
            continue;
        }
        let mut table = Table::new(&[
            ("  PACKAGE", Align::Left),
            ("CURRENT", Align::Right),
            ("WANTED", Align::Right),
            ("LATEST", Align::Right),
            ("BUMP", Align::Left),
            ("KIND", Align::Left),
            ("ADVISORIES", Align::Left),
        ]);
        for dep in rows {
            let advisories = if dep.vulnerabilities.is_empty() {
                Cell::styled(dep.error.clone().unwrap_or_default(), dim)
            } else {
                Cell::styled(
                    dep.vulnerabilities.join(" "),
                    AnsiColor::Red.on_default().bold(),
                )
            };
            table.push(vec![
                Cell::new(format!("  {}", dep.declared.name)),
                Cell::styled(
                    dep.declared.current.clone().unwrap_or_else(|| "-".into()),
                    dim,
                ),
                Cell::new(dep.wanted.clone().unwrap_or_default()),
                Cell::styled(dep.latest.clone().unwrap_or_default(), bold),
                bump_cell(dep.bump),
                Cell::styled(dep.declared.kind.label(), dim),
                advisories,
            ]);
        }
        table.write(&mut out)?;
        writeln!(out)?;
    }
    if shown_projects == 0 {
        writeln!(
            out,
            "Everything is up to date across {} projects.",
            count(projects.len() as u64)
        )?;
    } else {
        let red = AnsiColor::Red.on_default().bold();
        write!(
            out,
            "{}{} outdated{} ({} major) in {} of {} projects",
            bold.render(),
            count(outdated as u64),
            bold.render_reset(),
            count(majors as u64),
            count(shown_projects as u64),
            count(projects.len() as u64)
        )?;
        if vulnerable > 0 {
            write!(
                out,
                " · {}{} with known vulnerabilities{}",
                red.render(),
                count(vulnerable as u64),
                red.render_reset()
            )?;
        }
        writeln!(out)?;
    }
    Ok(if args.exit_code && (outdated > 0 || vulnerable > 0) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
