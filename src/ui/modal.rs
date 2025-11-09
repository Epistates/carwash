//! Base modal component with common patterns
//!
//! Provides shared modal rendering utilities and patterns used across
//! command palette, help, settings, and other modals.

use crate::ui::layout::centered_rect;
use crate::ui::styles::Colors;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Common modal styling and rendering utilities
pub struct ModalRenderer;

impl ModalRenderer {
    /// Render a modal with title and custom content
    pub fn render_modal(
        f: &mut Frame,
        title: &str,
        colors: Colors,
        percent_x: u16,
        percent_y: u16,
    ) -> Rect {
        let area = f.area();
        let popup_area = centered_rect(percent_x, percent_y, area);

        // Clear background
        f.render_widget(Clear, popup_area);

        // Render border
        let block = Block::default()
            .title(format!(" {} ", title))
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .style(
                Style::default()
                    .fg(colors.selection)
                    .add_modifier(Modifier::BOLD),
            );
        f.render_widget(block, popup_area);

        // Return inner area (excluding border)
        Rect {
            x: popup_area.x + 1,
            y: popup_area.y + 1,
            width: popup_area.width.saturating_sub(2),
            height: popup_area.height.saturating_sub(2),
        }
    }

    /// Render help text with key bindings
    pub fn render_help_lines(colors: Colors) -> Vec<Line<'static>> {
        vec![
            Line::from(vec![
                Span::styled("Arrows/hjkl", Style::default().fg(colors.selection).add_modifier(Modifier::BOLD)),
                Span::raw(" - Navigate"),
            ]),
            Line::from(vec![
                Span::styled("Space", Style::default().fg(colors.selection).add_modifier(Modifier::BOLD)),
                Span::raw(" - Select"),
            ]),
            Line::from(vec![
                Span::styled("Enter", Style::default().fg(colors.selection).add_modifier(Modifier::BOLD)),
                Span::raw(" - Confirm"),
            ]),
            Line::from(vec![
                Span::styled("Esc", Style::default().fg(colors.selection).add_modifier(Modifier::BOLD)),
                Span::raw(" - Cancel"),
            ]),
        ]
    }

    /// Render a loading indicator
    pub fn render_loading(f: &mut Frame, colors: Colors) {
        let area = f.area();
        let popup_area = centered_rect(40, 20, area);

        f.render_widget(Clear, popup_area);

        let loading = Paragraph::new("Loading...")
            .style(Style::default().fg(colors.text))
            .alignment(ratatui::layout::Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .style(Style::default().fg(colors.selection)),
            );

        f.render_widget(loading, popup_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modal_renderer_exists() {
        let _ = ModalRenderer;
    }
}
