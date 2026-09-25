//! Append-only journal of reclaimed space.

use crate::clean::DeleteMode;
use crate::model::ArtifactKind;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// Unix seconds.
    pub at: u64,
    pub path: PathBuf,
    pub bytes: u64,
    pub kind: ArtifactKind,
    #[serde(default)]
    pub ecosystem: Option<String>,
    pub mode: DeleteMode,
}

impl Record {
    pub fn now(
        path: PathBuf,
        bytes: u64,
        kind: ArtifactKind,
        ecosystem: Option<String>,
        mode: DeleteMode,
    ) -> Self {
        Self {
            at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            path,
            bytes,
            kind,
            ecosystem,
            mode,
        }
    }
}

pub fn append(file: &Path, records: &[Record]) -> std::io::Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?;
    let mut buf = Vec::new();
    for record in records {
        serde_json::to_writer(&mut buf, record).map_err(std::io::Error::other)?;
        buf.push(b'\n');
    }
    out.write_all(&buf)
}

/// Reads every well-formed record; malformed lines are skipped.
pub fn read(file: &Path) -> Vec<Record> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("h/history.jsonl");
        let record = Record::now(
            PathBuf::from("/p/target"),
            1_000,
            ArtifactKind::Build,
            Some("rust".into()),
            DeleteMode::Permanent,
        );
        append(&file, std::slice::from_ref(&record)).unwrap();
        append(&file, &[record]).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap()
            .write_all(b"garbage\n")
            .unwrap();
        let records = read(&file);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].bytes, 1_000);
    }
}
