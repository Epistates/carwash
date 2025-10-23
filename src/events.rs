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
