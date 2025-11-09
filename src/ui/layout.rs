//! Layout utilities and helpers for responsive UI design
//!
//! Provides layout calculation helpers and modern constraint-based layout patterns.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Layout preferences and responsive sizing
#[derive(Debug, Clone)]
pub struct LayoutPreferences {
    /// Left pane width percentage (project list)
    pub left_pane_percent: u16,
    /// Right pane width percentage (dependencies + output)
    pub right_pane_percent: u16,
    /// Top right pane height percentage (dependencies)
    pub top_right_percent: u16,
    /// Bottom right pane height percentage (output)
    pub bottom_right_percent: u16,
}

impl Default for LayoutPreferences {
    fn default() -> Self {
        Self {
            left_pane_percent: 40,
            right_pane_percent: 60,
            top_right_percent: 40,
            bottom_right_percent: 60,
        }
    }
}

impl LayoutPreferences {
    /// Create layout preferences with custom percentages
    pub fn new(left: u16, right: u16, top_right: u16, bottom_right: u16) -> Self {
        Self {
            left_pane_percent: left,
            right_pane_percent: right,
            top_right_percent: top_right,
            bottom_right_percent: bottom_right,
        }
    }

    /// Adjust left pane width
    pub fn adjust_left_pane(&mut self, percent: u16) {
        let percent = percent.max(20).min(80); // Constrain to reasonable range
        self.left_pane_percent = percent;
        self.right_pane_percent = 100 - percent;
    }

    /// Adjust top-right pane height
    pub fn adjust_top_right(&mut self, percent: u16) {
        let percent = percent.max(20).min(80);
        self.top_right_percent = percent;
        self.bottom_right_percent = 100 - percent;
    }
}

/// Helper to create a centered rectangle for modals
pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Advanced layout system with Flex support
pub struct ResponsiveLayout;

impl ResponsiveLayout {
    /// Create main vertical layout with flexible content and fixed status bar
    pub fn main_layout(area: Rect) -> [Rect; 2] {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);
        [chunks[0], chunks[1]]
    }

    /// Create horizontal split for left/right panes
    pub fn horizontal_split(area: Rect, left_percent: u16) -> [Rect; 2] {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(left_percent),
                Constraint::Percentage(100 - left_percent),
            ])
            .split(area);
        [chunks[0], chunks[1]]
    }

    /// Create vertical split for top/bottom panes
    pub fn vertical_split(area: Rect, top_percent: u16) -> [Rect; 2] {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(top_percent),
                Constraint::Percentage(100 - top_percent),
            ])
            .split(area);
        [chunks[0], chunks[1]]
    }

    /// Create three-column layout with flexible sizing
    pub fn three_column_layout(
        area: Rect,
        left_percent: u16,
        middle_percent: u16,
        right_percent: u16,
    ) -> [Rect; 3] {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(left_percent),
                Constraint::Percentage(middle_percent),
                Constraint::Percentage(right_percent),
            ])
            .split(area);
        [chunks[0], chunks[1], chunks[2]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_preferences_default() {
        let prefs = LayoutPreferences::default();
        assert_eq!(prefs.left_pane_percent, 40);
        assert_eq!(prefs.right_pane_percent, 60);
    }

    #[test]
    fn test_layout_preferences_adjust_left() {
        let mut prefs = LayoutPreferences::default();
        prefs.adjust_left_pane(50);
        assert_eq!(prefs.left_pane_percent, 50);
        assert_eq!(prefs.right_pane_percent, 50);
    }

    #[test]
    fn test_layout_preferences_clamp() {
        let mut prefs = LayoutPreferences::default();
        prefs.adjust_left_pane(95); // Should clamp to 80
        assert_eq!(prefs.left_pane_percent, 80);
    }

    #[test]
    fn test_centered_rect() {
        let area = Rect::new(0, 0, 100, 100);
        let centered = centered_rect(50, 50, area);
        assert!(centered.width < area.width);
        assert!(centered.height < area.height);
    }
}
