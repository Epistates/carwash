//! The Caches tab: per-user caches outside projects.

use super::app::{App, Effect, Level};
use super::keymap::Action;
use super::widgets::{pane, spinner};
use carwash_core::cache::{CachedSize, Fingerprint, SizeCache};
use carwash_core::caches::GlobalCache;
use carwash_core::{Size, fmt};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Remembered sizes younger than this are shown without measuring again...
const SIZES_FRESH_FOR: Duration = Duration::from_secs(24 * 60 * 60);
/// ...or younger than this when the cache's fingerprint shows no change.
const UNCHANGED_FRESH_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Default)]
pub struct CachesState {
    /// `None` until discovery finished.
    pub caches: Option<Vec<GlobalCache>>,
    pub cursor: usize,
    pub marked: HashSet<String>,
    /// Being cleaned or measured.
    pub busy: HashSet<String>,
    clean_armed: bool,
    requested: bool,
    /// The tab has been shown; stale sizes are measured from then on.
    opened: bool,
    /// The next discovery measures every cache, fresh or not.
    remeasure_all: bool,
    /// The user has moved the cursor.
    moved: bool,
}

impl CachesState {
    /// Installs discovered caches with their remembered sizes.
    fn set(&mut self, mut caches: Vec<GlobalCache>, sizes: &SizeCache) {
        for cache in &mut caches {
            cache.size = sizes.get(&cache.path).map(|c| c.size);
        }
        self.caches = Some(caches);
        self.sort();
    }

    /// Records a measurement; returns the cache's path.
    fn measured(
        &mut self,
        id: &str,
        size: Size,
        fingerprint: Option<Fingerprint>,
    ) -> Option<PathBuf> {
        self.busy.remove(id);
        let cache = self
            .caches
            .as_mut()
            .and_then(|c| c.iter_mut().find(|c| c.id == id))?;
        cache.size = Some(size);
        cache.fingerprint = fingerprint;
        let path = cache.path.clone();
        self.sort();
        Some(path)
    }

    /// Largest first. Once the user has moved, the cursor stays on the same cache; until
    /// then it stays on the first row.
    fn sort(&mut self) {
        let Some(caches) = &mut self.caches else {
            return;
        };
        let current = caches.get(self.cursor).map(|c| c.id.clone());
        caches.sort_by_key(|c| std::cmp::Reverse(c.size.map_or(0, |s| s.on_disk)));
        if !self.moved {
            return;
        }
        if let Some(position) = current.and_then(|id| caches.iter().position(|c| c.id == id)) {
            self.cursor = position;
        }
    }

    pub fn total(&self) -> u64 {
        self.caches
            .iter()
            .flatten()
            .filter_map(|c| c.size)
            .map(|s| s.on_disk)
            .sum()
    }
}

/// Whether `cache` must be measured again, given what was remembered about it.
fn is_stale(cache: &GlobalCache, remembered: Option<&CachedSize>, now: SystemTime) -> bool {
    let Some(remembered) = remembered else {
        return true;
    };
    let age = remembered.age_at(now);
    match (remembered.fingerprint, cache.fingerprint) {
        (Some(then), Some(current)) if then == current => age > UNCHANGED_FRESH_FOR,
        (Some(_), Some(_)) => true,
        _ => age > SIZES_FRESH_FOR,
    }
}

impl App {
    /// How long ago `cache`'s size was measured, in this session or an earlier one.
    pub fn cache_measured_age(&self, cache: &GlobalCache) -> Option<Duration> {
        self.cache.get(&cache.path).map(|c| c.age_at(self.now))
    }

    /// Age of the oldest size shown, when it is not from the last minute.
    pub fn caches_oldest_age(&self) -> Option<Duration> {
        self.caches
            .caches
            .iter()
            .flatten()
            .filter(|c| !self.caches.busy.contains(&c.id))
            .filter_map(|c| self.cache_measured_age(c))
            .max()
            .filter(|age| *age >= Duration::from_secs(60))
    }

    /// Discovery is cheap (a few dozen known paths), so it runs at startup; measuring waits
    /// until the tab is shown.
    pub fn discover_caches(&mut self) -> Vec<Effect> {
        self.caches.requested = true;
        vec![Effect::DiscoverCaches]
    }

