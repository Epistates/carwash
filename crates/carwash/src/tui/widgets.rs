//! Small rendering helpers shared by every tab.

use super::app::App;
use super::theme::Theme;
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders};

/// A rounded pane; focused panes get an accent border and title.
pub fn pane<'a>(theme: &Theme, title: String, focused: bool) -> Block<'a> {
    let color = if focused { theme.accent } else { theme.border };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.fg(color))
        .title(Span::styled(
            format!(" {title} "),
            if focused {
                theme.bold(theme.accent)
            } else {
                theme.muted()
            },
        ))
}

/// The current spinner frame.
pub fn spinner(app: &App) -> &'static str {
    app.glyphs.spinner[app.spinner % app.glyphs.spinner.len()]
}
