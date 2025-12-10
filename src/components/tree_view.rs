//! Tree view component for hierarchical project display
//!
//! This component renders the project tree with support for expanding/collapsing directories,
//! selecting projects, and showing project status indicators.

use crate::app::AppState;
use crate::components::Component;
use crate::events::Action;
use crate::tree::TreeNodeType;
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
};

pub struct TreeView {}

impl TreeView {
    pub fn new() -> Self {
        Self {}
    }

    /// Get the display name for a node with proper indentation and expand/collapse indicators
    /// Returns: (text, is_loading) tuple
    fn format_tree_line(state: &AppState, node_index: usize) -> (String, bool) {
        if node_index >= state.flattened_tree.items.len() {
            return (String::new(), false);
        }

        let (node, _) = &state.flattened_tree.items[node_index];
        let indent = "  ".repeat(node.depth);

        match &node.node_type {
            TreeNodeType::Directory { name, .. } => {
                let indicator = if node.loading {
                    // Show spinner when loading
                    "⠹".to_string()
                } else if node.expanded {
                    "▾".to_string()
                } else {
                    "▸".to_string()
                };
                (format!("{}{} {}", indent, indicator, name), node.loading)
            }
            TreeNodeType::Project(_) => (format!("{}  • {}", indent, node.node_type.name()), false),
        }
    }

    /// Get the style for a node based on its type and selection state
    fn get_node_style(state: &AppState, node_index: usize, is_selected: bool) -> Style {
        if node_index >= state.flattened_tree.items.len() {
            return Style::default();
        }

        let (node, _) = &state.flattened_tree.items[node_index];

        match &node.node_type {
            TreeNodeType::Directory { .. } => {
                // Loading or selected directories get cyan, others get muted purple
                if node.loading || is_selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::Rgb(150, 150, 200))
                        .add_modifier(Modifier::BOLD)
                }
            }
            TreeNodeType::Project(project) => {
                use crate::project::ProjectCheckStatus;

                let base_style = if is_selected {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                // Add status color based on project check status
                match project.check_status {
                    ProjectCheckStatus::Unchecked => base_style.fg(Color::DarkGray),
                    ProjectCheckStatus::Checking => {
                        base_style.fg(Color::Blue).add_modifier(Modifier::BOLD)
                    }
                    ProjectCheckStatus::HasUpdates => {
                        base_style.fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    }
                    ProjectCheckStatus::UpToDate => base_style.fg(Color::Green),
                }
            }
        }
    }

    /// Get the status indicator for a project
    fn get_project_status_indicator(project: &crate::project::Project) -> &'static str {
        use crate::project::ProjectCheckStatus;

        match project.check_status {
            ProjectCheckStatus::Unchecked => "⋯",
            ProjectCheckStatus::Checking => "⟳",
            ProjectCheckStatus::HasUpdates => "⚠",
            ProjectCheckStatus::UpToDate => "✓",
        }
    }
}

impl Component for TreeView {
    fn handle_key_events(&mut self, key: KeyCode, _state: &mut AppState) -> Option<Action> {
        match key {
            KeyCode::Up => Some(Action::SelectPrevious),
            KeyCode::Down => Some(Action::SelectNext),
            KeyCode::Right => Some(Action::SelectChild),
            KeyCode::Left => Some(Action::SelectParent),
            KeyCode::Char(' ') => Some(Action::ToggleSelection),
            _ => None,
        }
    }

    fn draw(&mut self, f: &mut Frame, state: &mut AppState, area: Rect) {
        // Create the list of items
        let items: Vec<ListItem> = state
            .flattened_tree
            .items
            .iter()
            .enumerate()
            .map(|(idx, (node, _))| {
                let is_selected = state.tree_selection.selected_index == Some(idx);
                let style = Self::get_node_style(state, idx, is_selected);
                let (line_text, _is_loading) = Self::format_tree_line(state, idx);

                // For projects, add status indicator
                let line = match &node.node_type {
                    TreeNodeType::Project(project) => {
                        let status_indicator = Self::get_project_status_indicator(project);
                        let indicator_span =
                            Span::styled(status_indicator, style.add_modifier(Modifier::BOLD));

                        // Extract just the project name part (skip indent and dots)
                        let project_part = format!("{}", project.name);
                        let text_span = Span::styled(project_part, style);

                        let mut line_spans = vec![
                            Span::raw("  ".to_string()),
                            indicator_span,
                            Span::raw(" ".to_string()),
                            text_span,
                        ];

                        // Add selection indicator
                        if is_selected {
                            line_spans.insert(0, Span::styled("▶ ", style));
                        }

                        Line::from(line_spans)
                    }
                    TreeNodeType::Directory { .. } => {
                        let mut line_spans = vec![Span::styled(line_text, style)];

                        // Add selection indicator
                        if is_selected {
                            line_spans.insert(0, Span::styled("▶ ", style));
                        }

                        Line::from(line_spans)
                    }
                };

                ListItem::new(line)
            })
            .collect();

        // Create the list widget
        let list = List::new(items)
            .block(
                Block::default()
                    .title(" Projects ")
                    .borders(Borders::ALL)
                    .border_type(ratatui::widgets::BorderType::Rounded),
            )
            .style(Style::default().fg(Color::White));

        // Render the list
        f.render_widget(list, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_view_creation() {
        let _tree_view = TreeView::new();
        // TreeView is a unit struct, just verify it can be instantiated
    }
}
