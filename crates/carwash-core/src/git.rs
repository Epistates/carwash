//! Batched git inspection of artifact directories.
//!
//! One `git ls-files` and one `git check-ignore` per repository answer, for every
//! candidate at once, whether it contains tracked files and whether it is ignored.

use crate::model::GitState;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Pathspecs per `git ls-files` invocation, to stay well under argument-length limits.
const CHUNK: usize = 256;

/// Inspects `paths` (absolute, all inside `repo`) and returns their git state in order.
/// Returns `GitState::Unknown` for every path when git is unavailable or fails.
pub fn inspect(repo: &Path, paths: &[PathBuf]) -> Vec<GitState> {
    let relative: Vec<Option<String>> = paths
        .iter()
        .map(|p| {
            p.strip_prefix(repo)
                .ok()
                .and_then(|r| r.to_str())
                .map(|r| r.replace(std::path::MAIN_SEPARATOR, "/"))
                .filter(|r| !r.is_empty())
        })
        .collect();
    let valid: Vec<&str> = relative.iter().flatten().map(String::as_str).collect();
    if valid.is_empty() {
        return vec![GitState::Unknown; paths.len()];
    }

    let Some(tracked) = tracked_counts(repo, &valid) else {
        return vec![GitState::Unknown; paths.len()];
    };
    let untracked: Vec<&str> = valid
        .iter()
        .copied()
        .filter(|r| !tracked.contains_key(*r))
        .collect();
    let ignored = ignored_set(repo, &untracked);

    relative
        .iter()
        .map(|rel| match rel {
            None => GitState::Unknown,
            Some(rel) => match tracked.get(rel.as_str()) {
                Some(&count) => GitState::Tracked(count),
                None => match &ignored {
                    Some(ignored) if ignored.contains_key(rel.as_str()) => GitState::Ignored,
                    Some(_) => GitState::Untracked,
                    None => GitState::Unknown,
                },
            },
        })
        .collect()
}

fn git(repo: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

/// Number of tracked files below each relative directory that has any.
fn tracked_counts<'a>(repo: &Path, relative: &[&'a str]) -> Option<HashMap<&'a str, u64>> {
    let mut counts: HashMap<&'a str, u64> = HashMap::new();
    for chunk in relative.chunks(CHUNK) {
        // Literal pathspecs so names containing `*` or `?` are not treated as globs.
        // (`check-ignore` rejects this setting, so it is scoped to `ls-files`.)
        let output = git(repo)
            .env("GIT_LITERAL_PATHSPECS", "1")
            .args(["ls-files", "-z", "--cached", "--"])
            .args(chunk)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        for file in output.stdout.split(|&b| b == 0).filter(|f| !f.is_empty()) {
            let Ok(file) = std::str::from_utf8(file) else {
                continue;
            };
            if let Some(dir) = chunk.iter().find(|dir| {
                file.len() > dir.len()
                    && file.starts_with(**dir)
                    && file.as_bytes()[dir.len()] == b'/'
            }) {
                *counts.entry(dir).or_default() += 1;
            } else if let Some(dir) = chunk.iter().find(|dir| file == **dir) {
                // A submodule (gitlink) is listed as the directory itself.
                *counts.entry(dir).or_default() += 1;
            }
        }
    }
    Some(counts)
}

/// Relative directories matched by an ignore rule.
fn ignored_set<'a>(repo: &Path, relative: &[&'a str]) -> Option<HashMap<&'a str, ()>> {
    if relative.is_empty() {
        return Some(HashMap::new());
    }
    let mut child = git(repo)
        .args(["check-ignore", "-z", "--stdin", "--no-index"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    // Write on a separate thread while this one drains stdout, so neither pipe can fill up
    // and deadlock.
    let output = std::thread::scope(|s| {
        s.spawn(move || -> std::io::Result<()> {
            for rel in relative {
                // A trailing slash lets directory-only patterns such as `target/` match.
                stdin.write_all(rel.as_bytes())?;
                stdin.write_all(b"/\0")?;
            }
            Ok(())
        });
        child.wait_with_output()
    })
    .ok()?;
    // Exit status 1 means "nothing ignored"; 128 is a fatal error.
    if output.status.code() == Some(128) {
        return None;
    }
    let mut ignored = HashMap::new();
    for path in output.stdout.split(|&b| b == 0).filter(|p| !p.is_empty()) {
        let Ok(path) = std::str::from_utf8(path) else {
            continue;
        };
        let path = path.trim_end_matches('/');
        if let Some(rel) = relative.iter().find(|r| **r == path) {
            ignored.insert(*rel, ());
        }
    }
    Some(ignored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn run(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn classifies_tracked_ignored_and_untracked() {
        if !git_available() {
            return;
        }
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        run(root, &["init", "-q"]);
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        for dir in ["target", "dist", "build"] {
            fs::create_dir(root.join(dir)).unwrap();
            fs::write(root.join(dir).join("file"), dir).unwrap();
        }
        run(root, &["add", "dist/file", ".gitignore"]);

        let states = inspect(
            root,
            &[root.join("target"), root.join("dist"), root.join("build")],
        );
        assert_eq!(
            states,
            vec![GitState::Ignored, GitState::Tracked(1), GitState::Untracked]
        );
    }

    #[test]
    fn outside_paths_are_unknown() {
        let states = inspect(Path::new("/repo"), &[PathBuf::from("/elsewhere/target")]);
        assert_eq!(states, vec![GitState::Unknown]);
    }
}