    /// Showing the tab measures caches with no remembered size, or a stale one.
    pub fn caches_effects(&mut self) -> Vec<Effect> {
        self.caches.opened = true;
        if !self.caches.requested {
            return self.discover_caches();
        }
        self.measure_caches()
    }

    pub fn on_caches_discovered(&mut self, caches: Vec<GlobalCache>) -> Vec<Effect> {
        self.caches.set(caches, &self.cache);
        self.measure_caches()
    }

    /// Remembers the size; once nothing is left to measure, saves the size cache so the
    /// next start has it even if this session does not end cleanly.
    pub fn on_cache_measured(
        &mut self,
        id: &str,
        size: Size,
        fingerprint: Option<Fingerprint>,
    ) -> Vec<Effect> {
        if let Some(path) = self.caches.measured(id, size, fingerprint) {
            self.cache.insert_with(path, size, fingerprint);
        }
        if self.caches.busy.is_empty() {
            vec![Effect::SaveSizes(self.cache.clone())]
        } else {
            Vec::new()
        }
    }

    fn measure_caches(&mut self) -> Vec<Effect> {
        let state = &mut self.caches;
        if !state.opened {
            return Vec::new();
        }
        let all = std::mem::take(&mut state.remeasure_all);
        let targets: Vec<(String, PathBuf)> = state
            .caches
            .iter()
            .flatten()
            .filter(|c| !state.busy.contains(&c.id))
            .filter(|c| all || is_stale(c, self.cache.get(&c.path), self.now))
            .map(|c| (c.id.clone(), c.path.clone()))
            .collect();
        if targets.is_empty() {
            return Vec::new();
        }
        state.busy.extend(targets.iter().map(|(id, _)| id.clone()));
        vec![Effect::MeasureCaches(targets)]
    }

    fn selected_caches(&self) -> Vec<GlobalCache> {
        let Some(caches) = &self.caches.caches else {
            return Vec::new();
        };
        let marked: Vec<GlobalCache> = caches
            .iter()
            .filter(|c| self.caches.marked.contains(&c.id))
            .cloned()
            .collect();
        if marked.is_empty() {
            caches
                .get(self.caches.cursor)
                .cloned()
                .into_iter()
                .collect()
        } else {
            marked
        }
    }

    /// What cleaning would do to the marked (or selected) caches.
    pub fn caches_clean_preview(&self) -> Option<String> {
        let selected = self.selected_caches();
        match selected.as_slice() {
            [] => None,
            [cache] => Some(match (cache.prune_task(), cache.deletable) {
                (Some(task), _) => format!("{}: runs `{}`", cache.name, task.command_line()),
                (None, true) => format!("{}: deletes {}", cache.name, cache.path.display()),
                (None, false) => format!("{}: its tool is not installed", cache.name),
            }),
            many => Some(format!(
                "{} marked caches · {}",
                many.len(),
                fmt::bytes(many.iter().filter_map(|c| c.size).map(|s| s.on_disk).sum())
            )),
        }
    }

