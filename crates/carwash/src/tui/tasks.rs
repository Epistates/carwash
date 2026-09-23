//! The Tasks tab: pick projects, run their scripts, watch output in real terminals.

use super::app::{App, Effect, Level};
use super::keymap::Action;
use super::query::Fuzzy;
use super::theme::Theme;
use carwash_core::tasks::Task;
use carwash_core::{EcoId, ProjectId};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tui_term::widget::PseudoTerminal;

/// Lines of scrollback kept per job.
const SCROLLBACK: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Projects,
    Tasks,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Exited(i32),
    Failed(String),
}

pub struct Job {
    pub id: u64,
    pub label: String,
    pub task: Task,
    pub parser: vt100::Parser,
    pub status: JobStatus,
    started: Instant,
    pub elapsed: Option<Duration>,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Job")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl Job {
    pub fn duration(&self) -> Duration {
        self.elapsed.unwrap_or_else(|| self.started.elapsed())
    }
}

/// A task run request, resolved by the runtime (which discovers the task in each project).
#[derive(Debug, Clone)]
pub struct RunTarget {
    pub label: String,
    pub path: PathBuf,
    pub ecosystems: Vec<EcoId>,
}

#[derive(Debug)]
pub struct TasksState {
    pub focus: Pane,
    pub filter: String,
    /// Projects shown, in display order.
    pub projects: Vec<ProjectId>,
    built_for: Option<(u64, String)>,
    pub project_cursor: usize,
    project_key: Option<ProjectId>,
    pub marked: HashSet<ProjectId>,
    /// Discovered tasks per project path; `None` while loading.
    pub tasks: HashMap<PathBuf, Option<Vec<Task>>>,
    pub task_cursor: usize,
    pub jobs: Vec<Job>,
    pub active: usize,
    /// Terminal size given to jobs, from the output pane.
    pub pty_size: (u16, u16),
    fuzzy: Fuzzy,
}

impl Default for TasksState {
    fn default() -> Self {
        Self {
            focus: Pane::Projects,
            filter: String::new(),
            projects: Vec::new(),
            built_for: None,
            project_cursor: 0,
            project_key: None,
            marked: HashSet::new(),
            tasks: HashMap::new(),
            task_cursor: 0,
            jobs: Vec::new(),
            active: 0,
            pty_size: (24, 80),
            fuzzy: Fuzzy::default(),
        }
    }
}

impl TasksState {
    pub fn running(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.status == JobStatus::Running)
            .count()
    }

    pub fn job_mut(&mut self, id: u64) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn add_job(&mut self, id: u64, label: String, task: Task) {
        let (rows, cols) = self.pty_size;
        self.jobs.push(Job {
            id,
            label,
            task,
            parser: vt100::Parser::new(rows.max(2), cols.max(10), SCROLLBACK),
            status: JobStatus::Running,
            started: Instant::now(),
            elapsed: None,
        });
        self.active = self.jobs.len() - 1;
    }

    pub fn finish_job(&mut self, id: u64, result: Result<i32, String>) {
        if let Some(job) = self.job_mut(id) {
            job.elapsed = Some(job.started.elapsed());
            job.status = match result {
                Ok(code) => JobStatus::Exited(code),
                Err(error) => {
                    let message = format!("\r\n\x1b[31m{error}\x1b[0m\r\n");
                    job.parser.process(message.as_bytes());
                    JobStatus::Failed(error)
                }
            };
        }
    }

    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.project_cursor = 0;
        self.project_key = None;
    }
}

impl App {
    /// Rebuilds the project list when projects arrive or the filter changes.
    pub fn ensure_task_projects(&mut self) {
        let stamp = (self.store.revision, self.tasks.filter.clone());
        if self.tasks.built_for.as_ref() == Some(&stamp) {
            return;
        }
        let pattern = (!self.tasks.filter.trim().is_empty()).then(|| {
            Pattern::parse(
                &self.tasks.filter,
                CaseMatching::Smart,
                Normalization::Smart,
            )
        });
        let mut projects: Vec<(String, ProjectId)> = self
            .store
            .projects
            .iter()
            .map(|p| (self.store.relative(&p.path).into_owned(), p))
            .filter(|(rel, p)| {
                pattern.as_ref().is_none_or(|pattern| {
                    self.tasks
                        .fuzzy
                        .matches_text(pattern, &format!("{rel} {}", p.name))
                })
            })
            .map(|(rel, p)| (rel, p.id))
            .collect();
        projects.sort();
        self.tasks.projects = projects.into_iter().map(|(_, id)| id).collect();
        self.tasks.built_for = Some(stamp);
        if let Some(position) = self
            .tasks
            .project_key
            .and_then(|key| self.tasks.projects.iter().position(|&p| p == key))
        {
            self.tasks.project_cursor = position;
        }
        self.tasks.project_cursor = self
            .tasks
            .project_cursor
            .min(self.tasks.projects.len().saturating_sub(1));
    }

