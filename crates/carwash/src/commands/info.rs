//! `carwash ecosystems` and `carwash history`.

use crate::cli::JsonArgs;
use crate::context::Context;
use crate::table::{Align, Cell, Table};
use anstyle::Style;
use anyhow::Result;
use carwash_core::{fmt, history};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Serialize)]
struct EcosystemOut<'a> {
    id: &'a str,
    name: &'a str,
    markers: &'a [String],
    lockfiles: &'a [String],
    artifacts: Vec<&'a str>,
}

pub fn ecosystems(ctx: &Context, args: &JsonArgs) -> Result<ExitCode> {
    let registry = ctx.registry();
    let list: Vec<EcosystemOut> = registry
        .ecosystems()
        .iter()
        .map(|eco| EcosystemOut {
            id: &eco.key,
            name: &eco.name,
            markers: &eco.markers,
            lockfiles: &eco.lockfiles,
            artifacts: eco
                .rules
                .iter()
                .map(|&r| registry.rule(r).path.as_str())
                .collect(),
        })
        .collect();
    let mut out = anstream::stdout().lock();
    if args.json {
        serde_json::to_writer_pretty(&mut out, &list)?;
        writeln!(out)?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = Table::new(&[
        ("ID", Align::Left),
        ("NAME", Align::Left),
        ("MARKERS", Align::Left),
        ("ARTIFACTS", Align::Left),
    ]);
    for eco in &list {
        table.push(vec![
            Cell::styled(eco.id, Style::new().bold()),
            Cell::new(eco.name),
            Cell::styled(eco.markers.join(" "), Style::new().dimmed()),
            Cell::new(eco.artifacts.join(" ")),
        ]);
    }
    table.write(&mut out)?;
    writeln!(out, "\n{} ecosystems", list.len())?;
    Ok(ExitCode::SUCCESS)
}

#[derive(Serialize)]
struct HistoryOut {
    total_bytes: u64,
    cleans: usize,
    by_ecosystem: BTreeMap<String, u64>,
    records: Vec<history::Record>,
}

pub fn history(ctx: &Context, args: &JsonArgs) -> Result<ExitCode> {
    let records = ctx
        .dirs
        .as_ref()
        .map(|d| history::read(&d.history_file()))
        .unwrap_or_default();
    let total: u64 = records.iter().map(|r| r.bytes).sum();
    let mut by_ecosystem: BTreeMap<String, u64> = BTreeMap::new();
    for record in &records {
        *by_ecosystem
            .entry(record.ecosystem.clone().unwrap_or_else(|| "other".into()))
            .or_default() += record.bytes;
    }
    let mut out = anstream::stdout().lock();
    if args.json {
        let output = HistoryOut {
            total_bytes: total,
            cleans: records.len(),
            by_ecosystem,
            records,
        };
        serde_json::to_writer_pretty(&mut out, &output)?;
        writeln!(out)?;
        return Ok(ExitCode::SUCCESS);
    }
    if records.is_empty() {
        writeln!(out, "Nothing reclaimed yet. Try `carwash clean --dry-run`.")?;
        return Ok(ExitCode::SUCCESS);
    }
    let bold = Style::new().bold();
    writeln!(
        out,
        "{}{}{} reclaimed across {}.\n",
        bold.render(),
        fmt::bytes(total),
        bold.render_reset(),
        super::directories(records.len())
    )?;
    let mut ranked: Vec<(&String, &u64)> = by_ecosystem.iter().collect();
    ranked.sort_by_key(|(_, bytes)| std::cmp::Reverse(**bytes));
    let mut table = Table::new(&[("ECOSYSTEM", Align::Left), ("FREED", Align::Right)]);
    for (eco, bytes) in ranked {
        table.push(vec![Cell::new(eco.as_str()), Cell::new(fmt::bytes(*bytes))]);
    }
    table.write(&mut out)?;

    writeln!(out, "\nRecent:")?;
    let now = SystemTime::now();
    let mut recent = Table::new(&[
        ("WHEN", Align::Right),
        ("FREED", Align::Right),
        ("PATH", Align::Left),
    ]);
    for record in records.iter().rev().take(10) {
        let when = UNIX_EPOCH + Duration::from_secs(record.at);
        recent.push(vec![
            Cell::styled(
                format!(
                    "{} ago",
                    fmt::age(now.duration_since(when).unwrap_or_default())
                ),
                Style::new().dimmed(),
            ),
            Cell::new(fmt::bytes(record.bytes)),
            Cell::new(record.path.display().to_string()),
        ]);
    }
    recent.write(&mut out)?;
    Ok(ExitCode::SUCCESS)
}