    pub fn on_caches_action(&mut self, action: Action) -> Vec<Effect> {
        if action != Action::Clean {
            self.caches.clean_armed = false;
        }
        let len = self.caches.caches.as_ref().map_or(0, Vec::len);
        let last = len.saturating_sub(1);
        let page = self.page.max(1);
        let state = &mut self.caches;
        if matches!(
            action,
            Action::Up
                | Action::Down
                | Action::PageUp
                | Action::PageDown
                | Action::Top
                | Action::Bottom
                | Action::Mark
        ) {
            state.moved = true;
        }
        match action {
            Action::Up => state.cursor = state.cursor.saturating_sub(1),
            Action::Down => state.cursor = (state.cursor + 1).min(last),
            Action::PageUp => state.cursor = state.cursor.saturating_sub(page),
            Action::PageDown => state.cursor = (state.cursor + page).min(last),
            Action::Top => state.cursor = 0,
            Action::Bottom => state.cursor = last,
            Action::Mark => {
                if let Some(id) = state
                    .caches
                    .as_ref()
                    .and_then(|c| c.get(state.cursor))
                    .map(|c| c.id.clone())
                {
                    if !state.marked.remove(&id) {
                        state.marked.insert(id);
                    }
                    state.cursor = (state.cursor + 1).min(last);
                }
            }
            Action::Unmark => state.marked.clear(),
            Action::Rescan => {
                state.remeasure_all = true;
                return self.discover_caches();
            }
            Action::Open => {
                if let Some(path) = self.selected_caches().first().map(|c| c.path.clone()) {
                    return vec![Effect::Reveal(path)];
                }
            }
            Action::Clean => {
                let selected = self.selected_caches();
                if selected.is_empty() {
                    return Vec::new();
                }
                let bytes: u64 = selected
                    .iter()
                    .filter_map(|c| c.size)
                    .map(|s| s.on_disk)
                    .sum();
                if !self.caches.clean_armed {
                    self.caches.clean_armed = true;
                    self.toast(
                        format!(
                            "Press d again to clean {} ({})",
                            if selected.len() == 1 {
                                selected[0].name.clone()
                            } else {
                                format!("{} caches", selected.len())
                            },
                            fmt::bytes(bytes)
                        ),
                        Level::Warn,
                    );
                    return Vec::new();
                }
                self.caches.clean_armed = false;
                for cache in &selected {
                    self.caches.marked.remove(&cache.id);
                    self.caches.busy.insert(cache.id.clone());
                }
                self.toast(
                    format!(
                        "Cleaning {}; prune commands run in the Tasks tab (2)",
                        fmt::bytes(bytes)
                    ),
                    Level::Info,
                );
                return vec![Effect::CleanCaches {
                    caches: selected,
                    size: self.tasks.pty_size,
                }];
            }
            _ => {}
        }
        Vec::new()
    }
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme;
    let Some(caches) = &app.caches.caches else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("{} Looking for caches…", spinner(app)),
                t.subtle(),
            ))
            .block(pane(&t, "Caches".into(), true)),
            area,
        );
        return;
    };
    let [table_area, detail] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(4)]).areas(area);
    let rows: Vec<Row> = caches
        .iter()
        .map(|cache| {
            let mark = if app.caches.busy.contains(&cache.id) {
                Span::styled(spinner(app), t.fg(t.accent))
            } else if app.caches.marked.contains(&cache.id) {
                Span::styled(app.glyphs.marked, t.bold(t.accent2))
            } else {
                Span::styled(app.glyphs.unmarked, t.muted())
            };
            let size = match cache.size {
                Some(size) => Span::styled(fmt::bytes(size.on_disk), t.size(size.on_disk)),
                None => Span::styled("…", t.muted()),
            };
            let action = match (cache.prune_task(), cache.deletable) {
                (Some(task), _) => Span::styled(task.command_line(), t.text()),
                (None, true) => Span::styled("delete", t.subtle()),
                (None, false) => Span::styled(
                    format!(
                        "needs `{}`",
                        cache.prune.as_ref().map_or("?", |p| p[0].as_str())
                    ),
                    t.fg(t.warning),
                ),
            };
            let badge = cache
                .ecosystem
                .as_deref()
                .and_then(|key| app.registry.find(key))
                .map(|eco| {
                    let e = app.registry.ecosystem(eco);
                    Span::styled(e.badge.clone(), t.fg(t.ecosystem(&e.key)))
                })
                .unwrap_or_default();
            Row::new(vec![
                Cell::from(mark),
                Cell::from(Line::from(size).alignment(Alignment::Right)),
                Cell::from(badge),
                Cell::from(Span::styled(cache.name.clone(), t.text())),
                Cell::from(action),
            ])
        })
        .collect();
    let mut title = format!(
        "Caches {} · {}",
        caches.len(),
        fmt::bytes(app.caches.total())
    );
    if !app.caches.marked.is_empty() {
        title.push_str(&format!(" · {} marked", app.caches.marked.len()));
    }
    if let Some(age) = app.caches_oldest_age() {
        title.push_str(&format!(
            " · sizes up to {} old (r to measure)",
            fmt::age(age)
        ));
    }
    let header = Row::new(
        ["", "SIZE", "", "CACHE", "CLEAN WITH"]
            .into_iter()
            .map(|h| Cell::from(Span::styled(h, t.bold(t.muted)))),
    );
    let mut state = TableState::default().with_selected(Some(app.caches.cursor));
    frame.render_stateful_widget(
        Table::new(
            rows,
            [
                Constraint::Length(1),
                Constraint::Length(9),
                Constraint::Length(2),
                Constraint::Fill(2),
                Constraint::Fill(1),
            ],
        )
        .header(header)
        .column_spacing(1)
        .row_highlight_style(t.selected_row())
        .block(pane(&t, title, true)),
        table_area,
        &mut state,
    );
    if let Some(cache) = caches.get(app.caches.cursor) {
        let measured = if app.caches.busy.contains(&cache.id) {
            "measuring…".to_owned()
        } else {
            match app.cache_measured_age(cache) {
                Some(age) if age < Duration::from_secs(60) => "measured just now".to_owned(),
                Some(age) => format!("measured {} ago", fmt::age(age)),
                None => String::new(),
            }
        };
        let mut lines = vec![Line::from(vec![
            Span::styled(cache.path.display().to_string(), t.subtle()),
            Span::styled(format!("  {measured}"), t.muted()),
        ])];
        if let Some(note) = &cache.note {
            lines.push(Line::from(Span::styled(note.clone(), t.muted())));
        }
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: true }).block(pane(
                &t,
                cache.name.clone(),
                false,
            )),
            detail,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::Msg;
    use crate::tui::app::tests::app;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::Path;

    fn press(app: &mut App, code: KeyCode) -> Vec<Effect> {
        app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }

    fn cache(id: &str) -> GlobalCache {
        GlobalCache {
            id: id.into(),
            name: id.into(),
            path: PathBuf::from(format!("/home/{id}")),
            ecosystem: None,
            deletable: true,
            prune: None,
            note: None,
            size: None,
            fingerprint: None,
        }
    }

    fn measured(id: &str, size: Size) -> Msg {
        Msg::CacheMeasured {
            id: id.into(),
            size,
            fingerprint: None,
        }
    }

    fn fingerprint(newest: u64) -> Option<Fingerprint> {
        Some(Fingerprint { newest, entries: 3 })
    }

    #[test]
    fn fingerprints_decide_how_long_a_remembered_size_holds() {
        let now = SystemTime::now();
        let mut sizes = SizeCache::default();
        sizes.insert_with(PathBuf::from("/home/a"), Size::default(), fingerprint(1));
        let remembered = sizes.get(Path::new("/home/a"));
        let two_days = now + Duration::from_secs(2 * 24 * 60 * 60);
        let two_weeks = now + Duration::from_secs(14 * 24 * 60 * 60);
        let with = |fp| GlobalCache {
            fingerprint: fp,
            ..cache("a")
        };
        // Unchanged: trusted for a week.
        assert!(!is_stale(&with(fingerprint(1)), remembered, two_days));
        assert!(is_stale(&with(fingerprint(1)), remembered, two_weeks));
        // Changed near the top: measured now.
        assert!(is_stale(&with(fingerprint(2)), remembered, now));
        // Unknown: a day.
        assert!(!is_stale(&with(None), remembered, now));
        assert!(is_stale(&with(None), remembered, two_days));
        assert!(is_stale(&with(fingerprint(1)), None, now));
    }

    #[test]
    fn finishing_the_measurements_saves_the_sizes() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        app.update(Msg::CachesDiscovered(vec![cache("a"), cache("b")]));
        let first = app.update(measured("a", Size::default()));
        assert!(!first.iter().any(|e| matches!(e, Effect::SaveSizes(_))));
        let last = app.update(Msg::CacheMeasured {
            id: "b".into(),
            size: Size::default(),
            fingerprint: fingerprint(9),
        });
        let Some(Effect::SaveSizes(sizes)) = last.first() else {
            panic!("expected a save, got {last:?}");
        };
        assert_eq!(
            sizes.get(Path::new("/home/b")).unwrap().fingerprint,
            fingerprint(9)
        );
    }

    #[test]
    fn caches_are_discovered_once_measured_and_sorted() {
        let mut app = app();
        let effects = press(&mut app, KeyCode::Char('4'));
        assert!(matches!(effects.first(), Some(Effect::DiscoverCaches)));
        assert!(press(&mut app, KeyCode::Char('4')).is_empty());
        app.update(Msg::CachesDiscovered(vec![cache("small"), cache("big")]));
        let size = |bytes| Size {
            on_disk: bytes,
            reclaimable: bytes,
            ..Size::default()
        };
        app.update(measured("small", size(10)));
        app.update(measured("big", size(1_000)));
        let ids: Vec<&str> = app
            .caches
            .caches
            .as_ref()
            .unwrap()
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ids, ["big", "small"]);
        assert_eq!(app.caches.total(), 1_010);
        assert!(app.caches.busy.is_empty());
        assert_eq!(app.caches.cursor, 0, "the largest cache is selected first");
        press(&mut app, KeyCode::Char('j'));
        app.update(measured("small", size(5_000)));
        assert_eq!(
            app.caches.cursor, 0,
            "after moving, the cursor follows its cache"
        );
    }

    fn measured_ids(effects: &[Effect]) -> Vec<&str> {
        effects
            .iter()
            .flat_map(|e| match e {
                Effect::MeasureCaches(targets) => {
                    targets.iter().map(|(id, _)| id.as_str()).collect()
                }
                _ => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn startup_discovers_but_measures_only_once_the_tab_is_shown() {
        let mut app = app();
        assert!(
            app.startup()
                .iter()
                .any(|e| matches!(e, Effect::DiscoverCaches))
        );
        let effects = app.update(Msg::CachesDiscovered(vec![cache("a")]));
        assert!(measured_ids(&effects).is_empty());
        let effects = press(&mut app, KeyCode::Char('4'));
        assert_eq!(measured_ids(&effects), ["a"]);
        // Showing the tab again does not measure twice.
        press(&mut app, KeyCode::Char('1'));
        assert!(measured_ids(&press(&mut app, KeyCode::Char('4'))).is_empty());
    }

    #[test]
    fn remembered_sizes_are_shown_and_only_stale_ones_measured() {
        let mut app = app();
        let size = Size {
            on_disk: 500,
            ..Size::default()
        };
        app.cache.insert(PathBuf::from("/home/fresh"), size);
        press(&mut app, KeyCode::Char('4'));
        let effects = app.update(Msg::CachesDiscovered(vec![cache("fresh"), cache("new")]));
        assert_eq!(measured_ids(&effects), ["new"]);
        assert_eq!(app.caches.total(), 500);

        // A day later the remembered size is stale; `r` measures everything regardless.
        let mut later = crate::tui::app::tests::app();
        later.cache.insert(PathBuf::from("/home/fresh"), size);
        later.now += SIZES_FRESH_FOR + Duration::from_secs(60);
        press(&mut later, KeyCode::Char('4'));
        let effects = later.update(Msg::CachesDiscovered(vec![cache("fresh")]));
        assert_eq!(measured_ids(&effects), ["fresh"]);

        app.update(measured("new", size));
        assert!(
            press(&mut app, KeyCode::Char('r'))
                .iter()
                .any(|e| matches!(e, Effect::DiscoverCaches))
        );
        let effects = app.update(Msg::CachesDiscovered(vec![cache("fresh"), cache("new")]));
        assert_eq!(measured_ids(&effects), ["fresh", "new"]);
    }

    #[test]
    fn measurements_are_remembered() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        app.update(Msg::CachesDiscovered(vec![cache("a")]));
        let size = Size {
            on_disk: 7,
            ..Size::default()
        };
        app.update(measured("a", size));
        assert_eq!(app.cache.get(Path::new("/home/a")).unwrap().size, size);
    }

    #[test]
    fn clean_preview_names_the_command() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        app.update(Msg::CachesDiscovered(vec![cache("a")]));
        assert_eq!(app.caches_clean_preview().unwrap(), "a: deletes /home/a");
    }

    #[test]
    fn the_wheel_scrolls_the_caches_list() {
        use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        let caches = (0..5).map(|i| cache(&format!("c{i}"))).collect();
        app.update(Msg::CachesDiscovered(caches));
        let reclaim_before = app.selected;
        app.update(Msg::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(app.caches.cursor, 3);
        assert_eq!(app.selected, reclaim_before);
    }

    #[test]
    fn cleaning_needs_a_second_press() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        app.update(Msg::CachesDiscovered(vec![cache("a"), cache("b")]));
        app.caches.busy.clear();
        press(&mut app, KeyCode::Char(' '));
        assert!(press(&mut app, KeyCode::Char('d')).is_empty());
        let effects = press(&mut app, KeyCode::Char('d'));
        let Some(Effect::CleanCaches { caches, .. }) = effects.first() else {
            panic!("clean expected, got {effects:?}");
        };
        assert_eq!(caches.len(), 1);
        assert!(app.caches.busy.contains(&caches[0].id));
    }
}
