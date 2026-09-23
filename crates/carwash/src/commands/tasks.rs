//! `carwash tasks` and `carwash run`: project scripts across every ecosystem.

use super::{count, display_path};
use crate::cli::{RunArgs, TasksArgs, WalkArgs};
use crate::context::Context;
use crate::table::{Align, Cell, Table};
use anstyle::{AnsiColor, Style};
use anyhow::{Context as _, Result, bail};
use carwash_core::tasks::{self, Task};
use carwash_core::{EcoId, Project};
use globset::{Glob, GlobSetBuilder};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct ProjectTasks {
    project: Project,
    tasks: Vec<Task>,
}

/// Discovers projects under the walk root (without sizes or git) and their tasks.
fn collect(
    ctx: &Context,
    walk: &WalkArgs,
    ecosystems: &[EcoId],
) -> Result<(PathBuf, Vec<ProjectTasks>)> {
    let (root, options) = ctx.scan_options(walk, false, false)?;
    let snapshot = ctx.run_scan(&root, &options, true);
    let registry = ctx.registry();
    let mut out: Vec<ProjectTasks> = snapshot
        .projects
        .into_iter()
        .filter(|p| !p.outside_root)
        .filter(|p| ecosystems.is_empty() || p.ecosystems.iter().any(|e| ecosystems.contains(e)))
        .map(|project| {
            let tasks = tasks::discover(&project.path, &project.ecosystems, registry);
            ProjectTasks { project, tasks }
        })
        .filter(|p| !p.tasks.is_empty())
        .collect();
    out.sort_by(|a, b| a.project.path.cmp(&b.project.path));
    Ok((root, out))
}

fn ecosystem_ids(ctx: &Context, keys: &[String]) -> Result<Vec<EcoId>> {
    keys.iter()
        .map(|key| {
            ctx.registry()
                .find(key)
                .with_context(|| format!("unknown ecosystem `{key}` (see `carwash ecosystems`)"))
        })
        .collect()
}

#[derive(Serialize)]
struct ProjectOut<'a> {
    path: &'a Path,
    name: &'a str,
    tasks: &'a [Task],
}

pub fn list(ctx: &Context, args: &TasksArgs) -> Result<ExitCode> {
    let ecosystems = ecosystem_ids(ctx, &args.ecosystems)?;
    let (root, projects) = collect(ctx, &args.walk, &ecosystems)?;
    let mut out = anstream::stdout().lock();
    if args.json {
        let list: Vec<ProjectOut> = projects
            .iter()
            .map(|p| ProjectOut {
                path: &p.project.path,
                name: &p.project.name,
                tasks: &p.tasks,
            })
            .collect();
        serde_json::to_writer_pretty(&mut out, &list)?;
        writeln!(out)?;
        return Ok(ExitCode::SUCCESS);
    }
    if projects.is_empty() {
        writeln!(out, "No tasks found under {}.", root.display())?;
        return Ok(ExitCode::SUCCESS);
    }
    let bold = Style::new().bold();
    let dim = Style::new().dimmed();
    if projects.len() == 1 || args.all {
        for entry in &projects {
            writeln!(
                out,
                "{}{}{}  {}{}{}",
                bold.render(),
                entry.project.name,
                bold.render_reset(),
                dim.render(),
                display_path(&entry.project.path, &root),
                dim.render_reset()
            )?;
            let mut table = Table::new(&[
                ("  TASK", Align::Left),
                ("COMMAND", Align::Left),
                ("SOURCE", Align::Left),
                ("DESCRIPTION", Align::Left),
            ]);
            for task in &entry.tasks {
                table.push(vec![
                    Cell::styled(format!("  {}", task.name), AnsiColor::Cyan.on_default()),
                    Cell::new(task.command_line()),
                    Cell::styled(task.source.clone(), dim),
                    Cell::styled(task.description.clone().unwrap_or_default(), dim),
                ]);
            }
            table.write(&mut out)?;
            writeln!(out)?;
        }
        return Ok(ExitCode::SUCCESS);
    }

    let mut by_name: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for entry in &projects {
        for task in &entry.tasks {
            let counts = by_name.entry(task.name.as_str()).or_default();
            if task.standard {
                counts.1 += 1;
            } else {
                counts.0 += 1;
            }
        }
    }
    let mut ranked: Vec<(&str, (usize, usize))> = by_name.into_iter().collect();
    ranked
        .sort_by_key(|(name, (scripts, standard))| (std::cmp::Reverse(scripts + standard), *name));
    let mut table = Table::new(&[
        ("TASK", Align::Left),
        ("PROJECTS", Align::Right),
        ("FROM SCRIPTS", Align::Right),
    ]);
    for (name, (scripts, standard)) in ranked.iter().take(40) {
        table.push(vec![
            Cell::styled(*name, AnsiColor::Cyan.on_default()),
            Cell::new(count((scripts + standard) as u64)),
            Cell::styled(count(*scripts as u64), dim),
        ]);
    }
    table.write(&mut out)?;
    writeln!(
        out,
        "\n{} tasks across {} projects. Run one with `carwash run <task>`; list per project with --all.",
        count(ranked.len() as u64),
        count(projects.len() as u64)
    )?;
    Ok(ExitCode::SUCCESS)
}

