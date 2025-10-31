//! Action handlers for state mutations
//!
//! This module contains handler functions for each Action type, providing
//! a clean separation between action dispatch and state mutation logic.

use crate::app::{AppState, Tab};
use crate::events::{Command, Mode};
use crate::project::Project;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use std::collections::HashSet;

/// Handle application quit
pub fn handle_quit(state: &mut AppState) {
    state.should_quit = true;
    let _ = state.settings.save();
}

/// Handle entering normal mode
pub fn handle_enter_normal_mode(state: &mut AppState) {
    // Clear updater state when leaving update wizard
    if state.mode == Mode::UpdateWizard {
        state.updater.outdated_dependencies.clear();
        state.updater.selected_dependencies.clear();
        state.updater.list_state.select(None);
        state.updater.locked_project_name = None; // Clear project lock
        state.updater.user_check_in_progress = false; // Clear check flag
    }
    state.mode = Mode::Normal;
}

/// Handle showing help
pub fn handle_show_help(state: &mut AppState) {
    state.mode = Mode::Help;
}

/// Show settings modal populated with persisted values
pub fn handle_show_settings(state: &mut AppState) {
    state.settings_modal.cache_minutes_input = state
        .settings_modal
        .cache_minutes_input
        .clone()
        .with_value(state.settings.cache_ttl_minutes.to_string());
    state.settings_modal.background_updates_enabled = state.settings.background_updates_enabled;
    state.settings_modal.error_message = None;
    state.mode = Mode::Settings;
}

/// Close settings modal without persisting changes
pub fn handle_close_settings(state: &mut AppState) {
    state.mode = Mode::Normal;
    state.settings_modal.error_message = None;
}

/// Handle completing project scan
pub fn handle_finish_project_scan(state: &mut AppState, projects: Vec<Project>) {
    state.all_projects = projects.clone();
    // Only show projects with dependencies
    state.projects = projects
        .into_iter()
        .filter(|p| !p.dependencies.is_empty())
        .collect();

    // Collect all workspace names and mark them as collapsed by default
    let workspace_names: HashSet<String> = state
        .projects
        .iter()
        .filter_map(|p| p.workspace_name.clone())
        .collect();
    state.collapsed_workspaces = workspace_names;

    if !state.projects.is_empty() {
        state.tree_state.select(Some(0));
    }
    state.is_scanning = false;
    state.mode = Mode::Normal;
}

/// Handle text input updates
pub fn handle_update_text_input(state: &mut AppState, s: String) {
    state.text_input.input = state.text_input.input.clone().with_value(s);
}

