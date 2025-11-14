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
        if self.in_progress >= PARALLEL_UPDATE_CHECKS {
            return None;
        }

        if let Some(task) = self.queue.pop_front() {
            self.in_progress += 1;
            Some(task)
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
pub async fn check_for_updates(state: &AppState, tx: mpsc::Sender<Action>) {
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
        check_dependencies_with_cache(
            project_name,
            deps,
            tx,
            true,
            Some(project_path),
            state.settings.cache_duration(),
        )
        .await;
    }
}

/// Check dependencies with intelligent caching and streaming updates
async fn check_single_dependency(
    dep: Dependency,
    client: &AsyncClient,
    tx: &mpsc::Sender<Action>,
    project_name: &str,
    use_cache: bool,
    cache_duration: std::time::Duration,
    now: SystemTime,
) -> Option<Dependency> {
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

        let _ = tx
            .send(Action::UpdateDependencyCheckStatus(
                project_name.to_string(),
                updated_dep.name.clone(),
                DependencyCheckStatus::Checking,
            ))
            .await;

        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.get_crate(&updated_dep.name),
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
        updated_dep.check_status = DependencyCheckStatus::Checked;
    }

    let _ = tx
        .send(Action::UpdateSingleDependency(
            project_name.to_string(),
            updated_dep.clone(),
        ))
        .await;

    Some(updated_dep)
}

pub async fn check_dependencies_with_cache(
    project_name: String,
    deps: Vec<Dependency>,
    tx: mpsc::Sender<Action>,
    use_cache: bool,
    project_path: Option<std::path::PathBuf>,
    cache_duration: std::time::Duration,
) {
    let cache = UpdateCache::new();
    let semaphore = Arc::new(Semaphore::new(PARALLEL_UPDATE_CHECKS));
    let client = match AsyncClient::new(
        "carwash/0.1.0 (https://github.com/epistates/carwash)",
        std::time::Duration::from_secs(1),
    ) {
        Ok(client) => client,
        Err(_) => {
            let _ = tx
                .send(Action::UpdateDependencies(project_name, deps))
                .await;
            return;
        }
    };
    let now = SystemTime::now();
    let mut tasks = Vec::new();

    for dep in deps {
        let semaphore_clone = semaphore.clone();
        let client_clone = client.clone();
        let tx_clone = tx.clone();
        let project_name_clone = project_name.clone();

        let task = tokio::spawn(async move {
            let _permit = semaphore_clone.acquire().await.ok()?;
            check_single_dependency(
                dep,
                &client_clone,
                &tx_clone,
                &project_name_clone,
                use_cache,
                cache_duration,
                now,
            )
            .await
        });
        tasks.push(task);
    }

    let mut updated_deps = Vec::new();
    for task in tasks {
        if let Ok(Some(dep)) = task.await {
            updated_deps.push(dep);
        }
    }

    if let Some(path) = project_path {
        if let Some(lock_hash) = UpdateCache::hash_cargo_lock(&path.join("Cargo.lock")) {
            let mut cached_deps = std::collections::HashMap::new();
            for dep in &updated_deps {
                if dep.latest_version.is_some() {
                    cached_deps.insert(
                        dep.name.clone(),
                        crate::cache::CachedDependency {
                            latest_version: dep.latest_version.clone(),
                            cached_at: dep.last_checked.unwrap_or_else(SystemTime::now),
                        },
                    );
                }
            }

            if !cached_deps.is_empty() {
                let _ = cache.save(&path, lock_hash, cached_deps.clone());
            }
        }
    }

    let _ = tx
        .send(Action::UpdateDependencies(project_name, updated_deps))
        .await;
}

/// Recalculate the size of a single project after a command (e.g., cargo clean)
async fn recalculate_project_size(project: &Project, tx: &mpsc::Sender<Action>) {
    let project_name = project.name.clone();
    let project_path = project.path.clone();
    let workspace_root = project.workspace_root.clone();
    let tx = tx.clone();

    tokio::spawn(async move {
        let total_size = crate::project::calculate_directory_size(&project_path);

        // For workspace members, look for target/ at workspace root
        // For standalone projects, look in the project directory
        let target_size = {
            let target_path = if let Some(ws_root) = workspace_root {
                ws_root.join("target")
            } else {
                project_path.join("target")
            };

            if target_path.exists() && target_path.is_dir() {
                crate::project::calculate_directory_size(&target_path)
            } else {
                Some(0)
            }
        };

        // Send update back to main thread
        let _ = tx
            .send(Action::UpdateProjectSize(
                project_name,
                total_size,
                target_size,
            ))
            .await;
    });
}

