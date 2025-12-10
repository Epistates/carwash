use carwash::Args;
use carwash::app::{AppState, reducer};
use carwash::cache::UpdateCache;
use carwash::components::{
    Component, dependencies::DependenciesPane, help::Help, output::TabbedOutputPane,
    palette::CommandPalette, projects::ProjectList, settings::SettingsModal, text_input::TextInput,
    updater::UpdateWizard,
};
use carwash::events::{Action, Command, Focus, Mode};
use carwash::project::{ProjectCheckStatus, find_rust_projects};
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

/// Save current dependency check progress to persistent cache
fn save_cache_progress(state: &AppState) {
    use std::collections::HashMap;

    let cache = UpdateCache::new();

    // Use all_projects, not filtered state.projects, so we cache ALL projects including those without dependencies
    for project in &state.all_projects {
        // Compute Cargo.lock hash if it exists
        let lock_path = project.path.join("Cargo.lock");
        if let Some(lock_hash) = UpdateCache::hash_cargo_lock(&lock_path) {
            // Build cache data from current dependencies
            let mut cached_deps = HashMap::new();
            for dep in &project.dependencies {
                // CRITICAL: Only save dependencies that have been CHECKED
                // If latest_version is None, the dep was never checked, so don't cache it
                if dep.latest_version.is_some() {
                    cached_deps.insert(
                        dep.name.clone(),
                        carwash::cache::CachedDependency {
                            latest_version: dep.latest_version.clone(),
                            cached_at: dep.last_checked.unwrap_or_else(std::time::SystemTime::now),
                        },
                    );
                }
            }

            // Save to cache (skip if no checked dependencies)
            if !cached_deps.is_empty() {
                let _ = cache.save(&project.path, lock_hash, cached_deps.clone());
            }
        }
    }
}

/// Load dependency check progress from persistent cache
fn load_cache_progress(projects: &mut [carwash::project::Project]) {
    use carwash::project::{DependencyCheckStatus, Project};

    let cache = UpdateCache::new();

    for project in projects.iter_mut() {
        // Compute Cargo.lock hash
        let lock_path = project.path.join("Cargo.lock");
        if let Some(lock_hash) = UpdateCache::hash_cargo_lock(&lock_path) {
            // Try to load cached data
            if let Some(cached_deps) = cache.load(&project.path, lock_hash) {
                // Apply cached data to dependencies
                for dep in &mut project.dependencies {
                    if let Some(cached_dep) = cached_deps.get(&dep.name) {
                        // Update with cached version info AND set the timestamp
                        dep.latest_version = cached_dep.latest_version.clone();
                        dep.check_status = DependencyCheckStatus::Checked;
                        // Set last_checked to the cache timestamp so checks respect cache duration
                        dep.last_checked = Some(cached_dep.cached_at);
                    }
                }

                // Calculate project check status from cached data
                project.check_status =
                    Project::compute_check_status_from_deps(&project.dependencies);
            }
        }
    }
}

