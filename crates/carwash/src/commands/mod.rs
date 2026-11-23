//! Non-interactive subcommands.

pub mod caches;
pub mod clean;
pub mod info;
pub mod os;
pub mod outdated;
pub mod pick;
pub mod scan;
pub mod tasks;

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

/// `numbered` adds a `#` column counting rows from 1, as `clean --pick` refers to them.
pub(crate) fn artifact_table(
    artifacts: &[&Artifact],
    root: &Path,
    registry: &carwash_core::Registry,
    hold: impl Fn(&Artifact) -> Option<Hold>,
    now: SystemTime,
    numbered: bool,
) -> Table {
    let mut columns = Vec::new();
    if numbered {
        columns.push(("#", Align::Right));
    }
    columns.extend([
        ("FREED", Align::Right),
        ("ON DISK", Align::Right),
        ("AGE", Align::Right),
        ("KIND", Align::Left),
        ("ECOSYSTEM", Align::Left),
        ("STATUS", Align::Left),
        ("NOTES", Align::Left),
        ("PATH", Align::Left),
    ]);
    let mut table = Table::new(&columns);
    for (row, artifact) in artifacts.iter().enumerate() {
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
        let mut cells = Vec::new();
        if numbered {
            cells.push(Cell::styled((row + 1).to_string(), Style::new().dimmed()));
        }
        cells.extend([
            freed,
            on_disk,
            Cell::styled(age, Style::new().dimmed()),
            Cell::new(artifact.kind.label()),
            Cell::new(ecosystem),
            status(hold(artifact)),
            Cell::styled(notes(artifact), Style::new().dimmed()),
            Cell::new(display_path(&artifact.path, root)),
        ]);
        table.push(cells);
    }
    table
}

/// How free space changed across a clean.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FreeChange {
    pub before: u64,
    pub after: u64,
    /// What the clean removed, by its own count.
    pub freed: u64,
    /// The filesystem is one where free space can keep rising after a large deletion.
    pub settles_late: bool,
}

impl FreeChange {
    /// Said when [`FreeChange::may_rise`].
    pub const NOTE: &str = "Free space was read right after cleaning. On APFS it can keep rising for a while after a large clean, so a figure below what was freed is not a failure. Check again in a few minutes.";

    /// `Free space: 77.7 GB → 81.3 GB (3.6 GB more)`.
    pub fn line(&self) -> String {
        let change = match self.after.cmp(&self.before) {
            std::cmp::Ordering::Greater => {
                format!(
                    "{} more",
                    carwash_core::fmt::bytes(self.after - self.before)
                )
            }
            std::cmp::Ordering::Less => {
                format!(
                    "{} less",
                    carwash_core::fmt::bytes(self.before - self.after)
                )
            }
            std::cmp::Ordering::Equal => "unchanged".to_owned(),
        };
        format!(
            "Free space: {} → {} ({change})",
            carwash_core::fmt::bytes(self.before),
            carwash_core::fmt::bytes(self.after)
        )
    }

    /// Whether the reading is likely to rise further: a large clean, on a filesystem that
    /// settles late, that so far shows clearly less than was freed.
    pub fn may_rise(&self) -> bool {
        let gained = self.after.saturating_sub(self.before);
        self.settles_late && self.freed >= GB && gained < self.freed / 10 * 9
    }
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
    fn free_space_change_is_spelled_out_and_flagged_only_when_it_may_still_rise() {
        let change = |before, after, freed, settles_late| FreeChange {
            before,
            after,
            freed,
            settles_late,
        };
        // The clean-up this was written for: 15.6 GB freed, 3.6 GB showing right after.
        let lagging = change(77_700_000_000, 81_300_000_000, 15_600_000_000, true);
        assert_eq!(
            lagging.line(),
            "Free space: 77.7 GB → 81.3 GB (3.60 GB more)"
        );
        assert!(lagging.may_rise());
        // Already there, a small clean, or a filesystem that reports at once: nothing to add.
        assert!(!change(77_700_000_000, 93_300_000_000, 15_600_000_000, true).may_rise());
        assert!(!change(10 * GB, 10 * GB, 500_000_000, true).may_rise());
        assert!(!change(77_700_000_000, 81_300_000_000, 15_600_000_000, false).may_rise());
        // Something else wrote more than was freed.
        let shrunk = change(5 * GB, 4 * GB, 2 * GB, true);
        assert_eq!(
            shrunk.line(),
            "Free space: 5.00 GB → 4.00 GB (1.00 GB less)"
        );
        assert!(shrunk.may_rise());
        assert!(change(GB, GB, 0, false).line().ends_with("(unchanged)"));
    }

    #[test]
    fn counts_have_separators() {
        assert_eq!(count(0u64), "0");
        assert_eq!(count(999u64), "999");
        assert_eq!(count(1_234u64), "1,234");
        assert_eq!(count(1_234_567u64), "1,234,567");
    }
}
