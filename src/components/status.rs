use crate::app::AppState;
use crate::components::Component;
use crate::events::{Action, Focus, Mode};
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
                    "Tab: cycle | Ctrl+[/]: tabs | ':' cmd | '/' search | 'u' update | '?' help | 'q' quit"
                } else {
                    "Tab: cycle | ':' cmd | '/' search | 't' theme | 'u' update | 's' settings | '?' help"
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

        let mode_span = Span::styled(
            format!(" {} ", mode_info.0),
            Style::default()
                .fg(Color::Black)
                .bg(mode_info.1)
                .add_modifier(Modifier::BOLD),
        );

        // Build background activity indicators
        let mut bg_spans: Vec<Span> = Vec::new();

        // Scanning indicator
        if app.is_scanning {
            bg_spans.push(Span::styled(
                " ⟳ Scanning ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        // Update checking indicator
        if app.is_checking_updates {
            bg_spans.push(Span::styled(
                " ⟳ Checking ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        // Pending background tasks indicator
        let pending_count = app.update_queue.queue.len();
        if pending_count > 0 {
            bg_spans.push(Span::styled(
                format!(" {} queued ", pending_count),
                Style::default().fg(Color::DarkGray),
            ));
        }

        // Focus indicator - only show in Normal mode
        let focus_info = if app.mode == Mode::Normal {
            let (icon, label) = match app.focus {
                Focus::Projects => ("📋", "Projects"),
                Focus::Dependencies => ("📦", "Dependencies"),
                Focus::Output => ("📄", "Output"),
            };
            Span::styled(
                format!(" {} {} ", icon, label),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled("", Style::default())
        };

        let selected_info = if !app.selected_projects.is_empty() {
            Span::styled(
                format!(" {} selected ", app.selected_projects.len()),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled("", Style::default())
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
            Span::raw("")
        };

        let help_text = Span::styled(
            format!(" {} ", mode_info.2),
            Style::default().fg(Color::DarkGray),
        );

        // Build the final status line
        let mut spans = vec![mode_span, Span::raw(" ")];

        // Add background activity spans
        for span in bg_spans {
            spans.push(span);
        }

        spans.push(focus_info);
        spans.push(selected_info);
        spans.push(tabs_info);

        // Only add separator if we have help text
        if !mode_info.2.is_empty() {
            spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
            spans.push(help_text);
        }

        let status_line = Line::from(spans);

        let status_bar =
            Paragraph::new(status_line).style(Style::default().bg(Color::Rgb(30, 30, 30)));

        f.render_widget(status_bar, area);
    }
}