    fn current_project(&self) -> Option<ProjectId> {
        self.tasks.projects.get(self.tasks.project_cursor).copied()
    }

    /// Tasks of the current project; requests discovery when not yet known.
    fn current_tasks(&mut self) -> (Option<&[Task]>, Option<Effect>) {
        let Some(project) = self.current_project().and_then(|id| self.store.project(id)) else {
            return (None, None);
        };
        let (path, ecosystems) = (project.path.clone(), project.ecosystems.clone());
        match self.tasks.tasks.entry(path) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                (entry.into_mut().as_deref(), None)
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let effect = Effect::DiscoverTasks {
                    path: entry.key().clone(),
                    ecosystems,
                };
                entry.insert(None);
                (None, Some(effect))
            }
        }
    }

    /// Effects needed to show the current screen (task discovery for the selected project).
    pub fn tasks_effects(&mut self) -> Vec<Effect> {
        self.ensure_task_projects();
        self.current_tasks().1.into_iter().collect()
    }

    fn target(&self, id: ProjectId) -> Option<RunTarget> {
        let project = self.store.project(id)?;
        let label = if project.path == self.store.root {
            project.name.clone()
        } else {
            self.store.relative(&project.path).into_owned()
        };
        Some(RunTarget {
            label,
            path: project.path.clone(),
            ecosystems: project.ecosystems.clone(),
        })
    }

    pub fn on_tasks_action(&mut self, action: Action) -> Vec<Effect> {
        self.ensure_task_projects();
        let tasks_len = self.current_tasks().0.map_or(0, <[Task]>::len);
        let page = self.page.max(1);
        let state = &mut self.tasks;
        let mut move_cursor = |delta: isize, to: Option<usize>| match state.focus {
            Pane::Projects => {
                let last = state.projects.len().saturating_sub(1);
                state.project_cursor = to
                    .unwrap_or_else(|| state.project_cursor.saturating_add_signed(delta))
                    .min(last);
                state.project_key = state.projects.get(state.project_cursor).copied();
                state.task_cursor = 0;
            }
            Pane::Tasks => {
                let last = tasks_len.saturating_sub(1);
                state.task_cursor = to
                    .unwrap_or_else(|| state.task_cursor.saturating_add_signed(delta))
                    .min(last);
            }
            Pane::Output => {
                if let Some(job) = state.jobs.get_mut(state.active) {
                    let screen = job.parser.screen_mut();
                    let current = screen.scrollback();
                    let target = match to {
                        Some(0) => usize::MAX,
                        Some(_) => 0,
                        None => current.saturating_add_signed(-delta),
                    };
                    screen.set_scrollback(target);
                }
            }
        };
        match action {
            Action::Up => move_cursor(-1, None),
            Action::Down => move_cursor(1, None),
            Action::PageUp => move_cursor(-(page as isize), None),
            Action::PageDown => move_cursor(page as isize, None),
            Action::Top => move_cursor(0, Some(0)),
            Action::Bottom => move_cursor(0, Some(usize::MAX)),
            Action::NextPane => {
                self.tasks.focus = match self.tasks.focus {
                    Pane::Projects => Pane::Tasks,
                    Pane::Tasks => Pane::Output,
                    Pane::Output => Pane::Projects,
                };
            }
            Action::PreviousPane => {
                self.tasks.focus = match self.tasks.focus {
                    Pane::Projects => Pane::Output,
                    Pane::Tasks => Pane::Projects,
                    Pane::Output => Pane::Tasks,
                };
            }
            Action::Mark => {
                if let Some(id) = self.current_project() {
                    if !self.tasks.marked.remove(&id) {
                        self.tasks.marked.insert(id);
                    }
                    if self.tasks.focus == Pane::Projects {
                        return self.on_tasks_action(Action::Down);
                    }
                }
            }
            Action::Unmark => self.tasks.marked.clear(),
            Action::Run => {
                if self.tasks.focus == Pane::Projects {
                    self.tasks.focus = Pane::Tasks;
                    return self.tasks_effects();
                }
                return self.run_selected_task();
            }
            Action::KillJob => {
                if let Some(job) = self.tasks.jobs.get(self.tasks.active)
                    && job.status == JobStatus::Running
                {
                    return vec![Effect::KillJob(job.id)];
                }
            }
            Action::NextJob => {
                if !self.tasks.jobs.is_empty() {
                    self.tasks.active = (self.tasks.active + 1) % self.tasks.jobs.len();
                }
            }
            Action::PreviousJob => {
                if !self.tasks.jobs.is_empty() {
                    self.tasks.active = self
                        .tasks
                        .active
                        .checked_sub(1)
                        .unwrap_or(self.tasks.jobs.len() - 1);
                }
            }
            Action::ClearJobs => {
                self.tasks.jobs.retain(|j| j.status == JobStatus::Running);
                self.tasks.active = self
                    .tasks
                    .active
                    .min(self.tasks.jobs.len().saturating_sub(1));
            }
            Action::Open => {
                if let Some(path) = self
                    .current_project()
                    .and_then(|id| self.store.project(id))
                    .map(|p| p.path.clone())
                {
                    return vec![Effect::Reveal(path)];
                }
            }
            _ => {}
        }
        self.tasks_effects()
    }

    fn run_selected_task(&mut self) -> Vec<Effect> {
        let cursor = self.tasks.task_cursor;
        let (tasks, effect) = self.current_tasks();
        let Some(task) = tasks.and_then(|t| t.get(cursor)).cloned() else {
            return effect.into_iter().collect();
        };
        let mut ids: Vec<ProjectId> = if self.tasks.marked.is_empty() {
            self.current_project().into_iter().collect()
        } else {
            self.tasks.marked.iter().copied().collect()
        };
        ids.sort_by_key(|id| self.store.project(*id).map(|p| p.path.clone()));
        let targets: Vec<RunTarget> = ids.into_iter().filter_map(|id| self.target(id)).collect();
        if targets.len() > 1 {
            self.toast(
                format!(
                    "Running `{}` in {} marked projects",
                    task.name,
                    targets.len()
                ),
                Level::Info,
            );
        }
        self.tasks.focus = Pane::Output;
        vec![Effect::RunTask {
            name: task.name,
            targets,
            size: self.tasks.pty_size,
        }]
    }
}

