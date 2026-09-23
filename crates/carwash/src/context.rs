//! Shared setup for every command: configuration, rules, engine, and scan helpers.

use crate::cli::{FilterArgs, GlobalArgs, WalkArgs};
use crate::config::Config;
use anyhow::{Context as _, Result, bail};
use carwash_core::cache::SizeCache;
use carwash_core::discover::Hidden;
use carwash_core::paths::Dirs;
use carwash_core::select::{Filter, Policy};
use carwash_core::{
    Cancel, Counters, DiscoverOptions, Engine, Registry, ScanEvent, ScanOptions, Snapshot,
};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub struct Context {
    pub config: Config,
    pub dirs: Option<Dirs>,
    pub engine: Engine,
}

impl Context {
    pub fn load(global: &GlobalArgs) -> Result<Self> {
        let dirs = Dirs::discover();
        let config_path = global
            .config
            .clone()
            .or_else(|| dirs.as_ref().map(Dirs::config_file));
        let config = match &config_path {
            Some(path) => Config::load(path)?,
            None => Config::default(),
        };
        let registry = match dirs.as_ref().map(Dirs::rules_file) {
            Some(rules) if rules.exists() => {
                let text = std::fs::read_to_string(&rules)
                    .with_context(|| format!("cannot read {}", rules.display()))?;
                Registry::with_overrides(&text)
                    .with_context(|| format!("invalid ecosystem rules in {}", rules.display()))?
            }
            _ => Registry::builtin(),
        };
        let threads = global
            .threads
            .or((config.scan.threads > 0).then_some(config.scan.threads));
        let engine = Engine::with_threads(registry, threads).context("cannot start I/O threads")?;
        Ok(Self {
            config,
            dirs,
            engine,
        })
    }

    pub fn registry(&self) -> &Registry {
        self.engine.registry()
    }

    /// Resolves the scan root and builds scan options from config and flags.
    pub fn scan_options(
        &self,
        walk: &WalkArgs,
        git: bool,
        measure: bool,
    ) -> Result<(PathBuf, ScanOptions)> {
        let root = std::fs::canonicalize(&walk.path)
            .with_context(|| format!("cannot open {}", walk.path.display()))?;
        if !root.is_dir() {
            bail!("{} is not a directory", root.display());
        }
        let mut exclude = self.config.excludes();
        exclude.extend(
            walk.exclude
                .iter()
                .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())),
        );
        // Scanning inside an excluded location is an explicit request: honour it.
        exclude.retain(|ex| !root.starts_with(ex));
        let scan = &self.config.scan;
        Ok((
            root,
            ScanOptions {
                discover: DiscoverOptions {
                    max_depth: walk.max_depth.or(scan.max_depth),
                    hidden: if walk.hidden || scan.include_hidden {
                        Hidden::Always
                    } else {
                        Hidden::InsideProjects
                    },
                    same_filesystem: !walk.cross_fs && scan.same_filesystem,
                    exclude,
                    include_enclosing: !walk.no_enclosing,
                },
                git,
                measure,
            },
        ))
    }

    pub fn filter(&self, args: &FilterArgs) -> Result<Filter> {
        let ecosystems = args
            .ecosystems
            .iter()
            .map(|key| {
                self.registry().find(key).with_context(|| {
                    format!("unknown ecosystem `{key}` (see `carwash ecosystems`)")
                })
            })
            .collect::<Result<_>>()?;
        Ok(Filter {
            min_size: args.min_size,
            older_than: args.older_than,
            kinds: args.kind.clone(),
            ecosystems,
        })
    }

    pub fn policy(&self, include_review: bool, include_recent: bool) -> Policy {
        Policy {
            include_review,
            recent: (!include_recent && self.config.clean.recent_days > 0)
                .then(|| Duration::from_secs(self.config.clean.recent_days * 86_400)),
        }
    }

    /// Runs a scan, showing progress on stderr when it is a terminal, and records measured
    /// sizes in the size cache.
    pub fn run_scan(&self, root: &Path, options: &ScanOptions, show_progress: bool) -> Snapshot {
        let counters = Counters::default();
        let snapshot = Mutex::new(Snapshot::default());
        let discovered = AtomicBool::new(false);
        let cancel = Cancel::new();

        let bar = if show_progress && std::io::stderr().is_terminal() {
            let bar = ProgressBar::new_spinner();
            bar.set_style(
                ProgressStyle::with_template("{spinner:.cyan} {msg}")
                    .expect("valid template")
                    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
            );
            bar.enable_steady_tick(Duration::from_millis(80));
            bar
        } else {
            ProgressBar::hidden()
        };

        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                self.engine
                    .scan(root, options, &counters, &cancel, &|event| {
                        if matches!(event, ScanEvent::DiscoveryFinished { .. }) {
                            discovered.store(true, Ordering::Relaxed);
                        }
                        snapshot.lock().expect("snapshot lock").apply(event);
                    });
            });
            while !worker.is_finished() {
                let artifacts = counters.artifacts.load(Ordering::Relaxed);
                let message = if discovered.load(Ordering::Relaxed) {
                    let found: u64 = snapshot
                        .lock()
                        .expect("snapshot lock")
                        .artifacts
                        .iter()
                        .filter_map(|a| a.size)
                        .map(|s| s.reclaimable)
                        .sum();
                    format!(
                        "Measuring {}/{} artifacts · {} reclaimable so far",
                        counters.measured.load(Ordering::Relaxed),
                        artifacts,
                        carwash_core::fmt::bytes(found)
                    )
                } else {
                    format!(
                        "Scanning {} dirs · {} projects · {} artifacts",
                        counters.dirs.load(Ordering::Relaxed),
                        counters.projects.load(Ordering::Relaxed),
                        artifacts
                    )
                };
                bar.set_message(message);
                std::thread::sleep(Duration::from_millis(80));
            }
        });
        bar.finish_and_clear();

        let mut snapshot = snapshot.into_inner().expect("snapshot lock");
        snapshot.sort();
        if options.measure {
            self.update_size_cache(|cache| {
                for artifact in &snapshot.artifacts {
                    if let Some(size) = artifact.size {
                        cache.insert(artifact.path.clone(), size);
                    }
                }
            });
        }
        snapshot
    }

    /// Loads, edits and saves the size cache; failures only cost a warm start.
    pub fn update_size_cache(&self, edit: impl FnOnce(&mut SizeCache)) {
        let Some(path) = self.dirs.as_ref().map(Dirs::size_cache_file) else {
            return;
        };
        let mut cache = SizeCache::load(&path);
        edit(&mut cache);
        if let Err(error) = cache.save(&path) {
            tracing::warn!(%error, "cannot save size cache");
        }
    }
}
