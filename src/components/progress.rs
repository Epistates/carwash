//! Progress visualization and tracking for parallel command execution
//!
//! Provides visual feedback for long-running operations, showing per-project
//! progress and overall completion status.

use crate::app::AppState;
use crate::components::Component;
use crate::events::Action;
use crate::ui::styles::Colors;
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph},
};
use std::time::{Duration, SystemTime};

/// Tracks progress of a single project's command execution
#[derive(Debug, Clone)]
pub struct ProjectProgress {
    /// Project name
    pub name: String,
    /// Current step (e.g., "Fetching", "Building", "Testing")
    pub current_step: String,
    /// Estimated percentage complete (0-100)
    pub progress: u16,
    /// Whether this project has finished
    pub is_finished: bool,
    /// Time when this project started
    pub started_at: SystemTime,
    /// Time when this project finished (if applicable)
    pub finished_at: Option<SystemTime>,
}

impl ProjectProgress {
    /// Create a new project progress tracker
    pub fn new(name: String) -> Self {
        Self {
            name,
            current_step: "Starting...".to_string(),
            progress: 0,
            is_finished: false,
            started_at: SystemTime::now(),
            finished_at: None,
        }
    }

    /// Get elapsed time since start
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed().unwrap_or(Duration::from_secs(0))
    }

    /// Get elapsed time or duration if finished
    pub fn total_duration(&self) -> Duration {
        if let Some(finished_at) = self.finished_at {
            finished_at
                .duration_since(self.started_at)
                .unwrap_or(Duration::from_secs(0))
        } else {
            self.elapsed()
        }
    }

    /// Mark this project as finished
    pub fn finish(&mut self) {
        self.is_finished = true;
        self.progress = 100;
        self.finished_at = Some(SystemTime::now());
        self.current_step = "Complete ✓".to_string();
    }

    /// Update progress step
    pub fn set_step(&mut self, step: String, progress: u16) {
        self.current_step = step;
        self.progress = progress.min(99); // Never show 100% until finished
    }
}

/// Overall progress state for multiple parallel commands
#[derive(Debug, Clone)]
pub struct ProgressState {
    /// Progress for each project
    pub projects: Vec<ProjectProgress>,
    /// Total number of projects to process
    pub total: usize,
    /// Title for the progress display
    pub title: String,
    /// Whether progress tracking is active
    pub is_active: bool,
}

impl ProgressState {
    /// Create a new progress state
    pub fn new(title: String, total: usize) -> Self {
        Self {
            projects: Vec::new(),
            total,
            title,
            is_active: true,
        }
    }

    /// Add a project to track
    pub fn add_project(&mut self, project: ProjectProgress) {
        self.projects.push(project);
    }

    /// Get overall progress percentage
    pub fn overall_progress(&self) -> u16 {
        if self.projects.is_empty() || self.total == 0 {
            return 0;
        }
        let completed = self.projects.iter().filter(|p| p.is_finished).count();
        ((completed as u32 * 100) / self.total as u32) as u16
    }

    /// Get number of completed projects
    pub fn completed_count(&self) -> usize {
        self.projects.iter().filter(|p| p.is_finished).count()
    }

    /// Check if all projects are finished
    pub fn is_complete(&self) -> bool {
        self.projects.len() > 0 && self.completed_count() == self.projects.len()
    }

    /// Clear progress state
    pub fn reset(&mut self) {
        self.projects.clear();
        self.is_active = false;
    }
}

/// Component for displaying progress information
pub struct ProgressComponent;

impl ProgressComponent {
    pub fn new() -> Self {
        Self
    }

    /// Render a progress bar for a single project
    pub fn render_project_progress(colors: Colors, progress: &ProjectProgress) -> Line<'static> {
        let progress_str = format!("[{}%]", progress.progress);
        let step_str = progress.current_step.clone();

        let elapsed_secs = progress.elapsed().as_secs();
        let elapsed_str = if elapsed_secs < 60 {
            format!("{}s", elapsed_secs)
        } else {
            format!("{}m{}s", elapsed_secs / 60, elapsed_secs % 60)
        };

