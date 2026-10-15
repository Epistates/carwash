//! The System tab: OS mode, the operating system's own caches, logs, trash and temporary
//! files.

use super::app::{App, Effect, Level};
use super::keymap::Action;
use super::widgets::{pane, spinner};
use crate::commands::os;
use carwash_core::fmt;
use carwash_core::sweep::{Skip, SweepReport};
use carwash_core::system::{Action as AreaAction, Area, Tier};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default)]
pub struct SystemState {
    /// `None` until discovery finished. Catalog order, as `carwash os` numbers them.
    pub areas: Option<Vec<Area>>,
    /// Dry-run sweeps: what cleaning each area would free.
    pub measured: HashMap<String, SweepReport>,
    /// Running programs keeping an area from being cleaned, as of its last measurement.
    pub blocked: HashMap<String, Vec<String>>,
    pub cursor: usize,
    pub marked: HashSet<String>,
    /// Being measured or cleaned.
    pub busy: HashSet<String>,
    clean_armed: bool,
    requested: bool,
    /// The tab has been shown; areas are measured from then on.
    opened: bool,
}

impl SystemState {
    fn get(&self, id: &str) -> Option<&Area> {
        self.areas.iter().flatten().find(|a| a.id == id)
    }

    /// What cleaning would free, when measured.
    pub fn frees(&self, area: &Area) -> Option<u64> {
        self.measured.get(&area.id).map(|r| r.removed.bytes)
    }

    fn is_blocked(&self, area: &Area) -> bool {
        self.blocked.get(&area.id).is_some_and(|b| !b.is_empty())
    }

    /// Ready to clean in the default tier: unblocked and not needing elevation.
    pub fn ready(&self) -> u64 {
        self.areas
            .iter()
            .flatten()
            .filter(|a| a.tier == Tier::Default && !self.is_blocked(a) && !os::needs_elevation(a))
            .filter_map(|a| self.frees(a))
            .sum()
    }
}

impl App {
    /// Discovery is cheap (known paths and a few globs), so it runs at startup; measuring
    /// waits until the tab is shown.
    pub fn discover_system(&mut self) -> Vec<Effect> {
        self.system.requested = true;
        vec![Effect::DiscoverSystem]
    }

    /// Showing the tab measures every area not measured yet.
    pub fn system_effects(&mut self) -> Vec<Effect> {
        self.system.opened = true;
        if !self.system.requested {
            return self.discover_system();
        }
        self.measure_system(false)
    }

    pub fn on_system_discovered(&mut self, areas: Vec<Area>) -> Vec<Effect> {
        let state = &mut self.system;
        state.marked.retain(|id| areas.iter().any(|a| &a.id == id));
        state.cursor = state.cursor.min(areas.len().saturating_sub(1));
        state.areas = Some(areas);
        self.measure_system(false)
    }

    pub fn on_system_measured(
        &mut self,
        id: String,
        report: SweepReport,
        blocked: Vec<String>,
    ) -> Vec<Effect> {
        self.system.busy.remove(&id);
        self.system.blocked.insert(id.clone(), blocked);
        self.system.measured.insert(id, report);
        Vec::new()
    }

    pub fn on_system_cleaned(&mut self, id: String, result: Result<u64, String>) -> Vec<Effect> {
        let name = self.system.get(&id).map_or(id.clone(), |a| a.name.clone());
        match result {
            Ok(freed) => self.toast(
                format!("Freed {} from {name}", fmt::bytes(freed)),
                Level::Info,
            ),
            Err(error) => self.toast(format!("{name}: {error}"), Level::Warn),
        }
        self.measure_area(&id)
    }

    /// Measures one area again, as after its command ran.
    pub fn measure_area(&mut self, id: &str) -> Vec<Effect> {
        let Some(area) = self.system.get(id).cloned() else {
            return Vec::new();
        };
        self.system.busy.insert(area.id.clone());
        vec![Effect::MeasureSystem(vec![area])]
    }

