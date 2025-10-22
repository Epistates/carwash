use crate::app::AppState;
use crate::events::Action;
use crate::components::Component;
use crate::events::Command;
use crossterm::event::KeyCode;
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};
use tui_input::{backend::crossterm::EventHandler, Input};

#[derive(Debug, Clone)]
pub struct CommandPaletteState {
    pub input: Input,
    pub filtered_commands: Vec<Command>,
    pub list_state: ratatui::widgets::ListState,
}

impl CommandPaletteState {
    pub fn new() -> Self {
        Self {
            input: Input::default(),
            filtered_commands: Vec::new(),
            list_state: ratatui::widgets::ListState::default(),
        }
    }
}

pub struct CommandPalette {}

impl CommandPalette {
    pub fn new() -> Self {
        Self {}
    }
}

impl Component for CommandPalette {
    fn handle_key_events(&mut self, key: KeyCode, app: &mut AppState) -> Option<Action> {
        match key {
            KeyCode::Enter => {
                let command = app.palette.list_state.selected()
                    .and_then(|i| app.palette.filtered_commands.get(i))
                    .cloned()
                    .unwrap_or_else(|| Command::Cargo { 
                        command: app.palette.input.value().to_string()
                    });
                Some(Action::ExecuteCommand(command))
            }
            KeyCode::Esc => Some(Action::EnterNormalMode),
            KeyCode::Down | KeyCode::Char('j') => Some(Action::PaletteSelectNext),
            KeyCode::Up | KeyCode::Char('k') => Some(Action::PaletteSelectPrevious),
            _ => {
                let mut input = app.palette.input.clone();
                if input.handle_event(&crossterm::event::Event::Key(key.into())).is_some() {
                    Some(Action::UpdatePaletteInput(input.value().to_string()))
                } else {
                    None
                }
            }
        }
    }

    fn draw(&mut self, f: &mut Frame, app: &mut AppState, area: Rect) {
        // Center the palette
        let popup_area = Self::centered_rect(60, 60, area);
        
        f.render_widget(Clear, popup_area);
        
        let chunks = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(3),
            ])
            .split(popup_area);

        // Input box
        let input_box = Paragraph::new(app.palette.input.value())
            .style(Style::default().fg(Color::Yellow))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Command ")
                    .border_style(Style::default().fg(Color::Cyan)),
            );
        f.render_widget(input_box, chunks[0]);

        // Command list
        let command_items: Vec<ListItem> = app
            .palette
            .filtered_commands
            .iter()
            .map(|cmd| {
                let text = match cmd {
                    Command::Cargo { command } => {
                        Line::from(vec![
                            Span::styled("cargo ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                            Span::styled(command, Style::default().fg(Color::White)),
                        ])
                    }
                    _ => Line::from(format!("{:?}", cmd)),
                };
                ListItem::new(text)
            })
            .collect();

        let list_title = if app.palette.filtered_commands.is_empty() {
            " Commands (no matches) "
        } else {
            " Commands (↑↓ to select) "
        };

        let list = List::new(command_items)
            .block(
                Block::default()
                    .title(list_title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::Rgb(60, 60, 80))
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");

        f.render_stateful_widget(list, chunks[1], &mut app.palette.list_state);

        // Help text
        let selected_info = if app.selected_projects.is_empty() {
            " No projects selected - select with Space first! "
        } else {
            " Enter: Run on selected projects | Esc: Cancel "
        };
        
        let help = Paragraph::new(selected_info)
            .style(if app.selected_projects.is_empty() {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            })
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(help, chunks[2]);
    }
}

impl CommandPalette {
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