        let name_width = 20;
        let name = if progress.name.len() > name_width {
            format!("{}...", &progress.name[..name_width - 3])
        } else {
            format!("{:<width$}", progress.name, width = name_width)
        };

        let status_color = if progress.is_finished {
            colors.success
        } else if progress.progress > 50 {
            colors.selection
        } else {
            colors.muted
        };

        let status_str = if progress.is_finished { "✓" } else { "⟳" };

        Line::from(vec![
            Span::styled(
                format!("{} ", status_str),
                Style::default().fg(status_color),
            ),
            Span::styled(name, Style::default().fg(colors.text)),
            Span::raw(" "),
            Span::styled(format!("{:<15}", step_str), Style::default().fg(colors.dim)),
            Span::styled(
                format!("{:>5}", progress_str),
                Style::default().fg(status_color),
            ),
            Span::raw(" "),
            Span::styled(elapsed_str, Style::default().fg(colors.muted)),
        ])
    }

    /// Render overall progress bar with gauge
    pub fn render_overall_progress(colors: Colors, state: &ProgressState) -> Paragraph<'static> {
        let overall = state.overall_progress();
        let completed = state.completed_count();

        let _gauge = Gauge::default()
            .block(
                Block::default()
                    .title(format!(" {} ({}/{}) ", state.title, completed, state.total))
                    .borders(Borders::ALL),
            )
            .gauge_style(Style::default().fg(colors.success))
            .percent(overall as u16)
            .label(format!("{}%", overall));

        Paragraph::new(format!("Overall: {}/{} complete", completed, state.total))
            .style(Style::default().fg(colors.text))
    }
}

impl Component for ProgressComponent {
    fn handle_key_events(&mut self, _key: KeyCode, _app: &mut AppState) -> Option<Action> {
        None
    }

    fn draw(&mut self, _f: &mut Frame, _app: &mut AppState, _area: Rect) {
        // Progress is rendered in the output pane, not as a standalone component
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_project_progress_new() {
        let progress = ProjectProgress::new("test_project".to_string());
        assert_eq!(progress.name, "test_project");
        assert_eq!(progress.progress, 0);
        assert!(!progress.is_finished);
    }

    #[test]
    fn test_project_progress_finish() {
        let mut progress = ProjectProgress::new("test".to_string());
        progress.finish();
        assert!(progress.is_finished);
        assert_eq!(progress.progress, 100);
        assert_eq!(progress.current_step, "Complete ✓");
    }

    #[test]
    fn test_project_progress_set_step() {
        let mut progress = ProjectProgress::new("test".to_string());
        progress.set_step("Building".to_string(), 50);
        assert_eq!(progress.current_step, "Building");
        assert_eq!(progress.progress, 50);
    }

    #[test]
    fn test_progress_state_new() {
        let state = ProgressState::new("Test".to_string(), 5);
        assert_eq!(state.total, 5);
        assert!(state.is_active);
    }

    #[test]
    fn test_progress_state_overall_progress() {
        let mut state = ProgressState::new("Test".to_string(), 3);
        let mut p1 = ProjectProgress::new("proj1".to_string());
        p1.finish();
        state.add_project(p1);

        assert_eq!(state.completed_count(), 1);
        assert_eq!(state.overall_progress(), 33); // 1/3 = ~33%
    }

    #[test]
    fn test_progress_state_complete() {
        let mut state = ProgressState::new("Test".to_string(), 2);
        let mut p1 = ProjectProgress::new("proj1".to_string());
        p1.finish();
        let mut p2 = ProjectProgress::new("proj2".to_string());
        p2.finish();
        state.add_project(p1);
        state.add_project(p2);

        assert!(state.is_complete());
    }

    #[test]
    fn test_progress_state_reset() {
        let mut state = ProgressState::new("Test".to_string(), 2);
        state.add_project(ProjectProgress::new("proj1".to_string()));
        assert_eq!(state.projects.len(), 1);

        state.reset();
        assert!(state.projects.is_empty());
        assert!(!state.is_active);
    }

    #[test]
    fn test_elapsed_time() {
        let progress = ProjectProgress::new("test".to_string());
        let elapsed = progress.elapsed();
        assert!(elapsed.as_secs() < 2); // Should be very quick
    }
}
