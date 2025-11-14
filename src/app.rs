//! Application state management
//!
//! This module defines the core application state and UI structure for CarWash.
//! It manages the project tree, command history, tabs, and various UI modes.

use crate::components::{
    filter::FilterState, palette::CommandPaletteState, progress::ProgressState,
    settings::SettingsModalState, text_input::TextInputState, updater::UpdateWizardState,
};
use crate::config::Config;
use crate::events::{Action, Focus, Mode};
use crate::project::Project;
use crate::runner::UpdateQueue;
use crate::settings::AppSettings;
use crate::tree::{FlattenedTree, TreeNode, TreeSelectionState};
use ratatui::widgets::ListState;
use std::collections::HashSet;

/// Represents the complete state of the CarWash application
///
/// This struct maintains all mutable state including the project list, UI selections,
/// command history, and various input states for different UI modes.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Whether the application should quit
    pub should_quit: bool,
    /// Whether the application is currently scanning for projects
    pub is_scanning: bool,
    /// Whether the application is checking for updates
    pub is_checking_updates: bool,
    /// Current application mode
    pub mode: Mode,
    /// Root node of the hierarchical project tree
    pub tree_root: Option<TreeNode>,
    /// Flattened view of the tree for rendering and navigation
    pub flattened_tree: FlattenedTree,
    /// Tree navigation and selection state
    pub tree_selection: TreeSelectionState,
    /// State of the project tree view (legacy, kept for compatibility)
    pub tree_state: ListState,
    /// Filtered list of projects currently displayed
    pub projects: Vec<Project>,
    /// Complete list of all discovered projects
    pub all_projects: Vec<Project>,
    /// Set of workspace names that are collapsed in the tree view
    pub collapsed_workspaces: HashSet<String>,
    /// Set of selected project paths
    pub selected_projects: HashSet<String>,
    /// Tab panes for command output
    pub tabs: Vec<Tab>,
    /// Index of the currently active tab
    pub active_tab: usize,
    /// History of executed commands
    pub command_history: Vec<String>,
    /// State of the command palette
    pub palette: CommandPaletteState,
    /// State of the update wizard
    pub updater: UpdateWizardState,
    /// State of text input fields
    pub text_input: TextInputState,
    /// Queue of pending update checks
    pub update_queue: UpdateQueue,
    /// Persistent user settings
    pub settings: AppSettings,
    /// Modal state for editing settings
    pub settings_modal: SettingsModalState,
    /// Filter/search state
    pub filter: FilterState,
    /// Application configuration (themes, layout, keybindings, etc.)
    pub config: Config,
    /// Progress tracking for command execution
    pub progress: Option<ProgressState>,
    /// Currently focused pane
    pub focus: Focus,
}

/// Represents a tab pane for displaying command output
#[derive(Debug, Clone)]
pub struct Tab {
    /// Title/name of the tab
    pub title: String,
    /// Buffer of output lines
    pub buffer: Vec<String>,
    /// Whether the command execution has finished
    pub is_finished: bool,
}

