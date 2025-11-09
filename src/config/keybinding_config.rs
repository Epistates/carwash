//! Keybinding configuration system
//!
//! Manages customizable keybindings with sensible defaults and user overrides.
//! Supports vim-style, emacs-style, and custom keybinding schemes.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Keybinding action identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyAction {
    // Navigation
    SelectNext,
    SelectPrevious,
    SelectUp,
    SelectDown,
    SelectParent,
    SelectChild,
    ToggleSelection,

    // Searching and filtering
    SearchProjects,
    EnterFilter,
    ExitFilter,

    // Theme and UI
    CycleTheme,
    IncreaseLeftPane,
    DecreaseLeftPane,
    IncreaseTopRight,
    DecreaseTopRight,
    ResetLayout,

    // Commands and modes
    ShowCommandPalette,
    ShowSettings,
    ShowHelp,
    StartUpdateWizard,
    CheckForUpdates,

    // Application
    Quit,
}

impl KeyAction {
    /// Get human-readable description for this action
    pub fn description(&self) -> &'static str {
        match self {
            KeyAction::SelectNext => "Select next item",
            KeyAction::SelectPrevious => "Select previous item",
            KeyAction::SelectUp => "Move selection up",
            KeyAction::SelectDown => "Move selection down",
            KeyAction::SelectParent => "Select parent/collapse",
            KeyAction::SelectChild => "Select child/expand",
            KeyAction::ToggleSelection => "Toggle project selection",
            KeyAction::SearchProjects => "Search projects",
            KeyAction::EnterFilter => "Enter filter mode",
            KeyAction::ExitFilter => "Exit filter mode",
            KeyAction::CycleTheme => "Cycle to next theme",
            KeyAction::IncreaseLeftPane => "Increase left pane width",
            KeyAction::DecreaseLeftPane => "Decrease left pane width",
            KeyAction::IncreaseTopRight => "Increase dependencies pane height",
            KeyAction::DecreaseTopRight => "Decrease dependencies pane height",
            KeyAction::ResetLayout => "Reset layout to defaults",
            KeyAction::ShowCommandPalette => "Show command palette",
            KeyAction::ShowSettings => "Show settings",
            KeyAction::ShowHelp => "Show help",
            KeyAction::StartUpdateWizard => "Start update wizard",
            KeyAction::CheckForUpdates => "Check for updates",
            KeyAction::Quit => "Quit application",
        }
    }
}

/// Keybinding configuration from TOML file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindingConfig {
    /// Keybinding scheme: "default", "vim", "emacs", or "custom"
    #[serde(default = "default_scheme")]
    pub scheme: String,

    /// Custom keybindings (action -> list of key strings)
    /// Example: "SelectNext" -> ["j", "Down"]
    #[serde(default)]
    pub custom: HashMap<String, Vec<String>>,
}

fn default_scheme() -> String {
    "default".to_string()
}

impl KeybindingConfig {
    /// Get keybindings for a specific scheme
    pub fn get_bindings(&self) -> HashMap<String, Vec<String>> {
        let mut bindings = self.default_bindings();

        // Apply custom overrides
        for (action, keys) in &self.custom {
            bindings.insert(action.clone(), keys.clone());
        }

        bindings
    }

    /// Default keybindings (what we ship with)
    pub fn default_bindings(&self) -> HashMap<String, Vec<String>> {
        match self.scheme.as_str() {
            "vim" => self.vim_bindings(),
            "emacs" => self.emacs_bindings(),
            _ => self.default_scheme_bindings(),
        }
    }

