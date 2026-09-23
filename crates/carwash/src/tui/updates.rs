//! The Updates tab: outdated and vulnerable dependencies per project, and applying updates.

use super::app::{App, Effect, Level};
use super::keymap::Action;
use super::query::Fuzzy;
use super::widgets::{pane, spinner};
use carwash_core::deps::{Bump, CheckProgress, Dependency, ProjectDeps, ProjectSpec, Source};
use carwash_core::{Project, ProjectId};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Projects,
    Dependencies,
}

#[derive(Debug)]
pub struct UpdatesState {
    pub focus: Pane,
    pub filter: String,
    pub projects: Vec<ProjectId>,
    built_for: Option<(u64, String)>,
    pub project_cursor: usize,
    project_key: Option<ProjectId>,
    pub marked_projects: HashSet<ProjectId>,
    pub results: HashMap<PathBuf, ProjectDeps>,
    pub checking: HashSet<PathBuf>,
    pub updating: HashSet<PathBuf>,
    pub dep_cursor: usize,
    /// Marked dependencies as (project path, package name).
    pub marked: HashSet<(PathBuf, String)>,
    upgrade_armed: bool,
    pub progress: Option<Arc<CheckProgress>>,
    fuzzy: Fuzzy,
}

impl Default for UpdatesState {
    fn default() -> Self {
        Self {
            focus: Pane::Projects,
            filter: String::new(),
            projects: Vec::new(),
            built_for: None,
            project_cursor: 0,
            project_key: None,
            marked_projects: HashSet::new(),
            results: HashMap::new(),
            checking: HashSet::new(),
            updating: HashSet::new(),
            dep_cursor: 0,
            marked: HashSet::new(),
            upgrade_armed: false,
            progress: None,
            fuzzy: Fuzzy::default(),
        }
    }
}

impl UpdatesState {
    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.project_cursor = 0;
        self.project_key = None;
    }

    pub fn finish_check(&mut self, results: Vec<ProjectDeps>) {
        for result in results {
            self.checking.remove(&result.path);
            self.results.insert(result.path.clone(), result);
        }
        if self.checking.is_empty() {
            self.progress = None;
        }
    }

    /// (outdated, vulnerable) across every checked project.
    pub fn totals(&self) -> (usize, usize) {
        self.results
            .values()
            .flat_map(|r| &r.dependencies)
            .fold((0, 0), |(o, v), d| {
                (
                    o + usize::from(d.is_outdated()),
                    v + usize::from(!d.vulnerabilities.is_empty()),
                )
            })
    }
}

/// Dependencies in display order: vulnerable, then by bump size, then name.
pub fn sorted(result: &ProjectDeps) -> Vec<&Dependency> {
    let mut deps: Vec<&Dependency> = result.dependencies.iter().collect();
    deps.sort_by(|a, b| {
        a.vulnerabilities
            .is_empty()
            .cmp(&b.vulnerabilities.is_empty())
            .then(b.bump.cmp(&a.bump))
            .then(a.declared.name.cmp(&b.declared.name))
    });
    deps
}

fn checkable(app: &App, project: &Project) -> bool {
    project
        .ecosystems
        .iter()
        .any(|&e| Source::for_ecosystem(&app.registry.ecosystem(e).key).is_some())
}

impl App {
    pub fn ensure_update_projects(&mut self) {
        let stamp = (self.store.revision, self.updates.filter.clone());
        if self.updates.built_for.as_ref() == Some(&stamp) {
            return;
        }
        let pattern = (!self.updates.filter.trim().is_empty()).then(|| {
            Pattern::parse(
                &self.updates.filter,
                CaseMatching::Smart,
                Normalization::Smart,
            )
        });
        let mut projects: Vec<(String, ProjectId)> = Vec::new();
        for project in &self.store.projects {
            if !checkable(self, project) {
                continue;
            }
            let rel = self.store.relative(&project.path).into_owned();
            let keep = pattern.as_ref().is_none_or(|pattern| {
                self.updates
                    .fuzzy
                    .matches_text(pattern, &format!("{rel} {}", project.name))
            });
            if keep {
                projects.push((rel, project.id));
            }
        }
        projects.sort();
        self.updates.projects = projects.into_iter().map(|(_, id)| id).collect();
        self.updates.built_for = Some(stamp);
        if let Some(position) = self
            .updates
            .project_key
            .and_then(|key| self.updates.projects.iter().position(|&p| p == key))
        {
            self.updates.project_cursor = position;
        }
        self.updates.project_cursor = self
            .updates
            .project_cursor
            .min(self.updates.projects.len().saturating_sub(1));
    }

