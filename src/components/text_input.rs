use crate::app::AppState;
use crate::components::Component;
use crate::events::Action;
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, Clear, Paragraph},
};
use tui_input::{Input, backend::crossterm::EventHandler};

#[derive(Debug, Clone)]
pub struct TextInputState {
    pub title: String,
    pub input: Input,
}

impl TextInputState {
    pub fn new() -> Self {
        Self {
            title: String::new(),
            input: Input::default(),
        }
    }
}

pub struct TextInput {}

impl TextInput {
    pub fn new() -> Self {
        Self {}
    }
}

impl Component for TextInput {
    fn handle_key_events(&mut self, key: KeyCode, app: &mut AppState) -> Option<Action> {
        match key {
            KeyCode::Esc => Some(Action::EnterNormalMode),
            KeyCode::Enter => {
                // another hack
                None
            }
            _ => {
                let mut input = app.text_input.input.clone();
                if input
                    .handle_event(&crossterm::event::Event::Key(key.into()))
                    .is_some()
                {
                    Some(Action::UpdateTextInput(input.value().to_string()))
                } else {
                    None
                }
            }
        }
    }

    fn draw(&mut self, f: &mut Frame, app: &mut AppState, area: Rect) {
        f.render_widget(Clear, area);
        let block = Block::default()
            .title(app.text_input.title.as_str())
            .borders(Borders::ALL);
        let para = Paragraph::new(app.text_input.input.value()).block(block);
        f.render_widget(para, area);
    }
}
