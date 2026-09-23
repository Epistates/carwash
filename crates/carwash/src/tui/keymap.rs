//! Key bindings: tables drive both dispatch and the help screen.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Expand,
    Collapse,
    ToggleExpand,
    ExpandAll,
    CollapseAll,
    Mark,
    MarkAll,
    Unmark,
    Clean,
    Search,
    Sort,
    Group,
    Details,
    Rescan,
    Open,
    NextPane,
    PreviousPane,
    Run,
    KillJob,
    NextJob,
    PreviousJob,
    ClearJobs,
    ShowReclaim,
    ShowTasks,
    Theme,
    Help,
    Quit,
}

/// The screen a binding applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Reclaim,
    Tasks,
}

impl Tab {
    pub const ALL: [Tab; 2] = [Tab::Reclaim, Tab::Tasks];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Reclaim => "Reclaim",
            Tab::Tasks => "Tasks",
        }
    }

    fn bindings(self) -> &'static [Binding] {
        match self {
            Tab::Reclaim => RECLAIM,
            Tab::Tasks => TASKS,
        }
    }
}

/// Where a binding is listed in the help screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Navigate,
    Select,
    View,
    Jobs,
    General,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Self::Navigate => "Navigate",
            Self::Select => "Select & clean",
            Self::View => "View",
            Self::Jobs => "Tasks & jobs",
            Self::General => "General",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Binding {
    pub keys: &'static [(KeyCode, KeyModifiers)],
    pub label: &'static str,
    pub action: Action,
    pub section: Section,
    pub description: &'static str,
}

const NONE: KeyModifiers = KeyModifiers::NONE;
const CTRL: KeyModifiers = KeyModifiers::CONTROL;

const fn bind(
    keys: &'static [(KeyCode, KeyModifiers)],
    label: &'static str,
    action: Action,
    section: Section,
    description: &'static str,
) -> Binding {
    Binding {
        keys,
        label,
        action,
        section,
        description,
    }
}

use Action as A;
use KeyCode::{Char, Down, End, Enter, Home, Left, PageDown, PageUp, Right, Tab as TabKey, Up};
use Section as S;

const MOVE: [Binding; 6] = [
    bind(
        &[(Up, NONE), (Char('k'), NONE)],
        "↑ k",
        A::Up,
        S::Navigate,
        "previous row",
    ),
    bind(
        &[(Down, NONE), (Char('j'), NONE)],
        "↓ j",
        A::Down,
        S::Navigate,
        "next row",
    ),
    bind(
        &[(PageUp, NONE), (Char('u'), CTRL)],
        "PgUp ^u",
        A::PageUp,
        S::Navigate,
        "page up",
    ),
    bind(
        &[(PageDown, NONE), (Char('d'), CTRL)],
        "PgDn ^d",
        A::PageDown,
        S::Navigate,
        "page down",
    ),
    bind(
        &[(Home, NONE), (Char('g'), NONE)],
        "Home g",
        A::Top,
        S::Navigate,
        "first row",
    ),
    bind(
        &[(End, NONE), (Char('G'), NONE)],
        "End G",
        A::Bottom,
        S::Navigate,
        "last row",
    ),
];

pub const GLOBAL: &[Binding] = &[
    bind(
        &[(Char('1'), NONE)],
        "1",
        A::ShowReclaim,
        S::General,
        "reclaim space",
    ),
    bind(&[(Char('2'), NONE)], "2", A::ShowTasks, S::General, "tasks"),
    bind(
        &[(Char('t'), NONE)],
        "t",
        A::Theme,
        S::General,
        "next theme",
    ),
    bind(
        &[(Char('?'), NONE), (KeyCode::F(1), NONE)],
        "?",
        A::Help,
        S::General,
        "help",
    ),
    bind(
        &[(Char('q'), NONE), (Char('c'), CTRL)],
        "q ^c",
        A::Quit,
        S::General,
        "quit",
    ),
];

pub const RECLAIM: &[Binding] = &[
    MOVE[0],
    MOVE[1],
    MOVE[2],
    MOVE[3],
    MOVE[4],
    MOVE[5],
    bind(
        &[(Right, NONE), (Char('l'), NONE)],
        "→ l",
        A::Expand,
        S::Navigate,
        "expand",
    ),
    bind(
        &[(Left, NONE), (Char('h'), NONE)],
        "← h",
        A::Collapse,
        S::Navigate,
        "collapse / go to parent",
    ),
    bind(
        &[(Enter, NONE)],
        "Enter",
        A::ToggleExpand,
        S::Navigate,
        "toggle expand",
    ),
    bind(
        &[(Char('E'), NONE)],
        "E",
        A::ExpandAll,
        S::Navigate,
        "expand all",
    ),
    bind(
        &[(Char('C'), NONE)],
        "C",
        A::CollapseAll,
        S::Navigate,
        "collapse all",
    ),
    bind(
        &[(Char(' '), NONE)],
        "Space",
        A::Mark,
        S::Select,
        "mark / unmark (directories mark their ready artifacts)",
    ),
    bind(
        &[(Char('a'), NONE)],
        "a",
        A::MarkAll,
        S::Select,
        "mark every ready artifact shown",
    ),
    bind(
        &[(Char('A'), NONE)],
        "A",
        A::Unmark,
        S::Select,
        "unmark everything",
    ),
    bind(
        &[
            (Char('d'), NONE),
            (Char('x'), NONE),
            (KeyCode::Delete, NONE),
        ],
        "d x",
        A::Clean,
        S::Select,
        "review and clean marked artifacts",
    ),
    bind(
        &[(Char('/'), NONE)],
        "/",
        A::Search,
        S::View,
        "filter: text, eco:rust kind:deps size>1g age>30d is:ready",
    ),
    bind(
        &[(Char('s'), NONE)],
        "s",
        A::Sort,
        S::View,
        "sort by size, age, name",
    ),
    bind(
        &[(TabKey, NONE)],
        "Tab",
        A::Group,
        S::View,
        "group as tree, projects, artifacts",
    ),
    bind(
        &[(Char('i'), NONE)],
        "i",
        A::Details,
        S::View,
        "toggle details panel",
    ),
    bind(&[(Char('r'), NONE)], "r", A::Rescan, S::View, "rescan"),
    bind(
        &[(Char('o'), NONE)],
        "o",
        A::Open,
        S::View,
        "reveal in file manager",
    ),
];