    fn measure_system(&mut self, all: bool) -> Vec<Effect> {
        let state = &mut self.system;
        if !state.opened {
            return Vec::new();
        }
        let targets: Vec<Area> = state
            .areas
            .iter()
            .flatten()
            .filter(|a| !state.busy.contains(&a.id))
            .filter(|a| all || !state.measured.contains_key(&a.id))
            .cloned()
            .collect();
        if targets.is_empty() {
            return Vec::new();
        }
        state.busy.extend(targets.iter().map(|a| a.id.clone()));
        vec![Effect::MeasureSystem(targets)]
    }

    fn selected_areas(&self) -> Vec<Area> {
        let Some(areas) = &self.system.areas else {
            return Vec::new();
        };
        let marked: Vec<Area> = areas
            .iter()
            .filter(|a| self.system.marked.contains(&a.id))
            .cloned()
            .collect();
        if marked.is_empty() {
            areas
                .get(self.system.cursor)
                .filter(|a| a.tier != Tier::Report)
                .cloned()
                .into_iter()
                .collect()
        } else {
            marked
        }
    }

    /// What cleaning would do to the marked (or selected) areas.
    pub fn system_clean_preview(&self) -> Option<String> {
        let selected = self.selected_areas();
        match selected.as_slice() {
            [] => None,
            [area] => Some(describe(area, self.system.frees(area))),
            many => Some(format!(
                "{} marked areas · frees about {}",
                many.len(),
                fmt::bytes(many.iter().filter_map(|a| self.system.frees(a)).sum())
            )),
        }
    }

    pub fn on_system_action(&mut self, action: Action) -> Vec<Effect> {
        if action != Action::Clean {
            self.system.clean_armed = false;
        }
        let len = self.system.areas.as_ref().map_or(0, Vec::len);
        let last = len.saturating_sub(1);
        let page = self.page.max(1);
        let state = &mut self.system;
        match action {
            Action::Up => state.cursor = state.cursor.saturating_sub(1),
            Action::Down => state.cursor = (state.cursor + 1).min(last),
            Action::PageUp => state.cursor = state.cursor.saturating_sub(page),
            Action::PageDown => state.cursor = (state.cursor + page).min(last),
            Action::Top => state.cursor = 0,
            Action::Bottom => state.cursor = last,
            Action::Mark => {
                let Some(area) = state
                    .areas
                    .as_ref()
                    .and_then(|a| a.get(state.cursor))
                    .cloned()
                else {
                    return Vec::new();
                };
                if area.tier == Tier::Report {
                    let how = area.how.clone().unwrap_or_default();
                    self.toast(format!("{} is report-only: {how}", area.name), Level::Warn);
                    return Vec::new();
                }
                if !state.marked.remove(&area.id) {
                    state.marked.insert(area.id.clone());
                }
                state.cursor = (state.cursor + 1).min(last);
            }
            Action::MarkAll => {
                let ready: Vec<String> = state
                    .areas
                    .iter()
                    .flatten()
                    .filter(|a| {
                        a.tier == Tier::Default && !state.is_blocked(a) && !os::needs_elevation(a)
                    })
                    .map(|a| a.id.clone())
                    .collect();
                let count = ready.len();
                state.marked.extend(ready);
                self.toast(
                    format!(
                        "Marked {count} default areas · {}",
                        fmt::bytes(self.system.ready())
                    ),
                    Level::Info,
                );
            }
            Action::Unmark => state.marked.clear(),
            Action::Rescan => {
                state.measured.clear();
                return self.discover_system();
            }
            Action::Open => {
                if let Some(root) = self
                    .system
                    .areas
                    .as_ref()
                    .and_then(|a| a.get(self.system.cursor))
                    .and_then(|a| a.roots.first().cloned())
                {
                    return vec![Effect::Reveal(root)];
                }
            }
            Action::Clean => return self.clean_system(),
            _ => {}
        }
        Vec::new()
    }

