//! Non-interactive subcommands.

pub mod clean;
pub mod info;
pub mod scan;

use anstyle::{AnsiColor, Style};
use carwash_core::select::Hold;
use carwash_core::{Artifact, GitState};
use std::path::Path;
use std::time::SystemTime;

use crate::table::{Align, Cell, Table};

const GB: u64 = 1_000_000_000;

pub(crate) fn size_style(bytes: u64) -> Style {
    if bytes >= 10 * GB {
        AnsiColor::Red.on_default().bold()
    } else if bytes >= GB {
        AnsiColor::Yellow.on_default().bold()
    } else if bytes >= 100_000_000 {
        Style::new()
    } else {
        Style::new().dimmed()
    }
}

pub(crate) fn display_path(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
        _ => path.display().to_string(),
    }
}

/// Status word and style for an artifact row.
pub(crate) fn status(hold: Option<Hold>) -> Cell {
    match hold {
        None => Cell::styled("ready", AnsiColor::Green.on_default()),
        Some(Hold::Recent) => Cell::styled("recent", AnsiColor::Blue.on_default()),
        Some(Hold::Review) => Cell::styled("review", AnsiColor::Yellow.on_default()),
        Some(Hold::Protected) => Cell::styled("protected", AnsiColor::Red.on_default()),
    }
}

/// Short notes explaining sizes or safety.
pub(crate) fn notes(artifact: &Artifact) -> String {
    let mut notes = Vec::new();
    if let Some(size) = artifact.size {
        if size.on_disk > 0 && size.shared() * 2 >= size.on_disk {
            notes.push(format!("shared {}%", size.shared() * 100 / size.on_disk));
        }
        if size.errors > 0 {
            notes.push(format!("{} unreadable", size.errors));
        }
    }
    if let GitState::Tracked(files) = artifact.git {
        notes.push(format!("{files} tracked"));
    }
    if artifact.outside_root {
        notes.push("enclosing".into());
    }
    notes.join(", ")
}

pub(crate) fn artifact_table(
    artifacts: &[&Artifact],
    root: &Path,
    registry: &carwash_core::Registry,
    hold: impl Fn(&Artifact) -> Option<Hold>,
    now: SystemTime,
) -> Table {
    let mut table = Table::new(&[
        ("FREED", Align::Right),
        ("ON DISK", Align::Right),
        ("AGE", Align::Right),
        ("KIND", Align::Left),
        ("ECOSYSTEM", Align::Left),
        ("STATUS", Align::Left),
        ("NOTES", Align::Left),
        ("PATH", Align::Left),
    ]);
    for artifact in artifacts {
        let (freed, on_disk) = match artifact.size {
            Some(size) => (
                Cell::styled(
                    carwash_core::fmt::bytes(size.reclaimable),
                    size_style(size.reclaimable),
                ),
                Cell::styled(
                    carwash_core::fmt::bytes(size.on_disk),
                    Style::new().dimmed(),
                ),
            ),
            None => (Cell::new("-"), Cell::new("-")),
        };
        let age = carwash_core::select::age(artifact, now)
            .map(carwash_core::fmt::age)
            .unwrap_or_else(|| "-".into());
        let ecosystem = artifact
            .ecosystem
            .map(|e| registry.ecosystem(e).key.clone())
            .unwrap_or_default();
        table.push(vec![
            freed,
            on_disk,
            Cell::styled(age, Style::new().dimmed()),
            Cell::new(artifact.kind.label()),
            Cell::new(ecosystem),
            status(hold(artifact)),
            Cell::styled(notes(artifact), Style::new().dimmed()),
            Cell::new(display_path(&artifact.path, root)),
        ]);
    }
    table
}

/// `1 directory`, `1,234 directories`.
pub(crate) fn directories(n: usize) -> String {
    match n {
        1 => "1 directory".into(),
        n => format!(
            "{} directories",
            count(u64::try_from(n).unwrap_or(u64::MAX))
        ),
    }
}

/// Formats a count with thousands separators.
pub(crate) fn count(n: impl Into<u64>) -> String {
    let digits = n.into().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_have_separators() {
        assert_eq!(count(0u64), "0");
        assert_eq!(count(999u64), "999");
        assert_eq!(count(1_234u64), "1,234");
        assert_eq!(count(1_234_567u64), "1,234,567");
    }
}
