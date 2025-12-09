use crate::app::AppState;
use crate::components::Component;
use crate::events::Action;
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Style},
    widgets::{Block, BorderType, Borders, Paragraph},
};
use std::time::SystemTime;

pub struct Spinner;

impl Spinner {
    pub fn new() -> Self {
        Self
    }
}

impl Component for Spinner {
    fn handle_key_events(&mut self, _key: KeyCode, _app: &mut AppState) -> Option<Action> {
        None
    }

    fn draw(&mut self, f: &mut Frame, _app: &mut AppState, area: Rect) {
        let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let idx = (timestamp / 80) as usize % frames.len(); // 80ms per frame
        let symbol = frames[idx];

        let text = format!(" {} Scanning for projects... ", symbol);
        
        // Calculate center for a small box
        let width = (text.len() as u16) + 2;
        let height = 3;
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        
        // Ensure we don't draw outside the screen or weirdly
        if x >= area.width || y >= area.height {
             return;
        }
        
        let centered_area = Rect::new(x, y, width, height);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan));

        let paragraph = Paragraph::new(text)
            .block(block)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Cyan));

        f.render_widget(paragraph, centered_area);
    }
}