    fn clean_system(&mut self) -> Vec<Effect> {
        let selected = self.selected_areas();
        if selected.is_empty() {
            self.toast(
                "Nothing to clean: Space marks an area, a marks the default tier",
                Level::Warn,
            );
            return Vec::new();
        }
        let blocked: Vec<&Area> = selected
            .iter()
            .filter(|a| self.system.is_blocked(a))
            .collect();
        if let [area] = blocked.as_slice()
            && selected.len() == 1
        {
            let apps = self.system.blocked[&area.id].join(", ");
            self.toast(
                format!("Close {apps} first to clean {}", area.name),
                Level::Warn,
            );
            return Vec::new();
        }
        if !self.system.clean_armed {
            self.system.clean_armed = true;
            let elevated: Vec<String> = selected
                .iter()
                .filter(|a| os::needs_elevation(a))
                .filter_map(|a| match &a.action {
                    AreaAction::Command { argv } => Some(os::privileged_line(argv, a.needs)),
                    _ => None,
                })
                .collect();
            let what = match selected.as_slice() {
                [area] => describe(area, self.system.frees(area)),
                many => format!(
                    "{} areas, about {}",
                    many.len(),
                    fmt::bytes(many.iter().filter_map(|a| self.system.frees(a)).sum())
                ),
            };
            let consent = if elevated.is_empty() {
                String::new()
            } else {
                format!(
                    " · runs with your password in the Tasks tab: {}",
                    elevated.join("; ")
                )
            };
            self.toast(
                format!("Press d again to clean {what}{consent}"),
                Level::Warn,
            );
            return Vec::new();
        }
        self.system.clean_armed = false;
        let areas: Vec<Area> = selected
            .into_iter()
            .filter(|a| !self.system.is_blocked(a))
            .collect();
        for area in &areas {
            self.system.marked.remove(&area.id);
            self.system.busy.insert(area.id.clone());
        }
        let commands = areas
            .iter()
            .filter(|a| matches!(a.action, AreaAction::Command { .. }))
            .count();
        self.toast(
            if commands > 0 {
                format!(
                    "Cleaning {} areas; commands run in the Tasks tab (2)",
                    areas.len()
                )
            } else {
                format!("Cleaning {} areas", areas.len())
            },
            Level::Info,
        );
        vec![Effect::CleanSystem {
            areas,
            size: self.tasks.pty_size,
        }]
    }
}

/// One line on what cleaning `area` does.
fn describe(area: &Area, frees: Option<u64>) -> String {
    let size = frees.map_or_else(String::new, |b| format!(" ({})", fmt::bytes(b)));
    match &area.action {
        AreaAction::Sweep => match area.older_than {
            Some(age) => format!("{}{size}, keeping the last {}", area.name, fmt::age(age)),
            None => format!("{}{size}", area.name),
        },
        AreaAction::Command { argv } => {
            format!(
                "{}: runs `{}`",
                area.name,
                os::privileged_line(argv, area.needs)
            )
        }
        AreaAction::Report => format!("{} is report-only", area.name),
    }
}