fn pane_block<'a>(theme: &Theme, title: String, focused: bool) -> Block<'a> {
    let color = if focused { theme.accent } else { theme.border };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.fg(color))
        .title(Span::styled(
            format!(" {title} "),
            if focused {
                theme.bold(theme.accent)
            } else {
                theme.muted()
            },
        ))
}

fn spinner(app: &App) -> &'static str {
    app.glyphs.spinner[app.spinner % app.glyphs.spinner.len()]
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    app.ensure_task_projects();
    let t = app.theme;
    let [left, output] =
        Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)]).areas(area);
    let [projects_area, tasks_area] =
        Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(left);

    // Projects.
    let marked = &app.tasks.marked;
    let items: Vec<ListItem> = app
        .tasks
        .projects
        .iter()
        .filter_map(|&id| app.store.project(id))
        .map(|project| {
            let mark = if marked.contains(&project.id) {
                Span::styled(format!("{} ", app.glyphs.marked), t.bold(t.accent2))
            } else {
                Span::styled(format!("{} ", app.glyphs.unmarked), t.muted())
            };
            let mut spans = vec![mark];
            for (i, &eco) in project.ecosystems.iter().take(2).enumerate() {
                let ecosystem = app.registry.ecosystem(eco);
                if i > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.push(Span::styled(
                    ecosystem.badge.clone(),
                    t.fg(t.ecosystem(&ecosystem.key)),
                ));
            }
            spans.push(Span::raw("  "));
            let label = if project.path == app.store.root {
                project.name.clone()
            } else {
                app.store.relative(&project.path).into_owned()
            };
            spans.push(Span::styled(label, t.text()));
            ListItem::new(Line::from(spans))
        })
        .collect();
    let mut title = format!("Projects {}", app.tasks.projects.len());
    if !app.tasks.marked.is_empty() {
        title.push_str(&format!(" · {} marked", app.tasks.marked.len()));
    }
    if !app.tasks.filter.is_empty() {
        title.push_str(&format!(" · “{}”", app.tasks.filter));
    }
    let mut state = ListState::default().with_selected(Some(app.tasks.project_cursor));
    frame.render_stateful_widget(
        List::new(items)
            .block(pane_block(
                &t,
                title,
                app.tasks.focus == super::tasks::Pane::Projects,
            ))
            .highlight_style(t.selected_row()),
        projects_area,
        &mut state,
    );

    // Tasks of the current project.
    let focus_tasks = app.tasks.focus == Pane::Tasks;
    let current = app
        .tasks
        .projects
        .get(app.tasks.project_cursor)
        .and_then(|&id| app.store.project(id))
        .map(|p| p.path.clone());
    let tasks = current
        .as_ref()
        .and_then(|path| app.tasks.tasks.get(path))
        .cloned()
        .flatten();
    match tasks {
        Some(tasks) if !tasks.is_empty() => {
            let width = tasks
                .iter()
                .map(|t| t.name.len())
                .max()
                .unwrap_or(0)
                .min(24);
            let items: Vec<ListItem> = tasks
                .iter()
                .map(|task| {
                    let detail = task
                        .description
                        .clone()
                        .filter(|d| *d != task.command_line())
                        .unwrap_or_else(|| task.command_line());
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{:<width$}  ", task.name), t.bold(t.accent)),
                        Span::styled(detail, t.subtle()),
                    ]))
                })
                .collect();
            let mut state =
                ListState::default().with_selected(focus_tasks.then_some(app.tasks.task_cursor));
            frame.render_stateful_widget(
                List::new(items)
                    .block(pane_block(
                        &t,
                        format!("Tasks {}", tasks.len()),
                        focus_tasks,
                    ))
                    .highlight_style(t.selected_row()),
                tasks_area,
                &mut state,
            );
        }
        Some(_) => frame.render_widget(
            Paragraph::new(Span::styled("No tasks found for this project.", t.subtle()))
                .block(pane_block(&t, "Tasks".into(), focus_tasks)),
            tasks_area,
        ),
        None => frame.render_widget(
            Paragraph::new(Span::styled(
                if current.is_some() {
                    "Looking for tasks…"
                } else {
                    "Select a project"
                },
                t.subtle(),
            ))
            .block(pane_block(&t, "Tasks".into(), focus_tasks)),
            tasks_area,
        ),
    }

    render_output(frame, app, output);
}

