use crate::app::AppState;
use crate::components::{
    Component, dependencies::DependenciesPane, help::Help, output::TabbedOutputPane,
    palette::CommandPalette, projects::ProjectList, status::StatusBar, text_input::TextInput,
    updater::UpdateWizard,
};
use crate::events::Mode;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph},
};

pub fn ui(f: &mut Frame, app: &mut AppState) {
    let mut dependencies = DependenciesPane::new();
    let mut output = TabbedOutputPane::new();
    let mut status = StatusBar::new();
    let mut project_list = ProjectList::new();

    if app.mode == Mode::Loading {
        let loading_text = "Scanning for projects...";
        let loading = Paragraph::new(loading_text)
            .block(Block::default().borders(Borders::ALL).title("Loading"))
            .alignment(Alignment::Center);
        f.render_widget(loading, f.area());
        return;
    }

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)].as_ref())
        .split(f.area());

    let top_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)].as_ref())
        .split(main_chunks[0]);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)].as_ref())
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
    }
}
