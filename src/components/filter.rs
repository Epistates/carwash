//! Filter/search component for project discovery
//!
//! Provides live search functionality similar to vim's `/` search mode.
//! Allows users to quickly find projects by name or path.

use crate::app::AppState;
use crate::events::Action;
use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::Rect;

/// State for filter/search functionality
#[derive(Debug, Clone, Default)]
pub struct FilterState {
    /// Current filter input text
    pub input: String,
    /// Filtered indices in the flattened tree
    pub filtered_indices: Vec<usize>,
    /// Current selection within filtered results
    pub selected: usize,
}

impl FilterState {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            filtered_indices: Vec::new(),
            selected: 0,
        }
    }

    /// Update the filter text and recompute filtered indices
    pub fn update_input(&mut self, input: String, app: &AppState) {
        self.input = input;
        self.selected = 0;
        self.recompute_matches(app);
    }

    /// Recompute which indices match the current filter
    pub fn recompute_matches(&mut self, app: &AppState) {
        self.filtered_indices.clear();

        if self.input.is_empty() {
            // If filter is empty, show all items
            self.filtered_indices = (0..app.flattened_tree.items.len()).collect();
        } else {
            let input_lower = self.input.to_lowercase();

            for (idx, (node, _)) in app.flattened_tree.items.iter().enumerate() {
                let matches = match &node.node_type {
                    crate::tree::TreeNodeType::Directory { name, .. } => {
                        name.to_lowercase().contains(&input_lower)
                    }
                    crate::tree::TreeNodeType::Project(project) => {
                        project.name.to_lowercase().contains(&input_lower)
                            || project
                                .path
                                .to_string_lossy()
                                .to_lowercase()
                                .contains(&input_lower)
                    }
                };

                if matches {
                    self.filtered_indices.push(idx);
                }
            }
        }

        // Clamp selection
        if self.selected >= self.filtered_indices.len() {
            self.selected = self.filtered_indices.len().saturating_sub(1);
        }
    }

    /// Get the currently selected index in the full tree
    pub fn selected_tree_index(&self) -> Option<usize> {
        self.filtered_indices.get(self.selected).copied()
    }

    /// Move selection to next match
    pub fn select_next(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.selected = (self.selected + 1) % self.filtered_indices.len();
        }
    }

    /// Move selection to previous match
    pub fn select_previous(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.selected = if self.selected == 0 {
                self.filtered_indices.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    /// Clear the filter input
    pub fn clear(&mut self) {
        self.input.clear();
        self.filtered_indices.clear();
        self.selected = 0;
    }
}

pub struct FilterComponent;

impl FilterComponent {
    pub fn new() -> Self {
        Self
    }

    pub fn handle_key_events(&mut self, key: KeyCode, _state: &mut AppState) -> Option<Action> {
        match key {
            KeyCode::Esc => Some(Action::ExitFilterMode),
            KeyCode::Enter => Some(Action::ExitFilterMode),
            _ => None,
        }
    }

    pub fn draw(&mut self, _f: &mut Frame, _state: &mut AppState, _area: Rect) {
        // Filter is rendered as a status line or modal overlay
        // The actual rendering is handled in the status bar component
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_state_new() {
        let filter = FilterState::new();
        assert!(filter.input.is_empty());
        assert!(filter.filtered_indices.is_empty());
        assert_eq!(filter.selected, 0);
    }

    #[test]
    fn test_filter_clear() {
        let mut filter = FilterState::new();
        filter.input = "test".to_string();
        filter.filtered_indices = vec![1, 2, 3];
        filter.selected = 2;

        filter.clear();

        assert!(filter.input.is_empty());
        assert!(filter.filtered_indices.is_empty());
        assert_eq!(filter.selected, 0);
    }

    #[test]
    fn test_filter_select_next() {
        let mut filter = FilterState::new();
        filter.filtered_indices = vec![0, 2, 5];
        filter.selected = 0;

        filter.select_next();
        assert_eq!(filter.selected, 1);

        filter.select_next();
        assert_eq!(filter.selected, 2);

        filter.select_next(); // wraps around
        assert_eq!(filter.selected, 0);
    }

    #[test]
    fn test_filter_select_previous() {
        let mut filter = FilterState::new();
        filter.filtered_indices = vec![0, 2, 5];
        filter.selected = 0;

        filter.select_previous();
        assert_eq!(filter.selected, 2);

        filter.select_previous();
        assert_eq!(filter.selected, 1);
    }
}
