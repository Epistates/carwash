use carwash::Args;
use carwash::app::{AppState, reducer};
use carwash::cache::DependencyVersionCache;
use carwash::components::{
    Component, dependencies::DependenciesPane, help::Help, output::TabbedOutputPane,
    palette::CommandPalette, projects::ProjectList, settings::SettingsModal, text_input::TextInput,
    updater::UpdateWizard,
};
use carwash::events::{Action, Command, Focus, Mode};
use carwash::project::ProjectCheckStatus;
use carwash::runner::{check_dependencies_with_cache, check_for_updates, run_command};
use carwash::tree::TreeNode;
use carwash::ui::ui;

use clap::Parser;
use crossterm::{
    event::{Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
};
use std::io;
use tokio::sync::mpsc;

use anyhow::Context;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Setup logging to file
    let file_appender = tracing_appender::rolling::never(std::env::temp_dir(), "carwash.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    // Set up panic handler to ensure clean terminal restoration
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        original_hook(panic_info);
    }));

    let args = Args::parse();

    // Check if we have a TTY (after argument parsing so --help works)
    if !crossterm::tty::IsTty::is_tty(&io::stdin()) {
        anyhow::bail!("CarWash requires an interactive terminal (TTY).");
    }

    let mut terminal = setup_terminal().context("Failed to set up terminal")?;

    // Clear screen immediately to prevent any error messages from showing
    let _ = terminal.clear();

    let mut state = AppState::new();
    let res = run_app(&mut terminal, &mut state, args.target_directory).await;

    restore_terminal().context("Failed to restore terminal")?;

    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
        std::process::exit(1);
    }

    Ok(())
}

fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("Failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("Failed to create terminal")
}

fn restore_terminal() -> anyhow::Result<()> {
    disable_raw_mode().context("Failed to disable raw mode")?;
    execute!(io::stdout(), LeaveAlternateScreen).context("Failed to leave alternate screen")?;
    Ok(())
}

/// Apply the global dep version cache to a slice of projects.
/// Sets latest_version, check_status, and last_checked on each dependency that has a cached entry.
fn apply_dep_version_cache(
    projects: &mut [carwash::project::Project],
    cache: &DependencyVersionCache,
    cache_duration: std::time::Duration,
) {
    use carwash::project::{DependencyCheckStatus, Project};

    for project in projects.iter_mut() {
        for dep in &mut project.dependencies {
            if let Some(entry) = cache.lookup(&dep.name, &dep.current_version, cache_duration) {
                dep.latest_version = Some(entry.latest.clone());
                dep.check_status = DependencyCheckStatus::Checked;
                dep.last_checked = Some(
                    std::time::SystemTime::UNIX_EPOCH
                        + std::time::Duration::from_secs(entry.checked_at),
                );
            }
        }
        project.check_status = Project::compute_check_status_from_deps(&project.dependencies);
    }
}

fn apply_dep_version_cache_to_tree(
    node: &mut TreeNode,
    cache: &DependencyVersionCache,
    cache_duration: std::time::Duration,
) {
    if let carwash::tree::TreeNodeType::Project(project) = &mut node.node_type {
        apply_dep_version_cache(std::slice::from_mut(project), cache, cache_duration);
    }

    for child in &mut node.children {
        apply_dep_version_cache_to_tree(child, cache, cache_duration);
    }
}

fn normalize_target_directory(target_directory: &str) -> String {
    let path = std::path::Path::new(target_directory);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    std::fs::canonicalize(&absolute)
        .unwrap_or(absolute)
        .to_string_lossy()
        .to_string()
}

