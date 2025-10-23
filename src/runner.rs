use crate::app::AppState;
use crate::events::Action;
use crate::project::{DependencyCheckStatus, Project};
use crates_io_api::AsyncClient;
use std::process::Stdio;
use std::time::SystemTime;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command as TokioCommand,
    sync::mpsc,
};

pub async fn check_for_updates(state: &AppState<'_>, tx: mpsc::Sender<Action>, use_cache: bool) {
    if let Some(project) = state.get_selected_project() {
        let deps = project.dependencies.clone();
        let tx_clone = tx.clone();
        let cache_duration = std::time::Duration::from_secs(5 * 60); // 5 minutes

        tokio::spawn(async move {
            let deps_fallback = deps.clone();

            // Add timeout to prevent hanging indefinitely
            let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                let client_result = AsyncClient::new(
                    "carwash/0.1.0 (https://github.com/epistates/carwash)",
                    std::time::Duration::from_secs(1),
                );

                match client_result {
                    Ok(client) => {
                        // Check dependencies in parallel for better performance
                        let mut tasks = Vec::new();
                        let now = SystemTime::now();

                        for dep in deps {
                            // Check cache if enabled
                            let should_check = if use_cache {
                                if let Some(last_checked) = dep.last_checked {
                                    if let Ok(elapsed) = now.duration_since(last_checked) {
                                        elapsed > cache_duration
                                    } else {
                                        true // If time comparison fails, check anyway
                                    }
                                } else {
                                    true // Never checked, so check now
                                }
                            } else {
                                true // No cache, always check
                            };

                            let client_clone = client.clone();
                            let task = tokio::spawn(async move {
                                let mut updated_dep = dep.clone();

                                if should_check {
                                    updated_dep.check_status = DependencyCheckStatus::Checking;

                                    // Add per-crate timeout
                                    match tokio::time::timeout(
                                        std::time::Duration::from_secs(5),
                                        client_clone.get_crate(&updated_dep.name),
                                    )
                                    .await
                                    {
                                        Ok(Ok(crate_info)) => {
                                            updated_dep.latest_version =
                                                Some(crate_info.crate_data.max_version);
                                            updated_dep.check_status =
                                                DependencyCheckStatus::Checked;
                                            updated_dep.last_checked = Some(SystemTime::now());
                                        }
                                        _ => {
                                            // Timeout or error - mark as checked but no latest version
                                            updated_dep.check_status =
                                                DependencyCheckStatus::Checked;
                                            updated_dep.last_checked = Some(SystemTime::now());
                                        }
                                    }
                                } else {
                                    // Using cached data
                                    updated_dep.check_status = DependencyCheckStatus::Checked;
                                }

                                updated_dep
                            });
                            tasks.push(task);
                        }

                        // Wait for all tasks to complete
                        let mut updated_deps = Vec::new();
                        for task in tasks {
                            if let Ok(dep) = task.await {
                                updated_deps.push(dep);
                            }
                        }
                        updated_deps
                    }
                    Err(_) => {
                        // Client creation error - return deps unchanged
                        deps_fallback.clone()
                    }
                }
            })
            .await;

            // Send result back regardless of success or timeout
            let final_deps = result.unwrap_or(deps_fallback);
            let _ = tx_clone.send(Action::UpdateDependencies(final_deps)).await;
        });
    } else {
        // No project selected - send action to clear loading state immediately
        let _ = tx.send(Action::UpdateDependencies(Vec::new())).await;
    }
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
                .kill_on_drop(true); // Clean up if the task is cancelled

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

            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();

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
