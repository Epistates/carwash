//! Theme management system for CarWash
//!
//! Handles theme switching, persistence, and global theme state.

use crate::ui::styles::{ColorScheme, Colors};
use std::sync::{Arc, Mutex};

/// Global theme manager using Arc<Mutex> for thread-safe access
pub static THEME_MANAGER: once_cell::sync::Lazy<Arc<Mutex<ThemeManager>>> =
    once_cell::sync::Lazy::new(|| Arc::new(Mutex::new(ThemeManager::new())));

/// Theme management for the application
#[derive(Debug, Clone)]
pub struct ThemeManager {
    current_scheme: ColorScheme,
    current_colors: Colors,
}

impl Default for ThemeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ThemeManager {
    /// Create a new theme manager with default theme
    pub fn new() -> Self {
        let scheme = ColorScheme::Dark;
        let colors = scheme.colors();
        Self {
            current_scheme: scheme,
            current_colors: colors,
        }
    }

    /// Get the current color scheme
    pub fn scheme(&self) -> ColorScheme {
        self.current_scheme
    }

    /// Get the current colors
    pub fn colors(&self) -> Colors {
        self.current_colors
    }

    /// Switch to a different color scheme
    pub fn set_scheme(&mut self, scheme: ColorScheme) {
        self.current_scheme = scheme;
        self.current_colors = scheme.colors();
    }

    /// Switch to the next theme in the cycle
    pub fn next_theme(&mut self) {
        let schemes = ColorScheme::all();
        let current_idx = schemes.iter().position(|&s| s == self.current_scheme).unwrap_or(0);
        let next_idx = (current_idx + 1) % schemes.len();
        self.set_scheme(schemes[next_idx]);
    }

    /// Switch to the previous theme in the cycle
    pub fn previous_theme(&mut self) {
        let schemes = ColorScheme::all();
        let current_idx = schemes.iter().position(|&s| s == self.current_scheme).unwrap_or(0);
        let next_idx = if current_idx == 0 {
            schemes.len() - 1
        } else {
            current_idx - 1
        };
        self.set_scheme(schemes[next_idx]);
    }
}

/// Theme trait for custom theme implementations
pub struct Theme {
    pub name: String,
    pub colors: Colors,
}

impl Theme {
    pub fn new(name: impl Into<String>, colors: Colors) -> Self {
        Self {
            name: name.into(),
            colors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_manager_creation() {
        let tm = ThemeManager::new();
        assert_eq!(tm.scheme(), ColorScheme::Dark);
    }

    #[test]
    fn test_next_theme() {
        let mut tm = ThemeManager::new();
        tm.next_theme();
        assert_ne!(tm.scheme(), ColorScheme::Dark);
    }

    #[test]
    fn test_previous_theme() {
        let mut tm = ThemeManager::new();
        tm.next_theme();
        tm.previous_theme();
        assert_eq!(tm.scheme(), ColorScheme::Dark);
    }
}
