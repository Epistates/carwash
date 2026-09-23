//! End-to-end tests of the `carwash` binary against fixture trees.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::Path;

const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Two old artifacts, one recent one, and one with a generic name.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "app/Cargo.toml", "[package]\nname = \"app\"\n");
    write(root, "app/target/CACHEDIR.TAG", CACHEDIR_TAG);
    write(root, "app/target/debug/bin", &"x".repeat(50_000));
    write(root, "web/package.json", "{\"name\":\"web\"}");
    write(root, "web/node_modules/pkg/index.js", &"y".repeat(20_000));
    write(root, "web/dist/bundle.js", "z");
    write(root, "fresh/package.json", "{}");
    write(root, "fresh/node_modules/pkg/index.js", "new");
    let old = filetime::FileTime::from_unix_time(1_600_000_000, 0);
    for rel in [
        "app/target/CACHEDIR.TAG",
        "app/target/debug/bin",
        "app/target/debug",
        "app/target",
        "web/node_modules/pkg/index.js",
        "web/node_modules/pkg",
        "web/node_modules",
        "web/dist/bundle.js",
        "web/dist",
    ] {
        filetime::set_file_mtime(root.join(rel), old).unwrap();
    }
    dir
}

fn carwash(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("carwash").unwrap();
    cmd.env("CARWASH_HOME", home)
        .env_remove("CARWASH_CONFIG")
        .env("NO_COLOR", "1");
    cmd
}

#[test]
fn scan_json_reports_artifacts_and_policy() {
    let tree = fixture();
    let home = tempfile::tempdir().unwrap();
    let output = carwash(home.path())
        .args(["scan", "--json"])
        .arg(tree.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["version"], 1);
    let artifacts = report["artifacts"].as_array().unwrap();
    let find = |suffix: &str| {
        artifacts
            .iter()
            .find(|a| a["path"].as_str().unwrap().ends_with(suffix))
            .unwrap_or_else(|| panic!("{suffix} missing"))
    };
    assert_eq!(find("app/target")["kind"], "build");
    assert_eq!(find("app/target")["ecosystem"], "rust");
    assert!(find("app/target")["hold"].is_null());
    assert_eq!(find("web/dist")["hold"], "needs review");
    assert_eq!(find("fresh/node_modules")["hold"], "recently used");
    assert!(find("app/target")["size"]["reclaimable"].as_u64().unwrap() >= 50_000);
}

#[test]
fn scan_table_summarises() {
    let tree = fixture();
    let home = tempfile::tempdir().unwrap();
    carwash(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("app/target"))
        .stdout(predicate::str::contains("reclaimable from 4 artifacts"));
}

#[test]
fn dry_run_deletes_nothing() {
    let tree = fixture();
    let home = tempfile::tempdir().unwrap();
    carwash(home.path())
        .args(["clean", "--dry-run"])
        .arg(tree.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Dry run"));
    assert!(tree.path().join("app/target").exists());
    assert!(tree.path().join("web/node_modules").exists());
}

#[test]
fn refuses_without_confirmation_when_not_interactive() {
    let tree = fixture();
    let home = tempfile::tempdir().unwrap();
    carwash(home.path())
        .arg("clean")
        .arg(tree.path())
        .write_stdin("")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--yes"));
    assert!(tree.path().join("app/target").exists());
}

#[test]
fn clean_removes_only_selected_and_records_history() {
    let tree = fixture();
    let home = tempfile::tempdir().unwrap();
    carwash(home.path())
        .args(["clean", "--yes"])
        .arg(tree.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Freed"));
    let root = tree.path();
    assert!(!root.join("app/target").exists());
    assert!(!root.join("web/node_modules").exists());
    // Held back by default: generic name and recently used.
    assert!(root.join("web/dist").exists());
    assert!(root.join("fresh/node_modules").exists());
    // Sources untouched.
    assert!(root.join("app/Cargo.toml").exists());

    let history = carwash(home.path())
        .args(["history", "--json"])
        .output()
        .unwrap();
    let history: serde_json::Value = serde_json::from_slice(&history.stdout).unwrap();
    assert_eq!(history["cleans"], 2);
    assert!(history["total_bytes"].as_u64().unwrap() >= 70_000);
}

#[test]
fn filters_narrow_the_selection() {
    let tree = fixture();
    let home = tempfile::tempdir().unwrap();
    carwash(home.path())
        .args(["clean", "--yes", "--kind", "deps", "--include-recent"])
        .arg(tree.path())
        .assert()
        .success();
    let root = tree.path();
    assert!(
        root.join("app/target").exists(),
        "build artifacts untouched"
    );
    assert!(!root.join("web/node_modules").exists());
    assert!(!root.join("fresh/node_modules").exists());
}

#[test]
fn rejects_unknown_ecosystems_and_bad_sizes() {
    let tree = fixture();
    let home = tempfile::tempdir().unwrap();
    carwash(home.path())
        .args(["scan", "-e", "cobol"])
        .arg(tree.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown ecosystem"));
    carwash(home.path())
        .args(["scan", "--min-size", "lots"])
        .arg(tree.path())
        .assert()
        .failure();
}

#[test]
fn user_rules_extend_the_registry() {
    let tree = fixture();
    write(tree.path(), "acme/acme.yml", "");
    write(tree.path(), "acme/.acme-cache/blob", "b");
    let home = tempfile::tempdir().unwrap();
    write(
        home.path(),
        "config/ecosystems.toml",
        "schema = 1\n[[ecosystem]]\nid = \"acme\"\nname = \"Acme\"\nmarkers = [\"acme.yml\"]\nartifacts = [{ path = \".acme-cache\", kind = \"cache\" }]\n",
    );
    carwash(home.path())
        .args(["scan", "-e", "acme"])
        .arg(tree.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("acme/.acme-cache"));
}

#[test]
fn ecosystems_lists_builtins() {
    let home = tempfile::tempdir().unwrap();
    carwash(home.path())
        .arg("ecosystems")
        .assert()
        .success()
        .stdout(predicate::str::contains("node_modules"))
        .stdout(predicate::str::contains("40 ecosystems"));
}
