//! The interactive UI.
//!
//! [`app::App`] holds all state and turns messages into effects; this module is the only
//! place with side effects: terminal I/O, engine threads, the filesystem.

mod app;
mod keymap;
mod pty;
mod query;
mod store;
mod tasks;
mod theme;
mod view;

use crate::context::Context;
use anyhow::Result;
use app::{App, Effect, Msg};
use carwash_core::clean::CleanOptions;
use carwash_core::{Cancel, Counters, ScanOptions, history};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use ratatui::crossterm::execute;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Messages processed before a redraw, so bursts of engine events cost one frame.
const MAX_BATCH: usize = 2_000;
const FAST_TICK: Duration = Duration::from_millis(90);
const IDLE_TICK: Duration = Duration::from_secs(1);

pub fn run(ctx: &Context, root: PathBuf, options: ScanOptions) -> Result<()> {
    let cache = ctx
        .dirs
        .as_ref()
        .map(|d| carwash_core::cache::SizeCache::load(&d.size_cache_file()))
        .unwrap_or_default();
    let mut app = App::new(
        root.clone(),
        ctx.engine.registry().clone(),
        cache,
        ctx.policy(false, false),
        ctx.config.clean.mode,
        theme::Theme::named(&ctx.config.ui.theme),
        theme::Glyphs::named(&ctx.config.ui.icons),
    );

    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        previous_hook(info);
    }));

    let (tx, rx) = crossbeam_channel::unbounded();
    let stop = Arc::new(AtomicBool::new(false));
    let input = spawn_input(tx.clone(), stop.clone());
    let mut runtime = Runtime {
        ctx,
        root,
        options,
        tx,
        scan_cancel: None,
        clean_cancel: None,
        jobs: HashMap::new(),
        next_job: 0,
        pty_size: (24, 80),
    };
    runtime.execute(Effect::Scan);
    runtime.execute(Effect::RefreshDisk);

    let result = event_loop(&mut terminal, &mut app, &mut runtime, &rx);

    stop.store(true, Ordering::Relaxed);
    runtime.cancel_all();
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    let _ = input.join();
    if let Some(dirs) = &ctx.dirs
        && let Err(error) = app.cache.save(&dirs.size_cache_file())
    {
        tracing::warn!(%error, "cannot save size cache");
    }
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    runtime: &mut Runtime<'_>,
    rx: &Receiver<Msg>,
) -> Result<()> {
    loop {
        if app.dirty {
            terminal.draw(|frame| view::render(frame, app))?;
            app.dirty = false;
            runtime.resize_jobs(app.tasks.pty_size);
        }
        let timeout = if app.animating() {
            FAST_TICK
        } else {
            IDLE_TICK
        };
        let first = match rx.recv_timeout(timeout) {
            Ok(msg) => msg,
            Err(RecvTimeoutError::Timeout) => Msg::Tick,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        let mut effects = app.update(first);
        for msg in rx.try_iter().take(MAX_BATCH) {
            effects.extend(app.update(msg));
        }
        if app.animating() {
            // Keep spinners moving even while events stream in.
            effects.extend(app.update(Msg::Tick));
        }
        for effect in effects {
            runtime.execute(effect);
        }
        if app.quit {
            return Ok(());
        }
    }
}

fn spawn_input(tx: Sender<Msg>, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            match event::poll(Duration::from_millis(50)) {
                Ok(true) => {
                    let msg = match event::read() {
                        Ok(Event::Key(key)) => Msg::Key(key),
                        Ok(Event::Mouse(mouse)) => Msg::Mouse(mouse),
                        Ok(Event::Resize(..)) => Msg::Resize,
                        Ok(_) => continue,
                        Err(_) => break,
                    };
                    if tx.send(msg).is_err() {
                        break;
                    }
                }
                Ok(false) => {}
                Err(_) => break,
            }
        }
    })
}

struct Runtime<'a> {
    ctx: &'a Context,
    root: PathBuf,
    options: ScanOptions,
    tx: Sender<Msg>,
    scan_cancel: Option<Cancel>,
    clean_cancel: Option<Cancel>,
    jobs: HashMap<u64, pty::PtyJob>,
    next_job: u64,
    pty_size: (u16, u16),
}

