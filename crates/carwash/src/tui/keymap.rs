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
    Check,
    CheckAll,
    Update,
    Upgrade,
    ShowReclaim,
    ShowTasks,
    ShowUpdates,
    ShowCaches,
    ChangeRoot,
    Theme,
    Help,
    Quit,
}

/// The screen a binding applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Reclaim,
    Tasks,
    Updates,
    Caches,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Reclaim, Tab::Tasks, Tab::Updates, Tab::Caches];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Reclaim => "Reclaim",
            Tab::Tasks => "Tasks",
            Tab::Updates => "Updates",
            Tab::Caches => "Caches",
        }
    }

    /// One sentence on what the tab is for (help screen, tab tooltip).
    pub fn about(self) -> &'static str {
        match self {
            Tab::Reclaim => {
                "Build outputs, dependency installs and caches inside the projects under this directory."
            }
            Tab::Tasks => {
                "Scripts and targets from package.json, justfiles, Makefiles and more, run in real terminals."
            }
            Tab::Updates => {
                "Outdated and vulnerable dependencies for Rust, JavaScript, Python and Go."
            }
            Tab::Caches => {
                "Per-user caches outside your projects. Sizes are remembered for a day; r measures again."
            }
        }
    }

    fn bindings(self) -> &'static [Binding] {
        match self {
            Tab::Reclaim => RECLAIM,
            Tab::Tasks => TASKS,
            Tab::Updates => UPDATES,
            Tab::Caches => CACHES,
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
    Updates,
    General,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Self::Navigate => "Navigate",
            Self::Select => "Select & clean",
            Self::View => "View",
            Self::Jobs => "Tasks & jobs",
            Self::Updates => "Dependencies",
            Self::General => "Any tab",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Binding {
    pub keys: &'static [(KeyCode, KeyModifiers)],
    pub label: &'static str,
    pub action: Action,
    pub section: Section,
    pub description: &'static str,
    /// Short label when the binding is offered in the footer.
    pub hint: Option<&'static str>,
}

impl Binding {
    const fn hint(mut self, hint: &'static str) -> Self {
        self.hint = Some(hint);
        self
    }

    /// The first key of the label, for the footer.
    pub fn key(&self) -> &'static str {
        self.label.split(' ').next().unwrap_or(self.label)
    }

    pub fn is_global(&self) -> bool {
        self.section == Section::General
    }
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
        hint: None,
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
        &[(Char('3'), NONE)],
        "3",
        A::ShowUpdates,
        S::General,
        "dependency updates",
    ),
    bind(
        &[(Char('4'), NONE)],
        "4",
        A::ShowCaches,
        S::General,
        "global caches",
    ),
    bind(
        &[(Char('p'), NONE)],
        "p",
        A::ChangeRoot,
        S::General,
        "scan another folder (Tab completes)",
    )
    .hint("folder"),
    bind(
        &[(Char('t'), NONE)],
        "t",
        A::Theme,
        S::General,
        "next theme",
    )
    .hint("theme"),
    bind(
        &[(Char('?'), NONE), (KeyCode::F(1), NONE)],
        "?",
        A::Help,
        S::General,
        "help",
    )
    .hint("help"),
    bind(
        &[(Char('q'), NONE), (Char('c'), CTRL)],
        "q ^c",
        A::Quit,
        S::General,
        "quit",
    )
    .hint("quit"),
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
    )
    .hint("mark"),
    bind(
        &[(Char('a'), NONE)],
        "a",
        A::MarkAll,
        S::Select,
        "mark every ready artifact shown",
    )
    .hint("mark ready"),
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
    )
    .hint("clean"),
    bind(
        &[(Char('/'), NONE)],
        "/",
        A::Search,
        S::View,
        "filter: text, eco:rust kind:deps size>1g age>30d is:ready",
    )
    .hint("filter"),
    bind(
        &[(Char('s'), NONE)],
        "s",
        A::Sort,
        S::View,
        "sort by size, age, name",
    )
    .hint("sort"),
    bind(
        &[(TabKey, NONE)],
        "Tab",
        A::Group,
        S::View,
        "group as tree, projects, artifacts",
    )
    .hint("group"),
    bind(
        &[(Char('i'), NONE)],
        "i",
        A::Details,
        S::View,
        "toggle details panel",
    )
    .hint("details"),
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
    )
    .hint("pane"),
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
    )
    .hint("filter"),
    bind(
        &[(Char(' '), NONE)],
        "Space",
        A::Mark,
        S::Jobs,
        "mark project (run a task in all marked)",
    )
    .hint("mark"),
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
    )
    .hint("run"),
    bind(
        &[(Char('x'), NONE)],
        "x",
        A::KillJob,
        S::Jobs,
        "stop the shown job",
    )
    .hint("stop"),
    bind(&[(Char(']'), NONE)], "]", A::NextJob, S::Jobs, "next job").hint("next job"),
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