    /// Default CarWash keybindings (blended vim+standard)
    fn default_scheme_bindings(&self) -> HashMap<String, Vec<String>> {
        vec![
            (
                "SelectNext".to_string(),
                vec!["j".to_string(), "Down".to_string()],
            ),
            (
                "SelectPrevious".to_string(),
                vec!["k".to_string(), "Up".to_string()],
            ),
            ("SelectUp".to_string(), vec!["Up".to_string()]),
            ("SelectDown".to_string(), vec!["Down".to_string()]),
            (
                "SelectParent".to_string(),
                vec!["h".to_string(), "Left".to_string()],
            ),
            (
                "SelectChild".to_string(),
                vec!["l".to_string(), "Right".to_string()],
            ),
            ("ToggleSelection".to_string(), vec!["space".to_string()]),
            ("SearchProjects".to_string(), vec!["/".to_string()]),
            ("EnterFilter".to_string(), vec!["/".to_string()]),
            ("ExitFilter".to_string(), vec!["Esc".to_string()]),
            (
                "CycleTheme".to_string(),
                vec!["t".to_string(), "T".to_string()],
            ),
            (
                "IncreaseLeftPane".to_string(),
                vec!["]".to_string(), "{".to_string()],
            ),
            ("DecreaseLeftPane".to_string(), vec!["[".to_string()]),
            (
                "IncreaseTopRight".to_string(),
                vec![")".to_string(), "+".to_string()],
            ),
            (
                "DecreaseTopRight".to_string(),
                vec!["-".to_string(), "(".to_string()],
            ),
            ("ResetLayout".to_string(), vec!["R".to_string()]),
            ("ShowCommandPalette".to_string(), vec![":".to_string()]),
            (
                "ShowSettings".to_string(),
                vec!["s".to_string(), "S".to_string()],
            ),
            ("ShowHelp".to_string(), vec!["?".to_string()]),
            ("StartUpdateWizard".to_string(), vec!["u".to_string()]),
            ("CheckForUpdates".to_string(), vec!["u".to_string()]),
            ("Quit".to_string(), vec!["q".to_string()]),
        ]
        .into_iter()
        .collect()
    }

    /// Vim-style keybindings
    fn vim_bindings(&self) -> HashMap<String, Vec<String>> {
        vec![
            ("SelectNext".to_string(), vec!["j".to_string()]),
            ("SelectPrevious".to_string(), vec!["k".to_string()]),
            ("SelectUp".to_string(), vec!["k".to_string()]),
            ("SelectDown".to_string(), vec!["j".to_string()]),
            ("SelectParent".to_string(), vec!["h".to_string()]),
            ("SelectChild".to_string(), vec!["l".to_string()]),
            ("ToggleSelection".to_string(), vec!["v".to_string()]),
            ("SearchProjects".to_string(), vec!["/".to_string()]),
            ("EnterFilter".to_string(), vec!["/".to_string()]),
            ("ExitFilter".to_string(), vec!["Esc".to_string()]),
            ("CycleTheme".to_string(), vec!["T".to_string()]),
            ("ShowCommandPalette".to_string(), vec![":".to_string()]),
            ("ShowSettings".to_string(), vec![";".to_string()]),
            ("ShowHelp".to_string(), vec!["?".to_string()]),
            ("StartUpdateWizard".to_string(), vec!["u".to_string()]),
            ("CheckForUpdates".to_string(), vec!["U".to_string()]),
            ("Quit".to_string(), vec!["q".to_string()]),
        ]
        .into_iter()
        .collect()
    }

    /// Emacs-style keybindings
    fn emacs_bindings(&self) -> HashMap<String, Vec<String>> {
        vec![
            ("SelectNext".to_string(), vec!["Down".to_string()]),
            ("SelectPrevious".to_string(), vec!["Up".to_string()]),
            ("SelectUp".to_string(), vec!["Up".to_string()]),
            ("SelectDown".to_string(), vec!["Down".to_string()]),
            ("SelectParent".to_string(), vec!["Left".to_string()]),
            ("SelectChild".to_string(), vec!["Right".to_string()]),
            ("ToggleSelection".to_string(), vec!["space".to_string()]),
            ("SearchProjects".to_string(), vec!["C-s".to_string()]),
            ("EnterFilter".to_string(), vec!["C-s".to_string()]),
            ("ExitFilter".to_string(), vec!["C-g".to_string()]),
            ("CycleTheme".to_string(), vec!["C-t".to_string()]),
            ("ShowCommandPalette".to_string(), vec!["C-x".to_string()]),
            ("ShowSettings".to_string(), vec!["C-,".to_string()]),
            ("ShowHelp".to_string(), vec!["C-h".to_string()]),
            ("StartUpdateWizard".to_string(), vec!["C-u".to_string()]),
            ("Quit".to_string(), vec!["C-c".to_string()]),
        ]
        .into_iter()
        .collect()
    }

    /// Get keybindings for a specific action
    pub fn get_action_keys(&self, action: &str) -> Option<Vec<String>> {
        self.get_bindings().get(action).cloned()
    }

