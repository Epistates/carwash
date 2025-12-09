//! UI module for CarWash
//!
//! This module contains all UI-related functionality including styling, theming,
//! layout management, and component rendering orchestration.

pub mod layout;
pub mod modal;
pub mod styles;
pub mod theme;

use crate::app::AppState;
use crate::components::{
    Component, dependencies::DependenciesPane, help::Help, output::TabbedOutputPane,
    palette::CommandPalette, projects::ProjectList, settings::SettingsModal, spinner::Spinner,
    status::StatusBar, text_input::TextInput, updater::UpdateWizard,
};
use crate::events::Mode;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
};

// Re-export commonly used items
pub use styles::{ColorScheme, Colors};
pub use theme::{Theme, ThemeManager};

/// Main UI rendering function
pub fn ui(f: &mut Frame, app: &mut AppState) {
    let mut dependencies = DependenciesPane::new();
    let mut output = TabbedOutputPane::new();
    let mut status = StatusBar::new();
    let mut project_list = ProjectList::new();

    if app.mode == Mode::Loading {
        let mut spinner = Spinner::new();
        spinner.draw(f, app, f.area());
        return;
    }

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)].as_ref())
        .split(f.area());

    // Use dynamic layout preferences from config
    let left_percent = app.config.layout.left_pane_percent;
    let right_percent = 100 - left_percent;
    let top_right_percent = app.config.layout.top_right_percent;
    let bottom_right_percent = 100 - top_right_percent;

    let top_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage(left_percent),
                Constraint::Percentage(right_percent),
            ]
            .as_ref(),
        )
        .split(main_chunks[0]);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage(top_right_percent),
                Constraint::Percentage(bottom_right_percent),
            ]
            .as_ref(),
        )
        .split(top_chunks[1]);

    project_list.draw(f, app, top_chunks[0]);
    dependencies.draw(f, app, right_chunks[0]);
    output.draw(f, app, right_chunks[1]);
    status.draw(f, app, main_chunks[1]);

    if app.mode == Mode::CommandPalette {
        let mut palette = CommandPalette::new();
        palette.draw(f, app, f.area());
    } else if app.mode == Mode::UpdateWizard {
        let mut updater = UpdateWizard::new();
        updater.draw(f, app, f.area());
    } else if app.mode == Mode::TextInput {
        let mut text_input = TextInput::new();
        text_input.draw(f, app, f.area());
    } else if app.mode == Mode::Help {
        let mut help = Help::new();
        help.draw(f, app, f.area());
    } else if app.mode == Mode::Settings {
        let mut settings = SettingsModal::new();
        settings.draw(f, app, f.area());
    }
}