impl Default for AppState {
    fn default() -> Self {
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
            tree_root: None,
            flattened_tree: FlattenedTree::new(),
            tree_selection: TreeSelectionState::new(),
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
            update_queue: UpdateQueue::new(),
            settings: AppSettings::load(),
            settings_modal: SettingsModalState::new(),
            filter: FilterState::new(),
            config: Config::load(),
            progress: None,
            focus: Focus::default(),
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the current active color scheme based on config
    pub fn current_colors(&self) -> crate::ui::styles::Colors {
        self.config.theme().current_scheme().colors()
    }

    /// Get keybindings from config
    pub fn get_keybindings(&self) -> std::collections::HashMap<String, Vec<String>> {
        self.config.keybindings.get_bindings()
    }

    /// Check if a key matches an action
    pub fn key_matches_action(&self, key: &str, action: &str) -> bool {
        if let Some(keys) = self.config.keybindings.get_action_keys(action) {
            keys.iter().any(|k| k.eq_ignore_ascii_case(key))
        } else {
            false
        }
    }

    pub fn get_selected_project(&self) -> Option<&Project> {
        if let Some(selected_index) = self.tree_state.selected() {
            // Get the node from the flattened tree
            if let Some((node, _index)) = self.flattened_tree.items.get(selected_index) {
                // Check if it's a project node (not a directory)
                if let crate::tree::TreeNodeType::Project(tree_project) = &node.node_type {
                    // IMPORTANT: The tree contains cloned copies of projects,
                    // but dependencies are updated in all_projects.
                    // So we need to look up the project by name in all_projects
                    // to get the current state with updated dependencies.
                    return self
                        .all_projects
                        .iter()
                        .find(|p| p.name == tree_project.name);
                }
            }
        }
        None
    }

    /// Get list of projects that should be visible (excluding collapsed workspace members)
    pub fn get_visible_projects(&self) -> Vec<&Project> {
        let mut last_workspace: Option<&String> = None;

        self.projects
            .iter()
            .filter(|project| match &project.workspace_name {
                Some(ws_name) => {
                    let is_new_workspace = last_workspace != Some(ws_name);
                    if is_new_workspace {
                        last_workspace = Some(ws_name);
                    }
                    is_new_workspace || !self.collapsed_workspaces.contains(ws_name)
                }
                None => {
                    last_workspace = None;
                    true
                }
            })
            .collect()
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

/// Main reducer function that dispatches actions to appropriate handlers
///
/// This function acts as a clean dispatch layer, delegating actual state
/// mutations to specialized handler functions in the handlers module.
pub fn reducer(state: &mut AppState, action: Action) {
    use crate::handlers::*;

    match action {
        Action::Quit => handle_quit(state),
        Action::EnterNormalMode => handle_enter_normal_mode(state),
        Action::ShowHelp => handle_show_help(state),
        Action::ShowSettings => handle_show_settings(state),
        Action::CloseSettings => handle_close_settings(state),
        Action::FinishProjectScan(projects, target_dir) => {
            handle_finish_project_scan(state, projects, target_dir)
        }
        Action::UpdateTextInput(s) => handle_update_text_input(state, s),
        Action::SelectNext => handle_select_next(state),
        Action::SelectPrevious => handle_select_previous(state),
        Action::SelectParent => handle_select_parent(state),
        Action::SelectChild => handle_select_child(state),
        Action::ToggleSelection => handle_toggle_selection(state),
        Action::ShowCommandPalette => handle_show_command_palette(state),
        Action::UpdatePaletteInput(input) => handle_update_palette_input(state, input),
        Action::PaletteSelectNext => handle_palette_select_next(state),
        Action::PaletteSelectPrevious => handle_palette_select_previous(state),
        Action::StartUpdateWizard => handle_start_update_wizard(state),
        Action::ToggleUpdateSelection => handle_toggle_update_selection(state),
        Action::CheckForUpdates => handle_check_for_updates(state),
        Action::SettingsUpdateCacheInput(input) => handle_settings_update_cache_input(state, input),
        Action::SettingsToggleBackground => handle_settings_toggle_background(state),
        Action::SaveSettings => handle_save_settings(state),
        Action::UpdateDependencies(project_name, deps) => {
            handle_update_dependencies(state, project_name, deps)
        }
        Action::UpdateDependenciesStreamStart(project_name) => {
            handle_update_dependencies_stream_start(state, project_name)
        }
        Action::UpdateSingleDependency(project_name, dep) => {
            handle_update_single_dependency(state, project_name, dep)
        }
        Action::UpdateDependencyCheckStatus(project_name, dep_name, status) => {
            handle_update_dependency_status(state, Some(project_name), dep_name, status)
        }
        Action::CreateTab(title) => handle_create_tab(state, title),
        Action::AddOutput(tab_index, line) => handle_add_output(state, tab_index, line),
        Action::FinishCommand(tab_index) => handle_finish_command(state, tab_index),
        Action::SwitchToTab(tab_index) => handle_switch_to_tab(state, tab_index),
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
            handle_update_dependency_status(state, None, dep_name, status)
        }
        Action::ProcessBackgroundUpdateQueue => {
            // Background update queue processing is handled in main event loop
        }
        Action::QueueBackgroundUpdate(project_name, is_priority) => {
            handle_queue_background_update(state, project_name, is_priority)
        }
        Action::UpdateProjectCheckStatus(project_name, check_status) => {
            handle_update_project_check_status(state, project_name, check_status)
        }
        Action::EnterFilterMode => handle_enter_filter_mode(state),
        Action::ExitFilterMode => handle_exit_filter_mode(state),
        Action::UpdateFilterInput(input) => handle_update_filter_input(state, input),
        Action::ClearFilter => handle_clear_filter(state),
        Action::CycleTheme => handle_cycle_theme(state),
        Action::SetTheme(theme_name) => handle_set_theme(state, theme_name),
        Action::IncreaseLeftPane => handle_increase_left_pane(state),
        Action::DecreaseLeftPane => handle_decrease_left_pane(state),
        Action::IncreaseTopRight => handle_increase_top_right(state),
        Action::DecreaseTopRight => handle_decrease_top_right(state),
        Action::ResetLayout => handle_reset_layout(state),
        Action::SaveConfig => handle_save_config(state),
        Action::ToggleShowAllFolders => handle_toggle_show_all_folders(state),
        Action::CalculateProjectSizes => {
            // Size calculation is handled in main event loop (async)
        }
        Action::UpdateProjectSize(project_name, total_size, target_size) => {
            handle_update_project_size(state, project_name, total_size, target_size)
        }
        Action::FocusNext => handle_focus_next(state),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Action, Mode};
    use crate::project::{Project, ProjectStatus};
    use std::path::PathBuf;

    fn create_test_project(name: &str) -> Project {
        Project {
            name: name.to_string(),
            path: PathBuf::from("test"),
            status: ProjectStatus::Pending,
            version: "0.1.0".to_string(),
            authors: vec![],
            dependencies: vec![],
            workspace_root: None,
            workspace_name: None,
            cargo_lock_hash: None,
            check_status: crate::project::ProjectCheckStatus::Unchecked,
            total_size: None,
            target_size: None,
        }
    }

    #[test]
    fn test_app_state_new() {
        let state = AppState::new();
        assert!(!state.should_quit);
        assert!(state.is_scanning);
        assert_eq!(state.mode, Mode::Loading);
        assert!(state.projects.is_empty());
        assert!(state.selected_projects.is_empty());
        assert!(!state.command_history.is_empty());
    }

    #[test]
    fn test_reducer_quit() {
        let mut state = AppState::new();
        assert!(!state.should_quit);

        reducer(&mut state, Action::Quit);
        assert!(state.should_quit);
    }

    #[test]
    fn test_reducer_enter_normal_mode() {
        let mut state = AppState::new();
        state.mode = Mode::Loading;

        reducer(&mut state, Action::EnterNormalMode);
        assert_eq!(state.mode, Mode::Normal);
    }

    #[test]
    fn test_reducer_show_help() {
        let mut state = AppState::new();

        reducer(&mut state, Action::ShowHelp);
        assert_eq!(state.mode, Mode::Help);
    }

    #[test]
    fn test_reducer_finish_project_scan() {
        let mut state = AppState::new();
        let mut project = create_test_project("test1");
        project.dependencies = vec![]; // Empty dependencies - should be filtered out
        let projects = vec![project];

        reducer(
            &mut state,
            Action::FinishProjectScan(projects, ".".to_string()),
        );
        assert!(!state.is_scanning);
        assert_eq!(state.mode, Mode::Normal);
        // Projects with empty dependencies should be filtered
        assert_eq!(state.projects.len(), 0);
    }

    #[test]
    fn test_reducer_toggle_selection() {
        let mut state = AppState::new();
        let project = create_test_project("test1");
        state.projects = vec![project.clone()];

        // Create a simple tree with one project node
        let project_node = crate::tree::TreeNode::project(project, 0);
        let mut flattened = crate::tree::FlattenedTree::new();
        flattened.items.push((project_node, 0));
        state.flattened_tree = flattened;
        state.tree_state.select(Some(0));

        // Select the project
        reducer(&mut state, Action::ToggleSelection);
        assert!(state.selected_projects.contains("test1"));

        // Deselect the project
        reducer(&mut state, Action::ToggleSelection);
        assert!(!state.selected_projects.contains("test1"));
    }

    #[test]
    fn test_reducer_show_command_palette() {
        let mut state = AppState::new();

        reducer(&mut state, Action::ShowCommandPalette);
        assert_eq!(state.mode, Mode::CommandPalette);
        assert!(!state.palette.filtered_commands.is_empty());
    }

    #[test]
    fn test_reducer_create_tab() {
        let mut state = AppState::new();
        let title = "Test Tab".to_string();

        reducer(&mut state, Action::CreateTab(title.clone()));
        assert_eq!(state.tabs.len(), 1);
        assert_eq!(state.tabs[0].title, title);
        assert!(!state.tabs[0].is_finished);
    }

    #[test]
    fn test_reducer_add_output() {
        let mut state = AppState::new();
        reducer(&mut state, Action::CreateTab("Test".to_string()));

        let output = "Test output".to_string();
        reducer(&mut state, Action::AddOutput(0, output.clone()));
        assert_eq!(state.tabs[0].buffer.len(), 1);
        assert_eq!(state.tabs[0].buffer[0], output);
    }

    #[test]
    fn test_reducer_finish_command() {
        let mut state = AppState::new();
        reducer(&mut state, Action::CreateTab("Test".to_string()));

        reducer(&mut state, Action::FinishCommand(0));
        assert!(state.tabs[0].is_finished);
    }

    #[test]
    fn test_reducer_switch_to_tab() {
        let mut state = AppState::new();
        reducer(&mut state, Action::CreateTab("Tab1".to_string()));
        reducer(&mut state, Action::CreateTab("Tab2".to_string()));

        state.active_tab = 0;
        reducer(&mut state, Action::SwitchToTab(1));
        assert_eq!(state.active_tab, 1);
    }

    #[test]
    fn test_get_visible_projects_with_collapsed_workspace() {
        let mut state = AppState::new();

        let mut project1 = create_test_project("workspace-member1");
        project1.workspace_name = Some("test-workspace".to_string());

        let mut project2 = create_test_project("workspace-member2");
        project2.workspace_name = Some("test-workspace".to_string());

        state.projects = vec![project1, project2];
        state
            .collapsed_workspaces
            .insert("test-workspace".to_string());

        let visible = state.get_visible_projects();
        // Only the first member of collapsed workspace should be visible
        assert_eq!(visible.len(), 1);
    }

    #[test]
    fn test_get_visible_projects_with_expanded_workspace() {
        let mut state = AppState::new();

        let mut project1 = create_test_project("workspace-member1");
        project1.workspace_name = Some("test-workspace".to_string());

        let mut project2 = create_test_project("workspace-member2");
        project2.workspace_name = Some("test-workspace".to_string());

        state.projects = vec![project1, project2];
        // Don't collapse the workspace

        let visible = state.get_visible_projects();
        // All members should be visible
        assert_eq!(visible.len(), 2);
    }

    #[test]
    fn test_tab_clone() {
        let tab = Tab {
            title: "Test".to_string(),
            buffer: vec!["line1".to_string()],
            is_finished: false,
        };

        let cloned = tab.clone();
        assert_eq!(tab.title, cloned.title);
        assert_eq!(tab.buffer.len(), cloned.buffer.len());
        assert_eq!(tab.is_finished, cloned.is_finished);
    }
}
