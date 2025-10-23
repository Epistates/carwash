use crate::app::AppState;
use crate::components::Component;
use crate::events::Action;
use crate::project::DependencyCheckStatus;
use crossterm::event::KeyCode;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

pub struct DependenciesPane {}

impl DependenciesPane {
    pub fn new() -> Self {
        Self {}
    }
}

impl Component for DependenciesPane {
    fn handle_key_events(
        &mut self,
        key: KeyCode,
        _app: &mut AppState,
    ) -> Option<Action> {
        match key {
            KeyCode::Char('u') => Some(Action::StartUpdateWizard),
            _ => None,
        }
    }

    fn draw(&mut self, f: &mut Frame, app: &mut AppState, area: Rect) {
        if let Some(p) = app.get_selected_project() {
            let mut dependency_items: Vec<ListItem> = Vec::new();
            let mut outdated_count = 0;
            let mut not_checked_count = 0;
            let mut checking_count = 0;
            
            for dep in &p.dependencies {
                let is_outdated = dep.latest_version.is_some() 
                    && dep.latest_version.as_ref().unwrap() != &dep.current_version;
                
                if is_outdated {
                    outdated_count += 1;
                }
                
                match dep.check_status {
                    DependencyCheckStatus::NotChecked => not_checked_count += 1,
                    DependencyCheckStatus::Checking => checking_count += 1,
                    DependencyCheckStatus::Checked => {},
                }
                
                let (icon, style) = match dep.check_status {
                    DependencyCheckStatus::NotChecked => {
                        ("⋯", Style::default().fg(Color::DarkGray))
                    }
                    DependencyCheckStatus::Checking => {
                        ("⟳", Style::default().fg(Color::Cyan))
                    }
                    DependencyCheckStatus::Checked => {
                        if is_outdated {
                            ("⚠", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
                        } else {
                            ("✓", Style::default().fg(Color::Green))
                        }
                    }
                };
                
                let line = if is_outdated && dep.check_status == DependencyCheckStatus::Checked {
                    Line::from(vec![
                        Span::styled(icon, style),
                        Span::raw(" "),
                        Span::styled(&dep.name, Style::default().fg(Color::White)),
                        Span::raw(" "),
                        Span::styled(&dep.current_version, Style::default().fg(Color::DarkGray)),
                        Span::styled(" → ", Style::default().fg(Color::Yellow)),
                        Span::styled(
                            dep.latest_version.as_ref().unwrap(),
                            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                        ),
                    ])
                } else {
                    let status_text = match dep.check_status {
                        DependencyCheckStatus::NotChecked => " (not checked)",
                        DependencyCheckStatus::Checking => " (checking...)",
                        DependencyCheckStatus::Checked => "",
                    };
                    
                    Line::from(vec![
                        Span::styled(icon, style),
                        Span::raw(" "),
                        Span::styled(&dep.name, Style::default().fg(Color::White)),
                        Span::raw(" "),
                        Span::styled(
                            format!("v{}", dep.current_version),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(
                            status_text,
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                        ),
                    ])
                };
                
                dependency_items.push(ListItem::new(line));
            }

            let (title, title_style) = if p.dependencies.is_empty() {
                (" Dependencies (none) ".to_string(), Style::default().fg(Color::DarkGray))
            } else if checking_count > 0 {
                (format!(" Dependencies (checking...) "), Style::default().fg(Color::Cyan))
            } else if not_checked_count > 0 {
                (format!(" Dependencies ({} not checked) ", p.dependencies.len()), Style::default().fg(Color::DarkGray))
            } else if outdated_count > 0 {
                (format!(" Dependencies ({} outdated) ", outdated_count), Style::default().fg(Color::Yellow))
            } else {
                (format!(" Dependencies ({} up-to-date) ", p.dependencies.len()), Style::default().fg(Color::Green))
            };

            let dependency_list = List::new(dependency_items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(title)
                        .border_style(title_style),
                );
            
            f.render_widget(dependency_list, area);
        } else {
            let empty = Paragraph::new("No project selected.\n\nSelect a project to view dependencies.")
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Dependencies ")
                )
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, area);
        }
    }
}
