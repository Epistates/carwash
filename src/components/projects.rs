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
    fn get_project_status(
        p: &crate::project::Project,
        colors: crate::ui::styles::Colors,
    ) -> (&'static str, Style) {
        use crate::project::ProjectCheckStatus;

        match p.check_status {
            ProjectCheckStatus::Unchecked => {
                // Gray - not checked yet or cache invalidated
                ("⋯", Style::default().fg(colors.muted))
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
                        .fg(colors.warning)
                        .add_modifier(Modifier::BOLD),
                )
            }
            ProjectCheckStatus::UpToDate => {
                // Green - all up to date
                ("✓", Style::default().fg(colors.success))
            }
        }
    }
}

impl ProjectList {
    /// Create a list item for a project from the tree (with indentation)
    fn create_tree_project_item<'a>(
        selected_projects: &'a std::collections::HashSet<String>,
        project: &'a crate::project::Project,
        depth: usize,
        is_selected: bool,
        colors: crate::ui::styles::Colors,
    ) -> ListItem<'a> {
        let (status_icon, status_style) = Self::get_project_status(project, colors);

        let is_checked = selected_projects.contains(&project.name);
        let checkbox_symbol = if is_checked { "☑" } else { "☐" };
        let checkbox_style = if is_checked {
            Style::default().fg(colors.success)
        } else {
            Style::default().fg(colors.muted)
        };

        let indicator = if is_selected { "▶ " } else { "  " };
        let indent = "  ".repeat(depth);

        let name_style = if is_selected {
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(colors.text)
        };

        // Build size information string if available
        let mut spans = vec![
            ratatui::text::Span::raw(indicator),
            ratatui::text::Span::raw(indent),
            ratatui::text::Span::styled(checkbox_symbol, checkbox_style),
            ratatui::text::Span::raw(" "),
            ratatui::text::Span::styled(status_icon, status_style),
            ratatui::text::Span::raw(" "),
            ratatui::text::Span::styled(&project.name, name_style),
        ];

        // Git status indicator
        if project.git_status == crate::project::GitStatus::Dirty {
             spans.push(ratatui::text::Span::raw(" "));
             spans.push(ratatui::text::Span::styled(
                 crate::ui::styles::StatusSymbols::GIT_DIRTY,
                 Style::default().fg(colors.warning).add_modifier(Modifier::BOLD),
             ));
        }

        // Add size information if available
        if let Some(target_size) = project.target_size {
            let size_str = crate::project::Project::format_size(target_size);
            let size_style = if target_size > 1_000_000_000 {
                // > 1GB: red warning
                Style::default()
                    .fg(colors.error)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else if target_size > 100_000_000 {
                // > 100MB: yellow warning
                Style::default().fg(colors.warning)
            } else if target_size > 0 {
                // > 0: dim gray
                Style::default().fg(colors.muted)
            } else {
                // 0 bytes: very dim
                Style::default().fg(Color::Rgb(80, 80, 80))
            };
            spans.push(ratatui::text::Span::raw(" "));
            spans.push(ratatui::text::Span::styled(
                format!("📦{}", size_str),
                size_style,
            ));
        }

        ListItem::new(ratatui::text::Line::from(spans))
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
        let mut items: Vec<ListItem> = Vec::new();
        let colors = app.current_colors();

        // Render from flattened tree instead of projects list
        for (idx, (node, _)) in app.flattened_tree.items.iter().enumerate() {
            let is_selected = selected_index == Some(idx);

            match &node.node_type {
                crate::tree::TreeNodeType::Directory { name, .. } => {
                    // Render directory node with expand/collapse indicator
                    let indicator = if is_selected { "▶ " } else { "  " };
                    let collapse_indicator = if node.expanded { "▾" } else { "▸" };
                    let indent = "  ".repeat(node.depth);

                    // Check if any children are selected
                    let has_selected_children = app
                        .flattened_tree
                        .items
                        .iter()
                        .skip(idx + 1)
                        .take_while(|(child_node, _)| child_node.depth > node.depth)
                        .filter_map(|(child_node, _)| {
                            if let crate::tree::TreeNodeType::Project(p) = &child_node.node_type {
                                Some(&p.name)
                            } else {
                                None
                            }
                        })
                        .any(|name| app.selected_projects.contains(name));

                    let checkbox_symbol = if has_selected_children { "☑" } else { "☐" };
                    let checkbox_style = if has_selected_children {
                        Style::default().fg(colors.success)
                    } else {
                        Style::default().fg(colors.muted)
                    };

                    let style = if is_selected {
                        Style::default()
                            .fg(colors.selection)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(colors.dim).add_modifier(Modifier::BOLD)
                    };

                    items.push(ListItem::new(ratatui::text::Line::from(vec![
                        ratatui::text::Span::raw(indicator),
                        ratatui::text::Span::raw(indent),
                        ratatui::text::Span::styled(checkbox_symbol, checkbox_style),
                        ratatui::text::Span::raw(" "),
                        ratatui::text::Span::styled(
                            format!("{} {}", collapse_indicator, name),
                            style,
                        ),
                    ])));
                }
                crate::tree::TreeNodeType::Project(project) => {
                    // Render project node
                    items.push(Self::create_tree_project_item(
                        &app.selected_projects,
                        project,
                        node.depth,
                        is_selected,
                        colors,
                    ));
                }
            }
        }

        let selected_count = app.selected_projects.len();
        let total_count = app
            .flattened_tree
            .items
            .iter()
            .filter(|(node, _)| node.node_type.is_project())
            .count();
        let all_count = app.all_projects.len();

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
                list_items.push(ListItem::new(ratatui::text::Line::from(
                    ratatui::text::Span::styled(line, Style::default().fg(colors.muted)),
                )));
            }
        }

        // Highlight border when focused
        let border_color = if app.focus == crate::events::Focus::Projects {
            Color::Cyan
        } else {
            colors.primary
        };

        let project_list = ratatui::widgets::List::new(list_items)
            .block(
                ratatui::widgets::Block::default()
                    .borders(ratatui::widgets::Borders::ALL)
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .title(title)
                    .border_style(Style::default().fg(border_color)),
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
