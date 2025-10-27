use crate::app::AppState;
use crate::components::Component;
use crate::events::Action;
use crate::project::Dependency;
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use std::collections::HashSet;

/// State for the update wizard UI component
///
/// The wizard is "locked" to a specific project when it opens, ensuring that
/// background update checks for other projects don't interfere with the display.
#[derive(Debug, Clone)]
pub struct UpdateWizardState {
    pub outdated_dependencies: Vec<Dependency>,
    pub selected_dependencies: HashSet<String>,
    pub list_state: ratatui::widgets::ListState,
    /// The project this wizard is locked to (prevents background updates from changing display)
    pub locked_project_name: Option<String>,
    /// Whether a user-initiated check is in progress for the locked project
    /// This prevents background checks from clearing the is_checking_updates flag prematurely
    pub user_check_in_progress: bool,
}

impl UpdateWizardState {
    pub fn new() -> Self {
        Self {
            outdated_dependencies: Vec::new(),
            selected_dependencies: HashSet::new(),
            list_state: ratatui::widgets::ListState::default(),
            locked_project_name: None,
            user_check_in_progress: false,
        }
    }
}

pub struct UpdateWizard {}

impl UpdateWizard {
    pub fn new() -> Self {
        Self {}
    }
}

impl Component for UpdateWizard {
    fn handle_key_events(&mut self, key: KeyCode, app: &mut AppState) -> Option<Action> {
        match key {
            KeyCode::Esc => {
                // Cancel any ongoing update check
                app.is_checking_updates = false;
                Some(Action::EnterNormalMode)
            }
            KeyCode::Char(' ') => Some(Action::ToggleUpdateSelection),
            KeyCode::Enter => {
                if !app.updater.selected_dependencies.is_empty() {
                    Some(Action::RunUpdate)
                } else {
                    None
                }
            }
            KeyCode::Char('a') => {
                // Select all
                for dep in &app.updater.outdated_dependencies {
                    app.updater.selected_dependencies.insert(dep.name.clone());
                }
                None
            }
            KeyCode::Char('n') => {
                // Select none
                app.updater.selected_dependencies.clear();
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = match app.updater.list_state.selected() {
                    Some(i) => {
                        if i >= app.updater.outdated_dependencies.len() - 1 {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                app.updater.list_state.select(Some(i));
                None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let i = match app.updater.list_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            app.updater.outdated_dependencies.len() - 1
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                app.updater.list_state.select(Some(i));
                None
            }
            _ => None,
        }
    }

    fn draw(&mut self, f: &mut Frame, app: &mut AppState, area: Rect) {
        let popup_area = Self::centered_rect(70, 70, area);

        f.render_widget(Clear, popup_area);

        let chunks = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(4),
            ])
            .split(popup_area);

        // Title - show the locked project name and checking status
        let title_text = if let Some(ref project_name) = app.updater.locked_project_name {
            if app.is_checking_updates {
                format!(" Update Dependencies - {} ⟳ ", project_name)
            } else {
                format!(" Update Dependencies - {} ", project_name)
            }
        } else if app.is_checking_updates {
            " Update Dependencies ⟳ ".to_string()
        } else {
            " Update Dependencies ".to_string()
        };

        let title = Block::default()
            .title(title_text)
            .title_alignment(Alignment::Center)
            .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
            .border_style(
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            );
        f.render_widget(title, chunks[0]);

        // Dependency list
        if app.updater.outdated_dependencies.is_empty() {
            // Check if dependencies have been checked (have latest_version set)
            // Use the LOCKED project, not the currently selected one (user may have moved cursor)
            let has_been_checked = if let Some(ref locked_name) = app.updater.locked_project_name {
                app.projects
                    .iter()
                    .find(|p| &p.name == locked_name)
                    .map(|project| {
                        !project.dependencies.is_empty()
                            && project
                                .dependencies
                                .iter()
                                .any(|d| d.latest_version.is_some())
                    })
                    .unwrap_or(false)
            } else {
                false
            };

            let empty_msg = if app.is_checking_updates {
                " ⟳ Checking for updates...\n\n Please wait or press Esc to cancel "
            } else if has_been_checked {
                " ✓ All dependencies are up to date!\n\n Press Esc to close "
            } else {
                " ⟳ Initializing...\n\n Press Esc to cancel "
            };

            let empty_para = Paragraph::new(empty_msg)
                .alignment(Alignment::Center)
                .style(
                    Style::default().fg(if app.is_checking_updates || !has_been_checked {
                        Color::Yellow
                    } else {
                        Color::Green
                    }),
                )
                .block(Block::default().borders(Borders::LEFT | Borders::RIGHT));
            f.render_widget(empty_para, chunks[1]);
        } else {
            let items: Vec<ListItem> = app
                .updater
                .outdated_dependencies
                .iter()
                .map(|dep| {
                    let is_selected = app.updater.selected_dependencies.contains(&dep.name);
                    let checkbox = if is_selected { "☑" } else { "☐" };

                    let line = Line::from(vec![
                        Span::styled(
                            checkbox,
                            if is_selected {
                                Style::default().fg(Color::Green)
                            } else {
                                Style::default().fg(Color::DarkGray)
                            },
                        ),
                        Span::raw("  "),
                        Span::styled(
                            &dep.name,
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(&dep.current_version, Style::default().fg(Color::Red)),
                        Span::styled(" → ", Style::default().fg(Color::Yellow)),
                        Span::styled(
                            dep.latest_version.as_ref().unwrap(),
                            Style::default()
                                .fg(Color::Green)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]);

                    ListItem::new(line)
                })
                .collect();

            let list = List::new(items)
                .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
                .highlight_style(
                    Style::default()
                        .bg(Color::Rgb(60, 40, 60))
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("▶ ");

            f.render_stateful_widget(list, chunks[1], &mut app.updater.list_state);
        }

        // Help footer
        let selected_count = app.updater.selected_dependencies.len();
        let total_count = app.updater.outdated_dependencies.len();

        let help_lines = vec![
            Line::from(vec![
                Span::styled(" Space", Style::default().fg(Color::Cyan)),
                Span::raw(": Toggle | "),
                Span::styled("a", Style::default().fg(Color::Cyan)),
                Span::raw(": All | "),
                Span::styled("n", Style::default().fg(Color::Cyan)),
                Span::raw(": None | "),
                Span::styled(
                    "Enter",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(": Update | "),
                Span::styled("Esc", Style::default().fg(Color::Red)),
                Span::raw(": Cancel "),
            ]),
            Line::from(""),
            Line::from(vec![Span::styled(
                format!(" {} of {} selected ", selected_count, total_count),
                if selected_count > 0 {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            )]),
        ];

        let footer = Paragraph::new(help_lines)
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Magenta)),
            );
        f.render_widget(footer, chunks[2]);
    }
}

impl UpdateWizard {
    fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
        let popup_layout = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ])
            .split(r);

        Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints([
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ])
            .split(popup_layout[1])[1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_wizard_state_new() {
        let state = UpdateWizardState::new();
        assert!(state.outdated_dependencies.is_empty());
        assert!(state.selected_dependencies.is_empty());
    }

    #[test]
    fn test_update_wizard_state_clone() {
        let mut state = UpdateWizardState::new();
        state.selected_dependencies.insert("serde".to_string());

        let cloned = state.clone();
        assert_eq!(
            state.selected_dependencies.len(),
            cloned.selected_dependencies.len()
        );
        assert!(cloned.selected_dependencies.contains("serde"));
    }
}