async fn handle_event(
    event: Event,
    state: &mut AppState,
    action_tx: &mpsc::Sender<Action>,
) -> io::Result<()> {
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
                        // If Output pane has focus, let it handle Tab for tab switching
                        // Otherwise, cycle focus between panes
                        if state.focus == Focus::Output {
                            let mut output = TabbedOutputPane::new();
                            output.handle_key_events(key.code, state)
                        } else {
                            Some(Action::FocusNext)
                        }
                    }
                    KeyCode::BackTab => {
                        // BackTab always handled by Output pane when it has focus
                        if state.focus == Focus::Output {
                            let mut output = TabbedOutputPane::new();
                            output.handle_key_events(key.code, state)
                        } else {
                            None
                        }
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
                    KeyCode::Char('r') | KeyCode::Char('R') => {
                        if key.modifiers.contains(KeyModifiers::SHIFT) {
                            Some(Action::ResetLayout)
                        } else {
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
                | Action::ProcessBackgroundUpdateQueue
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
                    // Save progress if quitting
                    if matches!(action, Action::Quit) {
                        save_cache_progress(state);
                    }
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
) -> io::Result<()> {
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

    // Spawn deep scan in background for search index (fire and forget)
    let action_tx_clone = action_tx.clone();
    let target_directory_clone = target_directory.clone();
    tokio::spawn(async move {
        let target_dir_for_scan = target_directory_clone.clone();

        // This can take a while, but it won't block the UI
        if let Ok(projects) =
            tokio::task::spawn_blocking(move || find_rust_projects(&target_dir_for_scan)).await
        {
            let _ = action_tx_clone
                .send(Action::FinishProjectScan(projects, target_directory_clone))
                .await;
        }
    });

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
                    tracing::error!("Draw error: {}", e);
                    return Err(e);
                }
            }
            Some(action) = action_rx.recv() => {
                match &action {
                    Action::InitializeTree(target_dir) => {
                        let target_dir = target_dir.clone();
                        reducer(state, action);

                        // Spawn async load of root directory children (non-blocking)
                        let tx = action_tx.clone();
                        let show_all = state.settings.show_all_folders;

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
                    Action::DirectoryLoaded(..) => {
                        reducer(state, action);
                    }
                    Action::SelectChild => {
                        reducer(state, action);

                        // After expanding, check if we need to async load children
                        if let Some(selected_idx) = state.tree_state.selected() {
                            if selected_idx < state.flattened_tree.items.len() {
                                let (node, _) = &state.flattened_tree.items[selected_idx];
                                // If node is a directory, expanded, but children not loaded, queue async load
                                if node.node_type.is_directory() && node.expanded && !node.children_loaded {
                                    let path = node.node_type.path().to_path_buf();
                                    let depth = node.depth + 1;  // Children are one level deeper
                                    let tx = action_tx.clone();
                                    let show_all = state.settings.show_all_folders;

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
                        }

                        // Spawn async load
                        let path_clone = path.clone();
                        let tx = action_tx.clone();
                        let show_all = state.settings.show_all_folders;
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

                        // THEN load cached dependency data (updates state with cache)
                        load_cache_progress(&mut state.all_projects);

                        // Also update the filtered projects list with the cache data
                        load_cache_progress(&mut state.projects);

                        // Reset any "Checking" status to "Unchecked" (app was interrupted)
                        reset_checking_status(state);

                        if state.settings.background_updates_enabled {
                            // NOW queue projects for background checks (after cache is loaded)
                            // Queue projects that:
                            // 1. Have expired cache (> cache TTL)
                            // 2. Have no cache (never checked)
                            // 3. Were interrupted (status is Unchecked)
                            // 4. Have ANY dependency that needs checking
                            let mut queue_idx = 0;
                            let cache_duration = state.settings.cache_duration();
                            for project in &state.all_projects {
                                // Skip projects with no dependencies
                                if project.dependencies.is_empty() {
                                    continue;
                                }

                                let needs_check = project.dependencies.iter().any(|dep| {
                                    if let Some(last_checked) = dep.last_checked {
                                        if let Ok(elapsed) =
                                            std::time::SystemTime::now().duration_since(last_checked)
                                        {
                                            elapsed > cache_duration
                                        } else {
                                            true // Invalid timestamp, needs check
                                        }
                                    } else {
                                        true // Never checked - needs check!
                                    }
                                });

                                // Also queue if status is Unchecked (was interrupted or never checked)
                                let needs_check =
                                    needs_check || project.check_status == ProjectCheckStatus::Unchecked;

                                if needs_check {
                                    let tx = action_tx.clone();
                                    let project_name = project.name.clone();

                                    // Stagger the queue operations to keep UI responsive
                                    let delay = std::time::Duration::from_millis(100 * queue_idx);
                                    queue_idx += 1;

                                    tokio::spawn(async move {
                                        tokio::time::sleep(delay).await;
                                        let _ = tx
                                            .send(Action::QueueBackgroundUpdate(
                                                project_name,
                                                false,
                                            ))
                                            .await;
                                    });
                                }
                            }
                        }

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
                        let selected_project_name = state
                            .get_selected_project()
                            .map(|p| p.name.clone());
                        let is_currently_checking_same_project = selected_project_name
                            .as_ref()
                            .map(|name| {
                                state.is_checking_updates
                                    && state
                                        .updater
                                        .locked_project_name
                                        .as_ref()
                                        == Some(name)
                            })
                            .unwrap_or(false);

                        // Process the action (may set pending_directory_check or open wizard)
                        reducer(state, action.clone());

                        // Check if this is a directory check (multiple projects)
                        if let Some(pending) = state.updater.pending_directory_check.take() {
                            // Queue all projects for background checking (not priority - no wizard)
                            for project_name in pending.project_names {
                                let _ = action_tx
                                    .send(Action::QueueBackgroundUpdate(project_name, false))
                                    .await;
                            }
                        } else if let Some(project_name) = state.updater.locked_project_name.clone() {
                            // Single project - open wizard and queue priority check
                            if is_currently_checking_same_project {
                                // Already processing this project; just keep displaying the wizard
                                continue;
                            }
                            let _ = action_tx
                                .send(Action::QueueBackgroundUpdate(project_name, true))
                                .await;
                        }
                    }
                    Action::StartBackgroundUpdateCheck => {
                        let action_tx_clone = action_tx.clone();
                        check_for_updates(state, action_tx_clone).await; // Use new non-blocking check
                        reducer(state, action);
                    }
                    Action::ProcessBackgroundUpdateQueue => {
                        // Check if there are tasks to process in the queue
                        if let Some(task) = state.update_queue.get_next_task() {
                            let action_tx_clone = action_tx.clone();
                            let project_name = task.project_name.clone();
                            let is_priority = task.is_priority;

                            // Find the project by name in all_projects (not filtered list) so background checks work for all projects
                            if let Some(project) = state.all_projects.iter().find(|p| p.name == project_name) {
                                let deps = project.dependencies.clone();
                                let project_path = project.path.clone();
                                let proj_name = project.name.clone();

                                // For priority tasks (user pressed 'u'), enter wizard mode IMMEDIATELY
                                if is_priority {
                                    // Enter wizard mode right away
                                    state.mode = Mode::UpdateWizard;
                                    state.is_checking_updates = true;

                                    // Show cached data immediately if available
                                    // Uses has_stable_update() to properly handle pre-release versions
                                    state.updater.outdated_dependencies = deps
                                        .iter()
                                        .filter(|d| d.has_stable_update())
                                        .cloned()
                                        .collect();

                                    state.updater.selected_dependencies.clear();

                                    // Select first item if there are outdated dependencies
                                    if !state.updater.outdated_dependencies.is_empty() {
                                        state.updater.list_state.select(Some(0));
                                    } else {
                                        state.updater.list_state.select(None);
                                    }
                                }

                                // CRITICAL FIX: Only set status to "Checking" if deps actually need checking
                                // Don't overwrite cached status if all deps are fresh
                                let now = std::time::SystemTime::now();
                                let cache_duration = state.settings.cache_duration();
                                let has_deps_needing_check = deps.iter().any(|dep| {
                                    if let Some(last_checked) = dep.last_checked {
                                        // Check if cache expired
                                        if let Ok(elapsed) = now.duration_since(last_checked) {
                                            elapsed > cache_duration
                                        } else {
                                            true // Invalid timestamp
                                        }
                                    } else {
                                        true // Never checked
                                    }
                                });

                                // If all deps are fresh in cache, skip the async check entirely
                                if !has_deps_needing_check {
                                    // All deps are fresh - no need to re-check
                                    state.is_checking_updates = false;
                                    state.update_queue.task_completed();

                                    // Continue queue for background tasks
                                    if !is_priority {
                                        let _ = action_tx.send(Action::ProcessBackgroundUpdateQueue).await;
                                    }
                                } else {
                                    // Some deps need checking - set status and spawn async check
                                    reducer(state, Action::UpdateProjectCheckStatus(
                                        proj_name.clone(),
                                        ProjectCheckStatus::Checking
                                    ));

                                    let action_tx_clone_2 = action_tx_clone.clone();
                                    let is_priority_task = is_priority;
                                    let cache_duration = state.settings.cache_duration();

                                    // Perform the update check asynchronously
                                    tokio::spawn(async move {
                                        check_dependencies_with_cache(
                                            proj_name,
                                            deps,
                                            action_tx_clone,
                                            true,  // use_cache=true to respect TTL
                                            Some(project_path),
                                            cache_duration,
                                        )
                                        .await;

                                        // Only continue queue for background tasks, not priority
                                        if !is_priority_task {
                                            let _ = action_tx_clone_2.send(Action::ProcessBackgroundUpdateQueue).await;
                                        }
                                    });
                                }
                            } else {
                                // Project not found, mark as complete so queue can continue
                                state.update_queue.task_completed();
                                // Try to process the next task immediately
                                let _ = action_tx.send(Action::ProcessBackgroundUpdateQueue).await;
                            }
                        }
                    }
                    Action::QueueBackgroundUpdate(_, _) => {
                        // Add to queue and start processing
                        reducer(state, action);
                        let _ = action_tx.send(Action::ProcessBackgroundUpdateQueue).await;
                    }
                    Action::UpdateDependencies(..) => {
                        // Mark the background update task as complete
                        state.update_queue.task_completed();
                        // Process the update results
                        reducer(state, action);
                        // Continue processing the queue
                        let _ = action_tx.send(Action::ProcessBackgroundUpdateQueue).await;
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

                        if state.settings.background_updates_enabled
                            && state.update_queue.has_pending_tasks()
                        {
                            let _ = action_tx
                                .send(Action::ProcessBackgroundUpdateQueue)
                                .await;
                        }
                    }
                    Action::RunUpdate => {
                        // Build the cargo update command for selected dependencies
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
                            if let Some(project) = state.get_selected_project() {
                                let project_name = project.name.clone();

                                // Temporarily clear selected projects and set only the current one
                                let previous_selection = state.selected_projects.clone();
                                state.selected_projects.clear();
                                state.selected_projects.insert(project_name.clone());

                                // Set pending reload - will be processed when FinishCommand is received
                                state.updater.pending_reload_project = Some(project_name);

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
                        // NOTE: reducer was already called in sync path (handle_component_events)
                        // This async handler only runs when there's a pending reload

                        // Check if we have a pending dependency reload after update
                        if let Some(project_name) = state.updater.pending_reload_project.take() {
                            // Find and reload the project in all_projects (source of truth)
                            if let Some(all_proj) = state.all_projects.iter_mut().find(|p| p.name == project_name) {
                                if let Ok(()) = all_proj.reload_dependencies() {
                                    // Successfully reloaded! Now sync to filtered projects list
                                    if let Some(proj) = state.projects.iter_mut().find(|p| p.name == project_name) {
                                        proj.dependencies = all_proj.dependencies.clone();
                                    }

                                    // Clear stale wizard state
                                    state.updater.outdated_dependencies.clear();

                                    // Now re-check with the FRESH dependencies to get latest versions
                                    let fresh_deps = all_proj.dependencies.clone();
                                    let project_path = all_proj.path.clone();
                                    let proj_name = all_proj.name.clone();
                                    let cache_duration = state.settings.cache_duration();
                                    let action_tx_clone = action_tx.clone();

                                    tokio::spawn(async move {
                                        // Re-check with fresh dependencies from disk
                                        check_dependencies_with_cache(
                                            proj_name,
                                            fresh_deps,
                                            action_tx_clone,
                                            false,  // Don't use cache - force fresh check
                                            Some(project_path),
                                            cache_duration,
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
                    save_cache_progress(state);
                    last_cache_save = std::time::Instant::now();
                }
                continue;
            }
        };

        if state.should_quit {
            return Ok(());
        }
    }
}