/// What a sweep left in place, briefly.
fn left_behind(report: &SweepReport) -> String {
    let parts: Vec<String> = report
        .skipped
        .iter()
        .filter(|(reason, tally)| **reason != Skip::Kept && tally.entries > 0)
        .map(|(reason, tally)| {
            let label = match reason {
                Skip::Recent => "recent",
                Skip::InUse => "in use",
                Skip::Denied => "not permitted",
                Skip::Cloud => "in the cloud",
                Skip::MountPoint => "on other disks",
                Skip::Special => "special",
                Skip::Changed => "changed",
                Skip::TooDeep => "too deep",
                Skip::Kept => "kept",
            };
            if tally.bytes > 0 {
                format!("{} {label} ({})", tally.entries, fmt::bytes(tally.bytes))
            } else {
                format!("{} {label}", tally.entries)
            }
        })
        .collect();
    parts.join(", ")
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme;
    let Some(areas) = &app.system.areas else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("{} Looking for system areas…", spinner(app)),
                t.subtle(),
            ))
            .block(pane(&t, "System".into(), true)),
            area,
        );
        return;
    };
    let [table_area, detail] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(5)]).areas(area);
    let state = &app.system;
    let rows: Vec<Row> = areas
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let mark = if state.busy.contains(&a.id) {
                Span::styled(spinner(app), t.fg(t.accent))
            } else if state.marked.contains(&a.id) {
                Span::styled(app.glyphs.marked, t.bold(t.accent2))
            } else if a.tier == Tier::Report {
                Span::styled(app.glyphs.locked, t.muted())
            } else {
                Span::styled(app.glyphs.unmarked, t.muted())
            };
            let frees = match (state.frees(a), &a.action) {
                (_, AreaAction::Command { .. }) if a.roots.is_empty() => {
                    Span::styled("-", t.muted())
                }
                (Some(bytes), _) => Span::styled(fmt::bytes(bytes), t.size(bytes)),
                (None, _) => Span::styled("…", t.muted()),
            };
            let tier = match a.tier {
                Tier::Default => Span::styled("default", t.fg(t.success)),
                Tier::OptIn => Span::styled("opt-in", t.fg(t.warning)),
                Tier::Report => Span::styled("report", t.muted()),
            };
            let note = match state.blocked.get(&a.id).filter(|b| !b.is_empty()) {
                Some(apps) => Span::styled(format!("close {}", apps.join(", ")), t.fg(t.warning)),
                None => match &a.action {
                    AreaAction::Command { argv } => {
                        Span::styled(os::privileged_line(argv, a.needs), t.subtle())
                    }
                    AreaAction::Report => Span::styled("by hand", t.muted()),
                    AreaAction::Sweep => match a.older_than {
                        Some(age) => Span::styled(format!("keeps {}", fmt::age(age)), t.muted()),
                        None => Span::raw(""),
                    },
                },
            };
            Row::new(vec![
                Cell::from(mark),
                Cell::from(
                    Line::from(Span::styled((i + 1).to_string(), t.muted()))
                        .alignment(Alignment::Right),
                ),
                Cell::from(Line::from(frees).alignment(Alignment::Right)),
                Cell::from(tier),
                Cell::from(Span::styled(a.name.clone(), t.text())),
                Cell::from(note),
            ])
        })
        .collect();
    let mut title = format!(
        "System {} · {} ready",
        areas.len(),
        fmt::bytes(state.ready())
    );
    if !state.marked.is_empty() {
        title.push_str(&format!(" · {} marked", state.marked.len()));
    }
    let header = Row::new(
        ["", "#", "FREES", "TIER", "AREA", "NOTES"]
            .into_iter()
            .map(|h| Cell::from(Span::styled(h, t.bold(t.muted)))),
    );
    let mut table_state = TableState::default().with_selected(Some(state.cursor));
    frame.render_stateful_widget(
        Table::new(
            rows,
            [
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(9),
                Constraint::Length(7),
                Constraint::Fill(2),
                Constraint::Fill(2),
            ],
        )
        .header(header)
        .column_spacing(1)
        .row_highlight_style(t.selected_row())
        .block(pane(&t, title, true)),
        table_area,
        &mut table_state,
    );
    let Some(selected) = areas.get(state.cursor) else {
        return;
    };
    let mut lines = Vec::new();
    let mut place = match selected.roots.as_slice() {
        [] => String::new(),
        [root] => root.display().to_string(),
        [root, rest @ ..] => format!("{} and {} more", root.display(), rest.len()),
    };
    if let Some(report) = state.measured.get(&selected.id) {
        let left = left_behind(report);
        if !left.is_empty() {
            place.push_str(&format!("  · leaves {left}"));
        }
        if os::lacks_disk_access(selected, report) {
            place.push_str(&format!("  · {}", os::DISK_ACCESS_HINT));
        }
    }
    if !place.is_empty() {
        lines.push(Line::from(Span::styled(place, t.subtle())));
    }
    if let Some(note) = &selected.note {
        lines.push(Line::from(Span::styled(note.clone(), t.muted())));
    }
    if let Some(how) = &selected.how {
        lines.push(Line::from(Span::styled(format!("How: {how}"), t.text())));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(pane(
            &t,
            selected.name.clone(),
            false,
        )),
        detail,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::Msg;
    use crate::tui::app::tests::app;
    use carwash_core::system::Need;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;

    fn press(app: &mut App, code: KeyCode) -> Vec<Effect> {
        app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }

    fn area(id: &str, tier: Tier, action: AreaAction) -> Area {
        Area {
            id: id.into(),
            name: id.into(),
            category: "caches".into(),
            tier,
            action,
            roots: vec![PathBuf::from(format!("/home/{id}"))],
            checked: true,
            older_than: None,
            keep: Vec::new(),
            apps: Vec::new(),
            needs: None,
            note: None,
            how: Some("by hand".into()),
        }
    }

    fn areas() -> Vec<Area> {
        let apt = Area {
            needs: Some(Need::Root),
            ..area(
                "apt",
                Tier::Default,
                AreaAction::Command {
                    argv: vec!["apt-get".into(), "clean".into()],
                },
            )
        };
        vec![
            area("caches", Tier::Default, AreaAction::Sweep),
            area("trash", Tier::OptIn, AreaAction::Sweep),
            area("backups", Tier::Report, AreaAction::Report),
            apt,
        ]
    }

    fn measured(id: &str, bytes: u64, blocked: &[&str]) -> Msg {
        let mut report = SweepReport::default();
        report.removed.bytes = bytes;
        Msg::SystemMeasured {
            id: id.into(),
            report,
            blocked: blocked.iter().map(|b| (*b).into()).collect(),
        }
    }

    fn open(app: &mut App) {
        press(app, KeyCode::Char('5'));
        app.update(Msg::SystemDiscovered(areas()));
    }

    #[test]
    fn discovered_at_startup_measured_when_shown() {
        let mut app = app();
        assert!(
            app.startup()
                .iter()
                .any(|e| matches!(e, Effect::DiscoverSystem))
        );
        let effects = app.update(Msg::SystemDiscovered(areas()));
        assert!(effects.is_empty(), "not measured before the tab is shown");
        let effects = press(&mut app, KeyCode::Char('5'));
        let Some(Effect::MeasureSystem(targets)) = effects.first() else {
            panic!("expected a measurement, got {effects:?}");
        };
        assert_eq!(targets.len(), 4);
        for id in ["caches", "trash", "backups", "apt"] {
            app.update(measured(id, 1_000, &[]));
        }
        press(&mut app, KeyCode::Char('1'));
        assert!(
            !press(&mut app, KeyCode::Char('5'))
                .iter()
                .any(|e| matches!(e, Effect::MeasureSystem(_))),
            "measured once per session"
        );
        // Only the unblocked default-tier sweep counts as ready; apt needs root.
        assert_eq!(app.system.ready(), 1_000);
    }

    #[test]
    fn report_areas_cannot_be_marked() {
        let mut app = app();
        open(&mut app);
        app.system.cursor = 2;
        press(&mut app, KeyCode::Char(' '));
        assert!(app.system.marked.is_empty());
        assert!(app.toast.as_ref().unwrap().text.contains("by hand"));
    }

    #[test]
    fn cleaning_needs_a_second_press_and_names_privileged_commands() {
        let mut app = app();
        open(&mut app);
        app.system.busy.clear();
        app.system.marked.insert("caches".into());
        app.system.marked.insert("apt".into());
        assert!(press(&mut app, KeyCode::Char('d')).is_empty());
        assert!(
            app.toast
                .as_ref()
                .unwrap()
                .text
                .contains("sudo apt-get clean"),
            "consent names the elevated command"
        );
        let effects = press(&mut app, KeyCode::Char('d'));
        let Some(Effect::CleanSystem { areas, .. }) = effects.first() else {
            panic!("clean expected, got {effects:?}");
        };
        assert_eq!(areas.len(), 2);
        assert!(app.system.busy.contains("caches"));
        assert!(app.system.marked.is_empty());
    }

    #[test]
    fn blocked_areas_are_not_cleaned() {
        let mut app = app();
        open(&mut app);
        app.update(measured("caches", 10, &["Safari"]));
        app.system.busy.clear();
        assert!(press(&mut app, KeyCode::Char('d')).is_empty());
        assert!(app.toast.as_ref().unwrap().text.contains("Close Safari"));
        assert!(
            press(&mut app, KeyCode::Char('d')).is_empty(),
            "still refused"
        );
    }

    #[test]
    fn mark_all_takes_the_ready_default_tier() {
        let mut app = app();
        open(&mut app);
        press(&mut app, KeyCode::Char('a'));
        let marked: Vec<&str> = app.system.marked.iter().map(String::as_str).collect();
        assert_eq!(marked, ["caches"], "not opt-in, report or elevated areas");
    }
}
