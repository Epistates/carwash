use crate::project::{Dependency, Project};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Cargo { command: String },
    SetTargetDir,
    Quit,
}

pub enum Action {
    FinishProjectScan(Vec<Project>),
    SelectNext,
    SelectPrevious,
    SelectParent,
    SelectChild,
    ToggleSelection,
    ShowCommandPalette,
    ShowHelp,
    EnterNormalMode,
    UpdatePaletteInput(String),
    PaletteSelectNext,
    PaletteSelectPrevious,
    ExecuteCommand(Command),
    UpdateTextInput(String),
    StartUpdateWizard,
    ToggleUpdateSelection,
    RunUpdate,
    CheckForUpdates,
    UpdateDependencies(Vec<Dependency>),
    StartBackgroundUpdateCheck,
    UpdateDependencyStatus(String, crate::project::DependencyCheckStatus),
    CreateTab(String),
    AddOutput(usize, String),
    FinishCommand(usize),
    SwitchToTab(usize),
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Loading,
    Normal,
    CommandPalette,
    UpdateWizard,
    TextInput,
    Help,
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
    }
}
