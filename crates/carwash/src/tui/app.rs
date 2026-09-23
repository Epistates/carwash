//! Application state and the update function (Elm architecture: messages in, effects out).

use super::caches::CachesState;
use super::keymap::{self, Action, Tab};
use super::query::{Fuzzy, Query};
use super::store::{self, EntryStatus, Grouping, RowKey, Rows, SortKey, Store, View};
use super::tasks::{After, JobStatus, RunTarget, TasksState};
use super::theme::{Glyphs, Theme};
use super::updates::UpdatesState;
use carwash_core::cache::SizeCache;
use carwash_core::caches::GlobalCache;
use carwash_core::clean::{CleanEvent, CleanItem, CleanReport, DeleteMode};
use carwash_core::deps::{CheckProgress, Dependency, ProjectDeps, ProjectSpec};
use carwash_core::history::Record;
use carwash_core::select::{Hold, Policy};
use carwash_core::tasks::Task;
use carwash_core::{ArtifactId, Counters, EcoId, Registry, ScanEvent, Size, fmt};
use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

#[derive(Debug)]
pub enum Msg {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize,
    Tick,
    ScanStarted(Arc<Counters>),
    Scan(ScanEvent),
    Clean(CleanEvent),
    CleanFinished(CleanReport),
    Disk {
        free: u64,
        total: u64,
    },
    TasksDiscovered(PathBuf, Vec<Task>),
    JobStarted {
        id: u64,
        label: String,
        task: Task,
        /// What to refresh when the job ends.
        after: After,
    },
    JobOutput(u64, Vec<u8>),
    JobExited(u64, Result<i32, String>),
    DepsProgress(Arc<CheckProgress>),
    DepsChecked(Vec<ProjectDeps>),
    /// Applying updates to the project at the path could not start.
    UpdateFailed(PathBuf, String),
    CachesDiscovered(Vec<GlobalCache>),
    CacheMeasured(String, Size),
}

#[derive(Debug)]
pub enum Effect {
    Scan,
    CancelScan,
    Clean {
        items: Vec<CleanItem>,
        mode: DeleteMode,
        allowed_roots: Vec<PathBuf>,
    },
    CancelClean,
    RecordHistory(Vec<Record>),
    Reveal(PathBuf),
    RefreshDisk,
    DiscoverTasks {
        path: PathBuf,
        ecosystems: Vec<EcoId>,
    },
    /// Runs task `name` in each target that has it, on terminals of `size` (rows, cols).
    RunTask {
        name: String,
        targets: Vec<RunTarget>,
        size: (u16, u16),
    },
    KillJob(u64),
    CheckDeps {
        specs: Vec<ProjectSpec>,
        refresh: bool,
    },
    /// Updates `deps` of the project at `dir` (to latest when `latest`), as jobs.
    ApplyUpdates {
        dir: PathBuf,
        label: String,
        deps: Vec<Dependency>,
        latest: bool,
        size: (u16, u16),
    },
    DiscoverCaches,
    /// Prunes (as jobs) or deletes each cache, then measures it again.
    CleanCaches {
        caches: Vec<GlobalCache>,
        size: (u16, u16),
    },
    MeasureCache {
        id: String,
        path: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
}

#[derive(Debug)]
pub struct Toast {
    pub text: String,
    pub level: Level,
    until: Instant,
}

#[derive(Debug)]
pub enum ReviewPhase {
    Confirm,
    Running {
        done_bytes: u64,
        done: usize,
        failed: usize,
        current: Option<PathBuf>,
    },
    Done {
        report: CleanReport,
        failures: Vec<(PathBuf, String)>,
    },
}

#[derive(Debug)]
pub struct Review {
    pub items: Vec<ArtifactId>,
    pub bytes: u64,
    pub mode: DeleteMode,
    pub phase: ReviewPhase,
    pub scroll: usize,
    /// Held-back artifacts the user marked explicitly, by reason.
    pub held: Vec<(Hold, usize)>,
    pub outside_root: usize,
    removed: Vec<(ArtifactId, u64)>,
    failures: Vec<(PathBuf, String)>,
}

#[derive(Debug)]
pub enum Mode {
    Browse,
    Search,
    Review(Box<Review>),
    Help,
}

#[derive(Debug, Default)]
pub struct ScanStatus {
    pub running: bool,
    pub discovered: bool,
    pub counters: Option<Arc<Counters>>,
    pub elapsed: Option<Duration>,
}

/// Geometry of the last rendered table, for mouse hit-testing.
#[derive(Debug, Default, Clone, Copy)]
pub struct TableGeometry {
    pub area: Rect,
    pub first_row_y: u16,
    pub offset: usize,
}

pub struct App {
    pub store: Store,
    pub registry: Arc<Registry>,
    pub cache: SizeCache,
    pub policy: Policy,
    pub delete_mode: DeleteMode,
    pub mode: Mode,
    pub grouping: Grouping,
    pub sort: SortKey,
    pub input: Input,
    pub query: Query,
    fuzzy: Fuzzy,
    pub marked: HashSet<ArtifactId>,
    pub expanded: HashSet<RowKey>,
    pub rows: Rows,
    rows_built_for: Option<(u64, u64)>,
    view_revision: u64,
    pub selected: usize,
    selected_key: Option<RowKey>,
    pub page: usize,
    pub geometry: TableGeometry,
    pub scan: ScanStatus,
    pub toast: Option<Toast>,
    pub theme: Theme,
    pub glyphs: Glyphs,
    pub show_details: bool,
    pub now: SystemTime,
    pub disk: Option<(u64, u64)>,
    pub spinner: usize,
    pub tab: Tab,
    pub tasks: TasksState,
    pub updates: UpdatesState,
    pub caches: CachesState,
    /// Quit was pressed once while jobs were running.
    quit_armed: bool,
    pub quit: bool,
    pub dirty: bool,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("root", &self.store.root)
            .field("mode", &self.mode)
            .field("rows", &self.rows.rows.len())
            .finish_non_exhaustive()
    }
}

