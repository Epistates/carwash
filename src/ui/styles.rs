//! Centralized style system for CarWash
//!
//! This module defines all colors, styles, and visual elements used throughout
//! the application. This provides a single source of truth for the visual design
//! and makes theming and customization straightforward.

use ratatui::style::{Color, Modifier, Style};

/// Core color definitions for CarWash
#[derive(Debug, Clone, Copy)]
pub struct Colors {
    /// Primary selection/focus color
    pub selection: Color,
    /// Success/active/completed status
    pub success: Color,
    /// Warning/attention needed
    pub warning: Color,
    /// Error/failure status
    pub error: Color,
    /// Muted/disabled/secondary text
    pub muted: Color,
    /// Default text color
    pub text: Color,
    /// Subtle background elements
    pub dim: Color,
    /// Primary foreground color (directories, headers)
    pub primary: Color,
}

impl Colors {
    /// Light theme colors
    pub fn light() -> Self {
        Self {
            selection: Color::Cyan,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            muted: Color::DarkGray,
            text: Color::White,
            dim: Color::Rgb(150, 150, 200),
            primary: Color::Cyan,
        }
    }

    /// Dark theme colors (default)
    pub fn dark() -> Self {
        Self {
            selection: Color::Cyan,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            muted: Color::DarkGray,
            text: Color::White,
            dim: Color::Rgb(150, 150, 200),
            primary: Color::Cyan,
        }
    }

    /// Custom color scheme from RGB values
    pub fn custom(
        selection: Color,
        success: Color,
        warning: Color,
        error: Color,
        muted: Color,
        text: Color,
        dim: Color,
        primary: Color,
    ) -> Self {
        Self {
            selection,
            success,
            warning,
            error,
            muted,
            text,
            dim,
            primary,
        }
    }
}

impl Default for Colors {
    fn default() -> Self {
        Self::dark()
    }
}

impl Colors {
    /// Nord theme - Arctic, north-bluish color palette
    pub fn nord() -> Self {
        Self {
            selection: Color::Rgb(136, 192, 208), // nord8
            success: Color::Rgb(163, 190, 140),   // nord14 - green
            warning: Color::Rgb(235, 203, 139),   // nord13 - yellow
            error: Color::Rgb(191, 97, 106),      // nord11 - red
            muted: Color::Rgb(76, 86, 106),       // nord3
            text: Color::Rgb(236, 239, 244),      // nord4
            dim: Color::Rgb(216, 222, 233),       // nord6
            primary: Color::Rgb(136, 192, 208),   // nord8 - cyan
        }
    }

    /// Dracula theme - Dark, vampiric colors
    pub fn dracula() -> Self {
        Self {
            selection: Color::Rgb(139, 233, 253), // cyan
            success: Color::Rgb(80, 250, 123),    // green
            warning: Color::Rgb(241, 250, 140),   // yellow
            error: Color::Rgb(255, 121, 198),     // pink
            muted: Color::Rgb(98, 114, 164),      // comment gray
            text: Color::Rgb(248, 248, 242),      // foreground
            dim: Color::Rgb(68, 71, 90),          // background
            primary: Color::Rgb(189, 147, 249),   // purple
        }
    }

    /// Cosmic theme - Neon Cyans and Deep Purples
    pub fn cosmic() -> Self {
        Self {
            selection: Color::Rgb(0, 255, 255), // Neon Cyan
            success: Color::Rgb(50, 255, 100),  // Bright Green
            warning: Color::Rgb(255, 200, 0),   // Bright Yellow
            error: Color::Rgb(255, 50, 80),     // Neon Red
            muted: Color::Rgb(100, 100, 140),   // Muted Purple
            text: Color::Rgb(240, 240, 255),    // White-ish
            dim: Color::Rgb(30, 30, 50),        // Dark Purple
            primary: Color::Rgb(180, 0, 255),   // Neon Purple
        }
    }
}

/// Named color scheme presets
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Dark,
    Light,
    Nord,
    Dracula,
    Cosmic,
}

impl ColorScheme {
    pub fn colors(self) -> Colors {
        match self {
            ColorScheme::Dark => Colors::dark(),
            ColorScheme::Light => Colors::light(),
            ColorScheme::Nord => Colors::nord(),
            ColorScheme::Dracula => Colors::dracula(),
            ColorScheme::Cosmic => Colors::cosmic(),
        }
    }

    pub fn all() -> &'static [ColorScheme] {
        &[
            ColorScheme::Dark,
            ColorScheme::Light,
            ColorScheme::Nord,
            ColorScheme::Dracula,
            ColorScheme::Cosmic,
        ]
    }
}

/// Styled components - named style presets for common UI elements
pub struct StyledComponent;

