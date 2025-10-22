use crate::components::{
    palette::CommandPaletteState, text_input::TextInputState, updater::UpdateWizardState,
};
use crate::events::{Action, Command, Mode};
use crate::project::Project;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::widgets::ListState;
use std::collections::HashSet;

#[derive(Debug)]
pub struct AppState<'a> {
    pub should_quit: bool,
    pub is_scanning: bool,
    pub is_checking_updates: bool,
    pub mode: Mode,
    pub tree_state: ListState,
    pub projects: Vec<Project>,
    pub selected_projects: HashSet<String>,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub command_history: Vec<String>,
    pub palette: CommandPaletteState,
    pub updater: UpdateWizardState,
    pub text_input: TextInputState,
    _phantom: std::marker::PhantomData<&'a ()>,
}

#[derive(Debug, Clone)]
pub struct Tab {
    pub title: String,
    pub buffer: Vec<String>,
    pub is_finished: bool,
}

impl<'a> Clone for AppState<'a> {
    fn clone(&self) -> Self {
        Self {
            should_quit: self.should_quit,
            is_scanning: self.is_scanning,
            is_checking_updates: self.is_checking_updates,
            mode: self.mode.clone(),
            tree_state: ListState::default(),
            projects: self.projects.clone(),
            selected_projects: self.selected_projects.clone(),
            tabs: self.tabs.clone(),
            active_tab: self.active_tab,
            command_history: self.command_history.clone(),
            palette: self.palette.clone(),
            updater: self.updater.clone(),
            text_input: self.text_input.clone(),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<'a> AppState<'a> {
    pub fn new() -> Self {
        let command_history = vec![
            "test".into(),
            "check".into(),
            "build".into(),
            "build --release".into(),
            "clean".into(),
            "clippy".into(),
            "clippy -- -D warnings".into(),
            "fmt".into(),
            "fmt -- --check".into(),
            "doc".into(),
            "doc --open".into(),
            "update".into(),
            "bench".into(),
            "run".into(),
            "run --release".into(),
        ];
        let mut tree_state = ListState::default();
        tree_state.select(Some(0));
        
        Self {
            should_quit: false,
            is_scanning: true,
            is_checking_updates: false,
            mode: Mode::Loading,
            tree_state,
            projects: Vec::new(),
            selected_projects: HashSet::new(),
            tabs: Vec::new(),
            active_tab: 0,
            command_history,
            palette: CommandPaletteState::new(),
            updater: UpdateWizardState::new(),
            text_input: TextInputState::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn get_selected_project(&self) -> Option<&Project> {
        if let Some(selected_index) = self.tree_state.selected() {
            self.projects.get(selected_index)
        } else {
            None
        }
    }
}

pub fn reducer(state: &mut AppState, action: Action) {
    match action {
        Action::Quit => state.should_quit = true,
        Action::EnterNormalMode => state.mode = Mode::Normal,
        Action::ShowHelp => state.mode = Mode::Help,
        Action::FinishProjectScan(projects) => {
            state.projects = projects;
            if !state.projects.is_empty() {
                state.tree_state.select(Some(0));
            }
            state.is_scanning = false;
            state.mode = Mode::Normal;
        }
        Action::UpdateTextInput(s) => {
            state.text_input.input = state.text_input.input.clone().with_value(s);
        }
        Action::SelectNext => {
            let i = match state.tree_state.selected() {
                Some(i) => {
                    if i >= state.projects.len().saturating_sub(1) {
                        0
                    } else {
                        i + 1
                    }
                }
                None => 0,
            };
            state.tree_state.select(Some(i));
        }
        Action::SelectPrevious => {
            let i = match state.tree_state.selected() {
                Some(i) => {
                    if i == 0 {
                        state.projects.len().saturating_sub(1)
                    } else {
                        i - 1
                    }
                }
                None => 0,
            };
            state.tree_state.select(Some(i));
        }
        Action::SelectParent => {}
        Action::SelectChild => {}
        Action::ToggleSelection => {
            if let Some(project_name) = state.get_selected_project().map(|p| p.name.clone()) {
                if !state.selected_projects.remove(&project_name) {
                    state.selected_projects.insert(project_name);
                }
            }
        }
        Action::ShowCommandPalette => {
            state.mode = Mode::CommandPalette;
            // Reset input
            state.palette.input = state.palette.input.clone().with_value(String::new());
            // Populate commands
            state.palette.filtered_commands = state.command_history
                .iter()
                .map(|c| Command::Cargo { 
                    command: c.clone()
                })
                .collect();
            // Ensure first item is selected
            if !state.palette.filtered_commands.is_empty() {
                state.palette.list_state.select(Some(0));
            } else {
                state.palette.list_state.select(None);
            }
        }
        Action::UpdatePaletteInput(input) => {
            state.palette.input = state.palette.input.clone().with_value(input.clone());
            
            if input.is_empty() {
                // Show all commands when input is empty
                state.palette.filtered_commands = state.command_history
                    .iter()
                    .map(|c| Command::Cargo { 
                        command: c.clone()
                    })
                    .collect();
            } else {
                // Filter by fuzzy match
                let matcher = SkimMatcherV2::default();
                state.palette.filtered_commands = state
                    .command_history
                    .iter()
                    .filter(|cmd| matcher.fuzzy_match(cmd, &input).is_some())
                    .map(|c| Command::Cargo { 
                        command: c.clone()
                    })
                    .collect();
            }
            
            // Select first item if available
            if !state.palette.filtered_commands.is_empty() {
                state.palette.list_state.select(Some(0));
            } else {
                state.palette.list_state.select(None);
            }
        }
        Action::PaletteSelectNext => {
            let i = match state.palette.list_state.selected() {
                Some(i) => {
                    if i >= state.palette.filtered_commands.len() - 1 {
                        0
                    } else {
                        i + 1
                    }
                }
                None => 0,
            };
            state.palette.list_state.select(Some(i));
        }
        Action::PaletteSelectPrevious => {
            let i = match state.palette.list_state.selected() {
                Some(i) => {
                    if i == 0 {
                        state.palette.filtered_commands.len() - 1
                    } else {
                        i - 1
                    }
                }
                None => 0,
            };
            state.palette.list_state.select(Some(i));
        }
        Action::StartUpdateWizard => {
            state.is_checking_updates = true;
            state.mode = Mode::UpdateWizard;
        }
        Action::ToggleUpdateSelection => {
            if let Some(index) = state.updater.list_state.selected() {
                if let Some(dep) = state.updater.outdated_dependencies.get(index) {
                    if !state.updater.selected_dependencies.remove(&dep.name) {
                        state.updater.selected_dependencies.insert(dep.name.clone());
                    }
                }
            }
        }
        Action::CheckForUpdates => {
            state.is_checking_updates = true;
        }
        Action::UpdateDependencies(deps) => {
            // Only process if we're still in UpdateWizard mode (not cancelled)
            if state.mode == Mode::UpdateWizard {
                if let Some(selected_project_name) = state.get_selected_project().map(|p| p.name.clone()) {
                    if let Some(proj) = state.projects.iter_mut().find(|p| p.name == selected_project_name) {
                        proj.dependencies = deps.clone();
                        state.updater.outdated_dependencies = deps
                            .into_iter()
                            .filter(|d| d.latest_version.is_some() && d.latest_version.as_ref().unwrap() != &d.current_version)
                            .collect();
                        // Select first item if there are outdated dependencies
                        if !state.updater.outdated_dependencies.is_empty() {
                            state.updater.list_state.select(Some(0));
                        }
                    }
                }
                state.is_checking_updates = false;
            }
            // If mode changed (cancelled), ignore the update results
        }
        Action::CreateTab(title) => {
            state.tabs.push(Tab {
                title,
                buffer: Vec::new(),
                is_finished: false,
            });
            state.active_tab = state.tabs.len() - 1;
        }
        Action::AddOutput(tab_index, line) => {
            if let Some(tab) = state.tabs.get_mut(tab_index) {
                tab.buffer.push(line);
            }
        }
        Action::FinishCommand(tab_index) => {
            if let Some(tab) = state.tabs.get_mut(tab_index) {
                tab.is_finished = true;
            }
        }
        Action::SwitchToTab(tab_index) => {
            if tab_index < state.tabs.len() {
                state.active_tab = tab_index;
            }
        }
        Action::ExecuteCommand(_command) => {
            // Command execution is handled in main event loop
        }
        Action::RunUpdate => {
            // Update execution is handled in main event loop
        }
    }
}
