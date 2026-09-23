//! Task execution and update checking
//!
//! This module handles the execution of cargo commands and dependency update checking.
//! Dependency update checks are user-initiated and scoped to one project at a time.

use crate::app::AppState;
use crate::events::Action;
use crate::project::{Dependency, DependencyCheckStatus, Project};
use crates_io_api::AsyncClient;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command as TokioCommand,
    sync::{Semaphore, mpsc},
};

const PARALLEL_UPDATE_CHECKS: usize = 5;

/// Check for updates on selected project with proper caching
/// This is called when user presses 'u' or opens update wizard
pub async fn check_for_updates(
    state: &AppState,
    tx: mpsc::Sender<Action>,
    dep_cache: Arc<tokio::sync::RwLock<crate::cache::DependencyVersionCache>>,
) {
    // CRITICAL FIX: When called from wizard, use the LOCKED project, not current selection!
    // User might have moved cursor after opening wizard
    let project_to_check = if state.mode == crate::events::Mode::UpdateWizard {
        // Wizard is open - use the locked project
        if let Some(ref locked_id) = state.updater.locked_project_id {
            state.all_projects.iter().find(|p| &p.path == locked_id)
        } else {
            state.get_selected_project()
        }
    } else {
        state.get_selected_project()
    };

    if let Some(project) = project_to_check {
        let deps = project.dependencies.clone();
        let project_id = project.path.clone();

        // Send initial action to show we're checking
        let _ = tx
            .send(Action::UpdateDependenciesStreamStart(project_id.clone()))
            .await;

        check_dependencies_with_cache(
            project_id,
            deps,
            tx,
            true,
            state.config.app.cache_duration(),
            dep_cache,
        )
        .await;
    }
}

/// Check dependencies with intelligent caching and streaming updates
async fn check_single_dependency(
    dep: Dependency,
    client: &AsyncClient,
    tx: &mpsc::Sender<Action>,
    project_id: &Path,
    use_cache: bool,
    cache_duration: std::time::Duration,
    now: SystemTime,
) -> Option<(Dependency, bool)> {
    let mut updated_dep = dep.clone();
    let mut checked_remote = false;
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
                project_id.to_path_buf(),
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
                checked_remote = true;
            }
            _ => {
                updated_dep.check_status = DependencyCheckStatus::NotChecked;
            }
        }

        // Only send update for deps we actually checked (avoid redundant UI updates).
        // Failed checks intentionally remain NotChecked so the project is not shown
        // as up to date from incomplete data.
        let _ = tx
            .send(Action::UpdateSingleDependency(
                project_id.to_path_buf(),
                updated_dep.clone(),
            ))
            .await;
    }
    // Skip sending UpdateSingleDependency for cached deps - they're already up to date

    Some((updated_dep, checked_remote))
}