const TOAST_DURATION: Duration = Duration::from_secs(4);

impl App {
    pub fn new(
        root: PathBuf,
        registry: Arc<Registry>,
        cache: SizeCache,
        policy: Policy,
        delete_mode: DeleteMode,
        theme: Theme,
        glyphs: Glyphs,
    ) -> Self {
        Self {
            store: Store::new(root),
            registry,
            cache,
            policy,
            delete_mode,
            mode: Mode::Browse,
            grouping: Grouping::Tree,
            sort: SortKey::Size,
            input: Input::default(),
            query: Query::default(),
            fuzzy: Fuzzy::default(),
            marked: HashSet::new(),
            expanded: HashSet::new(),
            rows: Rows::default(),
            rows_built_for: None,
            view_revision: 0,
            selected: 0,
            selected_key: None,
            page: 10,
            geometry: TableGeometry::default(),
            scan: ScanStatus::default(),
            toast: None,
            theme,
            glyphs,
            show_details: true,
            now: SystemTime::now(),
            disk: None,
            spinner: 0,
            tab: Tab::Reclaim,
            tasks: TasksState::default(),
            updates: UpdatesState::default(),
            caches: CachesState::default(),
            quit_armed: false,
            quit: false,
            dirty: true,
        }
    }

    /// Something is moving on screen, so the runtime should tick quickly.
    pub fn animating(&self) -> bool {
        self.scan.running
            || matches!(&self.mode, Mode::Review(r) if matches!(r.phase, ReviewPhase::Running { .. }))
            || self.toast.is_some()
            || self.tasks.running() > 0
            || !self.updates.checking.is_empty()
            || !self.caches.busy.is_empty()
    }

    pub(crate) fn toast(&mut self, text: impl Into<String>, level: Level) {
        self.toast = Some(Toast {
            text: text.into(),
            level,
            until: Instant::now() + TOAST_DURATION,
        });
    }

    fn view_changed(&mut self) {
        self.view_revision += 1;
    }

