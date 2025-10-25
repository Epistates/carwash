use crate::app::AppState;
use crate::components::Component;
use crate::events::Action;
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub struct Help {}

impl Help {
    pub fn new() -> Self {
        Self {}
    }
}

impl Component for Help {
    fn handle_key_events(&mut self, key: KeyCode, _app: &mut AppState) -> Option<Action> {
        match key {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => Some(Action::EnterNormalMode),
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
                Span::styled("  Tab / Sh+Tab ", Style::default().fg(Color::Cyan)),
                Span::raw("Cycle through output tabs"),
            ]),
            Line::from(vec![
                Span::styled("  PgUp / PgDown ", Style::default().fg(Color::Cyan)),
                Span::raw("Scroll output"),
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
            Line::from("  • Commands run in parallel across selected projects"),
            Line::from("  • Press 'u' on selected project to check for outdated dependencies"),
        ];

        let help_para = Paragraph::new(help_lines)
            .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
            .wrap(Wrap { trim: false })
            .scroll((0, 0));

        f.render_widget(help_para, chunks[1]);

        // Footer
        let footer = Paragraph::new(" Press ? or Esc to close ")
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