#[derive(Debug)]
struct Outcome {
    label: String,
    status: Result<i32, String>,
    elapsed: Duration,
}

pub fn run(ctx: &Context, args: &RunArgs) -> Result<ExitCode> {
    let ecosystems = ecosystem_ids(ctx, &args.ecosystems)?;
    let (root, projects) = collect(ctx, &args.walk, &ecosystems)?;
    let filter = if args.filter.is_empty() {
        None
    } else {
        let mut builder = GlobSetBuilder::new();
        for pattern in &args.filter {
            builder
                .add(Glob::new(pattern).with_context(|| format!("invalid --filter `{pattern}`"))?);
        }
        Some(builder.build()?)
    };
    let jobs: Vec<(String, Task)> = projects
        .into_iter()
        .filter(|p| {
            let rel = display_path(&p.project.path, &root);
            filter.as_ref().is_none_or(|f| f.is_match(&rel))
        })
        .filter_map(|p| {
            let label = if p.project.path == root {
                p.project.name.clone()
            } else {
                display_path(&p.project.path, &root)
            };
            p.tasks
                .into_iter()
                .find(|t| t.name == args.task)
                .map(|t| (label, t))
        })
        .collect();
    if jobs.is_empty() {
        bail!(
            "no project under {} has a task named `{}` (see `carwash tasks`)",
            root.display(),
            args.task
        );
    }

    let mut out = anstream::stdout().lock();
    if args.dry_run {
        for (label, task) in &jobs {
            writeln!(out, "{label}: {}", task.command_line())?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    drop(out);

    let parallel = args.jobs.max(1);
    let color = std::io::stdout().is_terminal();
    let width = jobs.iter().map(|(l, _)| l.len()).max().unwrap_or(0).min(40);
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let outcomes = Mutex::new(Vec::new());
    let print = Mutex::new(());

    std::thread::scope(|scope| {
        for worker in 0..parallel.min(jobs.len()) {
            let (jobs, next, failed, outcomes, print) = (&jobs, &next, &failed, &outcomes, &print);
            scope.spawn(move || {
                loop {
                    if args.fail_fast && failed.load(Ordering::Relaxed) {
                        return;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some((label, task)) = jobs.get(index) else {
                        return;
                    };
                    let started = Instant::now();
                    let status = if parallel == 1 {
                        run_inherited(label, task, print)
                    } else {
                        run_prefixed(label, task, width, worker, color, print)
                    };
                    if !matches!(status, Ok(0)) {
                        failed.store(true, Ordering::Relaxed);
                    }
                    outcomes.lock().expect("outcomes lock").push(Outcome {
                        label: label.clone(),
                        status,
                        elapsed: started.elapsed(),
                    });
                }
            });
        }
    });

    let mut outcomes = outcomes.into_inner().expect("outcomes lock");
    outcomes.sort_by(|a, b| a.label.cmp(&b.label));
    let mut out = anstream::stdout().lock();
    writeln!(out)?;
    let mut table = Table::new(&[
        ("PROJECT", Align::Left),
        ("RESULT", Align::Left),
        ("TIME", Align::Right),
    ]);
    let mut failures = 0;
    for outcome in &outcomes {
        let result = match &outcome.status {
            Ok(0) => Cell::styled("ok", AnsiColor::Green.on_default()),
            Ok(code) => {
                failures += 1;
                Cell::styled(format!("exit {code}"), AnsiColor::Red.on_default().bold())
            }
            Err(error) => {
                failures += 1;
                Cell::styled(error.clone(), AnsiColor::Red.on_default().bold())
            }
        };
        table.push(vec![
            Cell::new(outcome.label.clone()),
            result,
            Cell::styled(
                format!("{:.1}s", outcome.elapsed.as_secs_f64()),
                Style::new().dimmed(),
            ),
        ]);
    }
    table.write(&mut out)?;
    let skipped = jobs.len() - outcomes.len();
    let bold = Style::new().bold();
    write!(
        out,
        "\n{}`{}`{}: {} succeeded, {} failed",
        bold.render(),
        args.task,
        bold.render_reset(),
        outcomes.len() - failures,
        failures
    )?;
    if skipped > 0 {
        write!(out, ", {skipped} skipped (--fail-fast)")?;
    }
    writeln!(out)?;
    Ok(if failures == 0 && skipped == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn command(task: &Task) -> Command {
    let mut cmd = Command::new(&task.program);
    cmd.args(&task.args)
        .current_dir(&task.cwd)
        .stdin(Stdio::null());
    cmd
}

/// Sequential mode: the task owns the terminal, so colors and progress bars work natively.
fn run_inherited(label: &str, task: &Task, print: &Mutex<()>) -> Result<i32, String> {
    {
        let _guard = print.lock().expect("print lock");
        let accent = AnsiColor::Cyan.on_default().bold();
        anstream::println!(
            "\n{}▶ {label}{} {}",
            accent.render(),
            accent.render_reset(),
            task.command_line()
        );
    }
    command(task)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map(|s| s.code().unwrap_or(-1))
        .map_err(|e| spawn_error(task, &e))
}

fn spawn_error(task: &Task, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        format!("`{}` not found", task.program)
    } else {
        error.to_string()
    }
}

const LABEL_COLORS: [AnsiColor; 6] = [
    AnsiColor::Cyan,
    AnsiColor::Magenta,
    AnsiColor::Yellow,
    AnsiColor::Green,
    AnsiColor::Blue,
    AnsiColor::BrightRed,
];

/// Parallel mode: lines are prefixed with the project so interleaved output stays readable.
fn run_prefixed(
    label: &str,
    task: &Task,
    width: usize,
    worker: usize,
    color: bool,
    print: &Mutex<()>,
) -> Result<i32, String> {
    let mut cmd = command(task);
    if color {
        // Tools disable color on pipes; ask them to keep it since we pass it through.
        cmd.env("FORCE_COLOR", "1")
            .env("CLICOLOR_FORCE", "1")
            .env("CARGO_TERM_COLOR", "always");
    }
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| spawn_error(task, &e))?;
    let style = LABEL_COLORS[worker % LABEL_COLORS.len()].on_default();
    let prefix = format!(
        "{}{:<width$}{} │ ",
        style.render(),
        truncate(label, width),
        style.render_reset()
    );
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    std::thread::scope(|scope| {
        for stream in [
            Box::new(stdout) as Box<dyn std::io::Read + Send>,
            Box::new(stderr),
        ] {
            let prefix = &prefix;
            scope.spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let _guard = print.lock().expect("print lock");
                    anstream::println!("{prefix}{line}");
                }
            });
        }
    });
    child
        .wait()
        .map(|s| s.code().unwrap_or(-1))
        .map_err(|e| e.to_string())
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_owned()
    } else {
        let tail: String = text
            .chars()
            .rev()
            .take(width.saturating_sub(1))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("…{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_labels_keep_their_end() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("apps/web/frontend", 8), "…rontend");
    }
}