    /// Rebuilds rows if the store or the view changed, keeping the cursor on the same row.
    pub fn ensure_rows(&mut self) {
        let stamp = (self.store.revision, self.view_revision);
        if self.rows_built_for == Some(stamp) {
            return;
        }
        let mut view = View {
            grouping: self.grouping,
            sort: self.sort,
            query: &self.query,
            fuzzy: &mut self.fuzzy,
            marked: &self.marked,
            expanded: &self.expanded,
            policy: &self.policy,
            now: self.now,
        };
        self.rows = store::build(&self.store, &mut view);
        self.rows_built_for = Some(stamp);
        if let Some(position) = self
            .selected_key
            .as_ref()
            .and_then(|key| self.rows.position(key))
        {
            self.selected = position;
        }
        self.clamp_selection();
    }

    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.rows.rows.len().saturating_sub(1));
    }

    /// Moves the cursor on the user's behalf; from then on it follows that row when rows
    /// are re-sorted by streaming sizes. Until the user moves, it stays on the first row.
    fn select(&mut self, index: usize) {
        self.selected = index;
        self.clamp_selection();
        self.selected_key = self.rows.rows.get(self.selected).map(|r| r.key.clone());
    }

    pub fn selected_row(&self) -> Option<&store::Row> {
        self.rows.rows.get(self.selected)
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        self.dirty = true;
        match msg {
            Msg::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
            Msg::Key(_) => {
                self.dirty = false;
                Vec::new()
            }
            Msg::Mouse(mouse) => self.on_mouse(mouse),
            Msg::Resize => Vec::new(),
            Msg::Tick => {
                self.spinner = self.spinner.wrapping_add(1);
                self.now = SystemTime::now();
                if self
                    .toast
                    .as_ref()
                    .is_some_and(|t| Instant::now() >= t.until)
                {
                    self.toast = None;
                }
                Vec::new()
            }
            Msg::ScanStarted(counters) => {
                self.scan = ScanStatus {
                    running: true,
                    discovered: false,
                    counters: Some(counters),
                    elapsed: None,
                };
                Vec::new()
            }
            Msg::Scan(event) => self.on_scan(event),
            Msg::Clean(event) => self.on_clean(event),
            Msg::CleanFinished(report) => self.on_clean_finished(report),
            Msg::Disk { free, total } => {
                self.disk = Some((free, total));
                Vec::new()
            }
            Msg::TasksDiscovered(path, tasks) => {
                self.tasks.tasks.insert(path, Some(tasks));
                Vec::new()
            }
            Msg::JobStarted {
                id,
                label,
                task,
                after,
            } => {
                self.tasks.add_job(id, label, task, after);
                Vec::new()
            }
            Msg::CachesDiscovered(caches) => {
                self.caches.set(caches);
                Vec::new()
            }
            Msg::CacheMeasured(id, size) => {
                self.caches.measured(&id, size);
                Vec::new()
            }
            Msg::DepsProgress(progress) => {
                self.updates.progress = Some(progress);
                Vec::new()
            }
            Msg::DepsChecked(results) => {
                self.updates.finish_check(results);
                Vec::new()
            }
            Msg::UpdateFailed(path, error) => {
                self.updates.updating.remove(&path);
                self.toast(error, Level::Warn);
                Vec::new()
            }
            Msg::JobOutput(id, bytes) => {
                if let Some(job) = self.tasks.job_mut(id) {
                    job.parser.process(&bytes);
                }
                Vec::new()
            }
            Msg::JobExited(id, result) => {
                self.tasks.finish_job(id, result);
                if let Some(job) = self.tasks.jobs.iter().find(|j| j.id == id) {
                    let (text, level) = match &job.status {
                        JobStatus::Exited(0) => (
                            format!("✓ {} · {} succeeded", job.label, job.task.name),
                            Level::Info,
                        ),
                        JobStatus::Exited(code) => (
                            format!("✗ {} · {} failed (exit {code})", job.label, job.task.name),
                            Level::Warn,
                        ),
                        JobStatus::Failed(error) => (
                            format!("✗ {} · {}: {error}", job.label, job.task.name),
                            Level::Warn,
                        ),
                        JobStatus::Running => (String::new(), Level::Info),
                    };
                    if !text.is_empty() {
                        self.toast(text, level);
                    }
                }
                let after = self
                    .tasks
                    .jobs
                    .iter()
                    .find(|j| j.id == id)
                    .map(|j| j.after.clone())
                    .unwrap_or_default();
                // Refresh once every job with the same follow-up has finished.
                let pending = self
                    .tasks
                    .jobs
                    .iter()
                    .any(|j| j.status == JobStatus::Running && j.after == after);
                match after {
                    After::Recheck(path) if !pending => self.recheck(&path),
                    After::Remeasure { id, path } if !pending => {
                        vec![Effect::MeasureCache { id, path }]
                    }
                    _ => Vec::new(),
                }
            }
        }
    }

    fn on_scan(&mut self, event: ScanEvent) -> Vec<Effect> {
        match &event {
            ScanEvent::DiscoveryFinished { .. } => self.scan.discovered = true,
            ScanEvent::Measured { id, size } => {
                if let Some(entry) = self.store.entry(*id) {
                    let path = entry.artifact.path.clone();
                    self.cache.insert(path, *size);
                }
            }
            ScanEvent::Finished { elapsed } => {
                self.scan.running = false;
                self.scan.elapsed = Some(*elapsed);
            }
            _ => {}
        }
        self.store.apply(event, &self.cache);
        Vec::new()
    }

    fn on_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match &mut self.mode {
            Mode::Help => {
                self.mode = Mode::Browse;
                Vec::new()
            }
            Mode::Search => self.on_search_key(key),
            Mode::Review(_) => self.on_review_key(key),
            Mode::Browse => match keymap::action_for(self.tab, &key) {
                Some(action) => self.on_global_action(action),
                None => {
                    self.dirty = false;
                    Vec::new()
                }
            },
        }
    }

    /// Actions shared by every tab; the rest go to the current tab.
    fn on_global_action(&mut self, action: Action) -> Vec<Effect> {
        if action != Action::Quit {
            self.quit_armed = false;
        }
        match action {
            Action::ShowReclaim => self.tab = Tab::Reclaim,
            Action::ShowTasks => {
                self.tab = Tab::Tasks;
                return self.tasks_effects();
            }
            Action::Theme => {
                self.theme = self.theme.next();
                self.toast(format!("Theme: {}", self.theme.name), Level::Info);
            }
            Action::Help => self.mode = Mode::Help,
            Action::ShowUpdates => {
                self.tab = Tab::Updates;
                return self.updates_effects();
            }
            Action::ShowCaches => {
                self.tab = Tab::Caches;
                return self.caches_effects();
            }
            Action::Search if self.tab == Tab::Caches => self.dirty = false,
            Action::Search => {
                let current = match self.tab {
                    Tab::Reclaim => self.query.raw.clone(),
                    Tab::Tasks => self.tasks.filter.clone(),
                    Tab::Updates => self.updates.filter.clone(),
                    Tab::Caches => String::new(),
                };
                self.input = Input::new(current);
                self.mode = Mode::Search;
            }
            Action::Quit => {
                let running = self.tasks.running();
                if running > 0 && !self.quit_armed {
                    self.quit_armed = true;
                    self.toast(
                        format!("{running} jobs running: press q again to stop them and quit"),
                        Level::Warn,
                    );
                } else {
                    self.quit = true;
                    return vec![Effect::CancelScan];
                }
            }
            action => return self.on_tab_action(action),
        }
        Vec::new()
    }

    fn on_tab_action(&mut self, action: Action) -> Vec<Effect> {
        match self.tab {
            Tab::Reclaim => self.on_action(action),
            Tab::Tasks => self.on_tasks_action(action),
            Tab::Updates => self.on_updates_action(action),
            Tab::Caches => self.on_caches_action(action),
        }
    }

    fn on_search_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Esc => {
                self.input.reset();
                self.apply_search(String::new());
                self.mode = Mode::Browse;
            }
            KeyCode::Enter => self.mode = Mode::Browse,
            KeyCode::Up | KeyCode::Down => {
                let action = if key.code == KeyCode::Up {
                    Action::Up
                } else {
                    Action::Down
                };
                return self.on_tab_action(action);
            }
            _ => {
                if self.input.handle_event(&Event::Key(key)).is_some() {
                    let value = self.input.value().to_owned();
                    self.apply_search(value);
                }
            }
        }
        match self.tab {
            Tab::Reclaim => Vec::new(),
            Tab::Tasks => self.tasks_effects(),
            Tab::Updates => self.updates_effects(),
            Tab::Caches => Vec::new(),
        }
    }

    fn apply_search(&mut self, value: String) {
        match self.tab {
            Tab::Reclaim => self.set_query(value),
            Tab::Tasks => self.tasks.set_filter(value),
            Tab::Updates => self.updates.set_filter(value),
            Tab::Caches => {}
        }
    }

    fn set_query(&mut self, raw: String) {
        self.query = Query::parse(&raw, &self.registry);
        self.selected = 0;
        self.selected_key = None;
        self.view_changed();
    }

    fn on_review_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Mode::Review(review) = &mut self.mode else {
            return Vec::new();
        };
        match &review.phase {
            ReviewPhase::Confirm => match key.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    let items: Vec<CleanItem> = review
                        .items
                        .iter()
                        .filter_map(|id| self.store.entry(*id))
                        .map(|entry| CleanItem {
                            id: entry.artifact.id,
                            path: entry.artifact.path.clone(),
                            expected_bytes: entry.size().map_or(0, |s| s.reclaimable),
                        })
                        .collect();
                    let mode = review.mode;
                    review.phase = ReviewPhase::Running {
                        done_bytes: 0,
                        done: 0,
                        failed: 0,
                        current: None,
                    };
                    let mut allowed_roots = vec![self.store.root.clone()];
                    allowed_roots.extend(
                        self.store
                            .projects
                            .iter()
                            .filter(|p| p.outside_root)
                            .map(|p| p.path.clone()),
                    );
                    return vec![Effect::Clean {
                        items,
                        mode,
                        allowed_roots,
                    }];
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => self.mode = Mode::Browse,
                KeyCode::Char('t') => {
                    review.mode = match review.mode {
                        DeleteMode::Permanent => DeleteMode::Trash,
                        DeleteMode::Trash => DeleteMode::Permanent,
                    };
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    review.scroll = (review.scroll + 1).min(review.items.len().saturating_sub(1));
                }
                KeyCode::Up | KeyCode::Char('k') => review.scroll = review.scroll.saturating_sub(1),
                _ => self.dirty = false,
            },
            ReviewPhase::Running { .. } => {
                if key.code == KeyCode::Esc {
                    self.toast("Stopping after the directories in progress…", Level::Warn);
                    return vec![Effect::CancelClean];
                }
                self.dirty = false;
            }
            ReviewPhase::Done { .. } => self.mode = Mode::Browse,
        }
        Vec::new()
    }

    fn on_mouse(&mut self, mouse: MouseEvent) -> Vec<Effect> {
        if !matches!(self.mode, Mode::Browse) {
            self.dirty = false;
            return Vec::new();
        }
        if self.tab != Tab::Reclaim {
            // Other tabs record no row geometry; the wheel moves their own cursor.
            let action = match mouse.kind {
                MouseEventKind::ScrollDown => Action::Down,
                MouseEventKind::ScrollUp => Action::Up,
                _ => {
                    self.dirty = false;
                    return Vec::new();
                }
            };
            return (0..3).flat_map(|_| self.on_global_action(action)).collect();
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => self.move_by(3),
            MouseEventKind::ScrollUp => self.move_by(-3),
            MouseEventKind::Down(_) => {
                let g = self.geometry;
                let inside = mouse.column >= g.area.x
                    && mouse.column < g.area.x + g.area.width
                    && mouse.row >= g.first_row_y
                    && mouse.row < g.area.y + g.area.height;
                if inside {
                    let index = g.offset + usize::from(mouse.row - g.first_row_y);
                    if index < self.rows.rows.len() {
                        if index == self.selected {
                            return self.on_action(Action::ToggleExpand);
                        }
                        self.select(index);
                    }
                }
            }
            _ => self.dirty = false,
        }
        Vec::new()
    }

    fn move_by(&mut self, delta: isize) {
        if self.rows.rows.is_empty() {
            return;
        }
        let last = self.rows.rows.len() - 1;
        let target = self.selected.saturating_add_signed(delta).min(last);
        self.select(target);
    }

    fn on_action(&mut self, action: Action) -> Vec<Effect> {
        self.ensure_rows();
        match action {
            Action::Up => self.move_by(-1),
            Action::Down => self.move_by(1),
            Action::PageUp => self.move_by(-(self.page.max(1) as isize)),
            Action::PageDown => self.move_by(self.page.max(1) as isize),
            Action::Top => self.select(0),
            Action::Bottom => self.select(self.rows.rows.len().saturating_sub(1)),
            Action::Expand => {
                if let Some(row) = self.selected_row().cloned() {
                    if row.expandable && !row.expanded {
                        self.expanded.insert(row.key);
                        self.view_changed();
                    } else if row.expanded {
                        self.move_by(1);
                    }
                }
            }
            Action::Collapse => {
                if let Some(row) = self.selected_row().cloned() {
                    if row.expanded && self.query.is_empty() {
                        self.expanded.remove(&row.key);
                        self.view_changed();
                    } else if let Some(parent) = self.rows.rows[..self.selected]
                        .iter()
                        .rposition(|r| r.depth < row.depth)
                    {
                        self.select(parent);
                    }
                }
            }
            Action::ToggleExpand => {
                if let Some(row) = self.selected_row().cloned()
                    && row.expandable
                {
                    if !self.expanded.remove(&row.key) {
                        self.expanded.insert(row.key);
                    }
                    self.view_changed();
                }
            }
            Action::ExpandAll => {
                // Expanding reveals new expandable rows; repeat until stable.
                loop {
                    self.ensure_rows();
                    let before = self.expanded.len();
                    let keys: Vec<RowKey> = self
                        .rows
                        .rows
                        .iter()
                        .filter(|r| r.expandable)
                        .map(|r| r.key.clone())
                        .collect();
                    self.expanded.extend(keys);
                    if self.expanded.len() == before {
                        break;
                    }
                    self.view_changed();
                }
            }
            Action::CollapseAll => {
                self.expanded.clear();
                self.view_changed();
            }
            Action::Mark => self.mark_selected(),
            Action::MarkAll => self.mark_all(),
            Action::Unmark => {
                let n = self.marked.len();
                self.marked.clear();
                self.view_changed();
                if n > 0 {
                    self.toast(format!("Unmarked {n}"), Level::Info);
                }
            }
            Action::Clean => self.open_review(),
            Action::Sort => {
                self.sort = self.sort.next();
                self.view_changed();
            }
            Action::Group => {
                self.grouping = self.grouping.next();
                self.view_changed();
            }
            Action::Details => self.show_details = !self.show_details,
            Action::Rescan => {
                if self.scan.running {
                    self.toast("A scan is already running", Level::Warn);
                } else {
                    self.store = Store::new(self.store.root.clone());
                    self.marked.clear();
                    self.rows_built_for = None;
                    return vec![Effect::Scan, Effect::RefreshDisk];
                }
            }
            Action::Open => {
                if let Some(path) = self.selected_path() {
                    return vec![Effect::Reveal(path)];
                }
            }
            // Global actions and other tabs' actions.
            _ => self.dirty = false,
        }
        Vec::new()
    }

    pub fn selected_path(&self) -> Option<PathBuf> {
        match &self.selected_row()?.key {
            RowKey::Dir(path) => Some(path.clone()),
            RowKey::Project(id) => self.store.project(*id).map(|p| p.path.clone()),
            RowKey::Artifact(id) => self.store.entry(*id).map(|e| e.artifact.path.clone()),
            RowKey::Orphans => None,
        }
    }

    fn hold(&self, id: ArtifactId) -> Option<Hold> {
        self.store
            .entry(id)
            .and_then(|e| store::hold_for(e, &self.policy, self.now))
    }

    fn markable(&self, id: ArtifactId) -> bool {
        self.store
            .entry(id)
            .is_some_and(|e| e.status == EntryStatus::Present)
            && self.hold(id) != Some(Hold::Protected)
    }

    fn mark_selected(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        let ids: Vec<ArtifactId> = self.rows.artifacts(&row).to_vec();
        if let RowKey::Artifact(id) = row.key {
            if self.marked.remove(&id) {
            } else if self.markable(id) {
                self.marked.insert(id);
            } else {
                let reason = self
                    .store
                    .entry(id)
                    .map(|e| e.artifact.safety().describe())
                    .unwrap_or("unavailable");
                self.toast(format!("Protected: {reason}"), Level::Warn);
                return;
            }
        } else {
            let ready: Vec<ArtifactId> = ids
                .iter()
                .copied()
                .filter(|&id| self.markable(id) && self.hold(id).is_none())
                .collect();
            let held = ids
                .iter()
                .filter(|&&id| self.markable(id) && self.hold(id).is_some())
                .count();
            if !ready.is_empty() && ready.iter().all(|id| self.marked.contains(id)) {
                for id in &ids {
                    self.marked.remove(id);
                }
            } else {
                self.marked.extend(ready.iter().copied());
                if ready.is_empty() && held > 0 {
                    self.toast(
                        format!(
                            "Nothing ready here: {held} held back (expand to mark individually)"
                        ),
                        Level::Warn,
                    );
                } else if held > 0 {
                    self.toast(
                        format!(
                            "Marked {} · {held} held back (recent or needs review)",
                            ready.len()
                        ),
                        Level::Info,
                    );
                }
            }
        }
        self.view_changed();
        self.move_by(1);
    }

    fn mark_all(&mut self) {
        let ready: Vec<ArtifactId> = self
            .rows
            .order
            .iter()
            .copied()
            .filter(|&id| self.markable(id) && self.hold(id).is_none())
            .collect();
        let bytes: u64 = ready
            .iter()
            .filter_map(|id| self.store.entry(*id))
            .filter_map(|e| e.size())
            .map(|s| s.reclaimable)
            .sum();
        self.marked.extend(ready.iter().copied());
        self.view_changed();
        self.toast(
            format!(
                "Marked {} ready artifacts · {}",
                ready.len(),
                fmt::bytes(bytes)
            ),
            Level::Info,
        );
    }

    fn open_review(&mut self) {
        let mut items: Vec<ArtifactId> = self
            .marked
            .iter()
            .copied()
            .filter(|id| self.markable(*id))
            .collect();
        if items.is_empty() {
            self.toast(
                "Nothing marked: press Space to mark, a to mark everything ready",
                Level::Warn,
            );
            return;
        }
        let size = |id: &ArtifactId| {
            self.store
                .entry(*id)
                .and_then(|e| e.size())
                .map_or(0, |s| s.reclaimable)
        };
        items.sort_by_key(|id| std::cmp::Reverse(size(id)));
        let bytes = items.iter().map(size).sum();
        let mut held: Vec<(Hold, usize)> = Vec::new();
        for id in &items {
            if let Some(hold) = self.hold(*id) {
                match held.iter_mut().find(|(h, _)| *h == hold) {
                    Some((_, n)) => *n += 1,
                    None => held.push((hold, 1)),
                }
            }
        }
        let outside_root = items
            .iter()
            .filter(|id| {
                self.store
                    .entry(**id)
                    .is_some_and(|e| e.artifact.outside_root)
            })
            .count();
        self.mode = Mode::Review(Box::new(Review {
            items,
            bytes,
            mode: self.delete_mode,
            phase: ReviewPhase::Confirm,
            scroll: 0,
            held,
            outside_root,
            removed: Vec::new(),
            failures: Vec::new(),
        }));
    }

    fn on_clean(&mut self, event: CleanEvent) -> Vec<Effect> {
        match event {
            CleanEvent::Started { id } => {
                self.store.set_status(id, EntryStatus::Deleting);
                let path = self.store.entry(id).map(|e| e.artifact.path.clone());
                if let Mode::Review(review) = &mut self.mode
                    && let ReviewPhase::Running { current, .. } = &mut review.phase
                {
                    *current = path;
                }
            }
            CleanEvent::Removed { id, bytes } => {
                self.store.set_status(id, EntryStatus::Deleted);
                self.marked.remove(&id);
                if let Some(entry) = self.store.entry(id) {
                    let path = entry.artifact.path.clone();
                    self.cache.remove(&path);
                }
                if let Mode::Review(review) = &mut self.mode {
                    review.removed.push((id, bytes));
                    if let ReviewPhase::Running {
                        done_bytes, done, ..
                    } = &mut review.phase
                    {
                        *done_bytes += bytes;
                        *done += 1;
                    }
                }
                self.view_changed();
            }
            CleanEvent::Failed { id, error } => {
                self.store
                    .set_status(id, EntryStatus::Failed(error.clone()));
                let path = self
                    .store
                    .entry(id)
                    .map(|e| e.artifact.path.clone())
                    .unwrap_or_default();
                if let Mode::Review(review) = &mut self.mode {
                    review.failures.push((path, error));
                    if let ReviewPhase::Running { failed, .. } = &mut review.phase {
                        *failed += 1;
                    }
                }
            }
        }
        Vec::new()
    }

    fn on_clean_finished(&mut self, report: CleanReport) -> Vec<Effect> {
        let mut records = Vec::new();
        if let Mode::Review(review) = &mut self.mode {
            for (id, bytes) in &review.removed {
                if let Some(entry) = self.store.entry(*id) {
                    records.push(Record::now(
                        entry.artifact.path.clone(),
                        *bytes,
                        entry.artifact.kind,
                        entry
                            .artifact
                            .ecosystem
                            .map(|e| self.registry.ecosystem(e).key.clone()),
                        review.mode,
                    ));
                }
            }
            let failures = std::mem::take(&mut review.failures);
            review.phase = ReviewPhase::Done { report, failures };
        }
        // Items interrupted by a cancel are back to normal.
        let deleting: Vec<ArtifactId> = self
            .store
            .entries
            .iter()
            .filter(|e| e.status == EntryStatus::Deleting)
            .map(|e| e.artifact.id)
            .collect();
        for id in deleting {
            self.store.set_status(id, EntryStatus::Present);
        }
        self.view_changed();
        vec![Effect::RecordHistory(records), Effect::RefreshDisk]
    }

    /// Scan progress text for the header.
    pub fn scan_progress(&self) -> Option<String> {
        let counters = self.scan.counters.as_ref()?;
        if !self.scan.running {
            return None;
        }
        let artifacts = counters.artifacts.load(Ordering::Relaxed);
        Some(if self.scan.discovered {
            format!(
                "measuring {}/{}",
                counters.measured.load(Ordering::Relaxed),
                artifacts
            )
        } else {
            format!(
                "scanning {} dirs · {} projects · {} artifacts",
                crate::commands::count(counters.dirs.load(Ordering::Relaxed)),
                crate::commands::count(counters.projects.load(Ordering::Relaxed)),
                artifacts
            )
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use carwash_core::{
        Artifact, ArtifactKind, Detection, EcoId, GitState, Project, ProjectId, RuleId, Size,
    };
    use ratatui::crossterm::event::KeyModifiers;

    /// An app with two projects: a ready target, a recent node_modules, a protected vendor.
    pub(crate) fn app() -> App {
        let mut app = App::new(
            PathBuf::from("/w"),
            Arc::new(Registry::builtin()),
            SizeCache::default(),
            Policy::default(),
            DeleteMode::Permanent,
            Theme::ANSI,
            Glyphs::UNICODE,
        );
        let old = SystemTime::now() - Duration::from_secs(90 * 86_400);
        let project = |id: u32, path: &str| Project {
            id: ProjectId(id),
            path: PathBuf::from(path),
            name: path.rsplit('/').next().unwrap().into(),
            ecosystems: vec![EcoId(0)],
            parent: None,
            member_of: None,
            is_workspace: false,
            repo: None,
            last_activity: None,
            outside_root: false,
        };
        let artifact =
            |id: u32, project: u32, path: &str, bytes: u64, modified: SystemTime, git| Artifact {
                id: ArtifactId(id),
                path: PathBuf::from(path),
                project: Some(ProjectId(project)),
                ecosystem: Some(EcoId(0)),
                kind: ArtifactKind::Build,
                detection: Detection::Rule {
                    rule: RuleId(0),
                    confirmed: true,
                },
                ambiguous: false,
                modified: Some(modified),
                git,
                repo: None,
                size: Some(Size {
                    reclaimable: bytes,
                    on_disk: bytes,
                    newest: Some(modified),
                    ..Size::default()
                }),
                outside_root: false,
            };
        for event in [
            ScanEvent::Project(project(0, "/w/a")),
            ScanEvent::Project(project(1, "/w/b")),
            ScanEvent::Artifact(artifact(0, 0, "/w/a/target", 500, old, GitState::Ignored)),
            ScanEvent::Artifact(artifact(
                1,
                0,
                "/w/a/node_modules",
                300,
                SystemTime::now(),
                GitState::Ignored,
            )),
            ScanEvent::Artifact(artifact(
                2,
                1,
                "/w/b/vendor",
                200,
                old,
                GitState::Tracked(4),
            )),
        ] {
            app.update(Msg::Scan(event));
        }
        app.ensure_rows();
        app
    }

    fn press(app: &mut App, code: KeyCode) -> Vec<Effect> {
        let effects = app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
        app.ensure_rows();
        effects
    }

    #[test]
    fn marking_a_project_marks_only_ready_artifacts() {
        let mut app = app();
        assert_eq!(app.rows.rows[0].label, "a");
        press(&mut app, KeyCode::Char(' '));
        assert!(app.marked.contains(&ArtifactId(0)));
        assert!(
            !app.marked.contains(&ArtifactId(1)),
            "recently used is held back"
        );
        // Marking again unmarks.
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Char(' '));
        assert!(app.marked.is_empty());
    }

    #[test]
    fn protected_artifacts_cannot_be_marked() {
        let mut app = app();
        press(&mut app, KeyCode::Char('E'));
        let vendor = app
            .rows
            .position(&RowKey::Artifact(ArtifactId(2)))
            .expect("vendor row");
        app.select(vendor);
        press(&mut app, KeyCode::Char(' '));
        assert!(app.marked.is_empty());
        assert!(app.toast.as_ref().unwrap().text.contains("tracked"));
    }

    #[test]
    fn held_artifacts_can_be_marked_individually() {
        let mut app = app();
        press(&mut app, KeyCode::Char('E'));
        let modules = app.rows.position(&RowKey::Artifact(ArtifactId(1))).unwrap();
        app.select(modules);
        press(&mut app, KeyCode::Char(' '));
        assert!(app.marked.contains(&ArtifactId(1)));
    }

    #[test]
    fn review_flow_emits_clean_and_records_history() {
        let mut app = app();
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.marked.len(), 1);
        press(&mut app, KeyCode::Char('d'));
        let Mode::Review(review) = &app.mode else {
            panic!("review expected");
        };
        assert_eq!(review.bytes, 500);
        let effects = press(&mut app, KeyCode::Enter);
        let Some(Effect::Clean {
            items,
            mode,
            allowed_roots,
        }) = effects.into_iter().next()
        else {
            panic!("clean effect expected");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(mode, DeleteMode::Permanent);
        assert_eq!(allowed_roots, vec![PathBuf::from("/w")]);

        app.update(Msg::Clean(CleanEvent::Started { id: ArtifactId(0) }));
        app.update(Msg::Clean(CleanEvent::Removed {
            id: ArtifactId(0),
            bytes: 500,
        }));
        let effects = app.update(Msg::CleanFinished(CleanReport {
            removed: 1,
            bytes: 500,
            ..CleanReport::default()
        }));
        assert!(matches!(&effects[0], Effect::RecordHistory(r) if r.len() == 1));
        assert!(app.marked.is_empty());
        app.ensure_rows();
        assert!(
            app.rows
                .position(&RowKey::Artifact(ArtifactId(0)))
                .is_none()
        );
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Browse));
    }

    #[test]
    fn nothing_marked_shows_a_hint() {
        let mut app = app();
        press(&mut app, KeyCode::Char('d'));
        assert!(matches!(app.mode, Mode::Browse));
        assert!(app.toast.is_some());
    }

    #[test]
    fn search_filters_and_escape_restores() {
        let mut app = app();
        press(&mut app, KeyCode::Char('/'));
        for c in "vendor".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.rows.totals.artifacts, 1);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.rows.totals.artifacts, 3);
    }

    #[test]
    fn selection_follows_its_row_across_rebuilds() {
        let mut app = app();
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.selected_row().unwrap().label, "b");
        // A size change that reorders rows keeps the cursor on "b".
        app.update(Msg::Scan(ScanEvent::Measured {
            id: ArtifactId(2),
            size: Size {
                reclaimable: 10_000,
                on_disk: 10_000,
                ..Size::default()
            },
        }));
        app.ensure_rows();
        assert_eq!(app.selected_row().unwrap().label, "b");
        assert_eq!(app.selected, 0);
    }
}
