//! Theme configuration and management
//!
//! Handles loading, switching, and saving theme preferences from config files.

use crate::ui::styles::ColorScheme;
use serde::{Deserialize, Serialize};

/// Theme configuration stored in config file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    /// Current active theme name
    #[serde(default = "default_theme")]
    pub current: String,

    /// Custom theme definitions (for future use)
    #[serde(default)]
    pub custom: Option<CustomTheme>,
}

fn default_theme() -> String {
    "dark".to_string()
}

/// Custom theme definition (user-defined colors)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomTheme {
    /// Hex color for selection/focus
    pub selection: Option<String>,

    /// Hex color for success/completed
    pub success: Option<String>,

    /// Hex color for warning/attention
    pub warning: Option<String>,

    /// Hex color for error/failure
    pub error: Option<String>,

    /// Hex color for muted/disabled
    pub muted: Option<String>,

    /// Hex color for primary text
    pub text: Option<String>,

    /// Hex color for dim elements
    pub dim: Option<String>,

    /// Hex color for primary UI (directories, headers)
    pub primary: Option<String>,
}

impl ThemeConfig {
    /// Create a new theme config with specified scheme
    pub fn new(scheme: &str) -> Self {
        Self {
            current: scheme.to_string(),
            custom: None,
        }
    }

    /// Get the current color scheme
    pub fn current_scheme(&self) -> ColorScheme {
        match self.current.as_str() {
            "light" => ColorScheme::Light,
            "nord" => ColorScheme::Nord,
            "dracula" => ColorScheme::Dracula,
            "cosmic" => ColorScheme::Cosmic,
            _ => ColorScheme::Dark,
        }
    }

    /// Get the next theme in the cycle
    pub fn next_scheme(&self) -> ColorScheme {
        let current = self.current_scheme();
        match current {
            ColorScheme::Dark => ColorScheme::Light,
            ColorScheme::Light => ColorScheme::Nord,
            ColorScheme::Nord => ColorScheme::Dracula,
            ColorScheme::Dracula => ColorScheme::Cosmic,
            ColorScheme::Cosmic => ColorScheme::Dark,
        }
    }

    /// Get the theme name as a display string
    pub fn display_name(&self) -> String {
        match self.current.as_str() {
            "dark" => "Dark".to_string(),
            "light" => "Light".to_string(),
            "nord" => "Nord".to_string(),
            "dracula" => "Dracula".to_string(),
            "cosmic" => "Cosmic".to_string(),
            name => format!("Custom: {}", name),
        }
    }

    /// List all available theme names
    pub fn available_themes() -> Vec<&'static str> {
        vec!["dark", "light", "nord", "dracula", "cosmic"]
    }

    /// Set the active theme by name
    pub fn set_theme(&mut self, name: &str) {
        if Self::available_themes().contains(&name) || self.custom.is_some() {
            self.current = name.to_string();
        }
    }

    /// Switch to the next theme
    pub fn cycle_next(&mut self) {
        let next = self.next_scheme();
        self.current = match next {
            ColorScheme::Dark => "dark",
            ColorScheme::Light => "light",
            ColorScheme::Nord => "nord",
            ColorScheme::Dracula => "dracula",
            ColorScheme::Cosmic => "cosmic",
        }
        .to_string();
    }

    /// Get a list of all available themes as display strings
    pub fn available_themes_display() -> Vec<String> {
        vec![
            "Dark".to_string(),
            "Light".to_string(),
            "Nord".to_string(),
            "Dracula".to_string(),
            "Cosmic".to_string(),
        ]
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            current: default_theme(),
            custom: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_config_default() {
        let config = ThemeConfig::default();
        assert_eq!(config.current, "dark");
        assert!(config.custom.is_none());
    }

    #[test]
    fn test_current_scheme() {
        let config = ThemeConfig::new("dark");
        assert_eq!(config.current_scheme(), ColorScheme::Dark);

        let config = ThemeConfig::new("light");
        assert_eq!(config.current_scheme(), ColorScheme::Light);

        let config = ThemeConfig::new("nord");
        assert_eq!(config.current_scheme(), ColorScheme::Nord);

        let config = ThemeConfig::new("dracula");
        assert_eq!(config.current_scheme(), ColorScheme::Dracula);

        let config = ThemeConfig::new("cosmic");
        assert_eq!(config.current_scheme(), ColorScheme::Cosmic);
    }

    #[test]
    fn test_next_scheme() {
        let config = ThemeConfig::new("dark");
        assert_eq!(config.next_scheme(), ColorScheme::Light);

        let config = ThemeConfig::new("dracula");
        assert_eq!(config.next_scheme(), ColorScheme::Cosmic);

        let config = ThemeConfig::new("cosmic");
        assert_eq!(config.next_scheme(), ColorScheme::Dark);
    }

    #[test]
    fn test_cycle_next() {
        let mut config = ThemeConfig::new("dark");
        config.cycle_next();
        assert_eq!(config.current, "light");
        config.cycle_next();
        assert_eq!(config.current, "nord");
        config.cycle_next();
        assert_eq!(config.current, "dracula");
        config.cycle_next();
        assert_eq!(config.current, "cosmic");
        config.cycle_next();
        assert_eq!(config.current, "dark");
    }

    #[test]
    fn test_display_name() {
        let config = ThemeConfig::new("dark");
        assert_eq!(config.display_name(), "Dark");

        let mut config = ThemeConfig::new("custom");
        config.custom = Some(CustomTheme {
            selection: None,
            success: None,
            warning: None,
            error: None,
            muted: None,
            text: None,
            dim: None,
            primary: None,
        });
        assert_eq!(config.display_name(), "Custom: custom");
    }

    #[test]
    fn test_available_themes() {
        let themes = ThemeConfig::available_themes();
        assert_eq!(themes.len(), 5);
        assert!(themes.contains(&"dark"));
        assert!(themes.contains(&"light"));
        assert!(themes.contains(&"cosmic"));
    }

    #[test]
    fn test_config_serialization() {
        let config = ThemeConfig::default();
        let toml_str = toml::to_string(&config).expect("Failed to serialize");
        let deserialized: ThemeConfig = toml::from_str(&toml_str).expect("Failed to deserialize");
        assert_eq!(config.current, deserialized.current);
    }
}