impl Runtime<'_> {
    fn cancel_all(&mut self) {
        for cancel in [self.scan_cancel.take(), self.clean_cancel.take()]
            .into_iter()
            .flatten()
        {
            cancel.cancel();
        }
        for (_, job) in self.jobs.drain() {
            job.kill();
        }
    }

    /// Keeps every job's terminal the size of the output pane.
    fn resize_jobs(&mut self, size: (u16, u16)) {
        if size != self.pty_size {
            self.pty_size = size;
            for job in self.jobs.values() {
                job.resize(size);
            }
        }
    }

    fn execute(&mut self, effect: Effect) {
        match effect {
            Effect::Scan => {
                if let Some(previous) = self.scan_cancel.take() {
                    previous.cancel();
                }
                let cancel = Cancel::new();
                self.scan_cancel = Some(cancel.clone());
                let counters = Arc::new(Counters::default());
                let _ = self.tx.send(Msg::ScanStarted(counters.clone()));
                let engine = self.ctx.engine.clone();
                let (root, options, tx) =
                    (self.root.clone(), self.options.clone(), self.tx.clone());
                std::thread::spawn(move || {
                    engine.scan(&root, &options, &counters, &cancel, &|event| {
                        let _ = tx.send(Msg::Scan(event));
                    });
                });
            }
            Effect::CancelScan => {
                if let Some(cancel) = self.scan_cancel.take() {
                    cancel.cancel();
                }
            }
            Effect::Clean {
                items,
                mode,
                allowed_roots,
            } => {
                let cancel = Cancel::new();
                self.clean_cancel = Some(cancel.clone());
                let engine = self.ctx.engine.clone();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let options = CleanOptions {
                        mode,
                        allowed_roots,
                        dry_run: false,
                    };
                    let report = engine.clean(&items, &options, &cancel, &|event| {
                        let _ = tx.send(Msg::Clean(event));
                    });
                    let _ = tx.send(Msg::CleanFinished(report));
                });
            }
            Effect::CancelClean => {
                if let Some(cancel) = self.clean_cancel.take() {
                    cancel.cancel();
                }
            }
            Effect::RecordHistory(records) => {
                if let Some(dirs) = &self.ctx.dirs
                    && let Err(error) = history::append(&dirs.history_file(), &records)
                {
                    tracing::warn!(%error, "cannot write history");
                }
            }
            Effect::Reveal(path) => reveal(&path),
            Effect::DiscoverTasks { path, ecosystems } => {
                let (registry, tx) = (self.ctx.engine.registry().clone(), self.tx.clone());
                std::thread::spawn(move || {
                    let tasks = carwash_core::tasks::discover(&path, &ecosystems, &registry);
                    let _ = tx.send(Msg::TasksDiscovered(path, tasks));
                });
            }
            Effect::RunTask {
                name,
                targets,
                size,
            } => {
                let registry = self.ctx.engine.registry().clone();
                for target in targets {
                    let Some(task) =
                        carwash_core::tasks::discover(&target.path, &target.ecosystems, &registry)
                            .into_iter()
                            .find(|t| t.name == name)
                    else {
                        continue;
                    };
                    let id = self.next_job;
                    self.next_job += 1;
                    let _ = self.tx.send(Msg::JobStarted {
                        id,
                        label: target.label,
                        task: task.clone(),
                    });
                    match pty::spawn(id, &task, size, self.tx.clone()) {
                        Ok(job) => {
                            self.jobs.insert(id, job);
                        }
                        Err(error) => {
                            let _ = self.tx.send(Msg::JobExited(id, Err(error)));
                        }
                    }
                }
            }
            Effect::KillJob(id) => {
                if let Some(job) = self.jobs.remove(&id) {
                    job.kill();
                }
            }
            Effect::RefreshDisk => {
                let (root, tx) = (self.root.clone(), self.tx.clone());
                std::thread::spawn(move || {
                    if let (Ok(free), Ok(total)) =
                        (fs4::available_space(&root), fs4::total_space(&root))
                    {
                        let _ = tx.send(Msg::Disk { free, total });
                    }
                });
            }
        }
    }
}

/// Shows `path` in the platform file manager, detached from the terminal.
fn reveal(path: &Path) {
    use std::process::{Command, Stdio};
    let mut command = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    let _ = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
