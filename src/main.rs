use carwash::Args;
use carwash::app::{AppState, reducer};
use carwash::components::{
    Component, help::Help, palette::CommandPalette, projects::ProjectList, text_input::TextInput,
    updater::UpdateWizard,
};
use carwash::events::{Action, Command, Mode};
use carwash::project::find_rust_projects;
use carwash::runner::{
    UpdateCheckTask, check_for_updates, check_single_project_for_updates, run_command,
};
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
    // Check if we have a TTY
    if !crossterm::tty::IsTty::is_tty(&io::stdin()) {
        eprintln!("Error: CarWash requires an interactive terminal (TTY).");
        eprintln!("Please run directly in a terminal, not through pipes or redirects.");
        std::process::exit(1);
    }

    // Set up panic handler to ensure clean terminal restoration
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    let args = Args::parse();
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

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    state: &mut AppState<'_>,
    target_directory: String,
) -> io::Result<()> {
    let (action_tx, mut action_rx) = mpsc::channel(100);
    let mut event_stream = crossterm::event::EventStream::new();

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
                let _ = action_tx_clone
                    .send(Action::FinishProjectScan(projects.clone()))
                    .await;

                // Queue all projects for background update checking (non-priority)
                for project in projects {
                    if !project.dependencies.is_empty() {
                        let _ = action_tx_clone
                            .send(Action::QueueBackgroundUpdate(project.name))
                            .await;
                    }
                }
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
                                // Create a priority update check for selected project
                                if let Some(project) = state.get_selected_project() {
                                    state.update_queue.add_task(UpdateCheckTask {
                                        project_name: project.name.clone(),
                                        is_priority: true,
                                    });
                                    Some(Action::ProcessBackgroundUpdateQueue)
                                } else {
                                    None
                                }
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
                        Action::ExecuteCommand(_) | Action::StartUpdateWizard | Action::RunUpdate => {
                            // Send through channel for async handling
                            let _ = action_tx.send(action).await;
                        }
                        _ => {
                            // Handle synchronously through reducer
                            reducer(state, action);
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
                    Action::StartUpdateWizard => {
                        let action_tx_clone = action_tx.clone();
                        check_for_updates(state, action_tx_clone, true).await; // Use cache for manual checks
                        reducer(state, action);
                    }
                    Action::StartBackgroundUpdateCheck => {
                        let action_tx_clone = action_tx.clone();
                        check_for_updates(state, action_tx_clone, true).await; // Use cache for background checks
                        // Don't change mode or state - this happens in the background
                    }
                    Action::ProcessBackgroundUpdateQueue => {
                        // Check if there are tasks to process in the queue
                        if let Some(task) = state.update_queue.get_next_task() {
                            let action_tx_clone = action_tx.clone();
                            let project_name = task.project_name.clone();
                            let is_priority = task.is_priority;

                            // Find the project by name and extract dependencies
                            if let Some(project) = state.projects.iter().find(|p| p.name == project_name) {
                                let deps = project.dependencies.clone();

                                // For priority tasks, show the update wizard
                                if is_priority {
                                    state.is_checking_updates = true;
                                    state.mode = Mode::UpdateWizard;
                                }

                                let action_tx_clone_2 = action_tx_clone.clone();

                                // Perform the update check asynchronously
                                tokio::spawn(async move {
                                    check_single_project_for_updates(deps, action_tx_clone, false).await;

                                    // After check completes, queue up next task
                                    let _ = action_tx_clone_2.send(Action::ProcessBackgroundUpdateQueue).await;
                                });
                            } else {
                                state.update_queue.task_completed();
                            }
                        }
                    }
                    Action::QueueBackgroundUpdate(_project_name) => {
                        // Add to queue and start processing
                        reducer(state, action);
                        let _ = action_tx.send(Action::ProcessBackgroundUpdateQueue).await;
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
                                state.selected_projects.insert(project_name);

                                // Run the update command
                                run_command(&update_cmd, state, action_tx_clone, false).await;

                                // Restore previous selection state
                                state.selected_projects = previous_selection;

                                // After update completes, re-check this project's dependencies
                                // to refresh the UI with new versions
                                let project_for_recheck = state.get_selected_project().cloned();
                                if let Some(project) = project_for_recheck {
                                    // Clear the update wizard state first
                                    state.updater.selected_dependencies.clear();
                                    state.updater.outdated_dependencies.clear();

                                    // Trigger a background update check to refresh
                                    let action_tx_clone = action_tx.clone();
                                    let deps = project.dependencies.clone();
                                    tokio::spawn(async move {
                                        // Re-check this project's dependencies
                                        check_single_project_for_updates(deps, action_tx_clone, false).await; // Don't use cache after update
                                    });
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
                // Timeout to keep loop cycling - ensures UI updates
                continue;
            }
        };

        if state.should_quit {
            return Ok(());
        }
    }
}
