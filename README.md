# carwash

Reclaim disk space across every project on your machine. carwash finds build outputs, dependency
installs, virtualenvs and caches in 40 ecosystems, tells you which ones are safe to delete, and
cleans them. It also checks dependencies for updates and vulnerabilities and runs project tasks,
all from one TUI or a scriptable CLI.

[![Crates.io](https://img.shields.io/crates/v/carwash.svg)](https://crates.io/crates/carwash)
[![CI](https://github.com/epistates/carwash/actions/workflows/ci.yml/badge.svg)](https://github.com/epistates/carwash/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)

Point it at a single package, a monorepo, or a directory holding hundreds of repositories in
mixed languages:

```bash
carwash ~/work            # interactive
carwash scan ~/work       # what could be freed, largest first
carwash clean ~/work --older-than 30d
```

## What it finds

| | Examples |
|---|---|
| Build outputs | `target/`, `build/`, `dist/`, `.next/`, `DerivedData/`, `_build/`, `zig-out/`, `bin/` + `obj/` |
| Dependency installs | `node_modules/`, `vendor/`, `Pods/`, `deps/`, `.gradle/`, `renv/library/` |
| Environments | `.venv/`, `.tox/`, `.direnv/`, `.pixi/`, conda environments |
| Tool caches | `__pycache__/`, `.pytest_cache/`, `.turbo/`, `.parcel-cache/`, `.terraform/`, `.godot/` |
| Global caches | Cargo registry, old Rust toolchains, npm/pnpm/yarn/bun, pip/uv/poetry, Go modules, Gradle, Maven, Homebrew, Xcode DerivedData, iOS device support, simulators, and more |

Projects are recognised by their manifests (`Cargo.toml`, `package.json`, `pyproject.toml`,
`go.mod`, `pom.xml`, `*.csproj`, `Package.swift`, `pubspec.yaml`, `mix.exs`, `CMakeLists.txt`,
`*.uproject`, `project.godot`...). Directories that are unmistakable on their own are found
without a manifest too: a `CACHEDIR.TAG`, a `pyvenv.cfg`, a CMake or Meson build tree, a Cargo
`target/` whose `Cargo.toml` is gone. Run `carwash ecosystems` for the full list.

## Safety

Deleting the wrong directory is the one mistake a cleaner cannot make, so every artifact gets a
status before anything is selected:

- **protected**: git tracks files inside it (a committed `vendor/` or `dist/`). Never deleted.
- **review**: a generic name like `build` or `bin` that nothing confirms as generated, or one git
  reports as untracked rather than ignored. Needs `--include-review` or a look in the TUI.
- **recent**: modified in the last 7 days. Held back by default so you don't delete what you're
  using.
- **ready**: everything else.

Each target is re-checked just before deletion (still a directory, not a symlink, inside the
scanned root, no `.git` inside). Permanent deletion renames the directory to a staging name
first, which is instant and atomic, then removes it in parallel. An interrupted clean leaves a
`.carwash-trash-*` directory that the next scan offers to finish. `--trash` moves to the system
trash instead.

Sizes are allocated blocks, not apparent sizes, with hard links counted once. "Reclaimable" only
counts data whose every link is inside the artifact, so a pnpm `node_modules` hard-linked into
the global store shows how much deleting it would actually free.

## Install

```bash
cargo install --locked carwash
```

Or from a checkout: `cargo install --locked --path crates/carwash`. Requires Rust 1.88 or newer.

## The TUI

Run `carwash [PATH]`. Four tabs, switched with `1` to `4`:

1. **Reclaim**: every project and artifact as a tree (or flat by project or artifact, `Tab`),
   sorted by size. Mark with `Space`, press `d` to review and clean. `/` filters with fuzzy text
   plus facets: `eco:rust,node kind:deps size>1g age>30d is:ready`.
2. **Tasks**: scripts and targets from `package.json`, `deno.json`, `justfile`, `Makefile`,
   `Taskfile`, `mise`, Poe/PDM, Composer and Cargo aliases, plus standard commands per ecosystem.
   `Enter` runs the task in a real terminal inside the TUI; mark several projects to run it in
   all of them.
3. **Updates**: outdated and vulnerable dependencies for Rust, JavaScript, Python and Go. `u`
   updates within your requirements, `U` upgrades to latest (and edits manifests). Projects are
   re-checked after the update finishes.
4. **Caches**: global caches outside any project. `d` (twice) runs the tool's own prune command
   when it is installed, or deletes the directory when that is safe. Sizes are remembered, so
   the tab opens instantly. A cache is measured again when the top of its tree has changed
   (entries added or removed there), after a week otherwise, or when you press `r`.

`p` scans another folder without restarting: type a path (Tab completes, `~` and relative
paths work) and press Enter. Running tasks keep running.

The footer shows the current tab's keys on the left and the keys that work in any tab on the
right. Hover over a key for what it does (for `d`, exactly what it will delete or run), click it
to run it, or click a tab to switch. `?` lists every key. `t` cycles themes. The wheel scrolls
every tab; in Reclaim, clicking selects a row and clicking it again expands it.

## The CLI

| Command | |
|---|---|
| `carwash scan [PATH]` | List reclaimable artifacts. `--json`, `--sort`, `--limit`, filters below |
| `carwash clean [PATH]` | Show the plan, confirm, delete. `--pick 1,3-5`, `--free SIZE`, `--until-free SIZE`, `--dry-run`, `--yes`, `--trash`, `--include-review`, `--include-recent` |
| `carwash tasks [PATH]` | Tasks per project. `--all` for every project's list |
| `carwash run TASK [PATH]` | Run a task in every project that has it. `-j N`, `--filter GLOB`, `--fail-fast`, `--dry-run` |
| `carwash outdated [PATH]` | Outdated and vulnerable dependencies. `--exit-code` for CI, `--no-vulns`, `--refresh` |
| `carwash caches` | Global caches and their sizes |
| `carwash caches clean ID...` | Clean caches; a parent id such as `rustup-toolchains` selects all its children |
| `carwash ecosystems` | Recognised ecosystems |
| `carwash history` | Space reclaimed so far |
| `carwash completions SHELL` | Shell completions |

Filters shared by `scan` and `clean`: `--min-size 500MB`, `--older-than 3mo`,
`--kind build,deps,cache,env,other,leftover`, `-e rust,node`, `--exclude PATH`, `--max-depth N`,
`--hidden`, `--cross-fs`. Every listing command takes `--json`.

### Choosing exactly what to delete

`carwash scan` numbers its rows, and `carwash clean --pick` deletes the ones you name:

```bash
carwash scan ~/work
carwash clean --pick 1,3-5,7
carwash clean ~/work --pick ~/work/app/target --pick ~/work/web/node_modules
```

`--pick` also takes artifact paths (repeat the flag, or separate them with commas), which need
no earlier scan. A pick is an explicit choice, so
it can include recently used and needs-review artifacts, but never protected ones. carwash
scans the tree again before deleting and skips anything that is no longer an artifact.

### Scripts and agents

To make room before a big build, download or container pull, ask for the space you need
instead of cleaning everything:

```bash
carwash clean ~/work --until-free 50GB --yes --json   # make sure the disk has 50 GB available
carwash clean ~/work --free 20GB --yes --json         # reclaim about 20 GB
```

Both take the least recently used artifacts first and stop once the goal is reached, and only
ever pick what the default policy would clean: nothing protected, recently used or needing
review. `--until-free` does nothing when the disk already has the room. Add `--dry-run` to see
the plan first; `carwash scan --json` reports the disk's `available` bytes and what is
`selectable`.

To choose for yourself, read `carwash scan --json` (every artifact with its `path`, `size`,
`kind`, `hold` and `modified` time) and pass the paths you want to `clean --pick`. Prefer
paths over row numbers when anything else might run a scan meanwhile: numbers refer to the
most recent scan on the machine.

Without a terminal, `clean` refuses to delete unless given `--yes`. With `--json`, stdout is one
JSON document: `selected`, `held` (counts by reason), `skipped` (picks it refused, with the
reason), `removed`, `failed`, `bytes`, `free_before`, `free_after` and, with a goal,
`goal: { bytes, until_free, met }`. Exit status is 0 on success or when there is nothing to
do, 1 when a deletion failed, a pick was skipped or the goal could not be met, and 2 on
invalid usage or errors.

## Configuration

`~/.config/carwash/config.toml` (or `$CARWASH_CONFIG`). Every key is optional:

```toml
[scan]
exclude = ["~/work/archive"]   # never entered
default_excludes = true        # also skip ~/Library, ~/Pictures, ~/go/pkg/mod...
include_hidden = false         # hidden directories are entered inside projects either way
same_filesystem = true
threads = 0                    # 0: derived from the CPU count

[clean]
mode = "permanent"             # or "trash"
recent_days = 7

[updates]
cache_hours = 6                # reuse registry lookups
vulnerabilities = true         # query OSV

[ui]
theme = "gestalt"              # gestalt, latte, nord, dracula, ansi
icons = "unicode"              # or ascii
```

Ecosystems and global caches are data, not code. Add your own, or replace a built-in by reusing
its `id`, in `~/.config/carwash/ecosystems.toml` and `~/.config/carwash/caches.toml`. The
built-in files document every field:
[`builtin.toml`](crates/carwash-core/src/ecosystem/builtin.toml),
[`caches.toml`](crates/carwash-core/src/caches.toml).

carwash keeps a size cache and registry cache in `~/.cache/carwash`, history in
`~/.local/share/carwash`, and TUI logs in `~/.local/state/carwash/logs`. `CARWASH_HOME` puts all
of it under one directory.

## How it works

The workspace has two crates. `carwash-core` is the engine and has no UI code: parallel
discovery that reads names only and never descends into artifacts, batched git inspection
(`git ls-files`, `git check-ignore --stdin`), parallel measurement, the safety policy, and the
cleaner. `carwash` is the CLI and a ratatui TUI built as an Elm-style update loop, with tasks
running in pseudo-terminals.

## License

[MIT](./LICENSE)
