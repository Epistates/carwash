# Changelog

All notable changes to CarWash will be documented in this file.

## [0.1.2] - 2025-10-25

### ✨ Features

**Background Update System**
- Automatic background update checking on startup for all projects
- Non-blocking background checks process up to 5 projects concurrently
- Background update queue system with parallelization limiting

**Priority Update Queueing**
- User-initiated update checks (pressing 'u') now jump to the front of the queue
- Ensures responsive behavior when manually requesting updates
- Background checks continue processing in parallel

**UI Refresh After Updates**
- After successful `cargo update`, dependency versions are immediately refreshed
- UI now displays new versions from crates.io without manual rechecking
- Fresh cache-busting check performed after each update completes

### 🎮 Navigation Improvements

**Separated Navigation Contexts**
- **Tab Navigation**: Now uses `Tab` and `Shift+Tab` instead of left/right arrows
  - `Tab`: Move to next output tab (with wraparound)
  - `Shift+Tab`: Move to previous output tab (with wraparound)
- **Workspace Navigation**: Left/right arrows and h/l now dedicated to workspace operations
  - `←` or `h`: Collapse selected workspace
  - `→` or `l`: Expand selected workspace
- Eliminates conflicting dual-purpose keybindings
- Clearer, more intuitive navigation model

### 🐛 Bug Fixes

- Fixed UI not updating with new dependency versions after successful updates
- Resolved navigation conflicts between tab switching and workspace collapse/expand
- Improved queue processing to prevent race conditions in background checks

### 📚 Documentation

- Updated help screen with new `Tab`/`Shift+Tab` tab navigation keys
- Updated keyboard shortcuts to clarify workspace vs tab navigation
- Added documentation for background update queue system

---

## [0.1.1] - 2025-10-25

### 🐛 Bug Fixes

- Fixed relative path resolution for `.` (current directory) argument
  - `carwash` and `carwash .` now work correctly when run from a directory
  - Paths are now properly converted to absolute paths before project scanning
  - Added comprehensive test coverage for relative path handling

---

## [0.1.0] - 2025-10-21

### 🎉 Initial Release - World-Class Edition

#### ✨ Features

**Core Functionality**
- Multi-project Rust workspace management
- Automatic discovery of Cargo projects in directory trees
- Workspace-aware project detection (excludes workspace members)
- Parallel command execution across multiple projects
- Real-time output monitoring with colored syntax highlighting
- Interactive dependency update checker with crates.io integration

**User Interface**
- Beautiful TUI with modern, colorful design
- Three-panel layout: Projects | Dependencies/Output
- Smart project list with checkboxes and status indicators
- Scrollable output tabs with syntax-aware coloring
- Real-time status bar with mode indicators and counters
- Comprehensive help screen with keyboard shortcuts

**Command Palette**
- Fuzzy search filtering for cargo commands
- Vim-style keyboard navigation (j/k for up/down)
- Tab to toggle between "Selected Projects" and "All Projects" scope
- 15+ pre-configured cargo commands
- Custom command support

**Dependency Management**
- Check for outdated dependencies via crates.io API
- Interactive selection with 'a' (select all) and 'n' (select none)
- Visual diff showing current → latest versions
- Color-coded status: up-to-date (green), outdated (yellow)

**Enterprise Features**
- Comprehensive error handling with helpful error messages
- Process isolation with kill_on_drop for clean cancellation
- Execution timing and statistics
- Exit code reporting with success/failure indicators
- Graceful degradation when cargo or network is unavailable

#### 🎨 UI/UX

**Visual Design**
- ☑/☐ Checkboxes for project selection
- ▶ Indicator for current selection
- ⚙ Running command indicator
- ✓ Completion indicators
- ⚠ Warning indicators
- Color-coded output:
  - Red: errors
  - Yellow: warnings
  - Green: success
  - Cyan: info
  - Dark Gray: secondary text

**Keyboard Shortcuts**
- `↑↓` or `j/k`: Navigate
- `←→`: Switch tabs
- `Space`: Toggle selection
- `:`: Command palette
- `u`: Update checker
- `?`: Help
- `q`: Quit
- `Ctrl+C`: Force quit
- `Tab`: Toggle scope (in palette)
- `a`/`n`: Select all/none (in updater)
- `PgUp`/`PgDown`: Scroll output

#### 🏗️ Architecture

**Technology Stack**
- Ratatui 0.29.0 for TUI rendering
- Tokio 1.48.0 for async runtime
- Crossterm 0.28.1 for terminal control
- crates.io API 0.12.0 for dependency checks
- Fuzzy-matcher 0.3.7 for command search
- Cargo-lock 10.1.0 for lockfile parsing

**Code Quality**
- Component-based architecture
- Redux-style state management with reducer pattern
- Clean separation of concerns
- Comprehensive error handling
- Process safety with proper cleanup

#### 📚 Documentation

- Complete README with usage guide
- Inline help screen with all keyboard shortcuts
- Command examples and workflow guides
- Architecture documentation
- Contribution guidelines

#### 🐛 Bug Fixes

- Fixed tab indexing in parallel command execution
- Corrected tree state selection logic
- Fixed workspace member detection
- Improved error handling in crates.io API calls
- Fixed UI rendering race conditions

#### ⚡ Performance

- Parallel project scanning
- Concurrent command execution
- Efficient UI redraws
- Minimal memory footprint
- Fast fuzzy search

---

## Future Roadmap

### Planned for 0.2.0
- [ ] Configuration file support (.carwashrc)
- [ ] Persistent command history
- [ ] Export command outputs to file
- [ ] Custom color themes
- [ ] Project favorites/bookmarks

### Under Consideration
- [ ] Plugin system
- [ ] Remote project support
- [ ] CI/CD integration
- [ ] Git integration
- [ ] Test coverage reporting
- [ ] Performance profiling view