pub async fn check_dependencies_with_cache(
    project_id: PathBuf,
    deps: Vec<Dependency>,
    tx: mpsc::Sender<Action>,
    use_cache: bool,
    cache_duration: std::time::Duration,
    dep_cache: Arc<tokio::sync::RwLock<crate::cache::DependencyVersionCache>>,
) {
    let mut deps = deps;
    if use_cache {
        let cache = dep_cache.read().await;
        for dep in &mut deps {
            if let Some(entry) = cache.lookup(&dep.name, &dep.current_version, cache_duration) {
                dep.latest_version = Some(entry.latest.clone());
                dep.check_status = DependencyCheckStatus::Checked;
                dep.last_checked = Some(
                    std::time::SystemTime::UNIX_EPOCH
                        + std::time::Duration::from_secs(entry.checked_at),
                );
            }
        }
    }

    let semaphore = Arc::new(Semaphore::new(PARALLEL_UPDATE_CHECKS));
    let client = match AsyncClient::new(
        "carwash/0.1.0 (https://github.com/epistates/carwash)",
        std::time::Duration::from_secs(1),
    ) {
        Ok(client) => client,
        Err(_) => {
            let _ = tx.send(Action::UpdateDependencies(project_id, deps)).await;
            return;
        }
    };
    let now = SystemTime::now();
    let mut tasks = Vec::new();

    for dep in deps {
        let semaphore_clone = semaphore.clone();
        let client_clone = client.clone();
        let tx_clone = tx.clone();
        let project_id_clone = project_id.clone();

        let task = tokio::spawn(async move {
            let _permit = semaphore_clone.acquire().await.ok()?;
            check_single_dependency(
                dep,
                &client_clone,
                &tx_clone,
                &project_id_clone,
                use_cache,
                cache_duration,
                now,
            )
            .await
        });
        tasks.push(task);
    }

    let mut updated_deps = Vec::new();
    let mut remotely_checked_deps = Vec::new();
    for task in tasks {
        if let Ok(Some((dep, checked_remote))) = task.await {
            if checked_remote {
                remotely_checked_deps.push(dep.clone());
            }
            updated_deps.push(dep);
        }
    }

    // Update the global per-dependency version cache
    if !remotely_checked_deps.is_empty() {
        let mut cache = dep_cache.write().await;
        for dep in &remotely_checked_deps {
            if dep.check_status == DependencyCheckStatus::Checked
                && let Some(ref latest) = dep.latest_version
            {
                cache.insert(&dep.name, &dep.current_version, latest.clone());
            }
        }
        let _ = cache.save();
    }

    let _ = tx
        .send(Action::UpdateDependencies(project_id, updated_deps))
        .await;
}