async fn handle_event(
    event: Event,
    state: &mut AppState,
    action_tx: &mpsc::Sender<Action>,
) -> anyhow::Result<()> {
    if let Event::Key(key) = event {
        if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
            reducer(state, Action::Quit);
        }

        let action: Option<Action> = match state.mode {
            Mode::Loading => {
                // Allow quitting even while loading
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
                    _ => None,
                }
            }
            Mode::Normal => {
                // Handle normal mode keys without interfering with workspace navigation
                match key.code {
                    KeyCode::Tab => {
                        // Tab always cycles focus between panes
                        // Output pane uses h/l or Left/Right for tab switching
                        Some(Action::FocusNext)
                    }
                    KeyCode::BackTab => {
                        // Shift+Tab: cycle focus backwards
                        // Output -> Dependencies -> Projects -> Output
                        state.focus = match state.focus {
                            Focus::Projects => Focus::Output,
                            Focus::Dependencies => Focus::Projects,
                            Focus::Output => Focus::Dependencies,
                        };
                        None
                    }
                    KeyCode::Char('q') => Some(Action::Quit),
                    KeyCode::Char('?') => Some(Action::ShowHelp),
                    KeyCode::Char('s') | KeyCode::Char('S') => Some(Action::ShowSettings),
                    KeyCode::Char('t') | KeyCode::Char('T') => Some(Action::CycleTheme),
                    KeyCode::Char('a') | KeyCode::Char('A') => Some(Action::ToggleShowAllFolders),
                    KeyCode::Char(':') => Some(Action::ShowCommandPalette),
                    KeyCode::Char('/') => Some(Action::EnterFilterMode),
                    KeyCode::Char('u') => Some(Action::StartUpdateWizard),
                    // Ctrl+[ and Ctrl+] for output tab navigation (works regardless of focus)
                    KeyCode::Char('[') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        // Previous tab
                        if state.active_tab > 0 {
                            Some(Action::SwitchToTab(state.active_tab - 1))
                        } else if !state.tabs.is_empty() {
                            // Wrap around to last tab
                            Some(Action::SwitchToTab(state.tabs.len() - 1))
                        } else {
                            None
                        }
                    }
                    KeyCode::Char(']') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        // Next tab
                        if state.active_tab < state.tabs.len().saturating_sub(1) {
                            Some(Action::SwitchToTab(state.active_tab + 1))
                        } else if !state.tabs.is_empty() {
                            // Wrap around to first tab
                            Some(Action::SwitchToTab(0))
                        } else {
                            None
                        }
                    }
                    // Layout adjustment controls
                    KeyCode::Char('{') | KeyCode::Char('[') => Some(Action::DecreaseLeftPane),
                    KeyCode::Char('}') | KeyCode::Char(']') => Some(Action::IncreaseLeftPane),
                    KeyCode::Char('(') | KeyCode::Char('-') => Some(Action::IncreaseTopRight),
                    KeyCode::Char(')') | KeyCode::Char('+') => Some(Action::DecreaseTopRight),
                    KeyCode::Char('r') | KeyCode::Char('R')
                        if key.modifiers.contains(KeyModifiers::SHIFT) =>
                    {
                        Some(Action::ResetLayout)
                    }
                    KeyCode::Char('r') | KeyCode::Char('R') => {
                        // Dispatch to focused component
                        match state.focus {
                            Focus::Projects => {
                                let mut project_list = ProjectList::new();
                                project_list.handle_key_events(key.code, state)
                            }
                            Focus::Dependencies => {
                                let mut deps = DependenciesPane::new();
                                deps.handle_key_events(key.code, state)
                            }
                            Focus::Output => {
                                let mut output = TabbedOutputPane::new();
                                output.handle_key_events(key.code, state)
                            }
                        }
                    }
                    _ => {
                        // Dispatch to focused component
                        match state.focus {
                            Focus::Projects => {
                                let mut project_list = ProjectList::new();
                                project_list.handle_key_events(key.code, state)
                            }
                            Focus::Dependencies => {
                                let mut deps = DependenciesPane::new();
                                deps.handle_key_events(key.code, state)
                            }
                            Focus::Output => {
                                let mut output = TabbedOutputPane::new();
                                output.handle_key_events(key.code, state)
                            }
                        }
                    }
                }
            }
            Mode::CommandPalette => {
                let mut palette = CommandPalette::new();
                palette.handle_key_events(key.code, state)
            }
            Mode::UpdateWizard => {
                let mut updater = UpdateWizard::new();
                updater.handle_key_events(key.code, state)
            }
            Mode::TextInput => {
                let mut text_input = TextInput::new();
                text_input.handle_key_events(key.code, state)
            }
            Mode::Help => {
                let mut help = Help::new();
                help.handle_key_events(key.code, state)
            }
            Mode::Settings => {
                let mut settings = SettingsModal::new();
                settings.handle_key_events(key.code, state)
            }
            Mode::Filter => {
                // Handle filter mode keys
                match key.code {
                    KeyCode::Esc => Some(Action::ExitFilterMode),
                    KeyCode::Enter => Some(Action::ExitFilterMode),
                    KeyCode::Char(c) => Some(Action::UpdateFilterInput(
                        state.filter.input.clone() + &c.to_string(),
                    )),
                    KeyCode::Backspace => {
                        let mut input = state.filter.input.clone();
                        input.pop();
                        Some(Action::UpdateFilterInput(input))
                    }
                    KeyCode::Up => {
                        state.filter.select_previous();
                        None
                    }
                    KeyCode::Down => {
                        state.filter.select_next();
                        None
                    }
                    _ => None,
                }
            }
        };

        if let Some(action) = action {
            // Some actions need to be sent through the action channel for async processing
            match &action {
                Action::ExecuteCommand(_)
                | Action::StartUpdateWizard
                | Action::RunUpdate
                | Action::UpdateDependencies(..)
                | Action::UpdateSingleDependency(..)
                | Action::UpdateDependenciesStreamStart(_) => {
                    // Send through channel for async handling
                    let _ = action_tx.send(action).await;
                }
                Action::FinishCommand(_) => {
                    // Handle sync first, then check if we need async reload
                    reducer(state, action.clone());
                    // Only route to async if there's a pending update reload
                    if state.updater.pending_reload_project.is_some() {
                        let _ = action_tx.send(action).await;
                    }
                }
                _ => {
                    // Handle synchronously through reducer
                    reducer(state, action.clone());
                }
            }
        }
    }
    Ok(())
}

