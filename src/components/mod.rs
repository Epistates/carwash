use crate::app::AppState;
use crate::events::Action;
use crossterm::event::KeyCode;
use ratatui::{Frame, layout::Rect};

pub mod dependencies;
pub mod help;
pub mod output;
pub mod palette;
pub mod projects;
pub mod status;
pub mod text_input;
pub mod updater;

pub trait Component {
    fn handle_key_events(&mut self, key: KeyCode, app: &mut AppState) -> Option<Action>;
    fn draw(&mut self, f: &mut Frame, app: &mut AppState, area: Rect);
}
