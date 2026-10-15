//! `scan` numbers its rows and remembers them; `clean --pick` resolves numbers and paths.

use crate::cli::WalkArgs;
use crate::context::Context;
use anyhow::{Context as _, Result, bail};
use carwash_core::Artifact;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const VERSION: u32 = 1;

/// The numbered list a scan printed: row `n` is `artifacts[n - 1]`.
#[derive(Serialize, Deserialize)]
struct LastScan {
    version: u32,
    created: u64,
    /// Walk settings with `path` set to the canonical root, to scan the same tree again.
    walk: WalkArgs,
    artifacts: Vec<PathBuf>,
}

/// Remembers `artifacts` (in display order) as the most recent scan of `root`.
pub fn save(ctx: &Context, root: &Path, walk: &WalkArgs, artifacts: &[&Artifact]) {
    let Some(dirs) = &ctx.dirs else { return };
    let scan = LastScan {
        version: VERSION,
        created: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        walk: WalkArgs {
            path: root.to_path_buf(),
            ..walk.clone()
        },
        artifacts: artifacts.iter().map(|a| a.path.clone()).collect(),
    };
    let result = serde_json::to_vec(&scan)
        .map_err(std::io::Error::other)
        .and_then(|json| carwash_core::paths::write_atomic(&dirs.last_scan_file(), &json));
    if let Err(error) = result {
        tracing::warn!(%error, "cannot save the scan for --pick");
    }
}

/// What `--pick` resolved to: the tree to scan again and the artifact paths chosen in it.
pub struct Picks {
    pub walk: WalkArgs,
    pub paths: Vec<PathBuf>,
}

enum Token {
    Rows(usize, usize),
    Path(PathBuf),
}

fn parse(token: &str) -> Result<Token> {
    let number = |s: &str| -> Option<usize> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    let rows = match token.split_once('-') {
        None => number(token).map(|n| (n, n)),
        Some((a, b)) => number(a).zip(number(b)),
    };
    match rows {
        Some((0, _)) | Some((_, 0)) => bail!("--pick numbers start at 1, got `{token}`"),
        Some((a, b)) if a > b => bail!("--pick range `{token}` runs backwards"),
        Some((a, b)) => Ok(Token::Rows(a, b)),
        None => Ok(Token::Path(PathBuf::from(token))),
    }
}

/// Resolves `--pick` values against the last scan (numbers) and the filesystem (paths).
/// Numbers scan the last scan's tree again; paths alone scan `walk`.
pub fn resolve(ctx: &Context, values: &[String], walk: &WalkArgs) -> Result<Picks> {
    let tokens = values
        .iter()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(parse)
        .collect::<Result<Vec<_>>>()?;
    if tokens.is_empty() {
        bail!("--pick needs row numbers from `carwash scan` or artifact paths");
    }
    let mut walk = walk.clone();
    let mut last: Option<LastScan> = None;
    if tokens.iter().any(|t| matches!(t, Token::Rows(..))) {
        let scan = load(ctx)?;
        // The default PATH is `.`; anything else must be the tree the numbers came from.
        if walk.path != Path::new(".") {
            let asked = std::fs::canonicalize(&walk.path)
                .with_context(|| format!("cannot open {}", walk.path.display()))?;
            if asked != scan.walk.path {
                bail!(
                    "the numbers refer to the last scan, of {}; scan {} first or pass paths",
                    scan.walk.path.display(),
                    asked.display()
                );
            }
        }
        walk = scan.walk.clone();
        last = Some(scan);
    }

    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for token in tokens {
        match token {
            Token::Rows(first, last_row) => {
                let scan = last.as_ref().expect("loaded for row numbers");
                if last_row > scan.artifacts.len() {
                    bail!(
                        "--pick {last_row} is out of range: the last scan listed {} artifacts",
                        scan.artifacts.len()
                    );
                }
                for path in &scan.artifacts[first - 1..last_row] {
                    if seen.insert(path.clone()) {
                        paths.push(path.clone());
                    }
                }
            }
            Token::Path(path) => {
                // Missing paths stay as given; the clean reports them as not found.
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                if seen.insert(path.clone()) {
                    paths.push(path);
                }
            }
        }
    }
    Ok(Picks { walk, paths })
}

fn load(ctx: &Context) -> Result<LastScan> {
    let file = ctx
        .dirs
        .as_ref()
        .map(|d| d.last_scan_file())
        .context("no state directory to read the last scan from")?;
    let json = match std::fs::read(&file) {
        Ok(json) => json,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("no scan to pick from: run `carwash scan` first, or pass paths to --pick")
        }
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", file.display())),
    };
    let scan: LastScan = serde_json::from_slice(&json)
        .with_context(|| format!("cannot parse {}", file.display()))?;
    if scan.version != VERSION {
        bail!("the last scan was saved by another carwash version; run `carwash scan` again");
    }
    Ok(scan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(token: &str) -> Option<(usize, usize)> {
        match parse(token).unwrap() {
            Token::Rows(a, b) => Some((a, b)),
            Token::Path(_) => None,
        }
    }

    #[test]
    fn parses_numbers_ranges_and_paths() {
        assert_eq!(rows("3"), Some((3, 3)));
        assert_eq!(rows("3-5"), Some((3, 5)));
        assert_eq!(rows("my-app/node_modules"), None);
        assert_eq!(rows("3-"), None);
        assert!(parse("0").is_err());
        assert!(parse("5-3").is_err());
    }
}
