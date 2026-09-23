//! Discovery of runnable tasks: project scripts and ecosystem standard commands.
//!
//! Every task resolves to a program and arguments (no shell), run in the project directory.
//! Sources, in priority order: package.json scripts, deno tasks, justfile, Makefile,
//! Taskfile, mise, poe/pdm (pyproject), composer scripts, cargo aliases, then the standard
//! tasks each ecosystem declares in its rules (`cargo test`, `go test ./...`...).

use crate::ecosystem::{EcoId, Registry};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Task {
    pub name: String,
    /// Where the task comes from: a file name (`package.json`, `justfile`) or an ecosystem id
    /// for standard tasks.
    pub source: String,
    pub program: String,
    pub args: Vec<String>,
    pub description: Option<String>,
    pub cwd: PathBuf,
    /// Declared by an ecosystem rule rather than by the project.
    pub standard: bool,
}

impl Task {
    /// The command as a user would type it.
    pub fn command_line(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .map(quote)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@+,%".contains(c))
    {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

fn read(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// Package manager for a JavaScript project, from `packageManager` or the nearest lockfile
/// (workspace members share the root's lockfile).
pub fn node_package_manager(dir: &Path) -> &'static str {
    if let Some(manifest) = read(&dir.join("package.json"))
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&manifest)
        && let Some(pm) = json.get("packageManager").and_then(|v| v.as_str())
    {
        for known in ["pnpm", "yarn", "bun", "npm"] {
            if pm.starts_with(known) {
                return known;
            }
        }
    }
    for ancestor in dir.ancestors().take(6) {
        for (lockfile, pm) in [
            ("pnpm-lock.yaml", "pnpm"),
            ("yarn.lock", "yarn"),
            ("bun.lock", "bun"),
            ("bun.lockb", "bun"),
            ("package-lock.json", "npm"),
        ] {
            if ancestor.join(lockfile).is_file() {
                return pm;
            }
        }
    }
    "npm"
}

struct Collector<'a> {
    dir: &'a Path,
    tasks: Vec<Task>,
}

impl Collector<'_> {
    fn add(
        &mut self,
        name: &str,
        source: &str,
        argv: Vec<String>,
        description: Option<String>,
        standard: bool,
    ) {
        if name.is_empty() || self.tasks.iter().any(|t| t.name == name) {
            return;
        }
        let mut argv = argv.into_iter();
        let Some(program) = argv.next() else {
            return;
        };
        self.tasks.push(Task {
            name: name.to_owned(),
            source: source.to_owned(),
            program,
            args: argv.collect(),
            description: description.filter(|d| !d.is_empty()),
            cwd: self.dir.to_path_buf(),
            standard,
        });
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

/// Tasks available in `dir`, a project of `ecosystems`.
pub fn discover(dir: &Path, ecosystems: &[EcoId], registry: &Registry) -> Vec<Task> {
    let mut out = Collector {
        dir,
        tasks: Vec::new(),
    };
    let has = |name: &str| dir.join(name).exists();
    let is_node = ecosystems
        .iter()
        .any(|&e| registry.ecosystem(e).key == "node");
    let pm = if is_node {
        node_package_manager(dir)
    } else {
        "npm"
    };

    if is_node {
        package_json(&mut out, pm);
    }
    for name in ["deno.json", "deno.jsonc"] {
        if has(name) {
            deno(&mut out, name);
        }
    }
    for name in ["justfile", "Justfile", ".justfile"] {
        if let Some(source) = read(&dir.join(name)) {
            for (task, description) in justfile_recipes(&source) {
                out.add(
                    &task,
                    "justfile",
                    argv(&["just", &task]),
                    description,
                    false,
                );
            }
            break;
        }
    }
    for name in ["GNUmakefile", "makefile", "Makefile"] {
        if let Some(source) = read(&dir.join(name)) {
            for (task, description) in makefile_targets(&source) {
                out.add(
                    &task,
                    "Makefile",
                    argv(&["make", &task]),
                    description,
                    false,
                );
            }
            break;
        }
    }
    for name in [
        "Taskfile.yml",
        "Taskfile.yaml",
        "taskfile.yml",
        "taskfile.yaml",
    ] {
        if has(name) {
            taskfile(&mut out, name);
            break;
        }
    }
    for name in ["mise.toml", ".mise.toml"] {
        if has(name) {
            mise(&mut out, name);
        }
    }
    if has("pyproject.toml") {
        pyproject(&mut out);
    }
    if has("composer.json") {
        composer(&mut out);
    }
    for name in [".cargo/config.toml", ".cargo/config"] {
        if has(name) {
            cargo_aliases(&mut out, name);
        }
    }

    for &eco in ecosystems {
        let ecosystem = registry.ecosystem(eco);
        for spec in &ecosystem.tasks {
            if spec.when.as_deref().is_some_and(|f| !has(f))
                || spec.unless.as_deref().is_some_and(has)
            {
                continue;
            }
            let run: Vec<String> = spec.run.iter().map(|a| a.replace("{pm}", pm)).collect();
            out.add(
                &spec.name,
                &ecosystem.key,
                run,
                spec.description.clone(),
                true,
            );
        }
    }
    out.tasks
}

fn package_json(out: &mut Collector<'_>, pm: &str) {
    let Some(json) = read(&out.dir.join("package.json"))
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    else {
        return;
    };
    let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) else {
        return;
    };
    for (name, command) in scripts {
        // `pretest`/`posttest` run automatically around `test`.
        let hook = ["pre", "post"].iter().any(|p| {
            name.strip_prefix(p)
                .is_some_and(|base| scripts.contains_key(base))
        });
        if hook {
            continue;
        }
        out.add(
            name,
            "package.json",
            argv(&[pm, "run", name]),
            command.as_str().map(str::to_owned),
            false,
        );
    }
}

/// Removes `//` and `/* */` comments outside strings, for JSONC files such as `deno.jsonc`.
pub(crate) fn strip_json_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => {
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut previous = ' ';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn deno(out: &mut Collector<'_>, file: &str) {
    let Some(json) = read(&out.dir.join(file))
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&strip_json_comments(&s)).ok())
    else {
        return;
    };
    let Some(tasks) = json.get("tasks").and_then(|t| t.as_object()) else {
        return;
    };
    for (name, value) in tasks {
        let description = value
            .as_str()
            .or_else(|| value.get("description").and_then(|d| d.as_str()))
            .or_else(|| value.get("command").and_then(|d| d.as_str()))
            .map(str::to_owned);
        out.add(
            name,
            file,
            argv(&["deno", "task", name]),
            description,
            false,
        );
    }
}

