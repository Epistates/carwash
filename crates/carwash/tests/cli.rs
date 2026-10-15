//! End-to-end tests of the `carwash` binary against fixture trees.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::Path;

const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

/// `rel` with the platform's separator, as carwash prints paths.
fn native(rel: &str) -> String {
    rel.replace('/', std::path::MAIN_SEPARATOR_STR)
}

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
            .find(|a| a["path"].as_str().unwrap().ends_with(&native(suffix)))
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
        .stdout(predicate::str::contains(native("app/target")))
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
        .stdout(predicate::str::contains(native("acme/.acme-cache")));
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

/// Deep trees removed side by side once exhausted a low open-file limit ("Too many open
/// files"). Under a hard limit carwash cannot raise, every tree must still go.
#[cfg(unix)]
#[test]
fn clean_survives_a_low_open_file_limit() {
    let tree = tempfile::tempdir().unwrap();
    let old = filetime::FileTime::from_unix_time(1_600_000_000, 0);
    for project in 0..24 {
        let root = format!("p{project}");
        write(tree.path(), &format!("{root}/package.json"), "{}");
        for module in 0..8 {
            let deep = format!("{root}/node_modules/m{module}/a/b/c/d/e/f/g/h/node_modules/x/y");
            write(tree.path(), &format!("{deep}/index.js"), "x");
        }
        filetime::set_file_mtime(tree.path().join(format!("{root}/node_modules")), old).unwrap();
    }
    let home = tempfile::tempdir().unwrap();
    let bin = assert_cmd::cargo::cargo_bin("carwash");
    let script = format!(
        "ulimit -n 64 && exec '{}' clean --yes --include-recent '{}'",
        bin.display(),
        tree.path().display()
    );
    Command::new("sh")
        .args(["-c", &script])
        .env("CARWASH_HOME", home.path())
        .env_remove("CARWASH_CONFIG")
        .env("NO_COLOR", "1")
        .assert()
        .success();
    let left: Vec<String> = fs::read_dir(tree.path())
        .unwrap()
        .flat_map(|project| fs::read_dir(project.unwrap().path()).unwrap())
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "package.json")
        .collect();
    assert!(left.is_empty(), "left behind: {left:?}");
}

/// Two projects whose `node_modules` were last touched at different times.
fn aged_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, secs) in [("oldest", 1_500_000_000), ("older", 1_600_000_000)] {
        write(dir.path(), &format!("{name}/package.json"), "{}");
        write(
            dir.path(),
            &format!("{name}/node_modules/pkg/index.js"),
            &"x".repeat(20_000),
        );
        let time = filetime::FileTime::from_unix_time(secs, 0);
        for rel in [
            "node_modules/pkg/index.js",
            "node_modules/pkg",
            "node_modules",
        ] {
            filetime::set_file_mtime(dir.path().join(name).join(rel), time).unwrap();
        }
    }
    dir
}

#[test]
fn free_goal_takes_the_least_recently_used_first() {
    let tree = aged_fixture();
    let home = tempfile::tempdir().unwrap();
    let output = carwash(home.path())
        .args(["clean", "--free", "1B", "--yes", "--json"])
        .arg(tree.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["goal"]["met"], true);
    assert_eq!(report["removed"], 1);
    assert!(!tree.path().join("oldest/node_modules").exists());
    assert!(tree.path().join("older/node_modules").exists());
}

#[test]
fn unreachable_goal_fails_without_deleting_on_a_dry_run() {
    let tree = aged_fixture();
    let home = tempfile::tempdir().unwrap();
    let output = carwash(home.path())
        .args(["clean", "--free", "1TB", "--dry-run", "--json"])
        .arg(tree.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["goal"]["met"], false);
    assert_eq!(report["selected"].as_array().unwrap().len(), 2);
    assert!(report["free_before"].as_u64().is_some());
    assert!(tree.path().join("oldest/node_modules").exists());
}