pub const UPDATES: &[Binding] = &[
    MOVE[0],
    MOVE[1],
    MOVE[2],
    MOVE[3],
    MOVE[4],
    MOVE[5],
    bind(
        &[
            (TabKey, NONE),
            (KeyCode::BackTab, KeyModifiers::SHIFT),
            (KeyCode::BackTab, NONE),
            (Right, NONE),
            (Left, NONE),
            (Char('l'), NONE),
            (Char('h'), NONE),
        ],
        "Tab ← →",
        A::NextPane,
        S::Navigate,
        "switch pane",
    )
    .hint("pane"),
    bind(
        &[(Char('/'), NONE)],
        "/",
        A::Search,
        S::Navigate,
        "filter projects",
    )
    .hint("filter"),
    bind(
        &[(Enter, NONE)],
        "Enter",
        A::Run,
        S::Updates,
        "check the project (or marked projects)",
    )
    .hint("check"),
    bind(
        &[(Char('c'), NONE)],
        "c",
        A::Check,
        S::Updates,
        "check again, bypassing the cache",
    ),
    bind(
        &[(Char('C'), NONE)],
        "C",
        A::CheckAll,
        S::Updates,
        "check every project listed",
    )
    .hint("check all"),
    bind(
        &[(Char(' '), NONE)],
        "Space",
        A::Mark,
        S::Updates,
        "mark project / outdated dependency",
    )
    .hint("mark"),
    bind(
        &[(Char('a'), NONE)],
        "a",
        A::MarkAll,
        S::Updates,
        "mark all (projects or outdated dependencies)",
    ),
    bind(
        &[(Char('A'), NONE)],
        "A",
        A::Unmark,
        S::Updates,
        "unmark everything",
    ),
    bind(
        &[(Char('u'), NONE)],
        "u",
        A::Update,
        S::Updates,
        "update marked within their requirements",
    )
    .hint("update"),
    bind(
        &[(Char('U'), NONE)],
        "U",
        A::Upgrade,
        S::Updates,
        "upgrade marked to latest (edits manifests; press twice)",
    )
    .hint("upgrade"),
    bind(
        &[(Char('o'), NONE)],
        "o",
        A::Open,
        S::Updates,
        "reveal project in file manager",
    ),
];

pub const CACHES: &[Binding] = &[
    MOVE[0],
    MOVE[1],
    MOVE[2],
    MOVE[3],
    MOVE[4],
    MOVE[5],
    bind(
        &[(Char(' '), NONE)],
        "Space",
        A::Mark,
        S::Select,
        "mark / unmark a cache",
    )
    .hint("mark"),
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
        "clean marked (or selected) caches; press twice",
    )
    .hint("clean"),
    bind(
        &[(Char('r'), NONE)],
        "r",
        A::Rescan,
        S::View,
        "measure every cache again",
    )
    .hint("measure"),
    bind(
        &[(Char('o'), NONE)],
        "o",
        A::Open,
        S::View,
        "reveal in file manager",
    )
    .hint("reveal"),
];

/// Bindings shown in the help screen for `tab`, in display order.
pub fn help(tab: Tab) -> impl Iterator<Item = &'static Binding> {
    tab.bindings().iter().chain(GLOBAL)
}

/// The footer: `tab`'s hinted bindings, actions before navigation, then the global ones.
pub fn footer(tab: Tab) -> (Vec<&'static Binding>, Vec<&'static Binding>) {
    let mut local: Vec<&Binding> = tab.bindings().iter().filter(|b| b.hint.is_some()).collect();
    local.sort_by_key(|b| b.section == Section::Navigate);
    let global = GLOBAL.iter().filter(|b| b.hint.is_some()).collect();
    (local, global)
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
    fn every_tab_has_a_footer_and_global_keys_are_marked_global() {
        for tab in Tab::ALL {
            let (local, global) = footer(tab);
            assert!(!local.is_empty(), "{tab:?} has no footer hints");
            assert!(local.iter().all(|b| !b.is_global()));
            assert!(global.iter().all(|b| b.is_global()));
            assert!(global.iter().any(|b| b.action == A::Help));
            assert!(global.iter().any(|b| b.action == A::Quit));
        }
        let (tasks, _) = footer(Tab::Tasks);
        assert_eq!(tasks[0].action, A::Mark, "actions come before navigation");
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