/// Public recipes of a justfile with their doc comments.
pub fn justfile_recipes(source: &str) -> Vec<(String, Option<String>)> {
    let mut recipes = Vec::new();
    let mut comment: Option<String> = None;
    let mut private = false;
    for line in source.lines() {
        if line.starts_with(char::is_whitespace) || line.trim().is_empty() {
            if line.trim().is_empty() {
                comment = None;
                private = false;
            }
            continue;
        }
        let line = line.trim_end();
        if let Some(text) = line.strip_prefix('#') {
            if !text.starts_with('!') {
                comment = Some(text.trim().to_owned());
            }
            continue;
        }
        if line.starts_with('[') {
            if line.contains("private") {
                private = true;
            }
            if let Some(doc) = line
                .split_once("doc(")
                .and_then(|(_, rest)| rest.split(['\'', '"']).nth(1))
            {
                comment = Some(doc.to_owned());
            }
            continue;
        }
        let keyword = line.split_whitespace().next().unwrap_or_default();
        if matches!(
            keyword,
            "set" | "alias" | "export" | "import" | "mod" | "unexport"
        ) {
            comment = None;
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        if line[colon..].starts_with(":=") {
            comment = None;
            continue;
        }
        let head = line[..colon].trim_start_matches('@');
        let name = head.split_whitespace().next().unwrap_or_default();
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if valid && !name.starts_with('_') && !private {
            recipes.push((name.to_owned(), comment.take()));
        }
        comment = None;
        private = false;
    }
    recipes
}

/// Explicit targets of a Makefile, with `## description` comments.
pub fn makefile_targets(source: &str) -> Vec<(String, Option<String>)> {
    let mut targets: Vec<(String, Option<String>)> = Vec::new();
    let mut comment: Option<String> = None;
    for line in source.lines() {
        if line.starts_with('\t') {
            continue;
        }
        if let Some(text) = line.strip_prefix('#') {
            comment = Some(text.trim_start_matches('#').trim().to_owned());
            continue;
        }
        let Some(colon) = line.find(':') else {
            comment = None;
            continue;
        };
        let rest = &line[colon + 1..];
        if rest.starts_with('=') || line[..colon].contains('=') {
            comment = None;
            continue;
        }
        let inline = rest.split_once("##").map(|(_, d)| d.trim().to_owned());
        let description = inline.or_else(|| comment.take());
        for name in line[..colon].split_whitespace() {
            let valid = !name.starts_with('.')
                && !name.contains(['%', '$', '(', ')'])
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-./".contains(c));
            if valid && !targets.iter().any(|(t, _)| t == name) {
                targets.push((name.to_owned(), description.clone()));
            }
        }
        comment = None;
    }
    targets
}

fn taskfile(out: &mut Collector<'_>, file: &str) {
    let Some(doc) =
        read(&out.dir.join(file)).and_then(|s| yaml_serde::from_str::<yaml_serde::Value>(&s).ok())
    else {
        return;
    };
    let Some(tasks) = doc.get("tasks").and_then(|t| t.as_mapping()) else {
        return;
    };
    for (name, task) in tasks {
        let Some(name) = name.as_str() else { continue };
        if task
            .get("internal")
            .and_then(|i| i.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let description = task
            .get("desc")
            .or_else(|| task.get("summary"))
            .and_then(|d| d.as_str())
            .map(str::to_owned);
        out.add(name, file, argv(&["task", name]), description, false);
    }
}

fn mise(out: &mut Collector<'_>, file: &str) {
    let Some(table) = read(&out.dir.join(file)).and_then(|s| s.parse::<toml::Table>().ok()) else {
        return;
    };
    let Some(tasks) = table.get("tasks").and_then(|t| t.as_table()) else {
        return;
    };
    for (name, task) in tasks {
        let description = match task {
            toml::Value::String(run) => Some(run.clone()),
            toml::Value::Table(t) => t
                .get("description")
                .or_else(|| t.get("run"))
                .and_then(|d| d.as_str())
                .map(str::to_owned),
            _ => None,
        };
        let hidden = task
            .get("hide")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false);
        if !hidden {
            out.add(name, file, argv(&["mise", "run", name]), description, false);
        }
    }
}

fn pyproject(out: &mut Collector<'_>) {
    let Some(table) =
        read(&out.dir.join("pyproject.toml")).and_then(|s| s.parse::<toml::Table>().ok())
    else {
        return;
    };
    let tool = table.get("tool");
    let describe = |value: &toml::Value| match value {
        toml::Value::String(cmd) => Some(cmd.clone()),
        toml::Value::Table(t) => t
            .get("help")
            .or_else(|| t.get("cmd"))
            .or_else(|| t.get("shell"))
            .and_then(|d| d.as_str())
            .map(str::to_owned),
        _ => None,
    };
    if let Some(tasks) = tool
        .and_then(|t| t.get("poe"))
        .and_then(|p| p.get("tasks"))
        .and_then(|t| t.as_table())
    {
        for (name, value) in tasks.iter().filter(|(n, _)| !n.starts_with('_')) {
            out.add(
                name,
                "pyproject.toml",
                argv(&["poe", name]),
                describe(value),
                false,
            );
        }
    }
    if let Some(scripts) = tool
        .and_then(|t| t.get("pdm"))
        .and_then(|p| p.get("scripts"))
        .and_then(|s| s.as_table())
    {
        for (name, value) in scripts.iter().filter(|(n, _)| !n.starts_with('_')) {
            out.add(
                name,
                "pyproject.toml",
                argv(&["pdm", "run", name]),
                describe(value),
                false,
            );
        }
    }
}

fn composer(out: &mut Collector<'_>) {
    let Some(json) = read(&out.dir.join("composer.json"))
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    else {
        return;
    };
    let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) else {
        return;
    };
    for (name, command) in scripts {
        // Event hooks (pre-install-cmd, post-update-cmd...) are not meant to be run by hand.
        if name.starts_with("pre-") || name.starts_with("post-") {
            continue;
        }
        let description = match command {
            serde_json::Value::String(s) => Some(s.clone()),
            _ => None,
        };
        out.add(
            name,
            "composer.json",
            argv(&["composer", "run-script", name]),
            description,
            false,
        );
    }
}

