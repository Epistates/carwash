use crate::app::AppState;
use crate::components::Component;
use crate::events::Action;
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
};

pub struct Help {
    scroll: usize,
    scroll_state: ScrollbarState,
}

impl Help {
    pub fn new() -> Self {
        Self {
            scroll: 0,
            scroll_state: ScrollbarState::default(),
        }
    }
}

impl Component for Help {
    fn handle_key_events(&mut self, key: KeyCode, _app: &mut AppState) -> Option<Action> {
        match key {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => Some(Action::EnterNormalMode),
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                None
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10);
                None
            }
            KeyCode::Home => {
                self.scroll = 0;
                None
            }
            _ => None,
        }
    }

    fn draw(&mut self, f: &mut Frame, _app: &mut AppState, area: Rect) {
        let popup_area = Self::centered_rect(80, 85, area);

        f.render_widget(Clear, popup_area);

        let chunks = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(popup_area);

        // Title
        let title = Block::default()
            .title(" CarWash - Rust Project Manager ")
            .title_alignment(Alignment::Center)
            .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
            .border_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            );
        f.render_widget(title, chunks[0]);

        // Help content
        let help_lines = vec![
            Line::from(vec![Span::styled(
                "Navigation",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  ↑↓ / j k     ", Style::default().fg(Color::Cyan)),
                Span::raw("Navigate projects"),
            ]),
            Line::from(vec![
                Span::styled("  Space         ", Style::default().fg(Color::Cyan)),
                Span::raw("Toggle project selection"),
            ]),
            Line::from(vec![
                Span::styled("  ←→ / h l     ", Style::default().fg(Color::Cyan)),
                Span::raw("Collapse/Expand workspaces"),
            ]),
            Line::from(vec![
                Span::styled("  Tab / S-Tab  ", Style::default().fg(Color::Cyan)),
                Span::raw("Cycle focus between panes"),
            ]),
            Line::from(vec![
                Span::styled("  Ctrl+[ / ]   ", Style::default().fg(Color::Cyan)),
                Span::raw("Switch output tabs (works from any pane)"),
            ]),
            Line::from(vec![
                Span::styled("  - / +        ", Style::default().fg(Color::Cyan)),
                Span::raw("Adjust output pane height"),
            ]),
            Line::from(vec![
                Span::styled("  [ ] / { }    ", Style::default().fg(Color::Cyan)),
                Span::raw("Adjust left pane width"),
            ]),
            Line::from(vec![
                Span::styled("  Shift+R      ", Style::default().fg(Color::Cyan)),
                Span::raw("Reset layout to defaults"),
            ]),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Commands",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  :             ", Style::default().fg(Color::Cyan)),
                Span::raw("Open command palette"),
            ]),
            Line::from(vec![
                Span::styled("  u             ", Style::default().fg(Color::Cyan)),
                Span::raw("Check for dependency updates"),
            ]),
            Line::from(vec![
                Span::styled("  ?             ", Style::default().fg(Color::Cyan)),
                Span::raw("Toggle this help screen"),
            ]),
            Line::from(vec![
                Span::styled("  q             ", Style::default().fg(Color::Cyan)),
                Span::raw("Quit application"),
            ]),
            Line::from(vec![
                Span::styled("  Ctrl+C        ", Style::default().fg(Color::Cyan)),
                Span::raw("Force quit"),
            ]),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Command Palette",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Type          ", Style::default().fg(Color::Cyan)),
                Span::raw("Filter commands (fuzzy search)"),
            ]),
            Line::from(vec![
                Span::styled("  Tab           ", Style::default().fg(Color::Cyan)),
                Span::raw("Toggle scope (Selected/All projects)"),
            ]),
            Line::from(vec![
                Span::styled("  Enter         ", Style::default().fg(Color::Cyan)),
                Span::raw("Execute selected command"),
            ]),
            Line::from(vec![
                Span::styled("  Esc           ", Style::default().fg(Color::Cyan)),
                Span::raw("Cancel"),
            ]),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Common Commands",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  test          ", Style::default().fg(Color::Green)),
                Span::raw("Run tests"),
            ]),
            Line::from(vec![
                Span::styled("  check         ", Style::default().fg(Color::Green)),
                Span::raw("Check for errors"),
            ]),
            Line::from(vec![
                Span::styled("  build         ", Style::default().fg(Color::Green)),
                Span::raw("Build projects"),
            ]),
            Line::from(vec![
                Span::styled("  clean         ", Style::default().fg(Color::Green)),
                Span::raw("Clean build artifacts"),
            ]),
            Line::from(vec![
                Span::styled("  clippy        ", Style::default().fg(Color::Green)),
                Span::raw("Run Clippy lints"),
            ]),
            Line::from(vec![
                Span::styled("  fmt           ", Style::default().fg(Color::Green)),
                Span::raw("Format code"),
            ]),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Tips",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from("  • Select multiple projects with Space, then run commands on all"),
            Line::from(
                "  • Output is color-coded: errors (red), warnings (yellow), success (green)",
            ),
            Line::from("  • Use Tab to cycle focus: Projects → Dependencies → Output"),
            Line::from("  • Focus indicator shows current pane in the status bar"),
            Line::from("  • When Output is focused: ←→/hl switches tabs, j/k or PgUp/Dn scrolls"),
            Line::from("  • Use Ctrl+[ and Ctrl+] to switch output tabs from any pane"),
            Line::from("  • Commands run in parallel across selected projects"),
            Line::from("  • Press 'u' on selected project to check for outdated dependencies"),
        ];

        // Calculate content height and viewport
        let content_height = help_lines.len();
        let viewport_height = (chunks[1].height.saturating_sub(2)) as usize; // Subtract borders

        // Clamp scroll to valid range
        let max_scroll = content_height.saturating_sub(viewport_height);
        self.scroll = self.scroll.min(max_scroll);

        let help_para = Paragraph::new(help_lines)
            .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
            .wrap(Wrap { trim: false })
            .scroll((self.scroll as u16, 0));

        f.render_widget(help_para, chunks[1]);

        // Render scrollbar if content is larger than viewport
        if content_height > viewport_height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));

            let mut scrollbar_state = self
                .scroll_state
                .content_length(content_height)
                .viewport_content_length(viewport_height)
                .position(self.scroll);

            let scrollbar_area = Rect {
                x: chunks[1].x + chunks[1].width - 1,
                y: chunks[1].y + 1,
                width: 1,
                height: chunks[1].height.saturating_sub(2),
            };

            f.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
        }

        // Footer
        let footer_text = if content_height > viewport_height {
            " ↑↓/j k: scroll | PgUp/PgDn: page | Home: top | ? or Esc: close "
        } else {
            " Press ? or Esc to close "
        };
        let footer = Paragraph::new(footer_text)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            );
        f.render_widget(footer, chunks[2]);
    }
}

impl Help {
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
