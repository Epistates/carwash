# CarWash - Rust Project Manager

A TUI (Terminal User Interface) for managing multiple Rust projects with ease. CarWash provides an intuitive interface for running cargo commands across multiple projects simultaneously, managing dependencies, and monitoring build outputs.

[![Crates.io](https://img.shields.io/crates/v/carwash.svg)](https://crates.io/crates/carwash)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

<img src="assets/carwash.png" alt="Carwash" style="width: 100%; max-width: 100%; margin: 20px 0;"/>

## Features

### Core Features
- **Multi-Project Management**: Automatically discover and manage all Rust projects in a directory tree
- **Parallel Command Execution**: Run cargo commands across multiple projects simultaneously
- **Interactive Command Palette**: Fuzzy search through cargo commands with vim-style navigation
- **Real-time Output Monitoring**: Watch command output with colored syntax highlighting
- **Dependency Management**: Check for outdated dependencies and update them interactively
- **Workspace Support**: Intelligently handles Cargo workspaces and member projects

### Enterprise-Grade UX
- **Beautiful UI**: Modern, colorful interface with intuitive layouts
- **Status Indicators**: Real-time status updates with progress indicators
- **Smart Color Coding**: Errors (red), warnings (yellow), success (green)
- **Scrollable Output**: Navigate through large command outputs with keyboard shortcuts
- **Multiple Tabs**: Switch between running commands with arrow keys
- **Project Selection**: Select single or multiple projects with visual checkboxes

### Keyboard-Driven Workflow
- **Vim-style Navigation**: `j/k` for up/down, `h/l` for left/right
- **Quick Commands**: Press `:` to open command palette instantly
- **Space to Select**: Toggle project selection with spacebar
- **Esc to Cancel**: Consistent escape behavior throughout
- **Tab to Toggle**: Switch command scope between selected and all projects

## Installation

### Prerequisites
- **Rust 1.93.0+** (Rust Edition 2024)

### From crates.io (Recommended)

```bash
cargo install carwash
```

See the package on [crates.io](https://crates.io/crates/carwash).

### From source

```bash
# Clone the repository
git clone https://github.com/epistates/carwash.git
cd carwash

# Build and install
cargo install --path .

# Or run directly
cargo run -- /path/to/your/rust/projects
```

## Quick Start

```bash
# Run in current directory
carwash

# Run in specific directory
carwash ~/my-rust-projects

# Show help
carwash --help
```

## Usage Guide

### Navigation

| Key | Action |
|-----|--------|
| `↑`/`↓` or `j`/`k` | Navigate projects |
| `←`/`→` | Switch output tabs |
| `Space` | Toggle project selection |
| `PgUp`/`PgDown` | Scroll output |

### Commands

| Key | Action |
|-----|--------|
| `:` | Open command palette |
| `u` | Check for dependency updates |
| `?` | Show help screen |
| `q` | Quit application |
| `Ctrl+C` | Force quit |

### Command Palette

1. Press `:` to open the command palette
2. Type to filter commands (fuzzy search)
3. Use `↑`/`↓` or `j`/`k` to select a command
4. Press `Tab` to toggle between **Selected Projects** and **All Projects**
5. Press `Enter` to execute the command
6. Press `Esc` to cancel

## Architecture

CarWash is built with:

- **[Ratatui](https://github.com/ratatui-org/ratatui)**: Modern TUI framework for Rust
- **[Tokio](https://tokio.rs/)**: Async runtime for parallel command execution
- **[crates.io API](https://crates.io/)**: For checking dependency versions
- **Component-Based UI**: Modular, maintainable UI components
- **Redux-style State Management**: Predictable state updates with reducer pattern

### Project Structure

```
src/
├── main.rs              # Entry point and main event loop
├── app.rs               # Application state and reducer
├── handlers.rs          # State transition logic
├── project.rs           # Project discovery and parsing
├── runner.rs            # Command execution and updates
├── tree.rs              # Hierarchical navigation system
├── cache.rs             # Dependency check caching
├── settings.rs          # User preferences
├── config/              # Unified configuration system
│   ├── keybinding_config.rs
│   └── theme_config.rs
├── components/          # UI Components
│   ├── dependencies.rs
│   ├── output.rs
│   ├── projects.rs
│   └── ...
└── ui/                  # Visual orchestration
    ├── layout.rs
    ├── modal.rs
    └── theme.rs
```

## Customization

CarWash uses a carefully chosen color scheme optimized for terminal visibility:

- **Cyan**: Borders, titles, and interactive elements
- **Green**: Success states and selected items
- **Yellow**: Warnings and pending states
- **Red**: Errors and critical information
- **Magenta**: Special modes like update wizard
- **Dark Gray**: Secondary information and help text

## Contributing

Contributions are welcome! Areas for improvement:

- [ ] Custom command templates
- [ ] Project favorites/bookmarks
- [ ] Command history persistence
- [ ] Export/save command outputs
- [ ] Plugin system
- [ ] Remote project support

## 📝 License

[MIT](./LICENSE)

## Acknowledgments

Built with:
- [Ratatui](https://github.com/ratatui-org/ratatui) - Amazing TUI framework
- [Tokio](https://tokio.rs/) - Powerful async runtime
- [crates.io API](https://github.com/crates-io/crates.io) - Crate metadata

---

**Made with ❤️ for the Rust community**
