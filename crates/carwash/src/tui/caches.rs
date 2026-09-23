//! The Caches tab: per-user caches outside projects.

use super::app::{App, Effect, Level};
use super::keymap::Action;
use super::widgets::{pane, spinner};
use carwash_core::caches::GlobalCache;
use carwash_core::{Size, fmt};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};
use std::collections::HashSet;

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
}

impl CachesState {
    pub fn set(&mut self, mut caches: Vec<GlobalCache>) {
        if let Some(previous) = &self.caches {
            for cache in &mut caches {
                cache.size = previous
                    .iter()
                    .find(|p| p.id == cache.id)
                    .and_then(|p| p.size);
            }
        }
        self.busy.extend(caches.iter().map(|c| c.id.clone()));
        self.caches = Some(caches);
        self.sort();
    }

    pub fn measured(&mut self, id: &str, size: Size) {
        self.busy.remove(id);
        if let Some(cache) = self
            .caches
            .as_mut()
            .and_then(|c| c.iter_mut().find(|c| c.id == id))
        {
            cache.size = Some(size);
        }
        self.sort();
    }

    /// Largest first; the cursor stays on the same cache.
    fn sort(&mut self) {
        let Some(caches) = &mut self.caches else {
            return;
        };
        let current = caches.get(self.cursor).map(|c| c.id.clone());
        caches.sort_by_key(|c| std::cmp::Reverse(c.size.map_or(0, |s| s.on_disk)));
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

impl App {
    /// Opening the tab discovers and measures caches once.
    pub fn caches_effects(&mut self) -> Vec<Effect> {
        if self.caches.requested {
            return Vec::new();
        }
        self.caches.requested = true;
        vec![Effect::DiscoverCaches]
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

    pub fn on_caches_action(&mut self, action: Action) -> Vec<Effect> {
        if action != Action::Clean {
            self.caches.clean_armed = false;
        }
        let len = self.caches.caches.as_ref().map_or(0, Vec::len);
        let last = len.saturating_sub(1);
        let page = self.page.max(1);
        let state = &mut self.caches;
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
                state.requested = false;
                return self.caches_effects();
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
    let title = format!(
        "Caches {} · {}{}",
        caches.len(),
        fmt::bytes(app.caches.total()),
        if app.caches.marked.is_empty() {
            String::new()
        } else {
            format!(" · {} marked", app.caches.marked.len())
        }
    );
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
        let mut lines = vec![Line::from(Span::styled(
            cache.path.display().to_string(),
            t.subtle(),
        ))];
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
    use std::path::PathBuf;

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
        }
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
        app.update(Msg::CacheMeasured("small".into(), size(10)));
        app.update(Msg::CacheMeasured("big".into(), size(1_000)));
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
