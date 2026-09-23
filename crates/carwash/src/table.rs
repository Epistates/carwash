//! Minimal aligned, styled table output for the CLI.

use anstyle::Style;
use std::io::{self, Write};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

#[derive(Debug, Clone, Default)]
pub struct Cell {
    pub text: String,
    pub style: Style,
}

impl Cell {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::new(),
        }
    }

    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

#[derive(Debug)]
pub struct Table {
    headers: Vec<&'static str>,
    align: Vec<Align>,
    rows: Vec<Vec<Cell>>,
}

impl Table {
    pub fn new(columns: &[(&'static str, Align)]) -> Self {
        Self {
            headers: columns.iter().map(|(h, _)| *h).collect(),
            align: columns.iter().map(|(_, a)| *a).collect(),
            rows: Vec::new(),
        }
    }

    pub fn push(&mut self, row: Vec<Cell>) {
        debug_assert_eq!(row.len(), self.headers.len());
        self.rows.push(row);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn write(&self, out: &mut impl Write) -> io::Result<()> {
        let mut widths: Vec<usize> = self.headers.iter().map(|h| h.width()).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.text.width());
            }
        }
        let header = Style::new().bold().dimmed();
        let headers: Vec<Cell> = self
            .headers
            .iter()
            .map(|h| Cell::styled(*h, header))
            .collect();
        self.write_row(out, &headers, &widths)?;
        for row in &self.rows {
            self.write_row(out, row, &widths)?;
        }
        Ok(())
    }

    fn write_row(&self, out: &mut impl Write, row: &[Cell], widths: &[usize]) -> io::Result<()> {
        let last = row.len().saturating_sub(1);
        for (i, cell) in row.iter().enumerate() {
            let pad = widths[i].saturating_sub(cell.text.width());
            let styled = format!(
                "{}{}{}",
                cell.style.render(),
                cell.text,
                cell.style.render_reset()
            );
            match self.align[i] {
                Align::Right => write!(out, "{}{styled}", " ".repeat(pad))?,
                Align::Left if i == last => write!(out, "{styled}")?,
                Align::Left => write!(out, "{styled}{}", " ".repeat(pad))?,
            }
            if i != last {
                write!(out, "  ")?;
            }
        }
        writeln!(out)
    }
}
