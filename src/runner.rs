//! Task execution and update checking
//!
//! This module handles the execution of cargo commands and dependency update checking.
//! It manages a queue of tasks that are executed with proper concurrency limits and caching.

use crate::app::AppState;
use crate::cache::UpdateCache;
use crate::events::Action;
use crate::project::{Dependency, DependencyCheckStatus, Project};
use crates_io_api::AsyncClient;
use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command as TokioCommand,
    sync::{Semaphore, mpsc},
};

const PARALLEL_UPDATE_CHECKS: usize = 5;
const CACHE_DURATION_SECS: u64 = 5 * 60; // 5 minutes

/// A task to check for dependency updates on a project
#[derive(Debug, Clone)]
pub struct UpdateCheckTask {
    /// Name of the project to check
    pub project_name: String,
    /// Whether this is a priority task (user-initiated)
    pub is_priority: bool,
}

/// Queue for managing parallel update check tasks
///
/// The queue manages a limited number of concurrent update checks to avoid overwhelming
/// the crates.io API. Priority tasks (user-initiated) are processed before background tasks.
#[derive(Debug, Clone)]
pub struct UpdateQueue {
    /// Queue of pending tasks
    pub queue: VecDeque<UpdateCheckTask>,
    /// Number of tasks currently in progress
    pub in_progress: usize,
}

impl UpdateQueue {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            in_progress: 0,
        }
    }

    pub fn add_task(&mut self, task: UpdateCheckTask) {
        // Check if this project is already in the queue
        let already_queued = self
            .queue
            .iter()
            .any(|t| t.project_name == task.project_name);

        if already_queued {
            // If it's a priority task and the existing one isn't, upgrade it
            if task.is_priority {
                // Remove the existing non-priority task
                self.queue.retain(|t| t.project_name != task.project_name);
                // Add the priority version at the front
                self.queue.push_front(task);
            }
            // Otherwise skip (duplicate)
            return;
        }

        if task.is_priority {
            // Insert at the front for priority tasks
            self.queue.push_front(task);
        } else {
            // Append to the back for background tasks
            self.queue.push_back(task);
        }
    }

    pub fn get_next_task(&mut self) -> Option<UpdateCheckTask> {
        if self.in_progress < PARALLEL_UPDATE_CHECKS {
            self.in_progress += 1;
            self.queue.pop_front()
        } else {
            None
        }
    }

    pub fn task_completed(&mut self) {
        if self.in_progress > 0 {
            self.in_progress -= 1;
        }
    }

    pub fn has_pending_tasks(&self) -> bool {
        !self.queue.is_empty() || self.in_progress > 0
    }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.in_progress = 0;
    }
}

/// Check for updates on selected project with proper caching
/// This is called when user presses 'u' or opens update wizard
pub async fn check_for_updates(state: &AppState<'_>, tx: mpsc::Sender<Action>) {
    // CRITICAL FIX: When called from wizard, use the LOCKED project, not current selection!
    // User might have moved cursor after opening wizard
    let project_to_check = if state.mode == crate::events::Mode::UpdateWizard {
        // Wizard is open - use the locked project
        if let Some(ref locked_name) = state.updater.locked_project_name {
            state.all_projects.iter().find(|p| &p.name == locked_name)
        } else {
            state.get_selected_project()
        }
    } else {
        state.get_selected_project()
    };

    if let Some(project) = project_to_check {
        let deps = project.dependencies.clone();
        let project_name = project.name.clone();
        let project_path = project.path.clone();

        // Send initial action to show we're checking
        let _ = tx
            .send(Action::UpdateDependenciesStreamStart(project_name.clone()))
            .await;

        // Perform checks asynchronously - don't await here
        check_dependencies_with_cache(project_name, deps, tx, true, Some(project_path)).await;
    }
}