    fn update_project(&self) -> Option<&Project> {
        self.updates
            .projects
            .get(self.updates.project_cursor)
            .and_then(|&id| self.store.project(id))
    }

    fn spec(&self, id: ProjectId) -> Option<ProjectSpec> {
        self.store.project(id).map(|p| ProjectSpec {
            path: p.path.clone(),
            ecosystems: p.ecosystems.clone(),
        })
    }

    /// Checks `ids`, skipping projects already being checked.
    fn check(&mut self, ids: Vec<ProjectId>, refresh: bool) -> Vec<Effect> {
        let specs: Vec<ProjectSpec> = ids
            .into_iter()
            .filter_map(|id| self.spec(id))
            .filter(|s| !self.updates.checking.contains(&s.path))
            .collect();
        if specs.is_empty() {
            return Vec::new();
        }
        self.updates
            .checking
            .extend(specs.iter().map(|s| s.path.clone()));
        vec![Effect::CheckDeps { specs, refresh }]
    }

    /// Re-checks the project at `path` (after an update job finished).
    pub fn recheck(&mut self, path: &PathBuf) -> Vec<Effect> {
        self.updates.updating.remove(path);
        match self
            .store
            .projects
            .iter()
            .find(|p| &p.path == path)
            .map(|p| p.id)
        {
            Some(id) => self.check(vec![id], false),
            None => Vec::new(),
        }
    }

