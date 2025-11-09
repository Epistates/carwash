use crate::app::AppState;
use crate::components::Component;
use crate::events::{Action, Mode};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

pub struct StatusBar {}

impl StatusBar {
    pub fn new() -> Self {
        Self {}
    }
}

impl Component for StatusBar {
    fn handle_key_events(
        &mut self,
        _key: crossterm::event::KeyCode,
        _app: &mut AppState,
    ) -> Option<Action> {
        None
    }

    fn draw(&mut self, f: &mut Frame, app: &mut AppState, area: Rect) {
        let mode_info = match app.mode {
            Mode::Loading => ("LOADING", Color::Yellow, "Scanning for projects..."),
            Mode::Normal => {
                let hint = if app.tabs.len() > 1 {
                    "':' cmd | '/' search | 't' theme | 'u' updates | 's' set | ←→ nav | [](){} resize | 'R' reset | tabs: ←→ | '?' help"
                } else {
                    "':' cmd | '/' search | 't' theme | 'u' updates | 's' set | ←→ nav | [](){} resize | 'R' reset | '?' help | 'q' quit"
                };
                ("NORMAL", Color::Green, hint)
            }
            Mode::CommandPalette => (
                "COMMAND",
                Color::Cyan,
                "Type to filter | ↑↓ select | Enter run | Esc cancel",
            ),
            Mode::UpdateWizard => (
                "UPDATE",
                Color::Magenta,
                "Space select | ↑↓ navigate | Enter update | Esc cancel",
            ),
            Mode::Settings => (
                "SETTINGS",
                Color::Magenta,
                "Digits set cache | Space toggles background | Enter save | Esc cancel",
            ),
            Mode::TextInput => ("INPUT", Color::Blue, "Enter confirm | Esc cancel"),
            Mode::Help => ("HELP", Color::Yellow, "Esc or 'q' to close"),
            Mode::Filter => (
                "FILTER",
                Color::Cyan,
                "Type to search | ↑↓ navigate | Enter select | Esc cancel",
            ),
        };

        let status_line = if app.is_checking_updates {
            Line::from(vec![
                Span::styled(
                    " ⟳ ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "Checking for updates... ",
                    Style::default().fg(Color::Yellow),
                ),
            ])
        } else {
            let mode_span = Span::styled(
                format!(" {} ", mode_info.0),
                Style::default()
                    .fg(Color::Black)
                    .bg(mode_info.1)
                    .add_modifier(Modifier::BOLD),
            );

            let selected_info = if !app.selected_projects.is_empty() {
                Span::styled(
                    format!(" {} selected ", app.selected_projects.len()),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(" ", Style::default())
            };

            let tabs_info = if !app.tabs.is_empty() {
                let running = app.tabs.iter().filter(|t| !t.is_finished).count();
                let finished = app.tabs.len() - running;

                if running > 0 {
                    Span::styled(
                        format!(" ⚙ {}/{} running ", running, app.tabs.len()),
                        Style::default().fg(Color::Cyan),
                    )
                } else {
                    Span::styled(
                        format!(" ✓ {}/{} complete ", finished, app.tabs.len()),
                        Style::default().fg(Color::Green),
                    )
                }
            } else {
                Span::raw(" ")
            };

            let help_text = Span::styled(
                format!(" {} ", mode_info.2),
                Style::default().fg(Color::White),
            );

            Line::from(vec![
                mode_span,
                Span::raw(" "),
                selected_info,
                tabs_info,
                Span::raw("│"),
                help_text,
            ])
        };

        let status_bar =
            Paragraph::new(status_line).style(Style::default().bg(Color::Rgb(30, 30, 30)));

        f.render_widget(status_bar, area);
    }
}
