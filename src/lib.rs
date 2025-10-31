//! CarWash - A beautiful TUI for managing multiple Rust projects
//!
//! CarWash provides a terminal user interface for managing Rust projects,
//! checking dependencies, and running cargo commands across multiple projects
//! with workspace support.
//!
//! # Features
//!
//! - **Multi-Project Management**: Scan and manage multiple Rust projects in a single view
//! - **Workspace Support**: Full support for Cargo workspaces with hierarchical project organization
//! - **Dependency Checking**: Check for outdated dependencies across all projects
//! - **Parallel Execution**: Run cargo commands in parallel across multiple projects
//! - **Beautiful TUI**: Modern terminal interface with intuitive navigation
//! - **Command History**: Built-in command history for quick access to previous commands
//!
//! # Getting Started
//!
//! CarWash is primarily used as a CLI application. You can scan a directory for Rust projects:
//!
//! ```sh
//! carwash /path/to/projects
//! carwash .  # Scan current directory
//! ```
//!
//! # Library Usage
//!
//! While CarWash is primarily a CLI application, you can use its library components:
//!
//! ```ignore
//! use carwash::project::Project;
//! use carwash::runner::UpdateQueue;
//!
//! // Use public APIs for project discovery and management
//! ```
//!
//! # Modules
//!
//! - [`app`] - Application state management
//! - [`cache`] - Cache management for project data
//! - [`components`] - UI components (palette, text input, help, etc.)
//! - [`events`] - Event handling and command processing
//! - [`project`] - Project structure and dependency management
//! - [`runner`] - Task execution and update checking
//! - [`ui`] - Terminal UI rendering

pub mod app;
pub mod cache;
pub mod components;
pub mod events;
mod handlers;
pub mod project;
pub mod runner;
pub mod settings;
pub mod ui;

pub use clap::Parser;

/// Command-line arguments for CarWash
///
/// # Fields
///
/// * `target_directory` - The directory to scan for Rust projects (defaults to current directory)
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Target directory to scan for Rust projects
    #[arg(default_value = ".")]
    pub target_directory: String,
}