    /// Dependencies of the current project, in display order.
    fn current_deps(&self) -> Vec<Dependency> {
        self.update_project()
            .and_then(|p| self.updates.results.get(&p.path))
            .map(|r| sorted(r).into_iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Opening the tab checks the selected project once.
    pub fn updates_effects(&mut self) -> Vec<Effect> {
        self.ensure_update_projects();
        match self.update_project() {
            Some(project)
                if !self.updates.results.contains_key(&project.path)
                    && !self.updates.checking.contains(&project.path) =>
            {
                let id = project.id;
                self.check(vec![id], false)
            }
            _ => Vec::new(),
        }
    }

    pub fn on_updates_action(&mut self, action: Action) -> Vec<Effect> {
        self.ensure_update_projects();
        if !matches!(action, Action::Upgrade) {
            self.updates.upgrade_armed = false;
        }
        let deps = self.current_deps();
        let page = self.page.max(1) as isize;
        let focus = self.updates.focus;
        let mut move_to = |delta: isize, absolute: Option<usize>| {
            let state = &mut self.updates;
            match focus {
                Pane::Projects => {
                    let last = state.projects.len().saturating_sub(1);
                    state.project_cursor = absolute
                        .unwrap_or_else(|| state.project_cursor.saturating_add_signed(delta))
                        .min(last);
                    state.project_key = state.projects.get(state.project_cursor).copied();
                    state.dep_cursor = 0;
                }
                Pane::Dependencies => {
                    let last = deps.len().saturating_sub(1);
                    state.dep_cursor = absolute
                        .unwrap_or_else(|| state.dep_cursor.saturating_add_signed(delta))
                        .min(last);
                }
            }
        };
        match action {
            Action::Up => move_to(-1, None),
            Action::Down => move_to(1, None),
            Action::PageUp => move_to(-page, None),
            Action::PageDown => move_to(page, None),
            Action::Top => move_to(0, Some(0)),
            Action::Bottom => move_to(0, Some(usize::MAX)),
            Action::NextPane | Action::PreviousPane => {
                self.updates.focus = match self.updates.focus {
                    Pane::Projects => Pane::Dependencies,
                    Pane::Dependencies => Pane::Projects,
                };
            }
            Action::Mark => match self.updates.focus {
                Pane::Projects => {
                    if let Some(id) = self.update_project().map(|p| p.id) {
                        if !self.updates.marked_projects.remove(&id) {
                            self.updates.marked_projects.insert(id);
                        }
                        return self.on_updates_action(Action::Down);
                    }
                }
                Pane::Dependencies => {
                    if let (Some(project), Some(dep)) =
                        (self.update_project(), deps.get(self.updates.dep_cursor))
                    {
                        let key = (project.path.clone(), dep.declared.name.clone());
                        if !dep.is_outdated() {
                            self.toast(format!("{} is up to date", dep.declared.name), Level::Info);
                        } else if !self.updates.marked.remove(&key) {
                            self.updates.marked.insert(key);
                        }
                        return self.on_updates_action(Action::Down);
                    }
                }
            },
            Action::MarkAll => match self.updates.focus {
                Pane::Projects => {
                    self.updates
                        .marked_projects
                        .extend(self.updates.projects.iter().copied());
                }
                Pane::Dependencies => {
                    if let Some(path) = self.update_project().map(|p| p.path.clone()) {
                        for dep in deps.iter().filter(|d| d.is_outdated()) {
                            self.updates
                                .marked
                                .insert((path.clone(), dep.declared.name.clone()));
                        }
                    }
                }
            },
            Action::Unmark => {
                self.updates.marked.clear();
                self.updates.marked_projects.clear();
            }
            Action::Check | Action::Run if self.updates.focus == Pane::Projects => {
                let ids: Vec<ProjectId> = if self.updates.marked_projects.is_empty() {
                    self.update_project().map(|p| p.id).into_iter().collect()
                } else {
                    self.updates.marked_projects.iter().copied().collect()
                };
                return self.check(ids, true);
            }
            Action::Run => self.updates.focus = Pane::Projects,
            Action::Check => {
                let ids = self.update_project().map(|p| p.id).into_iter().collect();
                return self.check(ids, true);
            }
            Action::CheckAll => {
                let ids = self.updates.projects.clone();
                let n = ids.len();
                self.toast(format!("Checking {n} projects"), Level::Info);
                return self.check(ids, false);
            }
            Action::Update => return self.apply_updates(&deps, false),
            Action::Upgrade => {
                if !self.updates.upgrade_armed {
                    self.updates.upgrade_armed = true;
                    let n = self.selected_updates(&deps).len();
                    self.toast(
                        format!("Press U again to raise {n} requirements to the latest versions (edits manifests)"),
                        Level::Warn,
                    );
                    return Vec::new();
                }
                self.updates.upgrade_armed = false;
                return self.apply_updates(&deps, true);
            }
            Action::Open => {
                if let Some(path) = self.update_project().map(|p| p.path.clone()) {
                    return vec![Effect::Reveal(path)];
                }
            }
            _ => {}
        }
        self.updates_effects()
    }

    /// Marked outdated deps of the current project, or the one under the cursor.
    fn selected_updates(&self, deps: &[Dependency]) -> Vec<Dependency> {
        let Some(project) = self.update_project() else {
            return Vec::new();
        };
        let marked: Vec<Dependency> = deps
            .iter()
            .filter(|d| {
                d.is_outdated()
                    && self
                        .updates
                        .marked
                        .contains(&(project.path.clone(), d.declared.name.clone()))
            })
            .cloned()
            .collect();
        if !marked.is_empty() {
            return marked;
        }
        deps.get(self.updates.dep_cursor)
            .filter(|d| d.is_outdated() && self.updates.focus == Pane::Dependencies)
            .cloned()
            .into_iter()
            .collect()
    }

    fn apply_updates(&mut self, deps: &[Dependency], latest: bool) -> Vec<Effect> {
        let selected = self.selected_updates(deps);
        let Some(project) = self.update_project() else {
            return Vec::new();
        };
        if selected.is_empty() {
            self.toast(
                "Mark outdated dependencies with Space (a marks all), or select one",
                Level::Warn,
            );
            return Vec::new();
        }
        let (path, label) = (
            project.path.clone(),
            if project.path == self.store.root {
                project.name.clone()
            } else {
                self.store.relative(&project.path).into_owned()
            },
        );
        for dep in &selected {
            self.updates
                .marked
                .remove(&(path.clone(), dep.declared.name.clone()));
        }
        self.updates.updating.insert(path.clone());
        self.toast(
            format!(
                "{} {} in {label}: output in the Tasks tab (2)",
                if latest { "Upgrading" } else { "Updating" },
                crate::commands::count(selected.len() as u64)
            ),
            Level::Info,
        );
        vec![Effect::ApplyUpdates {
            dir: path,
            label,
            deps: selected,
            latest,
            size: self.tasks.pty_size,
        }]
    }
}

/// A version for display: build metadata (`+spec-1.1.0`) does not affect precedence and
/// only crowds the column.
fn version(v: Option<&str>) -> String {
    match v {
        Some(v) => v.split('+').next().unwrap_or(v).to_owned(),
        None => "-".to_owned(),
    }
}

fn bump_span(app: &App, bump: Bump) -> Span<'static> {
    let t = &app.theme;
    match bump {
        Bump::Major => Span::styled("major", t.bold(t.error)),
        Bump::Minor => Span::styled("minor", t.fg(t.warning)),
        Bump::Patch => Span::styled("patch", t.fg(t.success)),
        Bump::None => Span::raw(""),
    }
}

fn project_status(app: &App, project: &Project) -> Span<'static> {
    let t = &app.theme;
    if app.updates.checking.contains(&project.path) || app.updates.updating.contains(&project.path)
    {
        return Span::styled(format!("{:>4}", spinner(app)), t.fg(t.accent));
    }
    let Some(result) = app.updates.results.get(&project.path) else {
        return Span::styled("   ·", t.muted());
    };
    let vulnerable = result
        .dependencies
        .iter()
        .filter(|d| !d.vulnerabilities.is_empty())
        .count();
    let outdated: Vec<&Dependency> = result
        .dependencies
        .iter()
        .filter(|d| d.is_outdated())
        .collect();
    let worst = outdated.iter().map(|d| d.bump).max().unwrap_or_default();
    if vulnerable > 0 {
        Span::styled(format!("{:>3}!", vulnerable), t.bold(t.error))
    } else if !outdated.is_empty() {
        let color = match worst {
            Bump::Major => t.error,
            Bump::Minor => t.warning,
            _ => t.success,
        };
        Span::styled(format!("{:>3}↑", outdated.len()), t.fg(color))
    } else if !result.errors.is_empty() {
        Span::styled("   ?", t.fg(t.warning))
    } else {
        Span::styled("   ✓", t.fg(t.success))
    }
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    app.ensure_update_projects();
    let t = app.theme;
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(34), Constraint::Percentage(66)]).areas(area);

    let items: Vec<ListItem> = app
        .updates
        .projects
        .iter()
        .filter_map(|&id| app.store.project(id))
        .map(|project| {
            let mark = if app.updates.marked_projects.contains(&project.id) {
                Span::styled(format!("{} ", app.glyphs.marked), t.bold(t.accent2))
            } else {
                Span::styled(format!("{} ", app.glyphs.unmarked), t.muted())
            };
            let label = if project.path == app.store.root {
                project.name.clone()
            } else {
                app.store.relative(&project.path).into_owned()
            };
            ListItem::new(Line::from(vec![
                mark,
                project_status(app, project),
                Span::raw("  "),
                Span::styled(label, t.text()),
            ]))
        })
        .collect();
    let mut title = format!("Projects {}", app.updates.projects.len());
    if let Some(progress) = &app.updates.progress {
        title.push_str(&format!(
            " · checking {}/{}",
            progress.done.load(Ordering::Relaxed),
            progress.total.load(Ordering::Relaxed)
        ));
    }
    if !app.updates.filter.is_empty() {
        title.push_str(&format!(" · “{}”", app.updates.filter));
    }
    let mut state = ListState::default().with_selected(Some(app.updates.project_cursor));
    frame.render_stateful_widget(
        List::new(items)
            .block(pane(&t, title, app.updates.focus == Pane::Projects))
            .highlight_style(t.selected_row()),
        left,
        &mut state,
    );

    let focused = app.updates.focus == Pane::Dependencies;
    let Some(project) = app.update_project() else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "No Rust, JavaScript, Python or Go projects here.",
                t.subtle(),
            ))
            .block(pane(&t, "Dependencies".into(), focused)),
            right,
        );
        return;
    };
    let path = project.path.clone();
    let Some(result) = app.updates.results.get(&path) else {
        let message = if app.updates.checking.contains(&path) {
            format!("{} Checking registries…", spinner(app))
        } else {
            "Press c to check this project, C to check every project listed.".into()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(message, t.subtle())).block(pane(
                &t,
                "Dependencies".into(),
                focused,
            )),
            right,
        );
        return;
    };
    let deps = sorted(result);
    let outdated = deps.iter().filter(|d| d.is_outdated()).count();
    let title = format!("Dependencies {} · {outdated} outdated", deps.len());
    let block = pane(&t, title, focused);
    let inner = block.inner(right);
    frame.render_widget(block, right);
    let [table_area, notes] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(if result.errors.is_empty() { 0 } else { 2 }),
    ])
    .areas(inner);

    let rows: Vec<Row> = deps
        .iter()
        .map(|dep| {
            let marked = app
                .updates
                .marked
                .contains(&(path.clone(), dep.declared.name.clone()));
            let mark = if marked {
                Span::styled(app.glyphs.marked, t.bold(t.accent2))
            } else if dep.is_outdated() {
                Span::styled(app.glyphs.unmarked, t.muted())
            } else {
                Span::raw(" ")
            };
            let dim = !dep.is_outdated() && dep.vulnerabilities.is_empty();
            let name_style = if dim { t.muted() } else { t.text() };
            let latest_style = match dep.bump {
                Bump::Major => t.bold(t.error),
                Bump::Minor => t.fg(t.warning),
                Bump::Patch => t.fg(t.success),
                Bump::None => t.muted(),
            };
            let advisories = if dep.vulnerabilities.is_empty() {
                Span::styled(dep.error.clone().unwrap_or_default(), t.muted())
            } else {
                Span::styled(dep.vulnerabilities.join(" "), t.bold(t.error))
            };
            Row::new(vec![
                Cell::from(mark),
                Cell::from(Span::styled(dep.declared.name.clone(), name_style)),
                Cell::from(
                    Line::from(Span::styled(
                        version(dep.declared.current.as_deref()),
                        t.subtle(),
                    ))
                    .alignment(Alignment::Right),
                ),
                Cell::from(
                    Line::from(Span::styled(version(dep.wanted.as_deref()), t.text()))
                        .alignment(Alignment::Right),
                ),
                Cell::from(
                    Line::from(Span::styled(version(dep.latest.as_deref()), latest_style))
                        .alignment(Alignment::Right),
                ),
                Cell::from(bump_span(app, dep.bump)),
                Cell::from(Span::styled(dep.declared.kind.label(), t.muted())),
                Cell::from(advisories),
            ])
        })
        .collect();
    let header = Row::new(
        [
            "",
            "PACKAGE",
            "CURRENT",
            "WANTED",
            "LATEST",
            "BUMP",
            "KIND",
            "ADVISORIES",
        ]
        .into_iter()
        .map(|h| Cell::from(Span::styled(h, t.bold(t.muted)))),
    );
    let widths = [
        Constraint::Length(1),
        Constraint::Fill(2),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(5),
        Constraint::Length(8),
        Constraint::Fill(1),
    ];
    let mut state = TableState::default().with_selected(focused.then_some(app.updates.dep_cursor));
    frame.render_stateful_widget(
        Table::new(rows, widths)
            .header(header)
            .column_spacing(1)
            .row_highlight_style(t.selected_row()),
        table_area,
        &mut state,
    );
    if !result.errors.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                result.errors.join("; "),
                Style::new().fg(t.warning),
            ))
            .wrap(Wrap { trim: true }),
            notes,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::Msg;
    use crate::tui::app::tests::app;
    use carwash_core::deps::{Declared, DepKind};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn press(app: &mut App, code: KeyCode) -> Vec<Effect> {
        app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }

    fn dep(name: &str, current: &str, latest: &str, bump: Bump, vulns: &[&str]) -> Dependency {
        Dependency {
            declared: Declared {
                name: name.into(),
                requirement: "1".into(),
                current: Some(current.into()),
                kind: DepKind::Normal,
                source: Source::Crates,
                manifest: PathBuf::from("/w/a/Cargo.toml"),
            },
            wanted: Some(latest.into()),
            latest: Some(latest.into()),
            bump,
            wanted_bump: bump,
            vulnerabilities: vulns.iter().map(|s| (*s).to_owned()).collect(),
            error: None,
        }
    }

    fn checked(app: &mut App) {
        app.update(Msg::DepsChecked(vec![ProjectDeps {
            path: PathBuf::from("/w/a"),
            dependencies: vec![
                dep("current", "1.0.0", "1.0.0", Bump::None, &[]),
                dep("minor", "1.0.0", "1.2.0", Bump::Minor, &[]),
                dep("vulnerable", "1.0.0", "1.0.1", Bump::Patch, &["RUSTSEC-1"]),
                dep("major", "1.0.0", "2.0.0", Bump::Major, &[]),
            ],
            errors: Vec::new(),
        }]));
    }

    #[test]
    fn opening_checks_the_selected_project_once() {
        let mut app = app();
        // The fixture's projects have ecosystem 0 (rust), which carwash can check.
        let effects = press(&mut app, KeyCode::Char('3'));
        assert!(
            matches!(effects.first(), Some(Effect::CheckDeps { specs, .. }) if specs.len() == 1)
        );
        assert!(app.animating(), "spinner while checking");
        let again = press(&mut app, KeyCode::Char('3'));
        assert!(again.is_empty(), "already checking");
    }

    #[test]
    fn dependencies_sort_vulnerable_then_by_bump() {
        let mut app = app();
        checked(&mut app);
        let names: Vec<String> = sorted(&app.updates.results[&PathBuf::from("/w/a")])
            .into_iter()
            .map(|d| d.declared.name.clone())
            .collect();
        assert_eq!(names, ["vulnerable", "major", "minor", "current"]);
        assert_eq!(app.updates.totals(), (3, 1));
    }

    #[test]
    fn marking_and_updating_emit_jobs_for_the_marked_deps() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        checked(&mut app);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.updates.marked.len(), 3, "only outdated deps are marked");
        let effects = press(&mut app, KeyCode::Char('u'));
        let Some(Effect::ApplyUpdates {
            deps, latest, dir, ..
        }) = effects.first()
        else {
            panic!("updates expected, got {effects:?}");
        };
        assert!(!latest);
        assert_eq!(dir, &PathBuf::from("/w/a"));
        assert_eq!(deps.len(), 3);
        assert!(app.updates.updating.contains(&PathBuf::from("/w/a")));
    }

    #[test]
    fn upgrading_to_latest_needs_a_second_press() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        checked(&mut app);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('j'));
        assert!(press(&mut app, KeyCode::Char('U')).is_empty());
        assert!(app.toast.as_ref().unwrap().text.contains("again"));
        let effects = press(&mut app, KeyCode::Char('U'));
        assert!(matches!(
            effects.first(),
            Some(Effect::ApplyUpdates { latest: true, .. })
        ));
    }

    #[test]
    fn finished_update_jobs_trigger_a_recheck() {
        let mut app = app();
        app.updates.updating.insert(PathBuf::from("/w/a"));
        app.update(Msg::JobStarted {
            id: 3,
            label: "a".into(),
            task: carwash_core::tasks::Task {
                name: "update".into(),
                source: "update".into(),
                program: "cargo".into(),
                args: vec!["update".into()],
                description: None,
                cwd: PathBuf::from("/w/a"),
                standard: true,
            },
            after: crate::tui::tasks::After::Recheck(PathBuf::from("/w/a")),
        });
        let effects = app.update(Msg::JobExited(3, Ok(0)));
        assert!(matches!(effects.first(), Some(Effect::CheckDeps { .. })));
        assert!(!app.updates.updating.contains(&PathBuf::from("/w/a")));
    }

    #[test]
    fn build_metadata_is_hidden() {
        assert_eq!(version(Some("1.1.2+spec-1.1.0")), "1.1.2");
        assert_eq!(version(None), "-");
    }
}
