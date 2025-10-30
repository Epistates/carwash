use carwash::Args;
use carwash::app::{AppState, reducer};
use carwash::cache::UpdateCache;
use carwash::components::{
    Component, help::Help, palette::CommandPalette, projects::ProjectList, text_input::TextInput,
    updater::UpdateWizard,
};
use carwash::events::{Action, Command, Mode};
use carwash::project::{ProjectCheckStatus, find_rust_projects};
use carwash::runner::{check_dependencies_with_cache, check_for_updates, run_command};
use carwash::ui::ui;

use clap::Parser;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
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

#[tokio::main]
async fn main() -> io::Result<()> {
    // Set up panic handler to ensure clean terminal restoration
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    let args = Args::parse();

    // Check if we have a TTY (after argument parsing so --help works)
    if !crossterm::tty::IsTty::is_tty(&io::stdin()) {
        eprintln!("Error: CarWash requires an interactive terminal (TTY).");
        eprintln!("Please run directly in a terminal, not through pipes or redirects.");
        std::process::exit(1);
    }

    let target_directory = args.target_directory.clone();

    // Initialize terminal
    if let Err(e) = enable_raw_mode() {
        eprintln!("Error: Failed to enable raw mode: {}", e);
        eprintln!("Make sure you're running in a proper terminal.");
        std::process::exit(1);
    }

    let mut stdout = io::stdout();
    if let Err(e) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
        let _ = disable_raw_mode();
        eprintln!("Error: Failed to initialize terminal: {}", e);
        std::process::exit(1);
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(t) => t,
        Err(e) => {
            let _ = disable_raw_mode();
            eprintln!("Error: Failed to create terminal: {}", e);
            std::process::exit(1);
        }
    };

    // Clear screen immediately to prevent any error messages from showing
    let _ = terminal.clear();

    let mut state = AppState::new();
    let res = run_app(&mut terminal, &mut state, target_directory).await;

    // Clean up terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    // Report errors only after terminal is restored
    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
        std::process::exit(1);
    }

    Ok(())
}

/// Save current dependency check progress to persistent cache
fn save_cache_progress(state: &AppState) {
    use std::collections::HashMap;
    use std::io::Write;

    // Write debug to file instead of stderr (to avoid corrupting TUI)
    let mut log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/carwash-debug.log")
        .ok();

    if let Some(ref mut f) = log_file {
        let _ = writeln!(f, "\n=== SAVE CACHE DEBUG ===");
        let _ = writeln!(f, "Total projects: {}", state.all_projects.len());
    }

    let cache = UpdateCache::new();

    // Use all_projects, not filtered state.projects, so we cache ALL projects including those without dependencies
    for project in &state.all_projects {
        // Compute Cargo.lock hash if it exists
        let lock_path = project.path.join("Cargo.lock");
        if let Some(lock_hash) = UpdateCache::hash_cargo_lock(&lock_path) {
            // Build cache data from current dependencies
            let mut cached_deps = HashMap::new();
            let total_deps = project.dependencies.len();
            let mut checked_deps = 0;

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
                    checked_deps += 1;
                }
            }

            if let Some(ref mut f) = log_file {
                let _ = writeln!(
                    f,
                    "[QUIT] Project {}: {} total deps, {} checked, lock_hash={:x}",
                    project.name, total_deps, checked_deps, lock_hash
                );
            }

            // Save to cache (skip if no checked dependencies)
            if !cached_deps.is_empty() {
                match cache.save(&project.path, lock_hash, cached_deps.clone()) {
                    Ok(()) => {
                        if let Some(ref mut f) = log_file {
                            let _ = writeln!(
                                f,
                                "[QUIT] ✓ Saved cache for {} ({} deps)",
                                project.name,
                                cached_deps.len()
                            );
                        }
                    }
                    Err(e) => {
                        if let Some(ref mut f) = log_file {
                            let _ = writeln!(
                                f,
                                "[QUIT] ✗ Failed to save cache for {}: {}",
                                project.name, e
                            );
                        }
                    }
                }
            } else if let Some(ref mut f) = log_file {
                let _ = writeln!(f, "[QUIT] ⊘ Skipped {} (no checked deps)", project.name);
            }
        } else if let Some(ref mut f) = log_file {
            let _ = writeln!(f, "  ⊘ Skipped {} (no Cargo.lock)", project.name);
        }
    }

    if let Some(ref mut f) = log_file {
        let _ = writeln!(f, "=== END SAVE CACHE ===\n");
    }
}

