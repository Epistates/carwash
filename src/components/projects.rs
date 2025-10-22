use crate::app::AppState;
use crate::events::Action;
use crate::components::Component;
use crossterm::event::KeyCode;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
    Frame,
};

pub struct ProjectList {}

impl ProjectList {
    pub fn new() -> Self {
        Self {}
    }
}

impl Component for ProjectList {
    fn handle_key_events(&mut self, key: KeyCode, _app: &mut AppState) -> Option<Action> {
        match key {
            KeyCode::Down | KeyCode::Char('j') => Some(Action::SelectNext),
            KeyCode::Up | KeyCode::Char('k') => Some(Action::SelectPrevious),
            KeyCode::Left | KeyCode::Char('h') => Some(Action::SelectParent),
            KeyCode::Right | KeyCode::Char('l') => Some(Action::SelectChild),
            KeyCode::Char(' ') => Some(Action::ToggleSelection),
            _ => None,
        }
    }

    fn draw(&mut self, f: &mut Frame, app: &mut AppState, area: Rect) {
        let selected_index = app.tree_state.selected();
        
        let items: Vec<ListItem> = app
            .projects
            .iter()
            .enumerate()
            .map(|(idx, p)| {
                let is_selected = app.selected_projects.contains(&p.name);
                let is_highlighted = selected_index == Some(idx);
                
                let checkbox = if is_selected { "☑" } else { "☐" };
                let indicator = if is_highlighted { "▶ " } else { "  " };
                
                let name_style = if is_selected {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else if is_highlighted {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let checkbox_style = if is_selected {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                let line = Line::from(vec![
                    Span::raw(indicator),
                    Span::styled(checkbox, checkbox_style),
                    Span::raw(" "),
                    Span::styled(&p.name, name_style),
                    Span::styled(format!(" (v{})", p.version), Style::default().fg(Color::DarkGray)),
                ]);

                ListItem::new(line)
            })
            .collect();

        let selected_count = app.selected_projects.len();
        let total_count = app.projects.len();
        let all_count = app.all_projects.len();
        
        let title = if selected_count > 0 {
            if all_count > total_count {
                format!(" Projects ({}/{} selected, {}/{} shown) ", selected_count, total_count, total_count, all_count)
            } else {
                format!(" Projects ({}/{} selected) ", selected_count, total_count)
            }
        } else {
            if all_count > total_count {
                format!(" Projects ({}/{} shown) ", total_count, all_count)
            } else {
                format!(" Projects ({}) ", total_count)
            }
        };

        let help_text = if area.height > items.len() as u16 + 4 {
            "\n\n ↑↓/jk: Navigate\n Space: Select\n :: Command"
        } else {
            ""
        };

        let mut list_items = items;
        if !help_text.is_empty() {
            for line in help_text.lines().skip(1) {
                list_items.push(ListItem::new(Line::from(Span::styled(
                    line,
                    Style::default().fg(Color::DarkGray),
                ))));
            }
        }

        let project_list = List::new(list_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::Rgb(40, 40, 60))
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("");

        f.render_stateful_widget(project_list, area, &mut app.tree_state);
    }
}
