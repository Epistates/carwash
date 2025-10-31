//! Event handling and command processing
//!
//! This module defines the types for handling user input, application modes, and
//! commands that can be executed by the application.

use crate::project::{Dependency, Project};

/// Represents a command that can be executed in CarWash
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Execute a cargo command on selected projects
    Cargo { command: String },
    /// Set the target directory for scanning
    SetTargetDir,
    /// Quit the application
    Quit,
}

/// Application actions that can be performed by the user or internal events
#[derive(Debug, Clone)]
pub enum Action {
    /// Project scanning has finished with results
    FinishProjectScan(Vec<Project>),
    /// Move selection to next item
    SelectNext,
    /// Move selection to previous item
    SelectPrevious,
    /// Move selection to parent item
    SelectParent,
    /// Move selection to child item
    SelectChild,
    /// Toggle selection on current item
    ToggleSelection,
    /// Open the command palette
    ShowCommandPalette,
    /// Show help screen
    ShowHelp,
    /// Show settings modal
    ShowSettings,
    /// Close settings modal without saving
    CloseSettings,
    /// Enter normal mode
    EnterNormalMode,
    /// Update command palette input
    UpdatePaletteInput(String),
    /// Select next item in palette
    PaletteSelectNext,
    /// Select previous item in palette
    PaletteSelectPrevious,
    /// Execute a command
    ExecuteCommand(Command),
    /// Update text input buffer
    UpdateTextInput(String),
    /// Start the update wizard
    StartUpdateWizard,
    /// Toggle selection in update wizard
    ToggleUpdateSelection,
    /// Update cache duration text in settings modal
    SettingsUpdateCacheInput(String),
    /// Toggle background update preference in settings modal
    SettingsToggleBackground,
    /// Persist settings changes
    SaveSettings,
    /// Run selected updates
    RunUpdate,
    /// Check for dependency updates
    CheckForUpdates,
    /// Update dependency information with project name
    UpdateDependencies(String, Vec<Dependency>),
    /// Start background update checking
    StartBackgroundUpdateCheck,
    /// Update status of a specific dependency
    UpdateDependencyStatus(String, crate::project::DependencyCheckStatus),
    /// Stream start - beginning of update checks
    UpdateDependenciesStreamStart(String),
    /// Update a single dependency with latest info
    UpdateSingleDependency(String, Dependency),
    /// Update check status for a specific dependency (for UI streaming)
    UpdateDependencyCheckStatus(String, String, crate::project::DependencyCheckStatus),
    /// Create a new output tab
    CreateTab(String),
    /// Add output line to a tab
    AddOutput(usize, String),
    /// Mark command execution as finished
    FinishCommand(usize),
    /// Switch to a specific tab
    SwitchToTab(usize),
    /// Process pending background update tasks
    ProcessBackgroundUpdateQueue,
    /// Queue a project for update checking (background or priority)
    QueueBackgroundUpdate(String, bool),
    /// Update the visual check status of a project
    UpdateProjectCheckStatus(String, crate::project::ProjectCheckStatus),
    /// Quit the application
    Quit,
}

/// Current mode of the application UI
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Loading projects state
    Loading,
    /// Normal browsing mode
    Normal,
    /// Command palette is open
    CommandPalette,
    /// Update wizard is open
    UpdateWizard,
    /// Text input mode
    TextInput,
    /// Help screen is displayed
    Help,
    /// Settings modal is open
    Settings,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_equality() {
        let cmd1 = Command::Cargo {
            command: "build".to_string(),
        };
        let cmd2 = Command::Cargo {
            command: "build".to_string(),
        };
        assert_eq!(cmd1, cmd2);
    }

    #[test]
    fn test_command_clone() {
        let cmd = Command::Cargo {
            command: "test".to_string(),
        };
        let cloned = cmd.clone();
        assert_eq!(cmd, cloned);
    }

    #[test]
    fn test_mode_equality() {
        assert_eq!(Mode::Normal, Mode::Normal);
        assert_eq!(Mode::Loading, Mode::Loading);
        assert_eq!(Mode::CommandPalette, Mode::CommandPalette);
        assert_eq!(Mode::UpdateWizard, Mode::UpdateWizard);
        assert_eq!(Mode::TextInput, Mode::TextInput);
        assert_eq!(Mode::Help, Mode::Help);
        assert_eq!(Mode::Settings, Mode::Settings);
    }

    #[test]
    fn test_mode_clone() {
        let mode = Mode::Normal;
        let cloned = mode.clone();
        assert_eq!(mode, cloned);
    }

    #[test]
    fn test_mode_inequality() {
        assert_ne!(Mode::Normal, Mode::Loading);
        assert_ne!(Mode::CommandPalette, Mode::UpdateWizard);
        assert_ne!(Mode::Settings, Mode::Normal);
    }
}