/// Handle selecting next item in list
pub fn handle_select_next(state: &mut AppState) {
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

/// Handle selecting previous item in list
pub fn handle_select_previous(state: &mut AppState) {
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

/// Handle selecting parent (collapse workspace)
pub fn handle_select_parent(state: &mut AppState) {
    if let Some(workspace_name) = state.get_selected_workspace() {
        state.collapsed_workspaces.insert(workspace_name);
    }
}

/// Handle selecting child (expand workspace)
pub fn handle_select_child(state: &mut AppState) {
    if let Some(workspace_name) = state.get_selected_workspace() {
        state.collapsed_workspaces.remove(&workspace_name);
    }
}

/// Handle toggling project selection
pub fn handle_toggle_selection(state: &mut AppState) {
    if let Some(selected_index) = state.tree_state.selected() {
        let visible_projects = state.get_visible_projects();

        if let Some(project) = visible_projects.get(selected_index) {
            if let Some(workspace_name) = project.workspace_name.clone() {
                let is_workspace_header = visible_projects
                    .iter()
                    .position(|p| p.workspace_name.as_ref() == Some(&workspace_name))
                    == Some(selected_index);

                if is_workspace_header {
                    let workspace_members: Vec<String> = state
                        .projects
                        .iter()
                        .filter(|p| p.workspace_name.as_ref() == Some(&workspace_name))
                        .map(|p| p.name.clone())
                        .collect();

                    if workspace_members.is_empty() {
                        return;
                    }

                    let all_selected = workspace_members
                        .iter()
                        .all(|name| state.selected_projects.contains(name));

                    if all_selected {
                        for name in workspace_members {
                            state.selected_projects.remove(&name);
                        }
                    } else {
                        for name in workspace_members {
                            state.selected_projects.insert(name);
                        }
                    }
                    return;
                }
            }

            let project_name = project.name.clone();
            if !state.selected_projects.remove(&project_name) {
                state.selected_projects.insert(project_name);
            }
        }
    }
}

/// Update cache duration input while editing settings
pub fn handle_settings_update_cache_input(state: &mut AppState, input: String) {
    state.settings_modal.cache_minutes_input = state
        .settings_modal
        .cache_minutes_input
        .clone()
        .with_value(input);
    state.settings_modal.error_message = None;
}

/// Toggle background update preference while editing settings
pub fn handle_settings_toggle_background(state: &mut AppState) {
    state.settings_modal.background_updates_enabled =
        !state.settings_modal.background_updates_enabled;
}

/// Persist settings from the modal, validating input and updating background queue
pub fn handle_save_settings(state: &mut AppState) {
    let raw_value = state.settings_modal.cache_minutes_input.value().trim();
    let parsed_minutes = raw_value.parse::<u64>().ok().filter(|v| *v > 0);

    let minutes = match parsed_minutes {
        Some(value) => value,
        None => {
            state.settings_modal.error_message =
                Some("Cache TTL must be a positive number of minutes".to_string());
            return;
        }
    };

    let mut new_settings = state.settings.clone();
    let was_background_enabled = new_settings.background_updates_enabled;
    new_settings.cache_ttl_minutes = minutes;
    new_settings.background_updates_enabled = state.settings_modal.background_updates_enabled;

    if let Err(err) = new_settings.save() {
        state.settings_modal.error_message = Some(format!("Failed to save settings: {}", err));
        return;
    }

    state.settings = new_settings;
    state.settings_modal.error_message = None;
    state.mode = Mode::Normal;

    if !state.settings.background_updates_enabled {
        state.update_queue.clear();
        state.is_checking_updates = false;
        return;
    }

    if !was_background_enabled && state.settings.background_updates_enabled {
        let now = std::time::SystemTime::now();
        let cache_duration = state.settings.cache_duration();

        for project in &state.all_projects {
            if project.dependencies.is_empty() {
                continue;
            }

            let needs_check = project.dependencies.iter().any(|dep| {
                if let Some(last_checked) = dep.last_checked {
                    if let Ok(elapsed) = now.duration_since(last_checked) {
                        elapsed > cache_duration
                    } else {
                        true
                    }
                } else {
                    true
                }
            });

            if needs_check {
                state.update_queue.add_task(crate::runner::UpdateCheckTask {
                    project_name: project.name.clone(),
                    is_priority: false,
                });
            }
        }
    }
}

/// Handle showing command palette
pub fn handle_show_command_palette(state: &mut AppState) {
    state.mode = Mode::CommandPalette;
    // Reset input
    state.palette.input = state.palette.input.clone().with_value(String::new());
    // Populate commands
    state.palette.filtered_commands = state
        .command_history
        .iter()
        .map(|c| Command::Cargo { command: c.clone() })
        .collect();
    // Ensure first item is selected
    if !state.palette.filtered_commands.is_empty() {
        state.palette.list_state.select(Some(0));
    } else {
        state.palette.list_state.select(None);
    }
}

/// Handle updating command palette input
pub fn handle_update_palette_input(state: &mut AppState, input: String) {
    state.palette.input = state.palette.input.clone().with_value(input.clone());

    if input.is_empty() {
        // Show all commands when input is empty
        state.palette.filtered_commands = state
            .command_history
            .iter()
            .map(|c| Command::Cargo { command: c.clone() })
            .collect();
    } else {
        // Filter by fuzzy match
        let matcher = SkimMatcherV2::default();
        state.palette.filtered_commands = state
            .command_history
            .iter()
            .filter(|cmd| matcher.fuzzy_match(cmd, &input).is_some())
            .map(|c| Command::Cargo { command: c.clone() })
            .collect();
    }

    // Select first item if available
    if !state.palette.filtered_commands.is_empty() {
        state.palette.list_state.select(Some(0));
    } else {
        state.palette.list_state.select(None);
    }
}

/// Handle palette next selection
pub fn handle_palette_select_next(state: &mut AppState) {
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

/// Handle palette previous selection
pub fn handle_palette_select_previous(state: &mut AppState) {
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

/// Handle starting update wizard
pub fn handle_start_update_wizard(state: &mut AppState) {
    // Clear any stale updater state from previous wizard sessions
    state.updater.outdated_dependencies.clear();
    state.updater.selected_dependencies.clear();
    state.updater.list_state.select(None);

    // CRITICAL: Lock the wizard to the currently selected project
    // This prevents background updates for other projects from changing the wizard display
    if let Some(project) = state.get_selected_project() {
        let project_name = project.name.clone();

        // CRITICAL: Populate wizard with CURRENT dependency data immediately
        // This ensures the wizard shows up-to-date data even if background check
        // completed BEFORE the wizard opened (race condition fix)
        let outdated_deps: Vec<_> = project
            .dependencies
            .iter()
            .filter(|d| {
                d.latest_version
                    .as_ref()
                    .map(|latest| latest != &d.current_version)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        // Now we can mutate state.updater
        state.updater.locked_project_name = Some(project_name);
        state.updater.outdated_dependencies = outdated_deps;

        // Select first item if there are outdated dependencies
        if !state.updater.outdated_dependencies.is_empty() {
            state.updater.list_state.select(Some(0));
        }

        // CRITICAL FIX: Set user_check_in_progress IMMEDIATELY, not when StreamStart arrives
        // This prevents race condition where UpdateDependencies arrives before StreamStart
        // (happens when cache hits instantly)
        state.updater.user_check_in_progress = true;
    } else {
        state.updater.locked_project_name = None;
    }

    state.is_checking_updates = true;
    state.mode = Mode::UpdateWizard;
}

/// Handle toggling update selection
pub fn handle_toggle_update_selection(state: &mut AppState) {
    if let Some(index) = state.updater.list_state.selected() {
        if let Some(dep) = state.updater.outdated_dependencies.get(index) {
            if !state.updater.selected_dependencies.remove(&dep.name) {
                state.updater.selected_dependencies.insert(dep.name.clone());
            }
        }
    }
}

/// Handle check for updates request
pub fn handle_check_for_updates(state: &mut AppState) {
    state.is_checking_updates = true;
}

/// Handle updating dependencies for a project
pub fn handle_update_dependencies(
    state: &mut AppState,
    project_name: String,
    deps: Vec<crate::project::Dependency>,
) {
    // CRITICAL: Check if wizard is open AND if this is the LOCKED project
    // This prevents background updates for other projects from changing the wizard display
    let is_wizard_locked_project = state.mode == Mode::UpdateWizard
        && state.updater.locked_project_name.as_ref() == Some(&project_name);

    // Calculate project check status based on dependencies
    let new_check_status = Project::compute_check_status_from_deps(&deps);

    // Update in all_projects (the source of truth)
    // CRITICAL: ALWAYS update, even when wizard is open!
    // This ensures all 3 views (wizard, explorer, dependency pane) update simultaneously
    if let Some(all_proj) = state
        .all_projects
        .iter_mut()
        .find(|p| p.name == project_name)
    {
        all_proj.dependencies = deps.clone();
        all_proj.check_status = new_check_status.clone();
    }

    // Also update in filtered projects if it exists there
    // CRITICAL: ALWAYS update, even when wizard is open!
    if let Some(proj) = state.projects.iter_mut().find(|p| p.name == project_name) {
        proj.dependencies = deps.clone();
        proj.check_status = new_check_status;
    }

    // CRITICAL: Update wizard display UNCONDITIONALLY if this is the LOCKED project
    // Don't nest this inside the state.projects.find() block above!
    // The project might not be in the filtered list, but wizard should still update
    if is_wizard_locked_project {
        state.updater.outdated_dependencies = deps
            .into_iter()
            .filter(|d| {
                d.latest_version
                    .as_ref()
                    .map(|latest| latest != &d.current_version)
                    .unwrap_or(false)
            })
            .collect();

        // Select first item if there are outdated dependencies
        if !state.updater.outdated_dependencies.is_empty() {
            state.updater.list_state.select(Some(0));
        }

        // CRITICAL FIX: ALWAYS clear the checking flag when we update the wizard
        // Don't be too clever about user_check_in_progress - if we're updating the wizard
        // with dependency data for the locked project, the check is DONE. Period.
        // Any extra complexity just creates bugs.
        state.is_checking_updates = false;
        state.updater.user_check_in_progress = false;
    }
}

/// Handle start of dependency update stream
///
/// This is ONLY called for user-initiated checks (pressing 'u'), not background checks.
/// NOTE: This action may arrive AFTER UpdateDependencies when cache hits instantly!
pub fn handle_update_dependencies_stream_start(state: &mut AppState, _project_name: String) {
    // CRITICAL FIX: Don't blindly set is_checking_updates = true!
    // If UpdateDependencies already arrived (cache hit), it would have cleared the flag.
    // Setting it back to true here would leave the wizard stuck "checking" forever.
    //
    // We already set is_checking_updates = true in handle_start_update_wizard,
    // and user_check_in_progress = true there too. This action is just a signal that
    // the check started, but by the time we process it, the check might be done!
    //
    // So we do NOTHING here. The flag is already set correctly.

    // For non-wizard checks, we still want to set the flag
    if state.mode != Mode::UpdateWizard {
        state.is_checking_updates = true;
    }

    // Note: We DON'T set user_check_in_progress here anymore.
    // It's already set immediately in handle_start_update_wizard.
    // This prevents race conditions where UpdateDependencies arrives before this action.
}

/// Handle updating a single dependency
pub fn handle_update_single_dependency(
    state: &mut AppState,
    project_name: String,
    dep: crate::project::Dependency,
) {
    // CRITICAL: Check if wizard is open AND if this is the LOCKED project
    let is_wizard_locked_project = state.mode == Mode::UpdateWizard
        && state.updater.locked_project_name.as_ref() == Some(&project_name);

    // Update in all_projects first (source of truth)
    if let Some(all_proj) = state
        .all_projects
        .iter_mut()
        .find(|p| p.name == project_name)
    {
        if let Some(existing_dep) = all_proj
            .dependencies
            .iter_mut()
            .find(|d| d.name == dep.name)
        {
            *existing_dep = dep.clone();
        }
    }

    // Also update in filtered projects if it exists
    if let Some(proj) = state.projects.iter_mut().find(|p| p.name == project_name) {
        if let Some(existing_dep) = proj.dependencies.iter_mut().find(|d| d.name == dep.name) {
            *existing_dep = dep.clone();
        }

        // ONLY update wizard display if this is the LOCKED project
        if is_wizard_locked_project {
            state.updater.outdated_dependencies = proj
                .dependencies
                .iter()
                .filter(|d| {
                    d.latest_version
                        .as_ref()
                        .map(|latest| latest != &d.current_version)
                        .unwrap_or(false)
                })
                .cloned()
                .collect();
        }
    }
}

/// Handle tab creation
pub fn handle_create_tab(state: &mut AppState, title: String) {
    state.tabs.push(Tab {
        title,
        buffer: Vec::new(),
        is_finished: false,
    });
    state.active_tab = state.tabs.len() - 1;
}

/// Handle adding output to tab
pub fn handle_add_output(state: &mut AppState, tab_index: usize, line: String) {
    if let Some(tab) = state.tabs.get_mut(tab_index) {
        tab.buffer.push(line);
    }
}

/// Handle command finish
pub fn handle_finish_command(state: &mut AppState, tab_index: usize) {
    if let Some(tab) = state.tabs.get_mut(tab_index) {
        tab.is_finished = true;
    }
}

/// Handle switching to tab
pub fn handle_switch_to_tab(state: &mut AppState, tab_index: usize) {
    if tab_index < state.tabs.len() {
        state.active_tab = tab_index;
    }
}

/// Handle updating dependency status
pub fn handle_update_dependency_status(
    state: &mut AppState,
    project_name: Option<String>,
    dep_name: String,
    status: crate::project::DependencyCheckStatus,
) {
    let target_names: Vec<String> = if let Some(name) = project_name {
        vec![name]
    } else {
        state
            .get_selected_project()
            .map(|p| p.name.clone())
            .into_iter()
            .collect()
    };

    for project_name in target_names {
        if let Some(proj) = state
            .all_projects
            .iter_mut()
            .find(|p| p.name == project_name)
        {
            if let Some(dep) = proj.dependencies.iter_mut().find(|d| d.name == dep_name) {
                dep.check_status = status.clone();
            }
        }

        if let Some(proj) = state.projects.iter_mut().find(|p| p.name == project_name) {
            if let Some(dep) = proj.dependencies.iter_mut().find(|d| d.name == dep_name) {
                dep.check_status = status.clone();
            }
        }

        if state.mode == crate::events::Mode::UpdateWizard
            && state.updater.locked_project_name.as_ref() == Some(&project_name)
        {
            if let Some(dep) = state
                .updater
                .outdated_dependencies
                .iter_mut()
                .find(|d| d.name == dep_name)
            {
                dep.check_status = status.clone();
            }
        }
    }
}

/// Handle queuing background update
pub fn handle_queue_background_update(
    state: &mut AppState,
    project_name: String,
    is_priority: bool,
) {
    state.update_queue.add_task(crate::runner::UpdateCheckTask {
        project_name,
        is_priority,
    });
}

/// Handle updating project check status
pub fn handle_update_project_check_status(
    state: &mut AppState,
    project_name: String,
    check_status: crate::project::ProjectCheckStatus,
) {
    // Update in all_projects (source of truth)
    if let Some(proj) = state
        .all_projects
        .iter_mut()
        .find(|p| p.name == project_name)
    {
        proj.check_status = check_status.clone();
    }
    // Also update in filtered projects list
    if let Some(proj) = state.projects.iter_mut().find(|p| p.name == project_name) {
        proj.check_status = check_status;
    }
}
