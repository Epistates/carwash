//! CarWash - A beautiful TUI for managing multiple Rust projects
//!
//! CarWash provides a terminal user interface for managing Rust projects,
//! checking dependencies, and running cargo commands across multiple projects.

pub mod app;
pub mod components;
pub mod events;
pub mod project;
pub mod runner;
pub mod ui;

pub use clap::Parser;

/// Command-line arguments for CarWash
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Target directory to scan for Rust projects
    #[arg(default_value = ".")]
    pub target_directory: String,
}
