//! Key bindings: one table drives both dispatch and the help screen.

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
    Theme,
    Help,
    Quit,
}

/// Where a binding is listed in the help screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Navigate,
    Select,
    View,
    General,
}

impl Section {
    pub const ALL: [Section; 4] = [Self::Navigate, Self::Select, Self::View, Self::General];

    pub fn title(self) -> &'static str {
        match self {
            Self::Navigate => "Navigate",
            Self::Select => "Select & clean",
            Self::View => "View",
            Self::General => "General",
        }
    }
}

pub struct Binding {
    pub keys: &'static [(KeyCode, KeyModifiers)],
    pub label: &'static str,
    pub action: Action,
    pub section: Section,
    pub description: &'static str,
}

const NONE: KeyModifiers = KeyModifiers::NONE;
const CTRL: KeyModifiers = KeyModifiers::CONTROL;

pub const BINDINGS: &[Binding] = &[
    Binding {
        keys: &[(KeyCode::Up, NONE), (KeyCode::Char('k'), NONE)],
        label: "↑ k",
        action: Action::Up,
        section: Section::Navigate,
        description: "previous row",
    },
    Binding {
        keys: &[(KeyCode::Down, NONE), (KeyCode::Char('j'), NONE)],
        label: "↓ j",
        action: Action::Down,
        section: Section::Navigate,
        description: "next row",
    },
    Binding {
        keys: &[(KeyCode::PageUp, NONE), (KeyCode::Char('u'), CTRL)],
        label: "PgUp ^u",
        action: Action::PageUp,
        section: Section::Navigate,
        description: "page up",
    },
    Binding {
        keys: &[(KeyCode::PageDown, NONE), (KeyCode::Char('d'), CTRL)],
        label: "PgDn ^d",
        action: Action::PageDown,
        section: Section::Navigate,
        description: "page down",
    },
    Binding {
        keys: &[(KeyCode::Home, NONE), (KeyCode::Char('g'), NONE)],
        label: "Home g",
        action: Action::Top,
        section: Section::Navigate,
        description: "first row",
    },
    Binding {
        keys: &[(KeyCode::End, NONE), (KeyCode::Char('G'), NONE)],
        label: "End G",
        action: Action::Bottom,
        section: Section::Navigate,
        description: "last row",
    },
    Binding {
        keys: &[(KeyCode::Right, NONE), (KeyCode::Char('l'), NONE)],
        label: "→ l",
        action: Action::Expand,
        section: Section::Navigate,
        description: "expand",
    },
    Binding {
        keys: &[(KeyCode::Left, NONE), (KeyCode::Char('h'), NONE)],
        label: "← h",
        action: Action::Collapse,
        section: Section::Navigate,
        description: "collapse / go to parent",
    },
    Binding {
        keys: &[(KeyCode::Enter, NONE)],
        label: "Enter",
        action: Action::ToggleExpand,
        section: Section::Navigate,
        description: "toggle expand",
    },
    Binding {
        keys: &[(KeyCode::Char('E'), NONE)],
        label: "E",
        action: Action::ExpandAll,
        section: Section::Navigate,
        description: "expand all",
    },
    Binding {
        keys: &[(KeyCode::Char('C'), NONE)],
        label: "C",
        action: Action::CollapseAll,
        section: Section::Navigate,
        description: "collapse all",
    },
    Binding {
        keys: &[(KeyCode::Char(' '), NONE)],
        label: "Space",
        action: Action::Mark,
        section: Section::Select,
        description: "mark / unmark (directories mark their ready artifacts)",
    },
    Binding {
        keys: &[(KeyCode::Char('a'), NONE)],
        label: "a",
        action: Action::MarkAll,
        section: Section::Select,
        description: "mark every ready artifact shown",
    },
    Binding {
        keys: &[(KeyCode::Char('A'), NONE)],
        label: "A",
        action: Action::Unmark,
        section: Section::Select,
        description: "unmark everything",
    },
    Binding {
        keys: &[
            (KeyCode::Char('d'), NONE),
            (KeyCode::Char('x'), NONE),
            (KeyCode::Delete, NONE),
        ],
        label: "d x",
        action: Action::Clean,
        section: Section::Select,
        description: "review and clean marked artifacts",
    },
    Binding {
        keys: &[(KeyCode::Char('/'), NONE)],
        label: "/",
        action: Action::Search,
        section: Section::View,
        description: "filter: text, eco:rust kind:deps size>1g age>30d is:ready",
    },
    Binding {
        keys: &[(KeyCode::Char('s'), NONE)],
        label: "s",
        action: Action::Sort,
        section: Section::View,
        description: "sort by size, age, name",
    },
    Binding {
        keys: &[(KeyCode::Tab, NONE)],
        label: "Tab",
        action: Action::Group,
        section: Section::View,
        description: "group as tree, projects, artifacts",
    },
    Binding {
        keys: &[(KeyCode::Char('i'), NONE)],
        label: "i",
        action: Action::Details,
        section: Section::View,
        description: "toggle details panel",
    },
    Binding {
        keys: &[(KeyCode::Char('t'), NONE)],
        label: "t",
        action: Action::Theme,
        section: Section::View,
        description: "next theme",
    },
    Binding {
        keys: &[(KeyCode::Char('r'), NONE)],
        label: "r",
        action: Action::Rescan,
        section: Section::General,
        description: "rescan",
    },
    Binding {
        keys: &[(KeyCode::Char('o'), NONE)],
        label: "o",
        action: Action::Open,
        section: Section::General,
        description: "reveal in file manager",
    },
    Binding {
        keys: &[(KeyCode::Char('?'), NONE), (KeyCode::F(1), NONE)],
        label: "?",
        action: Action::Help,
        section: Section::General,
        description: "help",
    },
    Binding {
        keys: &[(KeyCode::Char('q'), NONE), (KeyCode::Char('c'), CTRL)],
        label: "q ^c",
        action: Action::Quit,
        section: Section::General,
        description: "quit",
    },
];

