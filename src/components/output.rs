use crate::app::AppState;
use crate::components::Component;
use crate::events::{Action, Focus};
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Tabs, Wrap,
    },
};

pub struct TabbedOutputPane {
    scroll_state: ScrollbarState,
}

impl TabbedOutputPane {
    pub fn new() -> Self {
        Self {
            scroll_state: ScrollbarState::default(),
        }
    }
}

impl Component for TabbedOutputPane {
    fn handle_key_events(&mut self, key: KeyCode, app: &mut AppState) -> Option<Action> {
        match key {
            // Tab/Esc: Leave output pane, cycle focus to next pane
            KeyCode::Tab | KeyCode::Esc => Some(Action::FocusNext),
            // BackTab: Cycle focus backwards (to Dependencies)
            KeyCode::BackTab => {
                // Focus::Output -> Focus::Dependencies (reverse of next())
                app.focus = Focus::Dependencies;
                None
            }
            // h/Left: Previous output tab
            KeyCode::Left | KeyCode::Char('h') => {
                if app.active_tab > 0 {
                    Some(Action::SwitchToTab(app.active_tab - 1))
                } else if !app.tabs.is_empty() {
                    // Wrap around to last tab
                    Some(Action::SwitchToTab(app.tabs.len() - 1))
                } else {
                    None
                }
            }
            // l/Right: Next output tab
            KeyCode::Right | KeyCode::Char('l') => {
                if app.active_tab < app.tabs.len().saturating_sub(1) {
                    Some(Action::SwitchToTab(app.active_tab + 1))
                } else if !app.tabs.is_empty() {
                    // Wrap around to first tab
                    Some(Action::SwitchToTab(0))
                } else {
                    None
                }
            }
            KeyCode::PageUp | KeyCode::Char('k') | KeyCode::Up => {
                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                    tab.scroll = tab.scroll.saturating_sub(10);
                }
                None
            }
            KeyCode::PageDown | KeyCode::Char('j') | KeyCode::Down => {
                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                    tab.scroll = (tab.scroll + 10).min(tab.buffer.len().saturating_sub(1));
                }
                None
            }
            _ => None,
        }
    }

    fn draw(&mut self, f: &mut Frame, app: &mut AppState, area: Rect) {
        let colors = app.current_colors();
        if area.height < 4 {
            return;
        }

        let chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                ratatui::layout::Constraint::Length(3),
                ratatui::layout::Constraint::Min(0),
            ])
            .split(area);

        // Render tabs
        if !app.tabs.is_empty() {
            let titles: Vec<Span> = app
                .tabs
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let style = if i == app.active_tab {
                        Style::default()
                            .fg(colors.warning)
                            .add_modifier(Modifier::BOLD)
                    } else if t.is_finished {
                        Style::default().fg(colors.success)
                    } else {
                        Style::default().fg(colors.selection)
                    };

                    let prefix = if t.is_finished { "✓ " } else { "⚙ " };
                    Span::styled(format!("{}{}", prefix, t.title), style)
                })
                .collect();

            // Highlight border when focused
            let border_color = if app.focus == Focus::Output {
                colors.selection
            } else {
                colors.primary
            };

            // Add tab indicator showing current/total tabs
            let tab_indicator = if app.tabs.len() > 1 {
                format!(" Output ({}/{}) ", app.active_tab + 1, app.tabs.len())
            } else {
                " Output ".to_string()
            };

            let tabs = Tabs::new(titles)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(ratatui::widgets::BorderType::Rounded)
                        .title(tab_indicator)
                        .border_style(Style::default().fg(border_color)),
                )
                .select(app.active_tab)
                .style(Style::default())
                .highlight_style(
                    Style::default()
                        .fg(colors.warning)
                        .add_modifier(Modifier::BOLD),
                );
            f.render_widget(tabs, chunks[0]);
        } else {
            let empty_block = Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .title(" Output ")
                .border_style(Style::default().fg(colors.primary));
            f.render_widget(empty_block, chunks[0]);
        }

        // Render active tab content
        // We use get_mut because we may need to cap the scroll
        if let Some(active_tab) = app.tabs.get_mut(app.active_tab) {
            let content_height = (chunks[1].height.saturating_sub(2)) as usize;

            // Update scroll bounds
            let max_scroll = active_tab.buffer.len().saturating_sub(content_height);
            active_tab.scroll = active_tab.scroll.min(max_scroll);
            let current_scroll = active_tab.scroll;

            let visible_content: Vec<Line> = active_tab
                .buffer
                .iter()
                .skip(current_scroll)
                .take(content_height)
                .map(|line| {
                    // Colorize output based on content
                    let style = if line.contains("error")
                        || line.contains("Error")
                        || line.contains("ERROR")
                    {
                        Style::default().fg(colors.error)
                    } else if line.contains("warning")
                        || line.contains("Warning")
                        || line.contains("WARN")
                    {
                        Style::default().fg(colors.warning)
                    } else if line.contains("Finished") || line.contains("success") {
                        Style::default().fg(colors.success)
                    } else if line.starts_with("   Compiling") || line.starts_with("    Checking") {
                        Style::default().fg(colors.selection)
                    } else {
                        Style::default()
                    };
                    Line::from(Span::styled(line.as_str(), style))
                })
                .collect();

            let status_info = if active_tab.is_finished {
                format!(
                    " [Finished] Line {}/{} ",
                    current_scroll + 1,
                    active_tab.buffer.len().max(1)
                )
            } else {
                format!(
                    " [Running...] Line {}/{} ",
                    current_scroll + 1,
                    active_tab.buffer.len().max(1)
                )
            };

            let output_para = Paragraph::new(visible_content)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(ratatui::widgets::BorderType::Rounded)
                        .title(status_info)
                        .border_style(Style::default().fg(if active_tab.is_finished {
                            colors.success
                        } else {
                            colors.selection
                        })),
                )
                .wrap(Wrap { trim: false });

            f.render_widget(output_para, chunks[1]);

            // Render scrollbar if content is larger than view
            if active_tab.buffer.len() > content_height {
                let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(Some("↑"))
                    .end_symbol(Some("↓"));

                let mut scrollbar_state = self
                    .scroll_state
                    .content_length(active_tab.buffer.len())
                    .viewport_content_length(content_height)
                    .position(current_scroll);

                let scrollbar_area = Rect {
                    x: chunks[1].x + chunks[1].width - 1,
                    y: chunks[1].y + 1,
                    width: 1,
                    height: chunks[1].height.saturating_sub(2),
                };

                f.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
            }
        } else {
            let empty_para = Paragraph::new(
                "No commands running.\n\nPress ':' to open command palette and run cargo commands.",
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .border_style(Style::default().fg(colors.primary)),
            )
            .style(Style::default().fg(colors.muted));
            f.render_widget(empty_para, chunks[1]);
        }
    }
}