async fn spawn_and_stream_command(
    command_str: &str,
    project: &Project,
    tx: &mpsc::Sender<Action>,
    tab_index: usize,
) -> anyhow::Result<()> {
    let start_time = std::time::Instant::now();
    let args: Vec<&str> = command_str.split_whitespace().collect();

    if args.is_empty() {
        anyhow::bail!("Empty command");
    }

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

    let mut child = cmd.spawn()?;

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

    let status = child.wait().await?;
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

        // Note: Size recalculation for clean commands is handled in run_command()
        // with sequential execution for workspace members to avoid race conditions
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

    Ok(())
}

pub async fn run_command(command_str: &str, state: &AppState, tx: mpsc::Sender<Action>) {
    if command_str.is_empty() {
        return;
    }

    let projects_to_run: Vec<Project> = state
        .projects
        .iter()
        .filter(|p| state.selected_projects.contains(&p.name))
        .cloned()
        .collect();

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

    let start_tab_count = state.tabs.len();
    let is_clean_command = command_str.contains("clean");

    // For clean commands on workspace members, we need to run sequentially to avoid race conditions
    // Group projects by workspace root
    if is_clean_command {
        use std::collections::HashMap;
        use std::path::PathBuf;

        let mut workspace_groups: HashMap<Option<PathBuf>, Vec<(usize, Project)>> = HashMap::new();
        for (i, project) in projects_to_run.into_iter().enumerate() {
            workspace_groups
                .entry(project.workspace_root.clone())
                .or_insert_with(Vec::new)
                .push((i, project));
        }

        // Process each workspace group
        for (_ws_root, group) in workspace_groups {
            let has_shared_target = group.len() > 1 && group[0].1.workspace_root.is_some();

            for (i, project) in group {
                let tx = tx.clone();
                let command_str = command_str.to_string();
                let tab_title = format!("{}: {}", command_str, project.name);
                let tab_index = start_tab_count + i;

                let _ = tx.send(Action::CreateTab(tab_title)).await;

                if has_shared_target {
                    // Run sequentially for workspace members sharing a target
                    if let Err(e) =
                        spawn_and_stream_command(&command_str, &project, &tx, tab_index).await
                    {
                        let _ = tx
                            .send(Action::AddOutput(tab_index, format!("❌ Error: {}", e)))
                            .await;
                    }
                    recalculate_project_size(&project, &tx).await;
                    let _ = tx.send(Action::FinishCommand(tab_index)).await;
                } else {
                    // Run in parallel for standalone projects
                    tokio::spawn(async move {
                        if let Err(e) =
                            spawn_and_stream_command(&command_str, &project, &tx, tab_index).await
                        {
                            let _ = tx
                                .send(Action::AddOutput(tab_index, format!("❌ Error: {}", e)))
                                .await;
                        }
                        recalculate_project_size(&project, &tx).await;
                        let _ = tx.send(Action::FinishCommand(tab_index)).await;
                    });
                }
            }
        }
    } else {
        // Non-clean commands: run in parallel as before
        for (i, project) in projects_to_run.into_iter().enumerate() {
            let tx = tx.clone();
            let command_str = command_str.to_string();
            let tab_title = format!("{}: {}", command_str, project.name);
            let tab_index = start_tab_count + i;

            let _ = tx.send(Action::CreateTab(tab_title)).await;

            tokio::spawn(async move {
                if let Err(e) =
                    spawn_and_stream_command(&command_str, &project, &tx, tab_index).await
                {
                    let _ = tx
                        .send(Action::AddOutput(tab_index, format!("❌ Error: {}", e)))
                        .await;
                }
                let _ = tx.send(Action::FinishCommand(tab_index)).await;
            });
        }
    }
}
