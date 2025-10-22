pub mod app;
pub mod components;
pub mod events;
pub mod project;
pub mod runner;
pub mod ui;

use crate::app::{reducer, AppState};
use crate::events::{Action, Mode, Command};
use crate::project::find_rust_projects;
use crate::ui::ui;
use crate::runner::{run_command, check_for_updates};
use crate::components::{
    help::Help, palette::CommandPalette, projects::ProjectList, text_input::TextInput,
    updater::UpdateWizard, Component,
};

use clap::Parser;
use crossterm::{
    event::{Event, KeyCode, KeyModifiers, EnableMouseCapture, DisableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{
    backend::{Backend, CrosstermBackend},
    Terminal,
};
use std::io;
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[arg(default_value = ".")]
    pub target_directory: String,
}

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
            tokio::task::spawn_blocking(move || find_rust_projects(&target_directory))
        ).await;
        
        match scan_result {
            Ok(Ok(projects)) => {
                let _ = action_tx_clone.send(Action::FinishProjectScan(projects)).await;
            }
            Ok(Err(_)) | Err(_) => {
                // Timeout or panic - send empty list and continue
                let _ = action_tx_clone.send(Action::FinishProjectScan(Vec::new())).await;
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
                        match key.code {
                            KeyCode::Char('q') => Some(Action::Quit),
                            KeyCode::Char('?') => Some(Action::ShowHelp),
                            KeyCode::Char(':') => Some(Action::ShowCommandPalette),
                            KeyCode::Char('u') => Some(Action::StartUpdateWizard),
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
                    reducer(state, action);
                }
            }
            Some(action) = action_rx.recv() => {
                match &action {
                    Action::ExecuteCommand(command) => {
                        match command {
                            Command::Cargo { command } => {
                                let action_tx_clone = action_tx.clone();
                                let command_str = command.clone();
                                // Always run on selected projects (on_all = false)
                                run_command(&command_str, state, action_tx_clone, false).await;
                                reducer(state, Action::EnterNormalMode);
                            }
                            _ => {}
                        }
                    }
                    Action::StartUpdateWizard => {
                        let action_tx_clone = action_tx.clone();
                        check_for_updates(state, action_tx_clone).await;
                        reducer(state, action);
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