fn reset_checking_status(state: &mut AppState) {
    for project in &mut state.all_projects {
        if project.check_status == ProjectCheckStatus::Checking {
            project.check_status = ProjectCheckStatus::Unchecked;
        }
    }
    for project in &mut state.projects {
        if project.check_status == ProjectCheckStatus::Checking {
            project.check_status = ProjectCheckStatus::Unchecked;
        }
    }
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    state: &mut AppState,
    target_directory: String,
) -> anyhow::Result<()> {
    let target_directory = normalize_target_directory(&target_directory);
    let target_path = std::path::Path::new(&target_directory);
    if !target_path.is_dir() {
        anyhow::bail!(
            "Target directory does not exist or is not a directory: {}",
            target_path.display()
        );
    }

    let (action_tx, mut action_rx) = mpsc::channel(100);
    let mut event_stream = crossterm::event::EventStream::new();

    // Set up frame rate for consistent redraws (following ratatui async pattern)
    const FRAMES_PER_SECOND: f32 = 30.0;
    let period = std::time::Duration::from_secs_f32(1.0 / FRAMES_PER_SECOND);
    let mut interval = tokio::time::interval(period);

    // Track last cache save time for periodic persistence
    let mut last_cache_save = std::time::Instant::now();

    // Trigger initial shallow scan immediately
    let init_tx = action_tx.clone();
    let init_target = target_directory.clone();
    tokio::spawn(async move {
        let _ = init_tx.send(Action::InitializeTree(init_target)).await;
    });

    // Load global per-dependency version cache (shared across all projects)
    let dep_cache = std::sync::Arc::new(tokio::sync::RwLock::new({
        let mut cache = DependencyVersionCache::load();
        // One-time migration from old per-project cache files
        if cache.is_empty() {
            cache = DependencyVersionCache::migrate_from_project_caches();
        }
        cache
    }));

    loop {
        tokio::select! {
            // Prioritize keyboard events with biased selection
            biased;

            // Handle keyboard events (events come in as they happen)
            Some(Ok(event)) = event_stream.next() => {
                handle_event(event, state, &action_tx).await?;
            }
            // Redraw at consistent frame rate (30 FPS)
            _ = interval.tick() => {
                if let Err(e) = terminal.draw(|f| ui(f, state)) {
                    tracing::error!("Draw error: {:?}", e);
                    anyhow::bail!("Draw error: {:?}", e);
                }
            }
            Some(action) = action_rx.recv() => {
                match &action {
                    Action::InitializeTree(target_dir) => {
                        let target_dir = target_dir.clone();
                        reducer(state, action);

                        let root_is_project = state
                            .tree_root
                            .as_ref()
                            .is_some_and(|root| root.node_type.is_project());

                        if root_is_project {
                            {
                                let cache = dep_cache.read().await;
                                let cache_duration = state.config.app.cache_duration();
                                apply_dep_version_cache(
                                    &mut state.all_projects,
                                    &cache,
                                    cache_duration,
                                );
                                apply_dep_version_cache(&mut state.projects, &cache, cache_duration);
                                if let Some(root) = &mut state.tree_root {
                                    apply_dep_version_cache_to_tree(root, &cache, cache_duration);
                                    state.flattened_tree =
                                        carwash::tree::FlattenedTree::from_tree(root);
                                }
                            }

                            reset_checking_status(state);

                            let _ = action_tx.send(Action::CalculateProjectSizes).await;
                        } else {
                            // Spawn async load of root directory children (non-blocking)
                            let tx = action_tx.clone();
                            let show_all = state.config.app.show_all_folders;

                            tokio::task::spawn_blocking(move || {
                                // Load children of root directory (depth 1, since root is depth 0)
                                let children = carwash::project::load_directory_children_async(
                                    std::path::Path::new(&target_dir),
                                    1,  // Root children are at depth 1
                                    show_all
                                );
                                let root_path = std::path::PathBuf::from(&target_dir);
                                let _ = tx.blocking_send(Action::DirectoryLoaded(root_path, children));
                            });
                        }
                    }
                    Action::DirectoryLoaded(path, children) => {
                        let path = path.clone();
                        let mut children = children.clone();
                        {
                            let cache = dep_cache.read().await;
                            let cache_duration = state.config.app.cache_duration();
                            for child in &mut children {
                                apply_dep_version_cache_to_tree(child, &cache, cache_duration);
                            }
                        }

                        let prev_all_count = state.all_projects.len();
                        reducer(state, Action::DirectoryLoaded(path, children));

                        if state.all_projects.len() > prev_all_count {
                            // Size calculations with concurrency limit (reuse existing pattern)
                            let size_semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
                            for project in &state.all_projects[prev_all_count..] {
                                let project_id = project.path.clone();
                                let path = project.path.clone();
                                let ws_root = project.workspace_root.clone();
                                let tx = action_tx.clone();
                                let sem = size_semaphore.clone();
                                tokio::spawn(async move {
                                    let _permit = sem.acquire().await.ok();
                                    let total = tokio::task::spawn_blocking({
                                        let path = path.clone();
                                        move || carwash::project::calculate_directory_size(&path)
                                    })
                                    .await
                                    .ok()
                                    .flatten();

                                    let target_path =
                                        ws_root.unwrap_or_else(|| path.clone()).join("target");
                                    let target = if target_path.exists() {
                                        tokio::task::spawn_blocking(move || {
                                            carwash::project::calculate_directory_size(&target_path)
                                        })
                                        .await
                                        .ok()
                                        .flatten()
                                    } else {
                                        Some(0)
                                    };

                                    let _ = tx
                                        .send(Action::UpdateProjectSize(project_id, total, target))
                                        .await;
                                });
                            }
                        }

                    }
                    Action::SelectChild => {
                        reducer(state, action);

                        // After expanding, check if we need to async load children
                        if let Some(selected_idx) = state.tree_state.selected()
                            && selected_idx < state.flattened_tree.items.len()
                        {
                            let (node, _) = &state.flattened_tree.items[selected_idx];
                            // If node is a directory, expanded, but children not loaded, queue async load
                            if node.node_type.is_directory() && node.expanded && !node.children_loaded {
                                let path = node.node_type.path().to_path_buf();
                                let depth = node.depth + 1;  // Children are one level deeper
                                let tx = action_tx.clone();
                                let show_all = state.config.app.show_all_folders;

                                // Mark as loading immediately
                                if let Some(root) = &mut state.tree_root {
                                    fn mark_loading(node: &mut TreeNode, target: &std::path::Path) -> bool {
                                        if node.node_type.path() == target {
                                            node.loading = true;
                                            return true;
                                        }
                                        for child in &mut node.children {
                                            if mark_loading(child, target) {
                                                return true;
                                            }
                                        }
                                        false
                                    }
                                    mark_loading(root, &path);
                                    state.flattened_tree = carwash::tree::FlattenedTree::from_tree(root);
                                }

                                // Spawn async load
                                tokio::task::spawn_blocking(move || {
                                    let children = carwash::project::load_directory_children_async(
                                        &path,
                                        depth,
                                        show_all
                                    );
                                    let _ = tx.blocking_send(Action::DirectoryLoaded(path, children));
                                });
                            }
                        }
                    }
                    Action::ExpandDirectory(path, depth) => {
                        // Mark node as loading immediately (UI feedback)
                        if let Some(root) = &mut state.tree_root {
                            fn mark_loading(node: &mut TreeNode, target: &std::path::Path) -> bool {
                                if node.node_type.path() == target {
                                    node.loading = true;
                                    return true;
                                }
                                for child in &mut node.children {
                                    if mark_loading(child, target) {
                                        return true;
                                    }
                                }
                                false
                            }
                            mark_loading(root, path);
                            state.flattened_tree = carwash::tree::FlattenedTree::from_tree(root);
                        }

                        // Spawn async load
                        let path_clone = path.clone();
                        let tx = action_tx.clone();
                        let show_all = state.config.app.show_all_folders;
                        let depth = *depth;

                        tokio::task::spawn_blocking(move || {
                            let children = carwash::project::load_directory_children_async(
                                &path_clone,
                                depth,
                                show_all
                            );
                            let _ = tx.blocking_send(Action::DirectoryLoaded(path_clone, children));
                        });
                    }
                    Action::ExecuteCommand(command) => {
                        if let Command::Cargo { command } = command {
                            let action_tx_clone = action_tx.clone();
                            let command_str = command.clone();
                            // Always run on selected projects (on_all = false)
                            run_command(&command_str, state, action_tx_clone).await;
                            reducer(state, Action::EnterNormalMode);
                        }
                    }
                    Action::FinishProjectScan(_, _) => {
                        // Process the scan result FIRST (copies projects to state)
                        reducer(state, action);

                        {
                            let cache = dep_cache.read().await;
                            let cache_duration = state.config.app.cache_duration();
                            apply_dep_version_cache(&mut state.all_projects, &cache, cache_duration);
                            apply_dep_version_cache(&mut state.projects, &cache, cache_duration);
                        }

                        // Reset any "Checking" status to "Unchecked" (app was interrupted)
                        reset_checking_status(state);

                        // Trigger size calculation in background (non-blocking)
                        let tx = action_tx.clone();
                        tokio::spawn(async move {
                            // Small delay to let UI render first
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            let _ = tx.send(Action::CalculateProjectSizes).await;
                        });
                    }
                    Action::CalculateProjectSizes => {
                        // Spawn size calculation tasks for all projects
                        carwash::handlers::handle_calculate_project_sizes(state, action_tx.clone()).await;
                    }
                    Action::StartUpdateWizard => {
                        let selected_project_id = state
                            .get_selected_project()
                            .map(|p| p.path.clone());
                        let is_currently_checking_same_project = selected_project_id
                            .as_ref()
                            .map(|project_id| {
                                state.is_checking_updates
                                    && state
                                        .updater
                                        .locked_project_id
                                        .as_deref()
                                        == Some(project_id.as_path())
                            })
                            .unwrap_or(false);
                        let is_checking_different_project =
                            state.is_checking_updates && !is_currently_checking_same_project;

                        if is_checking_different_project {
                            continue;
                        }

                        // Process the action. Directory selections intentionally do nothing:
                        // update checks are manual and scoped to one selected project.
                        reducer(state, action.clone());

                        if is_currently_checking_same_project {
                            // Already processing this project; just keep displaying the wizard.
                            continue;
                        }

                        if state.updater.locked_project_id.is_some() {
                            let action_tx_clone = action_tx.clone();
                            let state_snapshot = state.clone();
                            let dep_cache_clone = dep_cache.clone();
                            tokio::spawn(async move {
                                check_for_updates(&state_snapshot, action_tx_clone, dep_cache_clone).await;
                            });
                        }
                    }
                    Action::UpdateDependencies(..) => {
                        reducer(state, action);
                    }
                    Action::UpdateSingleDependency(..) => {
                        // Update individual dependency and continue
                        reducer(state, action);
                    }
                    Action::UpdateDependenciesStreamStart(_) => {
                        // Stream has started
                        reducer(state, action);
                    }
                    Action::SaveSettings => {
                        reducer(state, action.clone());
                    }
                    Action::RunUpdate => {
                        // Construct the cargo update command for selected dependencies
                        // Use name@version format to avoid ambiguity when multiple versions exist
                        let selected_deps: Vec<String> = state
                            .updater
                            .selected_dependencies
                            .iter()
                            .filter_map(|name| {
                                // Look up the current version from outdated_dependencies
                                state
                                    .updater
                                    .outdated_dependencies
                                    .iter()
                                    .find(|d| &d.name == name)
                                    .map(|d| format!("{}@{}", d.name, d.current_version))
                            })
                            .collect();
                        if !selected_deps.is_empty() {
                            let update_cmd = format!("update -p {}", selected_deps.join(" -p "));

                            // Run update only on the currently highlighted project
                            // (the one whose dependencies are shown in the update wizard)
                            if let Some(project_id) = state.updater.locked_project_id.clone() {
                                // Temporarily clear selected projects and set only the current one
                                let previous_selection = state.selected_projects.clone();
                                state.selected_projects.clear();
                                state.selected_projects.insert(project_id.clone());

                                // Set pending reload - will be processed when FinishCommand is received
                                state.updater.pending_reload_project = Some(project_id);

                                // Clear wizard selections (but keep wizard open until command finishes)
                                state.updater.selected_dependencies.clear();

                                // Run the update command (spawns async task)
                                run_command(&update_cmd, state, action_tx.clone()).await;

                                // Restore previous selection state
                                state.selected_projects = previous_selection;

                                // Exit wizard mode - the reload will happen via FinishCommand
                                reducer(state, Action::EnterNormalMode);
                            }
                        }
                    }
                    Action::FinishCommand(tab_index) => {
                        reducer(state, Action::FinishCommand(*tab_index));

                        // Check if we have a pending dependency reload after update
                        if let Some(project_id) = state.updater.pending_reload_project.take()
                            && let Some(all_proj) = state.all_projects.iter_mut().find(|p| p.path == project_id)
                            && let Ok(()) = all_proj.reload_dependencies()
                        {
                            // Successfully reloaded! Now sync to filtered projects list
                            if let Some(proj) = state.projects.iter_mut().find(|p| p.path == project_id) {
                                proj.dependencies = all_proj.dependencies.clone();
                            }

                            // Clear stale wizard state
                            state.updater.outdated_dependencies.clear();

                            // Now re-check with the FRESH dependencies to get latest versions
                            let fresh_deps = all_proj.dependencies.clone();
                            let proj_id = all_proj.path.clone();
                            let cache_duration = state.config.app.cache_duration();
                            let action_tx_clone = action_tx.clone();
                            let dep_cache_clone = dep_cache.clone();

                            tokio::spawn(async move {
                                // Re-check with fresh dependencies from disk
                                check_dependencies_with_cache(
                                    proj_id,
                                    fresh_deps,
                                    action_tx_clone,
                                    false,  // Don't use cache - force fresh check
                                    cache_duration,
                                    dep_cache_clone,
                                )
                                .await;
                            });

                            // Add notification to the tab
                            let _ = action_tx
                                .send(Action::AddOutput(
                                    *tab_index,
                                    "Dependencies reloaded. Re-checking for updates...".into(),
                                ))
                                .await;
                        }
                    }
                    _ => {
                        reducer(state, action);
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                // Periodic cache persistence (every 30 seconds)
                if last_cache_save.elapsed() > std::time::Duration::from_secs(30) {
                    let mut cache = dep_cache.write().await;
                    let _ = cache.save();
                    last_cache_save = std::time::Instant::now();
                }
                continue;
            }
        };

        if state.should_quit {
            let mut cache = dep_cache.write().await;
            let _ = cache.save();
            return Ok(());
        }
    }
}