pub fn action_for(key: &KeyEvent) -> Option<Action> {
    // Terminals disagree on whether shifted characters carry SHIFT; ignore it for chars.
    let modifiers = match key.code {
        KeyCode::Char(_) => key.modifiers - KeyModifiers::SHIFT,
        _ => key.modifiers,
    };
    BINDINGS.iter().find_map(|binding| {
        binding
            .keys
            .iter()
            .any(|&(code, mods)| {
                let mods = match code {
                    KeyCode::Char(_) => mods - KeyModifiers::SHIFT,
                    _ => mods,
                };
                code == key.code && mods == modifiers
            })
            .then_some(binding.action)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn dispatch() {
        assert_eq!(
            action_for(&key(KeyCode::Char('j'), NONE)),
            Some(Action::Down)
        );
        assert_eq!(
            action_for(&key(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            Some(Action::Bottom)
        );
        assert_eq!(
            action_for(&key(KeyCode::Char('G'), NONE)),
            Some(Action::Bottom)
        );
        assert_eq!(
            action_for(&key(KeyCode::Char('c'), CTRL)),
            Some(Action::Quit)
        );
        assert_eq!(
            action_for(&key(KeyCode::Char('d'), CTRL)),
            Some(Action::PageDown)
        );
        assert_eq!(
            action_for(&key(KeyCode::Char('d'), NONE)),
            Some(Action::Clean)
        );
        assert_eq!(action_for(&key(KeyCode::Char('z'), NONE)), None);
    }

    #[test]
    fn no_key_is_bound_twice() {
        let mut seen = std::collections::HashSet::new();
        for binding in BINDINGS {
            for &(code, mods) in binding.keys {
                let mods = match code {
                    KeyCode::Char(_) => mods - KeyModifiers::SHIFT,
                    _ => mods,
                };
                assert!(seen.insert((code, mods)), "{code:?} {mods:?} bound twice");
            }
        }
    }
}
