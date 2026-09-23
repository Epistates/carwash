use crate::app::AppState;
use crate::components::Component;
use crate::events::Action;
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use tui_input::{Input, backend::crossterm::EventHandler};

#[derive(Debug, Clone)]
pub struct SettingsModalState {
    pub cache_minutes_input: Input,
    pub error_message: Option<String>,
}

impl SettingsModalState {
    pub fn new() -> Self {
        Self {
            cache_minutes_input: Input::default(),
            error_message: None,
        }
    }
}

pub struct SettingsModal;

impl SettingsModal {
    pub fn new() -> Self {
        Self
    }

    fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
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
}

impl Component for SettingsModal {
    fn handle_key_events(&mut self, key: KeyCode, app: &mut AppState) -> Option<Action> {
        match key {
            KeyCode::Esc => Some(Action::CloseSettings),
            KeyCode::Enter => Some(Action::SaveSettings),
            _ => {
                let mut input = app.settings_modal.cache_minutes_input.clone();
                if input
                    .handle_event(&crossterm::event::Event::Key(key.into()))
                    .is_some()
                {
                    Some(Action::SettingsUpdateCacheInput(input.value().to_string()))
                } else {
                    None
                }
            }
        }
    }

    fn draw(&mut self, f: &mut Frame, app: &mut AppState, area: Rect) {
        let colors = app.current_colors();
        let popup_area = Self::centered_rect(60, 50, area);

        f.render_widget(Clear, popup_area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(popup_area);

        let title = Block::default()
            .title(" Settings ")
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(
                Style::default()
                    .fg(colors.selection)
                    .add_modifier(Modifier::BOLD),
            );
        f.render_widget(title, chunks[0]);

        let cache_label = Line::from(vec![
            Span::styled(" Cache TTL (minutes) ", Style::default().fg(colors.text)),
            Span::raw(" "),
            Span::styled(
                app.settings_modal.cache_minutes_input.value(),
                Style::default()
                    .fg(colors.warning)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);

        let cache_para = Paragraph::new(cache_label)
            .alignment(Alignment::Left)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .border_style(Style::default().fg(colors.dim)),
            );
        f.render_widget(cache_para, chunks[1]);

        let mut lines = vec![Line::from(vec![
            Span::styled(" Enter", Style::default().fg(colors.success)),
            Span::raw(": Save"),
            Span::raw("    "),
            Span::styled("Esc", Style::default().fg(colors.error)),
            Span::raw(": Cancel"),
        ])];

        if let Some(ref error) = app.settings_modal.error_message {
            lines.push(Line::from(vec![Span::styled(
                error.as_str(),
                Style::default()
                    .fg(colors.error)
                    .add_modifier(Modifier::BOLD | Modifier::ITALIC),
            )]));
        }

        let help_para = Paragraph::new(lines).alignment(Alignment::Center).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(colors.selection)),
        );
        f.render_widget(help_para, chunks[3]);
    }
}