/// Load dependency check progress from persistent cache
fn load_cache_progress(projects: &mut [carwash::project::Project]) {
    use carwash::project::{DependencyCheckStatus, Project};
    use std::io::Write;

    // Write debug to file instead of stderr (to avoid corrupting TUI)
    let mut log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/carwash-debug.log")
        .ok();

    if let Some(ref mut f) = log_file {
        let _ = writeln!(f, "\n=== LOAD CACHE DEBUG ===");
        let _ = writeln!(f, "Loading cache for {} projects", projects.len());
    }

    let cache = UpdateCache::new();
    let mut loaded_count = 0;

    for project in projects.iter_mut() {
        // Compute Cargo.lock hash
        let lock_path = project.path.join("Cargo.lock");
        if let Some(lock_hash) = UpdateCache::hash_cargo_lock(&lock_path) {
            if let Some(ref mut f) = log_file {
                let _ = writeln!(
                    f,
                    "[LOAD] Project {}: lock_hash={:x}",
                    project.name, lock_hash
                );
            }

            // Try to load cached data
            if let Some(cached_deps) = cache.load(&project.path, lock_hash) {
                let mut applied_count = 0;
                // Apply cached data to dependencies
                for dep in &mut project.dependencies {
                    if let Some(cached_dep) = cached_deps.get(&dep.name) {
                        // Update with cached version info AND set the timestamp
                        dep.latest_version = cached_dep.latest_version.clone();
                        dep.check_status = DependencyCheckStatus::Checked;
                        // Set last_checked to the cache timestamp so checks respect cache duration
                        dep.last_checked = Some(cached_dep.cached_at);
                        applied_count += 1;
                    }
                }

                if let Some(ref mut f) = log_file {
                    let _ = writeln!(
                        f,
                        "[LOAD] ✓ Project {}: Loaded {} cached deps",
                        project.name, applied_count
                    );
                }

                // Calculate project check status from cached data
                project.check_status =
                    Project::compute_check_status_from_deps(&project.dependencies);
                loaded_count += 1;
            } else if let Some(ref mut f) = log_file {
                let _ = writeln!(
                    f,
                    "[LOAD] ✗ Project {}: No cache found or hash mismatch",
                    project.name
                );
            }
        } else if let Some(ref mut f) = log_file {
            let _ = writeln!(f, "[LOAD] ⊘ Project {}: No Cargo.lock", project.name);
        }
    }

    if let Some(ref mut f) = log_file {
        let _ = writeln!(
            f,
            "Loaded cache for {}/{} projects",
            loaded_count,
            projects.len()
        );
        let _ = writeln!(f, "=== END LOAD CACHE ===\n");
    }
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    state: &mut AppState<'_>,
    target_directory: String,
) -> io::Result<()> {
    let (action_tx, mut action_rx) = mpsc::channel(100);
    let mut event_stream = crossterm::event::EventStream::new();

    // Track last cache save time for periodic persistence
    let mut last_cache_save = std::time::Instant::now();

    let action_tx_clone = action_tx.clone();
    tokio::spawn(async move {
        // Use tokio::spawn_blocking with timeout
        let scan_result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            tokio::task::spawn_blocking(move || find_rust_projects(&target_directory)),
        )
        .await;

        match scan_result {
            Ok(Ok(projects)) => {
                // Just send the scan result - queuing happens AFTER cache is loaded
                let _ = action_tx_clone
                    .send(Action::FinishProjectScan(projects))
                    .await;
            }
            Ok(Err(_)) | Err(_) => {
                // Timeout or panic - send empty list and continue
                let _ = action_tx_clone
                    .send(Action::FinishProjectScan(Vec::new()))
                    .await;
            }
        }
    });

    loop {
        // Draw UI (with timeout protection)
        if let Err(e) = terminal.draw(|f| ui(f, state)) {
            // If drawing fails, try to recover
            eprintln!("Draw error: {}", e);
            return Err(e);
        }

        tokio::select! {
            // Prioritize keyboard events with biased selection
            biased;

            Some(Ok(Event::Key(key))) = event_stream.next() => {
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
                            KeyCode::Char('q') => Some(Action::Quit),
                            KeyCode::Char('?') => Some(Action::ShowHelp),
                            KeyCode::Char(':') => Some(Action::ShowCommandPalette),
                            KeyCode::Char('u') => {
                                // Open update wizard for selected project
                                Some(Action::StartUpdateWizard)
                            },
                            _ => {
                                let mut project_list = ProjectList::new();
                                project_list.handle_key_events(key.code, state)
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
            Some(action) = action_rx.recv() => {
                match &action {
                    Action::ExecuteCommand(command) => {
                        if let Command::Cargo { command } = command {
                            let action_tx_clone = action_tx.clone();
                            let command_str = command.clone();
                            // Always run on selected projects (on_all = false)
                            run_command(&command_str, state, action_tx_clone, false).await;
                            reducer(state, Action::EnterNormalMode);
                        }
                    }
                    Action::FinishProjectScan(_) => {
                        // Process the scan result FIRST (copies projects to state)
                        reducer(state, action);

                        // THEN load cached dependency data (updates state with cache)
                        load_cache_progress(&mut state.all_projects);

                        // Also update the filtered projects list with the cache data
                        load_cache_progress(&mut state.projects);

                        // Reset any "Checking" status to "Unchecked" (app was interrupted)
                        // These projects will be queued below to resume checking
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

                        // NOW queue projects for background checks (after cache is loaded)
                        // Queue projects that:
                        // 1. Have expired cache (> 5 minutes)
                        // 2. Have no cache (never checked)
                        // 3. Were interrupted (status is Unchecked)
                        // 4. Have ANY dependency that needs checking
                        let mut queue_idx = 0;
                        for project in &state.all_projects {
                            // Skip projects with no dependencies
                            if project.dependencies.is_empty() {
                                continue;
                            }

                            // CRITICAL FIX: Check if ANY dependency needs checking, not just the first!
                            // A project should be queued if even ONE dep is uncached or expired
                            let needs_check = project.dependencies.iter().any(|dep| {
                                if let Some(last_checked) = dep.last_checked {
                                    // Check if cache expired (> 5 minutes)
                                    if let Ok(elapsed) = std::time::SystemTime::now().duration_since(last_checked) {
                                        elapsed > std::time::Duration::from_secs(5 * 60)
                                    } else {
                                        true // Invalid timestamp, needs check
                                    }
                                } else {
                                    true // Never checked - needs check!
                                }
                            });

                            // Also queue if status is Unchecked (was interrupted or never checked)
                            let needs_check = needs_check || project.check_status == ProjectCheckStatus::Unchecked;

                            if needs_check {
                                let tx = action_tx.clone();
                                let project_name = project.name.clone();

                                // Stagger the queue operations to keep UI responsive
                                let delay = std::time::Duration::from_millis(100 * queue_idx);
                                queue_idx += 1;

                                tokio::spawn(async move {
                                    tokio::time::sleep(delay).await;
                                    let _ = tx.send(Action::QueueBackgroundUpdate(project_name)).await;
                                });
                            }
                        }
                    }
                    Action::StartUpdateWizard => {
                        // Enter wizard mode and set the lock
                        reducer(state, action.clone());

                        // CRITICAL: Spawn the check in background - DON'T AWAIT!
                        // Awaiting here would freeze the UI until check completes
                        let action_tx_clone = action_tx.clone();

                        // Clone the state data we need before spawning
                        let locked_project_name = state.updater.locked_project_name.clone();
                        let all_projects = state.all_projects.clone();

                        tokio::spawn(async move {
                            // Find the locked project
                            if let Some(ref project_name) = locked_project_name {
                                if let Some(project) = all_projects.iter().find(|p| &p.name == project_name) {
                                    let deps = project.dependencies.clone();
                                    let project_path = project.path.clone();
                                    let name = project.name.clone();

                                    // Send stream start
                                    let _ = action_tx_clone.send(Action::UpdateDependenciesStreamStart(name.clone())).await;

                                    // Run the check
                                    check_dependencies_with_cache(name, deps, action_tx_clone, true, Some(project_path)).await;
                                }
                            }
                        });
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
                                    state.updater.outdated_dependencies = deps
                                        .iter()
                                        .filter(|d| {
                                            d.latest_version.is_some() &&
                                            d.latest_version.as_ref().unwrap() != &d.current_version
                                        })
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
                                let cache_duration = std::time::Duration::from_secs(5 * 60);
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

                                // Only set to "Checking" if we'll actually check something
                                if has_deps_needing_check {
                                    reducer(state, Action::UpdateProjectCheckStatus(
                                        proj_name.clone(),
                                        ProjectCheckStatus::Checking
                                    ));
                                }

                                let action_tx_clone_2 = action_tx_clone.clone();
                                let is_priority_task = is_priority;

                                // Perform the update check asynchronously
                                tokio::spawn(async move {
                                    // CRITICAL FIX: use_cache=true for ALL checks (background and priority)
                                    // This respects the 5-minute cache duration and avoids hammering crates.io
                                    check_dependencies_with_cache(
                                        proj_name,
                                        deps,
                                        action_tx_clone,
                                        true,  // ← use_cache=true!
                                        Some(project_path)
                                    ).await;

                                    // Only continue queue for background tasks, not priority
                                    if !is_priority_task {
                                        let _ = action_tx_clone_2.send(Action::ProcessBackgroundUpdateQueue).await;
                                    }
                                });
                            } else {
                                // Project not found, mark as complete so queue can continue
                                state.update_queue.task_completed();
                                // Try to process the next task immediately
                                let _ = action_tx.send(Action::ProcessBackgroundUpdateQueue).await;
                            }
                        }
                    }
                    Action::QueueBackgroundUpdate(_project_name) => {
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
                    Action::RunUpdate => {
                        // Build the cargo update command for selected dependencies
                        let selected_deps: Vec<String> = state.updater.selected_dependencies.iter().cloned().collect();
                        if !selected_deps.is_empty() {
                            let update_cmd = format!("update -p {}", selected_deps.join(" -p "));
                            let action_tx_clone = action_tx.clone();

                            // Run update only on the currently highlighted project
                            // (the one whose dependencies are shown in the update wizard)
                            if let Some(project) = state.get_selected_project() {
                                let project_name = project.name.clone();

                                // Temporarily clear selected projects and set only the current one
                                let previous_selection = state.selected_projects.clone();
                                state.selected_projects.clear();
                                state.selected_projects.insert(project_name.clone());

                                // Run the update command
                                run_command(&update_cmd, state, action_tx_clone.clone(), false).await;

                                // Restore previous selection state
                                state.selected_projects = previous_selection;

                                // CRITICAL FIX: After update completes, reload dependencies from disk
                                // This ensures we read the UPDATED Cargo.lock file, not stale in-memory data

                                // Find and reload the project in all_projects (source of truth)
                                if let Some(all_proj) = state.all_projects.iter_mut().find(|p| p.name == project_name) {
                                    if let Ok(()) = all_proj.reload_dependencies() {
                                        // Successfully reloaded! Now sync to filtered projects list
                                        if let Some(proj) = state.projects.iter_mut().find(|p| p.name == project_name) {
                                            proj.dependencies = all_proj.dependencies.clone();
                                        }

                                        // Clear the update wizard state
                                        state.updater.selected_dependencies.clear();
                                        state.updater.outdated_dependencies.clear();

                                        // Now re-check with the FRESH dependencies to get latest versions
                                        let fresh_deps = all_proj.dependencies.clone();
                                        let project_path = all_proj.path.clone();
                                        let proj_name = all_proj.name.clone();

                                        tokio::spawn(async move {
                                            // Re-check with fresh dependencies from disk
                                            check_dependencies_with_cache(
                                                proj_name,
                                                fresh_deps,
                                                action_tx_clone,
                                                false,  // Don't use cache - force fresh check
                                                Some(project_path)
                                            ).await;
                                        });
                                    }
                                }

                                reducer(state, Action::EnterNormalMode);
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
