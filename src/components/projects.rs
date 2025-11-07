use crate::app::AppState;
use crate::components::Component;
use crate::events::Action;
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::ListItem,
};

pub struct ProjectList {}

impl ProjectList {
    pub fn new() -> Self {
        Self {}
    }

    /// Get status indicator and color for a project based on its check status
    ///
    /// Visual indicators:
    /// - Gray (⋯): Unchecked or cache invalidated
    /// - Blue (⟳): Currently checking for updates
    /// - Yellow (⚠): Some dependencies are outdated
    /// - Green (✓): All dependencies up to date
    fn get_project_status(p: &crate::project::Project) -> (&'static str, Style) {
        use crate::project::ProjectCheckStatus;

        match p.check_status {
            ProjectCheckStatus::Unchecked => {
                // Gray - not checked yet or cache invalidated
                ("⋯", Style::default().fg(Color::DarkGray))
            }
            ProjectCheckStatus::Checking => {
                // Blue - currently being checked
                (
                    "⟳",
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                )
            }
            ProjectCheckStatus::HasUpdates => {
                // Yellow - has outdated dependencies
                (
                    "⚠",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
            }
            ProjectCheckStatus::UpToDate => {
                // Green - all up to date
                ("✓", Style::default().fg(Color::Green))
            }
        }
    }
}

impl ProjectList {
    fn create_workspace_header<'a>(
        collapsed_workspaces: &'a std::collections::HashSet<String>,
        projects: &'a [crate::project::Project],
        selected_projects: &'a std::collections::HashSet<String>,
        p: &'a crate::project::Project,
        visible_idx: usize,
        selected_index: Option<usize>,
    ) -> ListItem<'a> {
        let ws_name = p.workspace_name.as_ref().unwrap();
        let is_collapsed = collapsed_workspaces.contains(ws_name);
        let collapse_indicator = if is_collapsed { "▸" } else { "▾" };
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
        let workspace_members: Vec<&crate::project::Project> = projects
            .iter()
            .filter(|proj| proj.workspace_name.as_ref() == Some(ws_name))
            .collect();
        let selected_count = workspace_members
            .iter()
            .filter(|proj| selected_projects.contains(&proj.name))
            .count();
        let checkbox_symbol = if workspace_members.is_empty() || selected_count == 0 {
            "☐"
        } else if selected_count == workspace_members.len() {
            "☑"
        } else {
            "◪"
        };
        let checkbox_style =
            if selected_count == workspace_members.len() && selected_count > 0 {
                Style::default().fg(Color::Green)
            } else if selected_count > 0 {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
        let mut has_checking = false;
        let mut has_updates = false;
        let mut has_unchecked = false;
        for proj in &workspace_members {
            match proj.check_status {
                crate::project::ProjectCheckStatus::Checking => has_checking = true,
                crate::project::ProjectCheckStatus::HasUpdates => has_updates = true,
                crate::project::ProjectCheckStatus::Unchecked => has_unchecked = true,
                crate::project::ProjectCheckStatus::UpToDate => {}
            }
        }
        let (status_icon, status_style) = if has_checking {
            (
                "⟳",
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            )
        } else if has_updates {
            (
                "⚠",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else if has_unchecked {
            ("⋯", Style::default().fg(Color::DarkGray))
        } else {
            ("✓", Style::default().fg(Color::Green))
        };
        let line = ratatui::text::Line::from(vec![
            ratatui::text::Span::raw(indicator),
            ratatui::text::Span::styled(checkbox_symbol, checkbox_style),
            ratatui::text::Span::raw(" "),
            ratatui::text::Span::styled(status_icon, status_style),
            ratatui::text::Span::raw(" "),
            ratatui::text::Span::styled(
                format!(
                    "{} {} ({} projects)",
                    collapse_indicator,
                    ws_name,
                    workspace_members.len()
                ),
                header_style,
            ),
        ]);
        ListItem::new(line)
    }

    fn create_project_list_item<'a>(
        selected_projects: &'a std::collections::HashSet<String>,
        p: &'a crate::project::Project,
        visible_idx: usize,
        selected_index: Option<usize>,
    ) -> ListItem<'a> {
        let is_selected = selected_projects.contains(&p.name);
        let is_highlighted = selected_index == Some(visible_idx);
        let checkbox = if is_selected { "☑" } else { "☐" };
        let indicator = if is_highlighted { "▶ " } else { "  " };
        let (status_icon, status_style) = Self::get_project_status(p);
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
        let line = ratatui::text::Line::from(vec![
            ratatui::text::Span::raw(indicator),
            ratatui::text::Span::styled(checkbox, checkbox_style),
            ratatui::text::Span::raw(" "),
            ratatui::text::Span::styled(status_icon, status_style),
            ratatui::text::Span::raw(" "),
            ratatui::text::Span::styled(&p.name, name_style),
            ratatui::text::Span::styled(
                format!(" (v{})", p.version),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        ListItem::new(line)
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
        let visible_projects: Vec<crate::project::Project> =
            app.get_visible_projects().into_iter().cloned().collect();

        let mut items: Vec<ListItem> = Vec::new();
        let mut current_workspace: Option<String> = None;

        for (visible_idx, p) in visible_projects.iter().enumerate() {
            let is_first_in_workspace = match &p.workspace_name {
                Some(ws_name) => current_workspace.as_ref() != Some(ws_name),
                None => false,
            };

            if is_first_in_workspace {
                items.push(Self::create_workspace_header(
                    &app.collapsed_workspaces,
                    &app.projects,
                    &app.selected_projects,
                    p,
                    visible_idx,
                    selected_index,
                ));
                current_workspace = p.workspace_name.clone();
            } else {
                items.push(Self::create_project_list_item(
                    &app.selected_projects,
                    p,
                    visible_idx,
                    selected_index,
                ));
                if p.workspace_name.is_none() {
                    current_workspace = None;
                }
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
                list_items.push(ListItem::new(ratatui::text::Line::from(ratatui::text::Span::styled(
                    line,
                    Style::default().fg(Color::DarkGray),
                ))));
            }
        }

        let project_list = ratatui::widgets::List::new(list_items)
            .block(
                ratatui::widgets::Block::default()
                    .borders(ratatui::widgets::Borders::ALL)
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
