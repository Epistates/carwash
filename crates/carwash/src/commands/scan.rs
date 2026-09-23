//! `carwash scan`: report reclaimable artifacts.

use super::{artifact_table, count};
use crate::cli::{ScanArgs, SortKey};
use crate::context::Context;
use crate::report::ScanReport;
use anstyle::Style;
use anyhow::Result;
use carwash_core::select::Hold;
use carwash_core::{Artifact, fmt};
use std::collections::BTreeMap;
use std::io::Write;
use std::process::ExitCode;
use std::time::SystemTime;

const DEFAULT_ROWS: usize = 50;

pub fn run(ctx: &Context, args: &ScanArgs) -> Result<ExitCode> {
    let (root, options) = ctx.scan_options(&args.walk, !args.no_git, !args.no_size)?;
    let snapshot = ctx.run_scan(&root, &options, !args.json);
    let filter = ctx.filter(&args.filter)?;
    let policy = ctx.policy(false, false);
    let registry = ctx.registry();
    let mut out = anstream::stdout().lock();

    if args.json {
        let report = ScanReport::new(&root, &snapshot, registry, &filter, &policy);
        serde_json::to_writer_pretty(&mut out, &report)?;
        writeln!(out)?;
        return Ok(ExitCode::SUCCESS);
    }

    let now = SystemTime::now();
    let mut artifacts: Vec<&Artifact> = snapshot
        .artifacts
        .iter()
        .filter(|a| filter.matches(a, now))
        .collect();
    sort(&mut artifacts, args.sort, now);

    let limit = match args.limit {
        Some(0) => usize::MAX,
        Some(n) => n,
        None => DEFAULT_ROWS,
    };
    let shown: Vec<&Artifact> = artifacts.iter().take(limit).copied().collect();
    let table = artifact_table(&shown, &root, registry, |a| policy.hold(a, now), now);
    if table.is_empty() {
        writeln!(out, "No artifacts found under {}.", root.display())?;
    } else {
        table.write(&mut out)?;
        if artifacts.len() > shown.len() {
            writeln!(
                out,
                "{}… {} more (use --limit 0 to show all){}",
                Style::new().dimmed().render(),
                count(u64::try_from(artifacts.len() - shown.len()).unwrap_or(u64::MAX)),
                Style::new().dimmed().render_reset()
            )?;
        }
    }

    let mut total = 0u64;
    let mut on_disk = 0u64;
    let mut ready = 0u64;
    let mut held: BTreeMap<&'static str, (usize, u64)> = BTreeMap::new();
    for artifact in &artifacts {
        let size = artifact.size.unwrap_or_default();
        total += size.reclaimable;
        on_disk += size.on_disk;
        match policy.hold(artifact, now) {
            None => ready += size.reclaimable,
            Some(hold) => {
                let entry = held.entry(hold_label(hold)).or_default();
                entry.0 += 1;
                entry.1 += size.reclaimable;
            }
        }
    }
    let bold = Style::new().bold();
    let project_count = snapshot.projects.iter().filter(|p| !p.outside_root).count();
    writeln!(out)?;
    writeln!(
        out,
        "{}{} reclaimable{} from {} artifacts in {} projects ({} on disk)",
        bold.render(),
        fmt::bytes(total),
        bold.render_reset(),
        count(u64::try_from(artifacts.len()).unwrap_or(u64::MAX)),
        count(u64::try_from(project_count).unwrap_or(u64::MAX)),
        fmt::bytes(on_disk),
    )?;
    let mut parts = vec![format!("ready to clean {}", fmt::bytes(ready))];
    for (label, (n, bytes)) in &held {
        parts.push(format!("{n} {label} ({})", fmt::bytes(*bytes)));
    }
    writeln!(out, "{}", parts.join(" · "))?;
    writeln!(
        out,
        "{}Scanned {} directories in {:.1}s{}",
        Style::new().dimmed().render(),
        count(snapshot.dirs),
        snapshot.elapsed.as_secs_f64(),
        Style::new().dimmed().render_reset()
    )?;
    if !snapshot.warnings.is_empty() {
        writeln!(
            out,
            "{}{} directories could not be read{}",
            Style::new().dimmed().render(),
            snapshot.warnings.len(),
            Style::new().dimmed().render_reset()
        )?;
    }
    Ok(ExitCode::SUCCESS)
}

fn hold_label(hold: Hold) -> &'static str {
    match hold {
        Hold::Recent => "recently used",
        Hold::Review => "need review",
        Hold::Protected => "protected",
    }
}

pub(crate) fn sort(artifacts: &mut [&Artifact], key: SortKey, now: SystemTime) {
    match key {
        SortKey::Size => {
            artifacts.sort_by_key(|a| std::cmp::Reverse(a.size.map_or(0, |s| s.reclaimable)))
        }
        SortKey::Age => artifacts.sort_by_key(|a| {
            std::cmp::Reverse(carwash_core::select::age(a, now).unwrap_or_default())
        }),
        SortKey::Path => artifacts.sort_by(|a, b| a.path.cmp(&b.path)),
    }
}
