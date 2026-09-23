//! Discovery against a fixture tree shaped like a real `~/work`.

use carwash_core::discover::Hidden;
use carwash_core::{
    ArtifactKind, Cancel, Detection, DiscoverOptions, Engine, Registry, Safety, ScanOptions,
    Snapshot,
};
use std::fs;
use std::path::{Path, PathBuf};

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn root(&self) -> PathBuf {
        // Canonical so assertions compare equal on macOS (/var -> /private/var).
        fs::canonicalize(self.dir.path()).unwrap()
    }

    fn file(&self, rel: &str, contents: &str) -> &Self {
        let path = self.dir.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        self
    }

    fn dir(&self, rel: &str) -> &Self {
        fs::create_dir_all(self.dir.path().join(rel)).unwrap();
        self
    }
}

const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

fn build_fixture() -> Fixture {
    let f = Fixture::new();
    // Standalone Rust crate.
    f.file(
        "rust-app/Cargo.toml",
        "[package]\nname = \"rust-app\"\nversion = \"0.1.0\"\n",
    )
    .file("rust-app/Cargo.lock", "")
    .file("rust-app/src/main.rs", "fn main() {}")
    .file("rust-app/target/CACHEDIR.TAG", CACHEDIR_TAG)
    .file("rust-app/target/.rustc_info.json", "{}")
    .file("rust-app/target/debug/app", &"x".repeat(10_000));
    // Cargo workspace: one shared target at the root.
    f.file("ws/Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n")
        .file("ws/target/CACHEDIR.TAG", CACHEDIR_TAG)
        .file("ws/crates/a/Cargo.toml", "[package]\nname = \"a\"\n")
        .file("ws/crates/b/Cargo.toml", "[package]\nname = \"b\"\n");
    // pnpm monorepo.
    f.file(
        "web/package.json",
        r#"{"name":"web","workspaces":["packages/*"]}"#,
    )
    .file("web/pnpm-lock.yaml", "")
    .file("web/node_modules/react/package.json", r#"{"name":"react"}"#)
    .file("web/node_modules/react/node_modules/x/package.json", "{}")
    .file("web/.next/BUILD_ID", "1")
    .file("web/dist/app.js", "")
    .file("web/.nx/cache/abc", "")
    .file("web/.nx/installation/keep", "")
    .file("web/packages/ui/package.json", r#"{"name":"@web/ui"}"#)
    .file("web/packages/ui/node_modules/.modules.yaml", "");
    // Python project with a venv and caches deep in the source tree.
    f.file("py/pyproject.toml", "[project]\nname = \"svc\"\n")
        .file("py/.venv/pyvenv.cfg", "home = /usr/bin")
        .file("py/src/svc/__pycache__/mod.cpython-312.pyc", "")
        .file("py/src/svc/mod.py", "")
        .file("py/build/notes.txt", "");
    // Loose scripts: a venv and caches outside any project.
    f.file("scripts/venv/pyvenv.cfg", "home = /usr/bin")
        .file("scripts/tool.py", "")
        .file("scripts/__pycache__/tool.pyc", "")
        .file("stray/node_modules/left-pad/package.json", "{}");
    // CMake with nested CMakeLists and an out-of-source build directory.
    f.file("cpp/CMakeLists.txt", "")
        .file("cpp/lib/CMakeLists.txt", "")
        .file("cpp/build/CMakeCache.txt", "")
        .file("cpp/custom-out/CMakeCache.txt", "");
    // A monorepo package literally named `build` is a project, not an artifact.
    f.file("tools/package.json", r#"{"name":"tools"}"#)
        .file("tools/build/package.json", r#"{"name":"@tools/build"}"#);
    // Hidden directories are skipped.
    f.file(".hidden/package.json", "{}")
        .dir(".hidden/node_modules");
    // Unity.
    f.file(
        "game/ProjectSettings/ProjectVersion.txt",
        "m_EditorVersion: 6000.0",
    )
    .dir("game/Assets")
    .file("game/Library/ArtifactDB", "");
    // Gradle multi-project.
    f.file("jvm/settings.gradle.kts", "rootProject.name = \"shop\"\n")
        .file("jvm/app/build.gradle.kts", "")
        .file("jvm/app/build/intermediates/x", "");
    // A leftover from an interrupted clean.
    f.file("rust-app/.carwash-trash-99-0-target/f", "");
    // Hidden directories inside projects are entered: agent worktrees live there.
    f.file(
        "rust-app/.claude/worktrees/agent-1/.git",
        "gitdir: ../../../.git/worktrees/agent-1",
    )
    .file(
        "rust-app/.claude/worktrees/agent-1/Cargo.toml",
        "[package]\nname = \"rust-app\"\n",
    )
    .file(
        "rust-app/.claude/worktrees/agent-1/target/CACHEDIR.TAG",
        CACHEDIR_TAG,
    );
    // A Cargo target whose manifest was deleted, recognised by its fingerprints.
    f.dir("orphan/target/debug/.fingerprint");
    // A target directory that is actually a nested repository is never an artifact.
    f.file("vendored/Cargo.toml", "[package]\nname = \"vendored\"\n")
        .dir("vendored/target/.git");
    f
}

fn scan(root: &Path) -> Snapshot {
    let engine = Engine::with_threads(Registry::builtin(), Some(4)).unwrap();
    let options = ScanOptions {
        discover: DiscoverOptions::default(),
        git: false,
        measure: true,
    };
    Snapshot::collect(&engine, root, &options, &Cancel::new())
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn finds_projects_across_ecosystems() {
    let fixture = build_fixture();
    let root = fixture.root();
    let snapshot = scan(&root);
    let registry = Registry::builtin();

    let mut projects: Vec<(String, Vec<String>)> = snapshot
        .projects
        .iter()
        .map(|p| {
            (
                rel(&root, &p.path),
                p.ecosystems
                    .iter()
                    .map(|&e| registry.ecosystem(e).key.clone())
                    .collect(),
            )
        })
        .collect();
    projects.sort();
    let expected: Vec<(String, Vec<String>)> = [
        ("cpp", vec!["cmake"]),
        ("game", vec!["unity"]),
        ("jvm", vec!["gradle"]),
        ("jvm/app", vec!["gradle"]),
        ("py", vec!["python"]),
        ("rust-app", vec!["rust"]),
        ("rust-app/.claude/worktrees/agent-1", vec!["rust"]),
        ("tools", vec!["node"]),
        ("tools/build", vec!["node"]),
        ("vendored", vec!["rust"]),
        ("web", vec!["node"]),
        ("web/packages/ui", vec!["node"]),
        ("ws", vec!["rust"]),
        ("ws/crates/a", vec!["rust"]),
        ("ws/crates/b", vec!["rust"]),
    ]
    .into_iter()
    .map(|(p, e)| (p.to_owned(), e.into_iter().map(str::to_owned).collect()))
    .collect();
    assert_eq!(projects, expected);
}

#[test]
fn finds_artifacts_and_never_descends_into_them() {
    let fixture = build_fixture();
    let root = fixture.root();
    let snapshot = scan(&root);

    let mut artifacts: Vec<(String, ArtifactKind)> = snapshot
        .artifacts
        .iter()
        .map(|a| (rel(&root, &a.path), a.kind))
        .collect();
    artifacts.sort();
    use ArtifactKind::*;
    let expected: Vec<(String, ArtifactKind)> = [
        ("cpp/build", Build),
        ("cpp/custom-out", Build),
        ("game/Library", Cache),
        ("jvm/app/build", Build),
        ("orphan/target", Build),
        ("py/.venv", Environment),
        ("py/build", Build),
        ("py/src/svc/__pycache__", Cache),
        ("rust-app/.carwash-trash-99-0-target", Leftover),
        ("rust-app/.claude/worktrees/agent-1/target", Build),
        ("rust-app/target", Build),
        ("scripts/__pycache__", Cache),
        ("scripts/venv", Environment),
        ("stray/node_modules", Dependencies),
        ("web/.next", Build),
        ("web/.nx/cache", Cache),
        ("web/dist", Build),
        ("web/node_modules", Dependencies),
        ("web/packages/ui/node_modules", Dependencies),
        ("ws/target", Build),
    ]
    .into_iter()
    .map(|(p, k)| (p.to_owned(), k))
    .collect();
    assert_eq!(artifacts, expected);
}

#[test]
fn classifies_safety_by_evidence() {
    let fixture = build_fixture();
    let root = fixture.root();
    let snapshot = scan(&root);
    let by_path = |p: &str| {
        snapshot
            .artifacts
            .iter()
            .find(|a| rel(&root, &a.path) == p)
            .unwrap_or_else(|| panic!("missing {p}"))
    };

    // Generic names without confirming content need review (no git in the fixture).
    assert!(matches!(by_path("web/dist").safety(), Safety::Review(_)));
    assert!(matches!(by_path("py/build").safety(), Safety::Review(_)));
    // Confirmed by content.
    assert_eq!(by_path("jvm/app/build").safety(), Safety::Safe);
    assert!(matches!(
        by_path("jvm/app/build").detection,
        Detection::Rule {
            confirmed: true,
            ..
        }
    ));
    assert_eq!(by_path("rust-app/target").safety(), Safety::Safe);
    assert_eq!(by_path("scripts/venv").detection, Detection::PythonVenv);
    assert_eq!(by_path("cpp/custom-out").detection, Detection::CMakeBuild);
}

#[test]
fn attributes_owners_and_workspace_membership() {
    let fixture = build_fixture();
    let root = fixture.root();
    let snapshot = scan(&root);
    let project = |p: &str| {
        snapshot
            .projects
            .iter()
            .find(|x| rel(&root, &x.path) == p)
            .unwrap()
    };
    let ws = project("ws");
    assert!(ws.is_workspace);
    assert_eq!(project("ws/crates/a").member_of, Some(ws.id));
    assert_eq!(project("ws/crates/a").parent, Some(ws.id));
    assert_eq!(
        project("web/packages/ui").member_of,
        Some(project("web").id)
    );
    assert_eq!(project("jvm/app").member_of, Some(project("jvm").id));
    assert_eq!(project("rust-app").name, "rust-app");
    assert_eq!(project("py").name, "svc");
    assert_eq!(project("jvm").name, "shop");

    let pycache = snapshot
        .artifacts
        .iter()
        .find(|a| rel(&root, &a.path) == "py/src/svc/__pycache__")
        .unwrap();
    assert_eq!(pycache.project, Some(project("py").id));
    let stray = snapshot
        .artifacts
        .iter()
        .find(|a| rel(&root, &a.path) == "stray/node_modules")
        .unwrap();
    assert_eq!(stray.project, None);
}

#[test]
fn measures_every_artifact() {
    let fixture = build_fixture();
    let root = fixture.root();
    let snapshot = scan(&root);
    for artifact in &snapshot.artifacts {
        let size = artifact.size.expect("measured");
        assert_eq!(size.errors, 0, "{}", artifact.path.display());
    }
    let target = snapshot
        .artifacts
        .iter()
        .find(|a| rel(&root, &a.path) == "rust-app/target")
        .unwrap();
    assert!(target.size.unwrap().apparent >= 10_000);
}

#[test]
fn scanning_inside_a_member_reports_the_enclosing_workspace() {
    let fixture = build_fixture();
    let root = fixture.root();
    let member = root.join("ws/crates/a");
    let snapshot = scan(&member);

    let ws = snapshot
        .projects
        .iter()
        .find(|p| p.path == root.join("ws"))
        .expect("enclosing workspace reported");
    assert!(ws.outside_root);
    let a = snapshot.projects.iter().find(|p| p.path == member).unwrap();
    assert!(!a.outside_root);
    assert_eq!(a.member_of, Some(ws.id));

    let target = snapshot
        .artifacts
        .iter()
        .find(|t| t.path == root.join("ws/target"))
        .expect("shared target reported");
    assert!(target.outside_root);
    assert_eq!(target.project, Some(ws.id));
    // Siblings of the member are not scanned.
    assert!(
        !snapshot
            .projects
            .iter()
            .any(|p| p.path.ends_with("crates/b"))
    );
}

#[test]
fn exclusions_depth_and_hidden_options() {
    let fixture = build_fixture();
    let root = fixture.root();
    let engine = Engine::with_threads(Registry::builtin(), Some(2)).unwrap();
    let options = ScanOptions {
        discover: DiscoverOptions {
            exclude: vec![root.join("web")],
            hidden: Hidden::Always,
            max_depth: Some(1),
            ..DiscoverOptions::default()
        },
        git: false,
        measure: false,
    };
    let snapshot = Snapshot::collect(&engine, &root, &options, &Cancel::new());
    assert!(
        !snapshot
            .projects
            .iter()
            .any(|p| p.path.starts_with(root.join("web")))
    );
    assert!(
        snapshot
            .projects
            .iter()
            .any(|p| p.path == root.join(".hidden"))
    );
    // Depth 1 reaches `ws` but not `ws/crates/a`.
    assert!(snapshot.projects.iter().any(|p| p.path == root.join("ws")));
    assert!(
        !snapshot
            .projects
            .iter()
            .any(|p| p.path == root.join("ws/crates/a"))
    );
    assert!(snapshot.artifacts.iter().all(|a| a.size.is_none()));
}