impl StyledComponent {
    // Project/Directory Styles
    pub fn directory_selected(colors: Colors) -> Style {
        Style::default()
            .fg(colors.selection)
            .add_modifier(Modifier::BOLD)
    }

    pub fn directory_unselected(colors: Colors) -> Style {
        Style::default().fg(colors.dim).add_modifier(Modifier::BOLD)
    }

    pub fn project_selected(colors: Colors) -> Style {
        Style::default()
            .fg(colors.success)
            .add_modifier(Modifier::BOLD)
    }

    pub fn project_unselected(colors: Colors) -> Style {
        Style::default().fg(colors.text)
    }

    // Status Indicators
    pub fn status_unchecked(colors: Colors) -> Style {
        Style::default().fg(colors.muted)
    }

    pub fn status_checking(_colors: Colors) -> Style {
        Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD)
    }

    pub fn status_has_updates(colors: Colors) -> Style {
        Style::default()
            .fg(colors.warning)
            .add_modifier(Modifier::BOLD)
    }

    pub fn status_up_to_date(colors: Colors) -> Style {
        Style::default().fg(colors.success)
    }

    // Dependency Styles
    pub fn dependency_up_to_date(colors: Colors) -> Style {
        Style::default().fg(colors.success)
    }

    pub fn dependency_outdated(colors: Colors) -> Style {
        Style::default()
            .fg(colors.warning)
            .add_modifier(Modifier::BOLD)
    }

    pub fn dependency_checking(colors: Colors) -> Style {
        Style::default()
            .fg(colors.selection)
            .add_modifier(Modifier::BOLD)
    }

    pub fn dependency_unchecked(colors: Colors) -> Style {
        Style::default().fg(colors.muted)
    }

    // UI Component Styles
    pub fn block_border(colors: Colors) -> Style {
        Style::default().fg(colors.primary)
    }

    pub fn block_title(colors: Colors) -> Style {
        Style::default()
            .fg(colors.primary)
            .add_modifier(Modifier::BOLD)
    }

    pub fn status_bar_normal(colors: Colors) -> Style {
        Style::default().fg(colors.text).bg(Color::Rgb(40, 40, 60))
    }

    pub fn status_bar_mode_indicator(colors: Colors) -> Style {
        Style::default()
            .fg(colors.selection)
            .add_modifier(Modifier::BOLD)
            .bg(Color::Rgb(40, 40, 60))
    }

    pub fn command_palette_match(colors: Colors) -> Style {
        Style::default()
            .fg(colors.selection)
            .add_modifier(Modifier::BOLD)
    }

    pub fn modal_background(colors: Colors) -> Style {
        Style::default().fg(colors.text).bg(Color::Rgb(30, 30, 50))
    }

    pub fn modal_border(colors: Colors) -> Style {
        Style::default()
            .fg(colors.selection)
            .add_modifier(Modifier::BOLD)
    }

    pub fn input_field(colors: Colors) -> Style {
        Style::default().fg(colors.text).bg(Color::Rgb(20, 20, 40))
    }

    pub fn help_text(colors: Colors) -> Style {
        Style::default().fg(colors.text)
    }

    pub fn help_key(colors: Colors) -> Style {
        Style::default()
            .fg(colors.selection)
            .add_modifier(Modifier::BOLD)
    }

    pub fn warning_text(colors: Colors) -> Style {
        Style::default()
            .fg(colors.warning)
            .add_modifier(Modifier::BOLD)
    }

    pub fn error_text(colors: Colors) -> Style {
        Style::default()
            .fg(colors.error)
            .add_modifier(Modifier::BOLD)
    }

    pub fn success_text(colors: Colors) -> Style {
        Style::default()
            .fg(colors.success)
            .add_modifier(Modifier::BOLD)
    }
}

/// Status indicator symbols for consistent icon usage
pub struct StatusSymbols;

impl StatusSymbols {
    pub const UNCHECKED: &'static str = "⋯";
    pub const CHECKING: &'static str = "⟳";
    pub const HAS_UPDATES: &'static str = "⚠";
    pub const UP_TO_DATE: &'static str = "✓";
    pub const ARROW_RIGHT: &'static str = "→";
    pub const EXPAND: &'static str = "▾";
    pub const COLLAPSE: &'static str = "▸";
    pub const SELECTION: &'static str = "▶";
    pub const BULLET: &'static str = "•";
    pub const GIT_DIRTY: &'static str = "*";
    pub const GIT_CLEAN: &'static str = "";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_colors_dark_created() {
        let colors = Colors::dark();
        assert_eq!(colors.success, Color::Green);
    }

    #[test]
    fn test_color_scheme_all() {
        assert_eq!(ColorScheme::all().len(), 5);
    }

    #[test]
    fn test_styled_component_styles() {
        let colors = Colors::default();
        let style = StyledComponent::directory_selected(colors);
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }
}
