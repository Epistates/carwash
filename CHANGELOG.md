# Changelog

All notable changes to CarWash will be documented in this file.

## [Unreleased]

## [0.3.2] - 2025-11-14

### New Features

**Disk Space Analysis**
- See `target/` directory sizes for all your projects at a glance
- Color-coded indicators help identify space-hungry projects:
  - Red (>1GB): Time to clean!
  - Yellow (>100MB): Worth reviewing
  - Gray (<100MB): Reasonable size
  - Very dim (0B): Already clean
- Sizes update automatically in the background
- After running `cargo clean`, sizes recalculate instantly so you see the freed space immediately

**Smarter Workspace Handling**
- `crates/` directories are now transparent - workspace members appear directly under the workspace root
- No need to manually expand `crates/` folders anymore
- Cleaner view: `turbovault/crates/turbovault-core` becomes `turbovault/turbovault-core`
- Workspaces with both `[workspace]` and `[package]` sections now display correctly with all members visible

**Better Navigation**
- Focus indicator in status bar shows which pane is active (Projects, Dependencies, or Output)
- Tab key cycles between panes intelligently
- Ctrl+[ and Ctrl+] switch between output tabs from anywhere
- Output pane shows tab counter (e.g., "Output (2/5)")
- Help screen now scrolls properly with arrow keys, j/k, PgUp/PgDn, and Home

### Bug Fixes

- Fixed issue where pressing 'u' to check for updates would check the wrong project
- Fixed dependencies pane not showing when a project was selected
- Fixed workspace members not appearing in the tree view
- Fixed workspaces appearing as single projects instead of showing all members
- Fixed `cargo clean` race conditions when running on multiple workspace members simultaneously
  - Now runs sequentially within each workspace to prevent errors
  - Each project still gets its own output tab for full visibility
  - Standalone projects continue to run in parallel for speed

## [0.3.1] - 2025-11-10

### Hierarchical Tree Navigation

**New Tree-Based UI System**
- Introduced tree data structure with lazy loading support for large project hierarchies
- New interactive tree view component for project navigation
- Integrated hierarchical tree navigation throughout the application
- Added tree state management with selection tracking

**New Components**
- `TreeView` component for collapsible/expandable navigation
- `ProgressBar` component for visual feedback on operations
- `Filter` component for fuzzy search and filtering
- `Modal` component for dialogs and overlays
- Separated UI concerns into modular components

**Code Organization**
- Extracted UI rendering into dedicated `src/ui/` module structure
- Added `src/ui/layout.rs` for layout composition
- Added `src/ui/modal.rs` for modal dialogs
- Added `src/ui/styles.rs` for centralized styling
- Added `src/ui/theme.rs` for theme management
- Created `src/config/` module for configuration management
- Added `src/config/keybinding_config.rs` for keyboard shortcuts
- Added `src/config/theme_config.rs` for theme configuration

**Architecture Improvements**
- Restructured app state to support tree navigation
- Enhanced `Project` struct with additional metadata
- Improved event handling with new action types
- Better separation of concerns between components and state
- More modular and testable code structure

**Development Tools**
- Added `justfile` with convenient development recipes for common tasks
- Makes development workflow faster and more consistent

**Bug Fixes**
- Fixed event stream polling to prevent UI freeze with timeout
- Improved terminal responsiveness during intensive operations
- Better handling of rapid state changes

**Code Quality**
- Applied Clippy recommendations and fixed linting warnings
- Code formatting improvements throughout
- All 34+ tests passing
- Clippy clean with no warnings

**Dependencies**
- Updated to Cargo.toml compatibility
- Maintained all security patches
- No breaking changes to public API

### 🔄 Breaking Changes

None. Fully backward compatible with 0.3.0.

---

## [0.3.0] - 2025-11-07

### 🏗️ Major Refactoring Release

**Code Quality Improvements**
- Extracted helper functions throughout codebase for better modularity
- UI component rendering extracted into focused functions
- Reduced cognitive complexity in main rendering loops
- Improved testability with smaller, single-responsibility functions

**Dependency Pane Refactoring**
- Extracted `create_dependency_list_item()` for dependency rendering
- Extracted `get_title()` for status-aware title generation
- Replaced imperative loops with functional iterators
- Significantly improved readability of dependency display logic

**Project List Refactoring**
- Extracted `create_workspace_header()` for workspace row rendering
- Extracted `create_project_list_item()` for project row rendering
- Reduced main draw loop from 200+ to ~30 lines
- Better separation between data and presentation

**State Management Improvements**
- Refactored AppState to use derived Clone trait
- Implemented Default trait for cleaner initialization
- Optimized `get_visible_projects()` with iterator chains
- Better handling of workspace visibility logic

**Handler Function Improvements**
- Extracted `toggle_workspace_selection()` for workspace selection
- Extracted `toggle_single_project_selection()` for project selection
- Extracted `queue_background_updates_on_enable()` for settings updates
- Extracted `update_project_dependencies()` and `update_wizard_dependencies()`
- Simplified `handle_save_settings()` with cleaner error handling

**Terminal & Error Handling**
- Extracted `setup_terminal()` for initialization
- Extracted `restore_terminal()` for cleanup
- Extracted `reset_checking_status()` to deduplicate status reset
- Better error propagation with anyhow context
- Cleaner early returns with `anyhow::bail!()`

**Async Task Improvements**
- Extracted `check_single_dependency()` for individual checks
- Extracted `spawn_and_stream_command()` for command execution
- Better async task organization and separation
- Improved error handling in command spawning

