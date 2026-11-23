//! End-to-end tests of the `carwash` binary against fixture trees.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// Sets the modification time of a file or directory.
fn set_modified(path: &Path, time: SystemTime) {
    let mut options = fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_BACKUP_SEMANTICS, without which a directory cannot be opened.
        options.write(true).custom_flags(0x0200_0000);
    }
    #[cfg(not(windows))]
    options.read(true);
    options
        .open(path)
        .and_then(|file| file.set_times(fs::FileTimes::new().set_modified(time)))
        .unwrap();
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
    let old = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
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
        set_modified(&root.join(rel), old);
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
        .stdout(predicate::str::contains("Freed"))
        // The measured change is spelled out; a clean this small never draws the note
        // about free space that is still settling.
        .stdout(
            predicate::str::is_match(r"Free space: .+ → .+ \((.+ (more|less)|unchanged)\)")
                .unwrap(),
        )
        .stdout(predicate::str::contains("keep rising").not());
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
    let old = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    for project in 0..24 {
        let root = format!("p{project}");
        write(tree.path(), &format!("{root}/package.json"), "{}");
        for module in 0..8 {
            let deep = format!("{root}/node_modules/m{module}/a/b/c/d/e/f/g/h/node_modules/x/y");
            write(tree.path(), &format!("{deep}/index.js"), "x");
        }
        set_modified(&tree.path().join(format!("{root}/node_modules")), old);
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
        let time = UNIX_EPOCH + Duration::from_secs(secs);
        for rel in [
            "node_modules/pkg/index.js",
            "node_modules/pkg",
            "node_modules",
        ] {
            set_modified(&dir.path().join(name).join(rel), time);
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
    assert_eq!(report["free_space_may_rise"], false);
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

/// User-defined OS areas over `root`, for every platform, so tests do not depend on the
/// machine: a sweepable cache, a report-only area and one whose app is always running.
fn os_areas(home: &Path, root: &Path) {
    let mut toml = String::from("schema = 1\n");
    for platform in ["macos", "linux", "windows"] {
        for (id, extra) in [
            ("fixture-cache", ""),
            ("fixture-report", "tier = \"report\"\nhow = \"by hand\"\n"),
            // Every process name matches `*`, including carwash's own.
            ("fixture-busy", "apps = [\"*\"]\n"),
        ] {
            toml.push_str(&format!(
                "[[area]]\nid = \"{id}\"\nname = \"{id}\"\nplatform = \"{platform}\"\ncategory = \"caches\"\npaths = ['{}']\nnote = \"test\"\n{extra}",
                root.join(id).display()
            ));
        }
    }
    write(home, "config/system.toml", &toml);
    for id in ["fixture-cache", "fixture-report", "fixture-busy"] {
        write(root, &format!("{id}/sub/blob"), &"x".repeat(20_000));
    }
}

#[test]
fn os_lists_areas_with_what_cleaning_frees() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    os_areas(home.path(), root.path());
    let output = carwash(home.path())
        .args(["os", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let areas: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let area = |id: &str| {
        areas
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing"))
            .clone()
    };
    assert!(area("fixture-cache")["frees"].as_u64().unwrap() >= 20_000);
    assert_eq!(area("fixture-report")["tier"], "report");
    assert!(
        !area("fixture-busy")["blocked_by"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(area("fixture-cache")["index"].as_u64().unwrap() >= 1);
}

#[test]
fn os_clean_sweeps_named_areas_and_keeps_the_directory() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    os_areas(home.path(), root.path());
    let output = carwash(home.path())
        .args(["os", "clean", "fixture-cache", "--yes", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["areas"][0]["outcome"], "cleaned");
    assert!(report["freed"].as_u64().unwrap() >= 20_000);
    let cache = root.path().join("fixture-cache");
    assert!(cache.is_dir(), "the directory stays");
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
    assert!(root.path().join("fixture-busy/sub/blob").exists());
}

#[test]
fn os_clean_refuses_report_areas_and_respects_running_apps() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    os_areas(home.path(), root.path());
    carwash(home.path())
        .args(["os", "clean", "fixture-report", "--yes"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("report-only"));
    let output = carwash(home.path())
        .args(["os", "clean", "fixture-busy", "--yes", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["areas"][0]["outcome"], "blocked");
    assert!(root.path().join("fixture-busy/sub/blob").exists());
    assert!(root.path().join("fixture-report/sub/blob").exists());
    carwash(home.path())
        .args(["os", "clean", "no-such-area", "--yes"])
        .assert()
        .code(2);
}

/// `carwash caches` against a home directory and tools of the test's own making: nothing
/// outside the temporary directories is read, run or removed.
#[cfg(unix)]
mod caches {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    /// A home with npm's cache and three Rust toolchains, and a folder for stand-in tools.
    struct Machine {
        home: tempfile::TempDir,
        tools: tempfile::TempDir,
    }

    const RUSTUP_LIST: &str = "stable-aarch64-apple-darwin (active, default)\n1.90-aarch64-apple-darwin\n1.91-aarch64-apple-darwin\n";

    impl Machine {
        fn new() -> Self {
            let home = tempfile::tempdir().unwrap();
            write(
                home.path(),
                ".npm/_cacache/index/entry",
                &"x".repeat(20_000),
            );
            for toolchain in RUSTUP_LIST.lines() {
                let name = toolchain.split(' ').next().unwrap();
                write(
                    home.path(),
                    &format!(".rustup/toolchains/{name}/bin/rustc"),
                    "",
                );
            }
            write(
                home.path(),
                ".rustup/settings.toml",
                "default_toolchain = \"stable-aarch64-apple-darwin\"\n",
            );
            Self {
                home,
                tools: tempfile::tempdir().unwrap(),
            }
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.home.path().join(rel)
        }

        /// Installs a stand-in for `name` that notes it was run, then runs `body`.
        fn tool(&self, name: &str, body: &str) {
            let path = self.tools.path().join(name);
            let marker = self.tools.path().join(format!("{name}.ran"));
            fs::write(
                &path,
                format!("#!/bin/sh\n: > '{}'\n{body}\n", marker.display()),
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        fn ran(&self, name: &str) -> bool {
            self.tools.path().join(format!("{name}.ran")).exists()
        }

        /// carwash with this home and nothing else from the environment: on PATH only the
        /// stand-ins and the system's shell utilities, and no variable that points a cache
        /// somewhere real.
        fn carwash(&self) -> Command {
            let mut cmd = Command::cargo_bin("carwash").unwrap();
            cmd.env_clear()
                .env("HOME", self.home.path())
                .env("CARWASH_HOME", self.home.path().join(".carwash-home"))
                .env(
                    "PATH",
                    format!("{}:/usr/bin:/bin", self.tools.path().display()),
                )
                .env("NO_COLOR", "1");
            cmd
        }

        fn list(&self, extra: &[&str]) -> Vec<serde_json::Value> {
            let output = self
                .carwash()
                .args(["caches", "--json"])
                .args(extra)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            serde_json::from_slice(&output.stdout).unwrap()
        }
    }

    fn row<'a>(rows: &'a [serde_json::Value], id: &str) -> &'a serde_json::Value {
        rows.iter()
            .find(|row| row["id"] == id)
            .unwrap_or_else(|| panic!("no {id} among {rows:#?}"))
    }

    #[test]
    fn toolchains_are_listed_from_rustup_and_advised_by_the_projects_given() {
        let machine = Machine::new();
        machine.tool("rustup", &format!("printf '%s' '{RUSTUP_LIST}'"));
        let work = tempfile::tempdir().unwrap();
        write(work.path(), "app/Cargo.toml", "[package]\nname = \"app\"\n");
        write(work.path(), "app/rust-toolchain", "1.90\n");

        // Without projects nothing is known, and nothing is recommended.
        let rows = machine.list(&[]);
        assert!(machine.ran("rustup"));
        let pinned = row(&rows, "rustup-toolchains/1.90-aarch64-apple-darwin");
        assert_eq!(pinned["advice"]["verdict"], "unknown");
        assert_eq!(
            row(&rows, "rustup-toolchains/stable-aarch64-apple-darwin")["advice"]["verdict"],
            "protected"
        );

        let rows = machine.list(&["--projects", work.path().to_str().unwrap()]);
        let pinned = row(&rows, "rustup-toolchains/1.90-aarch64-apple-darwin");
        assert_eq!(pinned["advice"]["verdict"], "keep");
        assert_eq!(pinned["advice"]["reason"], "app needs it");
        let unused = row(&rows, "rustup-toolchains/1.91-aarch64-apple-darwin");
        assert_eq!(unused["advice"]["verdict"], "remove");
        let reason = unused["advice"]["reason"].as_str().unwrap();
        assert!(
            reason.starts_with("no project under ") && reason.ends_with(" needs it"),
            "{reason}"
        );

        // A folder with no Rust project in it is nothing to go by.
        let empty = tempfile::tempdir().unwrap();
        write(empty.path(), "web/package.json", "{}");
        let rows = machine.list(&["--projects", empty.path().to_str().unwrap()]);
        let unused = row(&rows, "rustup-toolchains/1.91-aarch64-apple-darwin");
        assert_eq!(unused["advice"]["verdict"], "unknown");
        machine
            .carwash()
            .args([
                "caches",
                "clean",
                "--recommended",
                "--dry-run",
                "--projects",
            ])
            .arg(empty.path())
            .assert()
            .success()
            .stdout(predicate::str::contains("rustup toolchain uninstall").not());
    }

    #[test]
    fn a_tool_that_fails_is_not_believed() {
        let machine = Machine::new();
        // Prints a list naming a toolchain that is not the default, then fails.
        machine.tool(
            "rustup",
            "printf '1.90-aarch64-apple-darwin (active, default)\\n'; exit 3",
        );
        let rows = machine.list(&[]);
        assert!(machine.ran("rustup"));
        // The folders and rustup's settings are read instead.
        assert_eq!(
            row(&rows, "rustup-toolchains/stable-aarch64-apple-darwin")["advice"]["verdict"],
            "protected"
        );
        assert_eq!(
            row(&rows, "rustup-toolchains/1.90-aarch64-apple-darwin")["advice"]["verdict"],
            "unknown"
        );
    }

    #[test]
    fn cleaning_one_cache_deletes_it_and_asks_no_other_tool() {
        let machine = Machine::new();
        machine.tool("rustup", &format!("printf '%s' '{RUSTUP_LIST}'"));
        machine.tool("xcrun", "exit 1");
        machine
            .carwash()
            .args(["caches", "clean", "npm", "--yes"])
            .assert()
            .success()
            .stdout(predicate::str::contains("freed"))
            .stdout(predicate::str::contains("Free space: "));
        assert!(!machine.path(".npm/_cacache").exists());
        assert!(machine.path(".npm").is_dir());
        assert!(!machine.ran("rustup"), "rustup was asked about npm's cache");
        assert!(!machine.ran("xcrun"));
        assert!(
            machine
                .path(".rustup/toolchains/1.90-aarch64-apple-darwin")
                .is_dir()
        );
    }

    #[test]
    fn a_deletion_that_fails_is_explained_on_stderr_and_its_remainder_is_listed() {
        let machine = Machine::new();
        // An entry that cannot be unlinked.
        let locked = machine.path(".npm/_cacache/index");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let output = machine
            .carwash()
            .args(["caches", "clean", "npm", "--yes"])
            .output()
            .unwrap();
        let (stdout, stderr) = (
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        );
        let rows = machine.list(&[]);
        let leftover = rows
            .iter()
            .find(|row| row["detail"]["kind"] == "leftover")
            .cloned();
        // Let the temporary directory be removed, whatever the assertions below say.
        if let Some(path) = leftover.as_ref().and_then(|row| row["path"].as_str()) {
            let _ = fs::set_permissions(
                Path::new(path).join("index"),
                fs::Permissions::from_mode(0o755),
            );
        }
        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
        if output.status.success() {
            // Root unlinks it regardless; there is no failure to observe.
            return;
        }
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains("failed to clean npm cache: "), "{stderr}");
        assert!(stderr.contains("what is left of it is in"), "{stderr}");
        assert!(!stdout.contains("failed to clean"), "{stdout}");
        let leftover = leftover.expect("the remainder is listed");
        assert_eq!(leftover["name"], "npm cache: leftover of _cacache");
        assert_eq!(leftover["advice"]["verdict"], "remove");
        assert!(
            leftover["id"]
                .as_str()
                .unwrap()
                .starts_with("npm/.carwash-trash-")
        );
        // The next clean of the cache takes the remainder with it.
        machine
            .carwash()
            .args(["caches", "clean", "npm", "--yes"])
            .assert()
            .success();
        assert!(
            !machine
                .list(&[])
                .iter()
                .any(|row| row["detail"]["kind"] == "leftover")
        );
    }

    /// `xcrun` is a stub that offers to install the developer tools when none are
    /// selected, so it must not be run then.
    #[cfg(target_os = "macos")]
    #[test]
    fn xcrun_is_only_run_when_a_developer_directory_is_selected() {
        let udid = "00000001-0000-4000-8000-000000000001";
        let devices = format!("Library/Developer/CoreSimulator/Devices/{udid}/data/file");
        let simulators = |machine: &Machine| {
            write(machine.home.path(), &devices, "x");
            let rows = machine.list(&[]);
            row(&rows, "simulators").clone()
        };

        let machine = Machine::new();
        machine.tool(
            "xcode-select",
            "echo 'xcode-select: error: unable to get active developer directory' >&2; exit 2",
        );
        machine.tool("xcrun", "exit 1");
        let set = simulators(&machine);
        assert!(machine.ran("xcode-select"));
        assert!(
            !machine.ran("xcrun"),
            "xcrun was run with no developer directory"
        );
        assert_eq!(set["name"], "Simulators", "listed as one entry, unexamined");

        // With a directory selected it is asked, and its failure is taken as no answer.
        let machine = Machine::new();
        machine.tool(
            "xcode-select",
            &format!("echo '{}'", machine.home.path().display()),
        );
        machine.tool("xcrun", "exit 1");
        let set = simulators(&machine);
        assert!(machine.ran("xcrun"));
        assert_eq!(set["name"], "Simulators");
    }
}