/// Recalculate the size of a single project after a command (e.g., cargo clean)
async fn recalculate_project_size(project: &Project, tx: &mpsc::Sender<Action>) {
    let project_id = project.path.clone();
    let project_path = project.path.clone();
    let workspace_root = project.workspace_root.clone();
    let tx = tx.clone();

    tokio::spawn(async move {
        // Use spawn_blocking for blocking WalkDir I/O to prevent UI lag
        let (total_size, target_size) = tokio::task::spawn_blocking(move || {
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

            (total_size, target_size)
        })
        .await
        .unwrap_or((None, None));

        // Send update back to main thread
        let _ = tx
            .send(Action::UpdateProjectSize(
                project_id,
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
    let args = shlex::split(command_str)
        .unwrap_or_else(|| command_str.split_whitespace().map(String::from).collect());

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

#[derive(Debug, Clone)]
struct CleanCommandTarget {
    tab_index: usize,
    command_project: Project,
    affected_projects: Vec<Project>,
}

fn clean_target_path(project: &Project) -> PathBuf {
    project
        .workspace_root
        .clone()
        .unwrap_or_else(|| project.path.clone())
}

fn clean_target_name(first_project: &Project, target_path: &Path) -> String {
    first_project.workspace_name.clone().unwrap_or_else(|| {
        target_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(first_project.name.as_str())
            .to_string()
    })
}

fn plan_clean_command_targets(
    projects: Vec<Project>,
    start_tab_count: usize,
) -> Vec<CleanCommandTarget> {
    let mut grouped_projects: Vec<(PathBuf, Vec<Project>)> = Vec::new();

    for project in projects {
        let target_path = clean_target_path(&project);
        if let Some((_, existing_projects)) = grouped_projects
            .iter_mut()
            .find(|(path, _)| *path == target_path)
        {
            existing_projects.push(project);
        } else {
            grouped_projects.push((target_path, vec![project]));
        }
    }

    grouped_projects
        .into_iter()
        .enumerate()
        .map(|(offset, (target_path, affected_projects))| {
            let mut command_project = affected_projects[0].clone();
            command_project.name = clean_target_name(&command_project, &target_path);
            command_project.path = target_path;
            command_project.workspace_root = None;

            CleanCommandTarget {
                tab_index: start_tab_count + offset,
                command_project,
                affected_projects,
            }
        })
        .collect()
}

pub async fn run_command(command_str: &str, state: &AppState, tx: mpsc::Sender<Action>) {
    if command_str.is_empty() {
        return;
    }

    let projects_to_run: Vec<Project> = state
        .all_projects
        .iter()
        .filter(|p| state.selected_projects.contains(&p.path))
        .cloned()
        .collect();

    if projects_to_run.is_empty() {
        let tab_index = state.tabs.len();
        let _ = tx
            .send(Action::CreateTab("Error: No projects selected".to_string()))
            .await;
        let _ = tx
            .send(Action::AddOutput(
                tab_index,
                "Please select at least one project to run commands.".to_string(),
            ))
            .await;
        let _ = tx.send(Action::FinishCommand(tab_index)).await;
        return;
    }

    let start_tab_count = state.tabs.len();
    let is_clean_command = shlex::split(command_str)
        .unwrap_or_else(|| command_str.split_whitespace().map(String::from).collect())
        .first()
        .is_some_and(|command| command == "clean");

    if is_clean_command {
        let clean_targets = plan_clean_command_targets(projects_to_run, start_tab_count);

        for target in &clean_targets {
            let tab_title = format!("{}: {}", command_str, target.command_project.name);
            let _ = tx.send(Action::CreateTab(tab_title)).await;
        }

        // `cargo clean` acts on the workspace target directory for workspace members.
        // Run it once per unique target directory, then refresh every affected row.
        for target in clean_targets {
            let tx = tx.clone();
            let command_str = command_str.to_string();

            tokio::spawn(async move {
                if let Err(e) = spawn_and_stream_command(
                    &command_str,
                    &target.command_project,
                    &tx,
                    target.tab_index,
                )
                .await
                {
                    let _ = tx
                        .send(Action::AddOutput(
                            target.tab_index,
                            format!("❌ Error: {}", e),
                        ))
                        .await;
                }

                for project in target.affected_projects {
                    recalculate_project_size(&project, &tx).await;
                }

                let _ = tx.send(Action::FinishCommand(target.tab_index)).await;
            });
        }
    } else {
        let projects_with_tabs: Vec<(usize, Project)> = projects_to_run
            .into_iter()
            .enumerate()
            .map(|(i, project)| (start_tab_count + i, project))
            .collect();

        for (_, project) in &projects_with_tabs {
            let tab_title = format!("{}: {}", command_str, project.name);
            let _ = tx.send(Action::CreateTab(tab_title)).await;
        }

        // Non-clean commands: run in parallel as before
        for (tab_index, project) in projects_with_tabs {
            let tx = tx.clone();
            let command_str = command_str.to_string();

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{GitStatus, ProjectCheckStatus, ProjectStatus};

    fn project(name: &str, path: &str, workspace_root: Option<&str>) -> Project {
        Project {
            name: name.to_string(),
            path: PathBuf::from(path),
            status: ProjectStatus::Pending,
            version: "0.1.0".to_string(),
            authors: vec![],
            dependencies: vec![],
            workspace_root: workspace_root.map(PathBuf::from),
            workspace_name: workspace_root.map(|_| "workspace".to_string()),
            cargo_lock_hash: None,
            check_status: ProjectCheckStatus::Unchecked,
            git_status: GitStatus::Clean,
            total_size: None,
            target_size: None,
        }
    }

    #[test]
    fn clean_targets_dedupe_workspace_members() {
        let member_a = project("member-a", "/repo/member-a", Some("/repo"));
        let member_b = project("member-b", "/repo/member-b", Some("/repo"));

        let targets = plan_clean_command_targets(vec![member_a, member_b], 3);

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].tab_index, 3);
        assert_eq!(targets[0].command_project.path, PathBuf::from("/repo"));
        assert_eq!(targets[0].affected_projects.len(), 2);
    }

    #[test]
    fn clean_targets_keep_standalone_projects_separate() {
        let workspace_member = project("member-a", "/repo/member-a", Some("/repo"));
        let standalone = project("tool", "/tools/tool", None);

        let targets = plan_clean_command_targets(vec![workspace_member, standalone], 10);

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].tab_index, 10);
        assert_eq!(targets[0].command_project.path, PathBuf::from("/repo"));
        assert_eq!(targets[1].tab_index, 11);
        assert_eq!(
            targets[1].command_project.path,
            PathBuf::from("/tools/tool")
        );
    }
}
