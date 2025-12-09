use crate::app::AppState;
use crate::components::Component;
use crate::events::{Action, Focus};
use crate::project::DependencyCheckStatus;
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

pub struct DependenciesPane {}

impl DependenciesPane {
    pub fn new() -> Self {
        Self {}
    }
}

impl DependenciesPane {
    // ... (new function remains the same)

    fn create_dependency_list_item<'a>(dep: &'a crate::project::Dependency) -> ListItem<'a> {
        let is_outdated = dep.has_stable_update();

        let (icon, style) = match dep.check_status {
            DependencyCheckStatus::NotChecked => ("⋯", Style::default().fg(Color::DarkGray)),
            DependencyCheckStatus::Checking => ("⟳", Style::default().fg(Color::Cyan)),
            DependencyCheckStatus::Checked => {
                if is_outdated {
                    (
                        "⚠",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(ratatui::style::Modifier::BOLD),
                    )
                } else {
                    ("✓", Style::default().fg(Color::Green))
                }
            }
        };

        let line = if is_outdated && dep.check_status == DependencyCheckStatus::Checked {
            let mut spans = vec![
                ratatui::text::Span::styled(icon, style),
                ratatui::text::Span::raw(" "),
                ratatui::text::Span::styled(&dep.name, Style::default().fg(Color::White)),
                ratatui::text::Span::raw(" "),
                ratatui::text::Span::styled(
                    &dep.current_version,
                    Style::default().fg(Color::DarkGray),
                ),
                ratatui::text::Span::styled(" → ", Style::default().fg(Color::Yellow)),
                ratatui::text::Span::styled(
                    dep.latest_version.as_ref().unwrap(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                ),
            ];

            // Add note for major version updates that require Cargo.toml change
            if let Some(note) = dep.update_note() {
                spans.push(ratatui::text::Span::styled(
                    format!(" ({})", note),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(ratatui::style::Modifier::ITALIC),
                ));
            }

            ratatui::text::Line::from(spans)
        } else {
            let status_text = match dep.check_status {
                DependencyCheckStatus::NotChecked => " (not checked)",
                DependencyCheckStatus::Checking => " (checking...)",
                DependencyCheckStatus::Checked => "",
            };

            ratatui::text::Line::from(vec![
                ratatui::text::Span::styled(icon, style),
                ratatui::text::Span::raw(" "),
                ratatui::text::Span::styled(&dep.name, Style::default().fg(Color::White)),
                ratatui::text::Span::raw(" "),
                ratatui::text::Span::styled(
                    format!("v{}", dep.current_version),
                    Style::default().fg(Color::DarkGray),
                ),
                ratatui::text::Span::styled(
                    status_text,
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(ratatui::style::Modifier::ITALIC),
                ),
            ])
        };

        ListItem::new(line)
    }

    fn get_title(
        dependencies: &[crate::project::Dependency],
        outdated_count: usize,
        not_checked_count: usize,
        checking_count: usize,
    ) -> (String, Style) {
        if dependencies.is_empty() {
            (
                " Dependencies (none) ".to_string(),
                Style::default().fg(Color::DarkGray),
            )
        } else if checking_count > 0 {
            (
                format!(" Dependencies (checking...) "),
                Style::default().fg(Color::Cyan),
            )
        } else if not_checked_count > 0 {
            (
                format!(" Dependencies ({} not checked) ", dependencies.len()),
                Style::default().fg(Color::DarkGray),
            )
        } else if outdated_count > 0 {
            (
                format!(" Dependencies ({} outdated) ", outdated_count),
                Style::default().fg(Color::Yellow),
            )
        } else {
            (
                format!(" Dependencies ({} up-to-date) ", dependencies.len()),
                Style::default().fg(Color::Green),
            )
        }
    }
}

impl Component for DependenciesPane {
    fn handle_key_events(&mut self, key: KeyCode, _app: &mut AppState) -> Option<Action> {
        match key {
            KeyCode::Char('u') => Some(Action::StartUpdateWizard),
            _ => None,
        }
    }

    fn draw(&mut self, f: &mut Frame, app: &mut AppState, area: Rect) {
        if let Some(p) = app.get_selected_project() {
            let mut outdated_count = 0;
            let mut not_checked_count = 0;
            let mut checking_count = 0;

            let dependency_items: Vec<ListItem> = p
                .dependencies
                .iter()
                .map(|dep| {
                    let is_outdated = dep.has_stable_update();

                    if is_outdated {
                        outdated_count += 1;
                    }

                    match dep.check_status {
                        DependencyCheckStatus::NotChecked => not_checked_count += 1,
                        DependencyCheckStatus::Checking => checking_count += 1,
                        DependencyCheckStatus::Checked => {}
                    }

                    Self::create_dependency_list_item(dep)
                })
                .collect();

            let (title, title_style) = Self::get_title(
                &p.dependencies,
                outdated_count,
                not_checked_count,
                checking_count,
            );

            // Highlight border when focused
            let border_style = if app.focus == Focus::Dependencies {
                Style::default().fg(Color::Cyan)
            } else {
                title_style
            };

            let dependency_list = List::new(dependency_items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(border_style),
            );

            f.render_widget(dependency_list, area);
        } else {
            let empty =
                Paragraph::new("No project selected.\n\nSelect a project to view dependencies.")
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" Dependencies "),
                    )
                    .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, area);
        }
    }
}
