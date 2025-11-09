//! Configuration management for CarWash
//!
//! This module handles loading, saving, and managing user configuration including
//! themes, keybindings, layout preferences, and other settings.

pub mod theme_config;
pub mod keybinding_config;

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use anyhow::{Context, Result};

pub use theme_config::ThemeConfig;
pub use keybinding_config::KeybindingConfig;

/// Main configuration structure for CarWash
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Theme configuration
    pub theme: ThemeConfig,

    /// Layout preferences
    #[serde(default)]
    pub layout: LayoutConfig,

    /// Keybinding configuration
    #[serde(default)]
    pub keybindings: KeybindingConfig,

    /// Progress visualization settings
    #[serde(default)]
    pub progress: ProgressConfig,
}

/// Layout preference configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutConfig {
    /// Left pane width percentage (projects)
    #[serde(default = "default_left_pane")]
    pub left_pane_percent: u16,

    /// Top-right pane height percentage (dependencies)
    #[serde(default = "default_top_right")]
    pub top_right_percent: u16,
}

fn default_left_pane() -> u16 {
    40
}

fn default_top_right() -> u16 {
    40
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            left_pane_percent: default_left_pane(),
            top_right_percent: default_top_right(),
        }
    }
}

// KeybindingConfig is now defined in keybinding_config.rs and re-exported above

/// Progress visualization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressConfig {
    /// Show visual progress bars
    #[serde(default = "default_show_bars")]
    pub show_visual_bars: bool,

    /// Animation speed (slow, normal, fast)
    #[serde(default = "default_animation_speed")]
    pub animation_speed: String,
}

fn default_show_bars() -> bool {
    true
}

fn default_animation_speed() -> String {
    "normal".to_string()
}

impl Default for ProgressConfig {
    fn default() -> Self {
        Self {
            show_visual_bars: default_show_bars(),
            animation_speed: default_animation_speed(),
        }
    }
}

impl Config {
    /// Get the path to the config file
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = directories::ProjectDirs::from("", "", "carwash")
            .context("Unable to determine config directory")?
            .config_dir()
            .to_path_buf();

        Ok(config_dir.join("config.toml"))
    }

    /// Load configuration from disk, or return defaults if file doesn't exist
    pub fn load() -> Self {
        match Self::config_path() {
            Ok(path) => {
                if path.exists() {
                    match fs::read_to_string(&path) {
                        Ok(content) => match toml::from_str::<Config>(&content) {
                            Ok(config) => return config,
                            Err(e) => {
                                eprintln!("Warning: Failed to parse config: {}", e);
                                Self::default()
                            }
                        },
                        Err(e) => {
                            eprintln!("Warning: Failed to read config: {}", e);
                            Self::default()
                        }
                    }
                } else {
                    Self::default()
                }
            }
            Err(e) => {
                eprintln!("Warning: Could not determine config path: {}", e);
                Self::default()
            }
        }
    }

    /// Save configuration to disk
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .context("Failed to create config directory")?;
        }

        let content = toml::to_string_pretty(self)
            .context("Failed to serialize config")?;

        fs::write(&path, content)
            .context("Failed to write config file")?;

        Ok(())
    }

    /// Get theme configuration reference
    pub fn theme(&self) -> &ThemeConfig {
        &self.theme
    }

    /// Get mutable theme configuration
    pub fn theme_mut(&mut self) -> &mut ThemeConfig {
        &mut self.theme
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: ThemeConfig::default(),
            layout: LayoutConfig::default(),
            keybindings: KeybindingConfig::default(),
            progress: ProgressConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.layout.left_pane_percent, 40);
        assert_eq!(config.layout.top_right_percent, 40);
        assert!(config.progress.show_visual_bars);
    }

    #[test]
    fn test_layout_config_default() {
        let layout = LayoutConfig::default();
        assert_eq!(layout.left_pane_percent, 40);
        assert_eq!(layout.top_right_percent, 40);
    }

    #[test]
    fn test_progress_config_default() {
        let progress = ProgressConfig::default();
        assert!(progress.show_visual_bars);
        assert_eq!(progress.animation_speed, "normal");
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let toml_str = toml::to_string(&config).expect("Failed to serialize");
        let deserialized: Config = toml::from_str(&toml_str)
            .expect("Failed to deserialize");
        assert_eq!(config.layout.left_pane_percent, deserialized.layout.left_pane_percent);
    }
}