**Dependencies**
- Updated clap: 4.5.50 → 4.5.51
- Updated chrono: unspecified → 0.4.42
- Updated directories: 5.0 → 6.0.0
- Updated serde_json: unspecified → 1.0.145
- Updated thiserror: 1.0.69 → 2.0.17
- Cleaned up unused transitive dependencies

**Code Quality**
- Applied Rust idioms throughout (derived Default, fixed clippy warnings)
- Improved code organization and maintainability
- Zero behavior changes - purely structural improvements
- All 34 tests passing
- Clippy clean with no warnings

### 🔄 Breaking Changes

None. Fully backward compatible.

---

## [0.2.2] - 2025-10-30

Improve codebase: refactor monolithic files
Improve UX: add settings 

## [0.2.1] - 2025-10-30

Removed Cargo.lock from .gitignore - thanks to [0323pin](https://github.com/0323pin)

## [0.2.0] - 2025-10-27

### 🎯 Major Improvements

**Cache Persistence System**
- Implemented robust cross-platform dependency cache using project directories
- Cache keys based on Cargo.lock hash for automatic invalidation on dependency changes
- Intelligent 5-minute cache duration with proper timestamp preservation
- Separate cache files per project to prevent cross-contamination
- Debug logging system for cache operations (writes to /tmp/carwash-debug.log)

**Background Update Intelligence**
- Failed dependency checks now automatically retry on next startup
- Queue logic checks ALL dependencies, not just first one, before deciding to skip
- Projects with ANY uncached/expired dependencies now auto-queue for background checking
- Conditional status updates: only shows "Checking" when actually performing network requests
- Prevents status flickering when loading from cache

**View Synchronization**
- Fixed race condition where explorer tree showed "checking" while dependency pane showed cached results
- All three views (explorer, wizard, dependency pane) now update simultaneously
- Background check results no longer interfere with wizard display when locked to specific project
- Proper project locking prevents stale data from affecting open wizards

### 🏗️ Architecture Refactoring

**Code Organization**
- Extracted 364-line monolithic reducer into focused handler functions in new `src/handlers.rs`
- 25+ specialized handler functions for clean separation of concerns
- Reducer simplified to pure action dispatch layer
- Improved code maintainability and testability

**Error Handling**
- Replaced generic `Box<dyn Error>` with `anyhow` for better error context in cache operations
- Added comprehensive error messages with file paths and operation context
- Better debugging with detailed error propagation

### Critical Bug Fixes

**Cache Timestamp Issues**
- Fixed cache.rs resetting timestamps to "now" on load (prevented cache expiry logic from working)
- Fixed runner.rs using quit time instead of actual check time when saving cache
- Fixed main.rs saving cache with current time instead of preserving dep.last_checked

**Race Conditions**
- Fixed UpdateDependenciesStreamStart arriving after UpdateDependencies (cache hit scenario)
- Fixed wizard update nested inside conditional that could fail silently
- Fixed background updates overwriting wizard display for locked project
- Fixed user_check_in_progress flag being set too late, causing premature flag clearing

**Queue and Status Management**
- Fixed background queue only checking first dependency instead of all dependencies
- Fixed status always being set to "Checking" even when all deps were cached
- Fixed duplicate cache loading on startup (now loads once correctly)

### UI/UX Improvements

**Update Wizard**
- Wizard now locks to selected project, preventing background updates from changing display
- Shows correct project name in wizard title
- Wizard populates with current dependency data immediately on open
- No more "checking for updates" spinner when data is already cached

**Status Indicators**
- Project status accurately reflects cache state
- No more 20-second "checking" delay when data is already cached
- Background checks for failed dependencies happen invisibly
- Status updates are instant when loading from cache

### 🧪 Testing & Quality

**Development Experience**
- Comprehensive debug logging to /tmp/carwash-debug.log
- Cache operations log project names, dep counts, lock hashes, and success/failure
- Load operations show cache hits/misses and hash mismatches
- All 34 tests passing
- Clippy clean with no warnings

### 📝 Technical Details

**Cache Format**
```json
{
  "lock_file_hash": 12345,
  "dependencies": {
    "serde": {
      "latest_version": "1.0.210",
      "cached_at": <SystemTime>
    }
  }
}
```

**Cache Invalidation**
- Automatic when Cargo.lock changes (hash mismatch)
- Manual via age (> 5 minutes old)
- Failed checks not cached (prevents caching errors)

### 🔄 Breaking Changes

None. Fully backward compatible.

---

## [0.1.2] - 2025-10-25

### Features

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

### Bug Fixes

- Fixed UI not updating with new dependency versions after successful updates
- Resolved navigation conflicts between tab switching and workspace collapse/expand
- Improved queue processing to prevent race conditions in background checks

### 📚 Documentation

- Updated help screen with new `Tab`/`Shift+Tab` tab navigation keys
- Updated keyboard shortcuts to clarify workspace vs tab navigation
- Added documentation for background update queue system

---

## [0.1.1] - 2025-10-25

### Bug Fixes

- Fixed relative path resolution for `.` (current directory) argument
  - `carwash` and `carwash .` now work correctly when run from a directory
  - Paths are now properly converted to absolute paths before project scanning
  - Added comprehensive test coverage for relative path handling

---

## [0.1.0] - 2025-10-21

### 🎉 Initial Release - World-Class Edition

#### Features

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

#### UI/UX

**Visual Design**
- Checkboxes for project selection
- Indicator for current selection
- Running command indicator
- Completion indicators
- Warning indicators
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

#### Bug Fixes

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