/// Check dependencies with intelligent caching and streaming updates
pub async fn check_dependencies_with_cache(
    project_name: String,
    deps: Vec<Dependency>,
    tx: mpsc::Sender<Action>,
    use_cache: bool,
    project_path: Option<std::path::PathBuf>,
) {
    // DEBUG: Log function entry
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/carwash-debug.log")
    {
        let _ = writeln!(
            f,
            "[CHECK START] Project {} with {} deps (use_cache={})",
            project_name,
            deps.len(),
            use_cache
        );
    }

    let cache = UpdateCache::new();
    let semaphore = Arc::new(Semaphore::new(PARALLEL_UPDATE_CHECKS));
    let client_result = AsyncClient::new(
        "carwash/0.1.0 (https://github.com/epistates/carwash)",
        std::time::Duration::from_secs(1),
    );

    let client = match client_result {
        Ok(client) => client,
        Err(e) => {
            // DEBUG: Log client failure
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/carwash-debug.log")
            {
                let _ = writeln!(
                    f,
                    "[CHECK ERROR] Project {}: Failed to create API client: {:?}",
                    project_name, e
                );
            }
            // Client failed - send deps with no updates
            let _ = tx
                .send(Action::UpdateDependencies(project_name, deps))
                .await;
            return;
        }
    };
    let now = SystemTime::now();
    let cache_duration = std::time::Duration::from_secs(CACHE_DURATION_SECS);
    let mut tasks = Vec::new();

    for dep in deps {
        let semaphore_clone = semaphore.clone();
        let client_clone = client.clone();
        let tx_clone = tx.clone();
        let project_name_clone = project_name.clone();

        let task = tokio::spawn(async move {
            // Acquire semaphore permit (limits concurrent requests)
            let _permit = semaphore_clone.acquire().await.ok()?;

            let mut updated_dep = dep.clone();
            let should_check = if use_cache {
                if let Some(last_checked) = updated_dep.last_checked {
                    if let Ok(elapsed) = now.duration_since(last_checked) {
                        elapsed > cache_duration
                    } else {
                        true
                    }
                } else {
                    true
                }
            } else {
                true
            };

            if should_check {
                updated_dep.check_status = DependencyCheckStatus::Checking;

                // Send UI update showing this dep is being checked
                let _ = tx_clone
                    .send(Action::UpdateDependencyCheckStatus(
                        updated_dep.name.clone(),
                        DependencyCheckStatus::Checking,
                    ))
                    .await;

                // Perform the check with timeout
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    client_clone.get_crate(&updated_dep.name),
                )
                .await
                {
                    Ok(Ok(crate_info)) => {
                        updated_dep.latest_version = Some(crate_info.crate_data.max_version);
                        updated_dep.check_status = DependencyCheckStatus::Checked;
                        updated_dep.last_checked = Some(SystemTime::now());
                    }
                    _ => {
                        updated_dep.check_status = DependencyCheckStatus::Checked;
                        updated_dep.last_checked = Some(SystemTime::now());
                    }
                }
            } else {
                // Using cached data
                updated_dep.check_status = DependencyCheckStatus::Checked;
            }

            // Send individual update for UI streaming
            let _ = tx_clone
                .send(Action::UpdateSingleDependency(
                    project_name_clone,
                    updated_dep.clone(),
                ))
                .await;

            Some(updated_dep)
        });
        tasks.push(task);
    }

    // Collect all results but don't block UI
    let mut updated_deps = Vec::new();
    for task in tasks {
        if let Ok(Some(dep)) = task.await {
            updated_deps.push(dep);
        }
    }

    // Save cache if we have a project path
    if let Some(path) = project_path {
        if let Some(lock_hash) = UpdateCache::hash_cargo_lock(&path.join("Cargo.lock")) {
            let mut cached_deps = std::collections::HashMap::new();
            let mut success_count = 0;
            let mut failed_count = 0;

            for dep in &updated_deps {
                // CRITICAL: Only cache dependencies that were successfully checked
                // If latest_version is None (network error, timeout, etc.), don't cache it
                if dep.latest_version.is_some() {
                    cached_deps.insert(
                        dep.name.clone(),
                        crate::cache::CachedDependency {
                            latest_version: dep.latest_version.clone(),
                            // CRITICAL: Use the actual check time, not current time!
                            cached_at: dep.last_checked.unwrap_or_else(SystemTime::now),
                        },
                    );
                    success_count += 1;
                } else {
                    failed_count += 1;
                }
            }

            // Debug logging
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/carwash-debug.log")
            {
                let _ = writeln!(
                    f,
                    "[RUNNER] Project {}: {} succeeded, {} failed, lock_hash={:x}",
                    project_name, success_count, failed_count, lock_hash
                );
            }

            // Only save if we have checked dependencies
            if !cached_deps.is_empty() {
                match cache.save(&path, lock_hash, cached_deps.clone()) {
                    Ok(()) => {
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open("/tmp/carwash-debug.log")
                        {
                            let _ = writeln!(
                                f,
                                "[RUNNER] ✓ Saved cache for {} ({} deps)",
                                project_name,
                                cached_deps.len()
                            );
                        }
                    }
                    Err(e) => {
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open("/tmp/carwash-debug.log")
                        {
                            let _ = writeln!(
                                f,
                                "[RUNNER] ✗ Failed to save cache for {}: {}",
                                project_name, e
                            );
                        }
                    }
                }
            } else if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/carwash-debug.log")
            {
                let _ = writeln!(
                    f,
                    "[RUNNER] ⊘ Skipped {} (no successful checks)",
                    project_name
                );
            }
        }
    }

    // Send final batch update
    let _ = tx
        .send(Action::UpdateDependencies(project_name, updated_deps))
        .await;
}

