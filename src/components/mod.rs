//! UI components for the CarWash application
//!
//! This module contains all the reusable UI components that make up the CarWash interface.
//! Each component follows the [`Component`] trait interface for consistent event handling and rendering.

use crate::app::AppState;
use crate::events::Action;
use crossterm::event::KeyCode;
use ratatui::{Frame, layout::Rect};

pub mod dependencies;
pub mod help;
pub mod output;
pub mod palette;
pub mod projects;
pub mod settings;
pub mod status;
pub mod text_input;
pub mod updater;

/// Trait for UI components in CarWash
///
/// All UI components implement this trait to provide a consistent interface for
/// event handling and rendering.
pub trait Component {
    /// Handle keyboard input for this component
    ///
    /// # Arguments
    ///
    /// * `key` - The key that was pressed
    /// * `app` - Mutable reference to the application state
    ///
    /// # Returns
    ///
    /// Returns an [`Action`] if the key triggered an action, or `None` otherwise.
    fn handle_key_events(&mut self, key: KeyCode, app: &mut AppState) -> Option<Action>;

    /// Render this component to the terminal
    ///
    /// # Arguments
    ///
    /// * `f` - The terminal frame to draw to
    /// * `app` - Mutable reference to the application state
    /// * `area` - The rectangular area to draw within
    fn draw(&mut self, f: &mut Frame, app: &mut AppState, area: Rect);
}
