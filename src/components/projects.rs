use crate::app::AppState;
use crate::components::Component;
use crate::events::Action;
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
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

        // Get visible projects - need to collect to avoid borrow conflicts
        let visible_projects: Vec<_> = app.get_visible_projects().into_iter().cloned().collect();

        let mut items: Vec<ListItem> = Vec::new();
        let mut current_workspace: Option<String> = None;

        for (visible_idx, p) in visible_projects.iter().enumerate() {
            // Check if this is the first project of a workspace
            let is_first_in_workspace = match &p.workspace_name {
                Some(ws_name) => current_workspace.as_ref() != Some(ws_name),
                None => false,
            };

            if is_first_in_workspace {
                let ws_name = p.workspace_name.as_ref().unwrap();
                let is_collapsed = app.collapsed_workspaces.contains(ws_name);
                let collapse_indicator = if is_collapsed { "▸" } else { "▾" };

                // This is the workspace header row (first project acts as the header)
                let is_highlighted = selected_index == Some(visible_idx);
                let indicator = if is_highlighted { "▶ " } else { "  " };

                let header_style = if is_highlighted {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::Rgb(150, 150, 200))
                        .add_modifier(Modifier::BOLD)
                };

                let line = Line::from(vec![
                    Span::raw(indicator),
                    Span::styled(
                        format!(
                            "{} {} ({} projects)",
                            collapse_indicator,
                            ws_name,
                            app.projects
                                .iter()
                                .filter(|proj| proj.workspace_name.as_ref() == Some(ws_name))
                                .count()
                        ),
                        header_style,
                    ),
                ]);

                items.push(ListItem::new(line));
                current_workspace = Some(ws_name.clone());
            } else if p.workspace_name.is_some() {
                // This is a workspace member (only visible if workspace is expanded)
                let is_selected = app.selected_projects.contains(&p.name);
                let is_highlighted = selected_index == Some(visible_idx);

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
                    Span::raw("  "), // Indent for workspace member
                    Span::raw(indicator),
                    Span::styled(checkbox, checkbox_style),
                    Span::raw(" "),
                    Span::styled(&p.name, name_style),
                    Span::styled(
                        format!(" (v{})", p.version),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);

                items.push(ListItem::new(line));
            } else {
                // Standalone project
                current_workspace = None;

                let is_selected = app.selected_projects.contains(&p.name);
                let is_highlighted = selected_index == Some(visible_idx);

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
                    Span::styled(
                        format!(" (v{})", p.version),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);

                items.push(ListItem::new(line));
            }
        }

        let selected_count = app.selected_projects.len();
        let total_count = visible_projects.len();
        let all_count = app.projects.len();

        let title = if selected_count > 0 {
            if all_count > total_count {
                format!(
                    " Projects ({}/{} selected, {}/{} shown) ",
                    selected_count, total_count, total_count, all_count
                )
            } else {
                format!(" Projects ({}/{} selected) ", selected_count, total_count)
            }
        } else if all_count > total_count {
            format!(" Projects ({}/{} shown) ", total_count, all_count)
        } else {
            format!(" Projects ({}) ", total_count)
        };

        let help_text = if area.height > items.len() as u16 + 4 {
            "\n\n ↑↓/jk: Navigate\n ←→/hl: Collapse/Expand\n Space: Select\n :: Command"
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