fn render_output(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme;
    let focused = app.tasks.focus == Pane::Output;
    let running = app.tasks.running();
    let title = if app.tasks.jobs.is_empty() {
        "Output".to_string()
    } else {
        format!("Output · {} jobs, {running} running", app.tasks.jobs.len())
    };
    let block = pane_block(&t, title, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.tasks.jobs.is_empty() {
        let hint = vec![
            Line::from(Span::styled(
                "Pick a project, then a task, and press Enter.",
                t.subtle(),
            )),
            Line::from(Span::styled(
                "Mark several projects with Space to run a task in all of them.",
                t.muted(),
            )),
        ];
        frame.render_widget(Paragraph::new(hint).wrap(Wrap { trim: false }), inner);
        return;
    }
    let [strip, info, terminal] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(inner);

    // Job strip: one chip per job, the active one highlighted.
    let mut chips: Vec<Span> = Vec::new();
    for (i, job) in app.tasks.jobs.iter().enumerate() {
        let (glyph, color) = match job.status {
            JobStatus::Running => (spinner(app), t.accent),
            JobStatus::Exited(0) => ("✓", t.success),
            JobStatus::Exited(_) | JobStatus::Failed(_) => ("✗", t.error),
        };
        let mut style = t.fg(color);
        if i == app.tasks.active {
            style = style.add_modifier(Modifier::REVERSED | Modifier::BOLD);
        }
        chips.push(Span::styled(
            format!(" {glyph} {}·{} ", job.label, job.task.name),
            style,
        ));
        chips.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(chips)), strip);

    let (rows, cols) = (terminal.height.max(2), terminal.width.max(10));
    app.tasks.pty_size = (rows, cols);
    let active = app.tasks.active.min(app.tasks.jobs.len() - 1);
    let job = &mut app.tasks.jobs[active];
    if job.parser.screen().size() != (rows, cols) {
        job.parser.screen_mut().set_size(rows, cols);
    }
    let status = match &job.status {
        JobStatus::Running => Span::styled("running", t.fg(t.accent)),
        JobStatus::Exited(0) => Span::styled("succeeded", t.fg(t.success)),
        JobStatus::Exited(code) => Span::styled(format!("exit {code}"), t.bold(t.error)),
        JobStatus::Failed(error) => Span::styled(error.clone(), t.bold(t.error)),
    };
    let scrolled = job.parser.screen().scrollback();
    let mut line = vec![
        Span::styled(format!("$ {}", job.task.command_line()), t.subtle()),
        Span::styled("  ", t.muted()),
        status,
        Span::styled(format!("  {:.1}s", job.duration().as_secs_f64()), t.muted()),
    ];
    if scrolled > 0 {
        line.push(Span::styled(format!("  ↑{scrolled}"), t.fg(t.warning)));
    }
    frame.render_widget(Paragraph::new(Line::from(line)), info);
    let mut cursor = tui_term::widget::Cursor::default();
    if job.status != JobStatus::Running || job.parser.screen().hide_cursor() {
        cursor.hide();
    }
    frame.render_widget(
        PseudoTerminal::new(job.parser.screen()).cursor(cursor),
        terminal,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::Msg;
    use crate::tui::app::tests::app;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn press(app: &mut App, code: KeyCode) -> Vec<Effect> {
        app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }

    fn task(name: &str, cwd: &str) -> Task {
        Task {
            name: name.into(),
            source: "justfile".into(),
            program: "just".into(),
            args: vec![name.into()],
            description: None,
            cwd: PathBuf::from(cwd),
            standard: false,
        }
    }

    #[test]
    fn opening_the_tab_discovers_tasks_and_enter_runs_them() {
        let mut app = app();
        let effects = press(&mut app, KeyCode::Char('2'));
        let Some(Effect::DiscoverTasks { path, .. }) = effects.first() else {
            panic!("discovery expected, got {effects:?}");
        };
        assert_eq!(path, &PathBuf::from("/w/a"));
        app.update(Msg::TasksDiscovered(
            path.clone(),
            vec![task("lint", "/w/a"), task("test", "/w/a")],
        ));

        press(&mut app, KeyCode::Enter); // focus the task list
        press(&mut app, KeyCode::Char('j'));
        let effects = press(&mut app, KeyCode::Enter);
        let Some(Effect::RunTask { name, targets, .. }) = effects.first() else {
            panic!("run expected, got {effects:?}");
        };
        assert_eq!(name, "test");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].path, PathBuf::from("/w/a"));
        assert_eq!(app.tasks.focus, Pane::Output);
    }

    #[test]
    fn marked_projects_all_receive_the_task() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        app.update(Msg::TasksDiscovered(
            PathBuf::from("/w/a"),
            vec![task("test", "/w/a")],
        ));
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.tasks.marked.len(), 2);
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Tab);
        let effects = press(&mut app, KeyCode::Enter);
        let Some(Effect::RunTask { targets, .. }) = effects.first() else {
            panic!("run expected, got {effects:?}");
        };
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn job_lifecycle_updates_the_screen_and_status() {
        let mut app = app();
        app.update(Msg::JobStarted {
            id: 7,
            label: "a".into(),
            task: task("test", "/w/a"),
        });
        assert_eq!(app.tasks.running(), 1);
        assert!(app.animating());
        app.update(Msg::JobOutput(
            7,
            b"hello \x1b[32mworld\x1b[0m\r\n".to_vec(),
        ));
        assert!(
            app.tasks.jobs[0]
                .parser
                .screen()
                .contents()
                .contains("hello world")
        );
        app.update(Msg::JobExited(7, Ok(1)));
        assert_eq!(app.tasks.jobs[0].status, JobStatus::Exited(1));
        assert!(app.toast.as_ref().unwrap().text.contains("failed"));

        app.update(Msg::JobStarted {
            id: 8,
            label: "b".into(),
            task: task("x", "/w/b"),
        });
        app.update(Msg::JobExited(8, Err("`just` not found".into())));
        assert!(
            app.tasks.jobs[1]
                .parser
                .screen()
                .contents()
                .contains("not found")
        );
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('c'));
        assert!(app.tasks.jobs.is_empty(), "finished jobs cleared");
    }

    #[test]
    fn quitting_with_running_jobs_needs_confirmation() {
        let mut app = app();
        app.update(Msg::JobStarted {
            id: 1,
            label: "a".into(),
            task: task("dev", "/w/a"),
        });
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.quit);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
    }
}