pub async fn run_command(
    command_str: &str,
    state: &AppState<'_>,
    tx: mpsc::Sender<Action>,
    on_all: bool,
) {
    if command_str.is_empty() {
        return;
    }

    let projects_to_run: Vec<Project> = if on_all {
        state.projects.clone()
    } else {
        state
            .projects
            .iter()
            .filter(|p| state.selected_projects.contains(&p.name))
            .cloned()
            .collect()
    };

    if projects_to_run.is_empty() {
        let _ = tx
            .send(Action::CreateTab("Error: No projects selected".to_string()))
            .await;
        let _ = tx
            .send(Action::AddOutput(
                0,
                "Please select at least one project to run commands.".to_string(),
            ))
            .await;
        let _ = tx.send(Action::FinishCommand(0)).await;
        return;
    }

    // Get the starting tab index before spawning tasks
    let start_tab_count = state.tabs.len();

    for (i, project) in projects_to_run.into_iter().enumerate() {
        let tx = tx.clone();
        let command_str = command_str.to_string();
        let tab_title = format!("{}: {}", command_str, project.name);
        let tab_index = start_tab_count + i;

        // Create tab first
        let _ = tx.send(Action::CreateTab(tab_title)).await;

        tokio::spawn(async move {
            let start_time = std::time::Instant::now();
            let args: Vec<&str> = command_str.split_whitespace().collect();

            if args.is_empty() {
                let _ = tx
                    .send(Action::AddOutput(
                        tab_index,
                        "Error: Empty command".to_string(),
                    ))
                    .await;
                let _ = tx.send(Action::FinishCommand(tab_index)).await;
                return;
            }

            // Log command start
            let _ = tx
                .send(Action::AddOutput(
                    tab_index,
                    format!("$ cargo {} (in {})", command_str, project.path.display()),
                ))
                .await;
            let _ = tx.send(Action::AddOutput(tab_index, "".to_string())).await;

            let mut cmd = TokioCommand::new("cargo");
            cmd.args(&args)
                .current_dir(&project.path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);

            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(e) => {
                    let _ = tx
                        .send(Action::AddOutput(
                            tab_index,
                            format!("❌ Error: Failed to spawn cargo process: {}", e),
                        ))
                        .await;
                    let _ = tx.send(Action::AddOutput(tab_index, "".to_string())).await;
                    let _ = tx
                        .send(Action::AddOutput(tab_index, "Possible causes:".to_string()))
                        .await;
                    let _ = tx
                        .send(Action::AddOutput(
                            tab_index,
                            "  - cargo is not installed or not in PATH".to_string(),
                        ))
                        .await;
                    let _ = tx
                        .send(Action::AddOutput(
                            tab_index,
                            "  - Project directory is invalid".to_string(),
                        ))
                        .await;
                    let _ = tx.send(Action::FinishCommand(tab_index)).await;
                    return;
                }
            };

            let stdout = child.stdout.take().expect("stdout was configured as piped");
            let stderr = child.stderr.take().expect("stderr was configured as piped");

            let mut stdout_reader = BufReader::new(stdout).lines();
            let mut stderr_reader = BufReader::new(stderr).lines();

            let mut line_count = 0;
            loop {
                tokio::select! {
                    Ok(Some(line)) = stdout_reader.next_line() => {
                        let _ = tx.send(Action::AddOutput(tab_index, line)).await;
                        line_count += 1;
                    }
                    Ok(Some(line)) = stderr_reader.next_line() => {
                        let _ = tx.send(Action::AddOutput(tab_index, line)).await;
                        line_count += 1;
                    }
                    else => break,
                }
            }

            let status = match child.wait().await {
                Ok(status) => status,
                Err(e) => {
                    let _ = tx
                        .send(Action::AddOutput(
                            tab_index,
                            format!("❌ Error: Failed to wait for process: {}", e),
                        ))
                        .await;
                    let _ = tx.send(Action::FinishCommand(tab_index)).await;
                    return;
                }
            };

            let elapsed = start_time.elapsed();
            let _ = tx.send(Action::AddOutput(tab_index, "".to_string())).await;

            if status.success() {
                let _ = tx
                    .send(Action::AddOutput(
                        tab_index,
                        format!(
                            "✓ Finished successfully in {:.2}s ({} lines)",
                            elapsed.as_secs_f64(),
                            line_count
                        ),
                    ))
                    .await;
            } else {
                let code = status.code().unwrap_or(-1);
                let _ = tx
                    .send(Action::AddOutput(
                        tab_index,
                        format!(
                            "❌ Failed with exit code {} after {:.2}s",
                            code,
                            elapsed.as_secs_f64()
                        ),
                    ))
                    .await;
            }

            let _ = tx.send(Action::FinishCommand(tab_index)).await;
        });
    }
}