fn cargo_aliases(out: &mut Collector<'_>, file: &str) {
    let Some(table) = read(&out.dir.join(file)).and_then(|s| s.parse::<toml::Table>().ok()) else {
        return;
    };
    let Some(aliases) = table.get("alias").and_then(|a| a.as_table()) else {
        return;
    };
    for (name, value) in aliases {
        let description = match value {
            toml::Value::String(s) => Some(format!("cargo {s}")),
            toml::Value::Array(parts) => Some(format!(
                "cargo {}",
                parts
                    .iter()
                    .filter_map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            )),
            _ => None,
        };
        out.add(
            name,
            "cargo alias",
            argv(&["cargo", name]),
            description,
            false,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, contents: &str) {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn names(tasks: &[Task]) -> Vec<&str> {
        tasks.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn justfile_recipes_skip_private_settings_and_variables() {
        let source = r#"
set shell := ["bash", "-c"]
version := "1.0"

# Build everything
build *args:
    cargo build {{args}}

[private]
helper:
    echo hi

_hidden:
    true

[doc('Run the tests')]
@test: build
    cargo test
alias t := test
"#;
        assert_eq!(
            justfile_recipes(source),
            vec![
                ("build".to_owned(), Some("Build everything".to_owned())),
                ("test".to_owned(), Some("Run the tests".to_owned())),
            ]
        );
    }

    #[test]
    fn makefile_targets_with_descriptions() {
        let source = "\
CC := cc
.PHONY: all test
all: build ## Build it all
# Run tests
test:
\t./run-tests
%.o: %.c
\t$(CC) -c $<
docs/site: docs
lint fmt:
";
        assert_eq!(
            makefile_targets(source),
            vec![
                ("all".to_owned(), Some("Build it all".to_owned())),
                ("test".to_owned(), Some("Run tests".to_owned())),
                ("docs/site".to_owned(), None),
                ("lint".to_owned(), None),
                ("fmt".to_owned(), None),
            ]
        );
    }

    #[test]
    fn jsonc_comments_are_stripped() {
        let source =
            "{ // tasks\n \"tasks\": { \"dev\": \"deno run a.ts // not a comment\" } /* end */ }";
        let json: serde_json::Value = serde_json::from_str(&strip_json_comments(source)).unwrap();
        assert_eq!(json["tasks"]["dev"], "deno run a.ts // not a comment");
    }

    #[test]
    fn node_project_uses_its_package_manager() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"scripts":{"build":"tsc","pretest":"x","test":"vitest","prepare":"husky"}}"#,
        );
        write(dir.path(), "pnpm-lock.yaml", "");
        let registry = Registry::builtin();
        let node = registry.find("node").unwrap();
        let tasks = discover(dir.path(), &[node], &registry);
        let build = tasks.iter().find(|t| t.name == "build").unwrap();
        assert_eq!(build.command_line(), "pnpm run build");
        assert_eq!(build.description.as_deref(), Some("tsc"));
        assert!(
            !names(&tasks).contains(&"pretest"),
            "hook of an existing script"
        );
        assert!(
            names(&tasks).contains(&"prepare"),
            "not a hook: no `pare` script"
        );
        let install = tasks.iter().find(|t| t.name == "install").unwrap();
        assert!(install.standard);
        assert_eq!(install.command_line(), "pnpm install");
    }

    #[test]
    fn package_manager_field_wins_and_members_use_the_root_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "yarn.lock", "");
        write(dir.path(), "packages/a/package.json", "{}");
        assert_eq!(node_package_manager(&dir.path().join("packages/a")), "yarn");
        write(
            dir.path(),
            "packages/b/package.json",
            r#"{"packageManager":"bun@1.1.0"}"#,
        );
        assert_eq!(node_package_manager(&dir.path().join("packages/b")), "bun");
    }

    #[test]
    fn project_scripts_shadow_standard_tasks() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Cargo.toml", "[package]\nname = \"x\"\n");
        write(dir.path(), "justfile", "test:\n    cargo nextest run\n");
        write(
            dir.path(),
            ".cargo/config.toml",
            "[alias]\nxtask = \"run -p xtask --\"\n",
        );
        let registry = Registry::builtin();
        let rust = registry.find("rust").unwrap();
        let tasks = discover(dir.path(), &[rust], &registry);
        let test = tasks.iter().find(|t| t.name == "test").unwrap();
        assert_eq!(test.command_line(), "just test");
        assert!(!test.standard);
        let xtask = tasks.iter().find(|t| t.name == "xtask").unwrap();
        assert_eq!(xtask.command_line(), "cargo xtask");
        assert!(tasks.iter().any(|t| t.name == "clippy" && t.standard));
    }

    #[test]
    fn standard_tasks_respect_when_and_unless() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "build.gradle.kts", "");
        let registry = Registry::builtin();
        let gradle = registry.find("gradle").unwrap();
        let build = |tasks: Vec<Task>| {
            tasks
                .into_iter()
                .find(|t| t.name == "build")
                .unwrap()
                .command_line()
        };
        assert_eq!(
            build(discover(dir.path(), &[gradle], &registry)),
            "gradle build"
        );
        write(dir.path(), "gradlew", "");
        assert_eq!(
            build(discover(dir.path(), &[gradle], &registry)),
            "./gradlew build"
        );
    }

    #[test]
    fn taskfile_mise_pyproject_and_composer() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Taskfile.yml",
            "version: '3'\ntasks:\n  dev:\n    desc: Start dev\n    cmds: [go run .]\n  secret:\n    internal: true\n",
        );
        write(
            dir.path(),
            "mise.toml",
            "[tasks.lint]\ndescription = \"Lint all\"\nrun = \"eslint .\"\n",
        );
        write(
            dir.path(),
            "pyproject.toml",
            "[tool.poe.tasks]\nserve = \"uvicorn app:app\"\n[tool.pdm.scripts]\nmigrate = { cmd = \"alembic upgrade head\" }\n",
        );
        write(
            dir.path(),
            "composer.json",
            r#"{"scripts":{"test":"phpunit","post-install-cmd":"x"}}"#,
        );
        let tasks = discover(dir.path(), &[], &Registry::builtin());
        let lines: Vec<(String, String)> = tasks
            .iter()
            .map(|t| (t.name.clone(), t.command_line()))
            .collect();
        for expected in [
            ("dev", "task dev"),
            ("lint", "mise run lint"),
            ("serve", "poe serve"),
            ("migrate", "pdm run migrate"),
            ("test", "composer run-script test"),
        ] {
            assert!(
                lines.contains(&(expected.0.to_owned(), expected.1.to_owned())),
                "{expected:?} in {lines:?}"
            );
        }
        assert!(!names(&tasks).contains(&"secret"));
        assert!(!names(&tasks).contains(&"post-install-cmd"));
    }

    #[test]
    fn command_lines_quote_when_needed() {
        let task = Task {
            name: "x".into(),
            source: "s".into(),
            program: "go".into(),
            args: vec!["test".into(), "./...".into(), "it's".into()],
            description: None,
            cwd: PathBuf::new(),
            standard: true,
        };
        assert_eq!(task.command_line(), r"go test ./... 'it'\''s'");
    }
}
