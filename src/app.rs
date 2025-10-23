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
    pub all_projects: Vec<Project>,
    pub collapsed_workspaces: HashSet<String>, // Track which workspaces are collapsed
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
            all_projects: self.all_projects.clone(),
            collapsed_workspaces: self.collapsed_workspaces.clone(),
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
            all_projects: Vec::new(),
            collapsed_workspaces: HashSet::new(), // All workspaces start collapsed
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
            // Get visible projects (accounting for collapsed workspaces)
            let visible_projects = self.get_visible_projects();
            visible_projects.get(selected_index).copied()
        } else {
            None
        }
    }

    /// Get list of projects that should be visible (excluding collapsed workspace members)
    pub fn get_visible_projects(&self) -> Vec<&Project> {
        let mut visible = Vec::new();
        let mut last_workspace: Option<String> = None;
        
        for project in &self.projects {
            match &project.workspace_name {
                Some(ws_name) => {
                    // If workspace changed or this is the first in a workspace, always show
                    if last_workspace.as_ref() != Some(ws_name) {
                        visible.push(project);
                        last_workspace = Some(ws_name.clone());
                    } else if !self.collapsed_workspaces.contains(ws_name) {
                        // Show member if workspace is expanded
                        visible.push(project);
                    }
                }
                None => {
                    // Standalone project, always show
                    visible.push(project);
                    last_workspace = None;
                }
            }
        }
        
        visible
    }

    /// Get the currently selected workspace name (if cursor is on a workspace item)
    pub fn get_selected_workspace(&self) -> Option<String> {
        if let Some(project) = self.get_selected_project() {
            project.workspace_name.clone()
        } else {
            None
        }
    }
}

pub fn reducer(state: &mut AppState, action: Action) {
    match action {
        Action::Quit => state.should_quit = true,
        Action::EnterNormalMode => {
            // Clear updater state when leaving update wizard
            if state.mode == Mode::UpdateWizard {
                state.updater.outdated_dependencies.clear();
                state.updater.selected_dependencies.clear();
                state.updater.list_state.select(None);
            }
            state.mode = Mode::Normal;
        }
        Action::ShowHelp => state.mode = Mode::Help,
        Action::FinishProjectScan(projects) => {
            state.all_projects = projects.clone();
            // Only show projects with dependencies
            state.projects = projects.into_iter()
                .filter(|p| !p.dependencies.is_empty())
                .collect();
            
            // Collect all workspace names and mark them as collapsed by default
            let workspace_names: HashSet<String> = state.projects.iter()
                .filter_map(|p| p.workspace_name.clone())
                .collect();
            state.collapsed_workspaces = workspace_names;
            
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
            let visible_count = state.get_visible_projects().len();
            let i = match state.tree_state.selected() {
                Some(i) => {
                    if i >= visible_count.saturating_sub(1) {
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
            let visible_count = state.get_visible_projects().len();
            let i = match state.tree_state.selected() {
                Some(i) => {
                    if i == 0 {
                        visible_count.saturating_sub(1)
                    } else {
                        i - 1
                    }
                }
                None => 0,
            };
            state.tree_state.select(Some(i));
        }
        Action::SelectParent => {
            // Collapse workspace if we're on a workspace member
            if let Some(workspace_name) = state.get_selected_workspace() {
                state.collapsed_workspaces.insert(workspace_name);
            }
        }
        Action::SelectChild => {
            // Expand workspace if we're on a workspace member
            if let Some(workspace_name) = state.get_selected_workspace() {
                state.collapsed_workspaces.remove(&workspace_name);
            }
        }
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
            // Clear any stale updater state from previous wizard sessions
            state.updater.outdated_dependencies.clear();
            state.updater.selected_dependencies.clear();
            state.updater.list_state.select(None);
            
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
        Action::StartBackgroundUpdateCheck => {
            // Background update check is handled in main event loop
        }
        Action::UpdateDependencyStatus(dep_name, status) => {
            // Update the status of a specific dependency
            if let Some(selected_project_name) = state.get_selected_project().map(|p| p.name.clone()) {
                if let Some(proj) = state.projects.iter_mut().find(|p| p.name == selected_project_name) {
                    if let Some(dep) = proj.dependencies.iter_mut().find(|d| d.name == dep_name) {
                        dep.check_status = status;
                    }
                }
            }
        }
    }
}