pub const TASKS: &[Binding] = &[
    MOVE[0],
    MOVE[1],
    MOVE[2],
    MOVE[3],
    MOVE[4],
    MOVE[5],
    bind(
        &[(TabKey, NONE), (Right, NONE), (Char('l'), NONE)],
        "Tab → l",
        A::NextPane,
        S::Navigate,
        "next pane",
    ),
    bind(
        &[
            (KeyCode::BackTab, KeyModifiers::SHIFT),
            (KeyCode::BackTab, NONE),
            (Left, NONE),
            (Char('h'), NONE),
        ],
        "⇧Tab ← h",
        A::PreviousPane,
        S::Navigate,
        "previous pane",
    ),
    bind(
        &[(Char('/'), NONE)],
        "/",
        A::Search,
        S::Navigate,
        "filter projects",
    ),
    bind(
        &[(Char(' '), NONE)],
        "Space",
        A::Mark,
        S::Jobs,
        "mark project (run a task in all marked)",
    ),
    bind(
        &[(Char('A'), NONE)],
        "A",
        A::Unmark,
        S::Jobs,
        "unmark all projects",
    ),
    bind(
        &[(Enter, NONE)],
        "Enter",
        A::Run,
        S::Jobs,
        "run the selected task",
    ),
    bind(
        &[(Char('x'), NONE)],
        "x",
        A::KillJob,
        S::Jobs,
        "stop the shown job",
    ),
    bind(&[(Char(']'), NONE)], "]", A::NextJob, S::Jobs, "next job"),
    bind(
        &[(Char('['), NONE)],
        "[",
        A::PreviousJob,
        S::Jobs,
        "previous job",
    ),
    bind(
        &[(Char('c'), NONE)],
        "c",
        A::ClearJobs,
        S::Jobs,
        "clear finished jobs",
    ),
    bind(
        &[(Char('o'), NONE)],
        "o",
        A::Open,
        S::Jobs,
        "reveal project in file manager",
    ),
];

/// Bindings shown in the help screen for `tab`, in display order.
pub fn help(tab: Tab) -> impl Iterator<Item = &'static Binding> {
    tab.bindings().iter().chain(GLOBAL)
}

fn normalize(code: KeyCode, modifiers: KeyModifiers) -> (KeyCode, KeyModifiers) {
    // Terminals disagree on whether shifted characters carry SHIFT; ignore it for chars.
    match code {
        Char(_) => (code, modifiers - KeyModifiers::SHIFT),
        _ => (code, modifiers),
    }
}

pub fn action_for(tab: Tab, key: &KeyEvent) -> Option<Action> {
    let pressed = normalize(key.code, key.modifiers);
    tab.bindings()
        .iter()
        .chain(GLOBAL)
        .find(|b| b.keys.iter().any(|&(c, m)| normalize(c, m) == pressed))
        .map(|b| b.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn dispatch() {
        let r = Tab::Reclaim;
        assert_eq!(action_for(r, &key(Char('j'), NONE)), Some(A::Down));
        assert_eq!(
            action_for(r, &key(Char('G'), KeyModifiers::SHIFT)),
            Some(A::Bottom)
        );
        assert_eq!(action_for(r, &key(Char('c'), CTRL)), Some(A::Quit));
        assert_eq!(action_for(r, &key(Char('d'), CTRL)), Some(A::PageDown));
        assert_eq!(action_for(r, &key(Char('d'), NONE)), Some(A::Clean));
        assert_eq!(action_for(r, &key(TabKey, NONE)), Some(A::Group));
        assert_eq!(
            action_for(Tab::Tasks, &key(TabKey, NONE)),
            Some(A::NextPane)
        );
        assert_eq!(
            action_for(Tab::Tasks, &key(Char('2'), NONE)),
            Some(A::ShowTasks)
        );
        assert_eq!(action_for(r, &key(Char('z'), NONE)), None);
    }

    #[test]
    fn no_key_is_bound_twice_in_a_tab() {
        for tab in Tab::ALL {
            let mut seen = std::collections::HashSet::new();
            for binding in help(tab) {
                for &(code, mods) in binding.keys {
                    assert!(
                        seen.insert(normalize(code, mods)),
                        "{code:?} {mods:?} bound twice in {tab:?}"
                    );
                }
            }
        }
    }
}