    /// Get all actions with their keybindings and descriptions
    pub fn actions_with_keys(&self) -> Vec<(String, Vec<String>, &'static str)> {
        let bindings = self.get_bindings();
        vec![
            (
                "SelectNext".to_string(),
                bindings.get("SelectNext").cloned().unwrap_or_default(),
                KeyAction::SelectNext.description(),
            ),
            (
                "SelectPrevious".to_string(),
                bindings.get("SelectPrevious").cloned().unwrap_or_default(),
                KeyAction::SelectPrevious.description(),
            ),
            (
                "SelectParent".to_string(),
                bindings.get("SelectParent").cloned().unwrap_or_default(),
                KeyAction::SelectParent.description(),
            ),
            (
                "SelectChild".to_string(),
                bindings.get("SelectChild").cloned().unwrap_or_default(),
                KeyAction::SelectChild.description(),
            ),
            (
                "ToggleSelection".to_string(),
                bindings.get("ToggleSelection").cloned().unwrap_or_default(),
                KeyAction::ToggleSelection.description(),
            ),
            (
                "SearchProjects".to_string(),
                bindings.get("SearchProjects").cloned().unwrap_or_default(),
                KeyAction::SearchProjects.description(),
            ),
            (
                "CycleTheme".to_string(),
                bindings.get("CycleTheme").cloned().unwrap_or_default(),
                KeyAction::CycleTheme.description(),
            ),
            (
                "ShowCommandPalette".to_string(),
                bindings
                    .get("ShowCommandPalette")
                    .cloned()
                    .unwrap_or_default(),
                KeyAction::ShowCommandPalette.description(),
            ),
            (
                "ShowSettings".to_string(),
                bindings.get("ShowSettings").cloned().unwrap_or_default(),
                KeyAction::ShowSettings.description(),
            ),
            (
                "ShowHelp".to_string(),
                bindings.get("ShowHelp").cloned().unwrap_or_default(),
                KeyAction::ShowHelp.description(),
            ),
            (
                "StartUpdateWizard".to_string(),
                bindings
                    .get("StartUpdateWizard")
                    .cloned()
                    .unwrap_or_default(),
                KeyAction::StartUpdateWizard.description(),
            ),
            (
                "Quit".to_string(),
                bindings.get("Quit").cloned().unwrap_or_default(),
                KeyAction::Quit.description(),
            ),
        ]
    }
}

impl Default for KeybindingConfig {
    fn default() -> Self {
        Self {
            scheme: default_scheme(),
            custom: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keybinding_config_default() {
        let config = KeybindingConfig::default();
        assert_eq!(config.scheme, "default");
        assert!(config.custom.is_empty());
    }

    #[test]
    fn test_default_scheme_bindings() {
        let config = KeybindingConfig::default();
        let bindings = config.default_bindings();
        assert!(bindings.contains_key("SelectNext"));
        assert!(bindings.contains_key("Quit"));
    }

    #[test]
    fn test_vim_scheme_bindings() {
        let config = KeybindingConfig {
            scheme: "vim".to_string(),
            custom: HashMap::new(),
        };
        let bindings = config.default_bindings();
        assert_eq!(bindings.get("SelectNext"), Some(&vec!["j".to_string()]));
    }

    #[test]
    fn test_emacs_scheme_bindings() {
        let config = KeybindingConfig {
            scheme: "emacs".to_string(),
            custom: HashMap::new(),
        };
        let bindings = config.default_bindings();
        assert!(bindings.contains_key("SelectNext"));
    }

    #[test]
    fn test_custom_overrides() {
        let mut config = KeybindingConfig::default();
        config
            .custom
            .insert("SelectNext".to_string(), vec!["w".to_string()]);

        let bindings = config.get_bindings();
        assert_eq!(bindings.get("SelectNext"), Some(&vec!["w".to_string()]));
    }

    #[test]
    fn test_get_action_keys() {
        let config = KeybindingConfig::default();
        assert!(config.get_action_keys("SelectNext").is_some());
        assert!(config.get_action_keys("Quit").is_some());
    }

    #[test]
    fn test_key_action_descriptions() {
        assert_eq!(KeyAction::SelectNext.description(), "Select next item");
        assert_eq!(KeyAction::Quit.description(), "Quit application");
    }

    #[test]
    fn test_serialization() {
        let config = KeybindingConfig::default();
        let toml_str = toml::to_string(&config).expect("Failed to serialize");
        let deserialized: KeybindingConfig =
            toml::from_str(&toml_str).expect("Failed to deserialize");
        assert_eq!(config.scheme, deserialized.scheme);
    }
}