#[test]
fn until_free_does_nothing_when_there_is_room() {
    let tree = aged_fixture();
    let home = tempfile::tempdir().unwrap();
    carwash(home.path())
        .args(["clean", "--until-free", "1B", "--yes"])
        .arg(tree.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to do"));
    assert!(tree.path().join("oldest/node_modules").exists());
}

#[test]
fn goals_refuse_the_trash() {
    let tree = aged_fixture();
    let home = tempfile::tempdir().unwrap();
    carwash(home.path())
        .args(["clean", "--free", "1GB", "--trash", "--yes"])
        .arg(tree.path())
        .assert()
        .code(2);
    assert!(tree.path().join("oldest/node_modules").exists());
}

/// Row number of the artifact at `rel` in a `scan --json` report.
fn index_of(report: &serde_json::Value, rel: &str) -> u64 {
    report["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["path"].as_str().unwrap().ends_with(&native(rel)))
        .unwrap_or_else(|| panic!("{rel} missing"))["index"]
        .as_u64()
        .unwrap()
}

#[test]
fn pick_deletes_rows_of_the_last_scan() {
    let tree = fixture();
    let home = tempfile::tempdir().unwrap();
    let output = carwash(home.path())
        .args(["scan", "--json"])
        .arg(tree.path())
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let modules = index_of(&report, "web/node_modules");
    // Recently used, so a plain clean would hold it back; picking it is explicit.
    let fresh = index_of(&report, "fresh/node_modules");
    let picks = format!("{},{}", modules.min(fresh), modules.max(fresh));
    carwash(home.path())
        .args(["clean", "--yes", "--pick", &picks])
        .assert()
        .success()
        .stdout(predicate::str::contains("Freed"));
    let root = tree.path();
    assert!(!root.join("web/node_modules").exists());
    assert!(!root.join("fresh/node_modules").exists());
    assert!(root.join("app/target").exists());
    assert!(root.join("web/dist").exists());
}

#[test]
fn pick_takes_ranges_and_paths() {
    let tree = fixture();
    let home = tempfile::tempdir().unwrap();
    carwash(home.path())
        .args(["scan", "--sort", "path"])
        .arg(tree.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("--pick"));
    // By path: app/target, fresh/node_modules, web/dist, web/node_modules.
    let target = tree.path().join("web/node_modules");
    let output = carwash(home.path())
        .args(["clean", "--dry-run", "--json", "--pick", "1-2"])
        .arg("--pick")
        .arg(&target)
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut selected: Vec<String> = report["selected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["path"].as_str().unwrap().to_string())
        .collect();
    selected.sort();
    assert_eq!(selected.len(), 3, "{selected:?}");
    assert!(selected[0].ends_with(&native("app/target")));
    assert!(selected[1].ends_with(&native("fresh/node_modules")));
    assert!(selected[2].ends_with(&native("web/node_modules")));
}

#[test]
fn pick_skips_what_is_not_an_artifact() {
    let tree = fixture();
    let home = tempfile::tempdir().unwrap();
    let output = carwash(home.path())
        .args(["clean", "--yes", "--json", "--pick"])
        .arg(tree.path().join("app"))
        .arg(tree.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["skipped"].as_array().unwrap().len(), 1);
    assert_eq!(report["removed"], 0);
    assert!(tree.path().join("app/Cargo.toml").exists());
}

#[test]
fn pick_never_deletes_protected_artifacts() {
    let git = |repo: &Path, args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .is_ok_and(|o| o.status.success())
    };
    let repo = tempfile::tempdir().unwrap();
    if !git(repo.path(), &["init", "-q"]) {
        return; // git unavailable
    }
    write(repo.path(), "package.json", "{}");
    write(repo.path(), "node_modules/pkg/index.js", "tracked");
    assert!(git(repo.path(), &["add", "node_modules/pkg/index.js"]));
    let home = tempfile::tempdir().unwrap();
    let output = carwash(home.path())
        .args(["clean", "--yes", "--json", "--pick"])
        .arg(repo.path().join("node_modules"))
        .arg(repo.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let reason = report["skipped"][0]["error"].as_str().unwrap();
    assert!(reason.starts_with("protected"), "{reason}");
    assert!(repo.path().join("node_modules/pkg/index.js").exists());
}

#[test]
fn pick_numbers_are_checked_against_the_last_scan() {
    let tree = fixture();
    let other = fixture();
    let home = tempfile::tempdir().unwrap();
    carwash(home.path())
        .args(["clean", "--yes", "--pick", "1"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("carwash scan"));
    carwash(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success();
    carwash(home.path())
        .args(["clean", "--yes", "--pick", "99"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("out of range"));
    carwash(home.path())
        .args(["clean", "--yes", "--pick", "1"])
        .arg(other.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("last scan"));
    assert!(other.path().join("app/target").exists());
    assert!(tree.path().join("app/target").exists());
}
