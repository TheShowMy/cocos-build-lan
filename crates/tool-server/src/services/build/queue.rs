use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::time::sleep;
use tracing::{error, info};

use crate::{
    error::AppError,
    models::{BuildStartRequest, PackageTaskRuntime, PackageTaskStatus},
    services::feishu::{
        self, BuildFinishedNotification, FailedTaskNotification, FinishedTaskSummary,
    },
    state::{AppState, RuntimeFlushMode},
};

use super::{
    logging::append_task_logs,
    task::{collect_requested_tasks, handle_task_failure, run_single_task, validate_package_tasks},
};

#[derive(Debug, Clone)]
struct QueueCleanupTarget {
    task_id: String,
    project_id: String,
}

#[derive(Debug, Clone)]
struct QueueTaskSnapshot {
    task_id: String,
    task_name: String,
    project_name: String,
    branch: String,
}

/// 停止请求超过该时长仍未完成，由看门狗强制标记失败，防止界面永远停留在"停止中"。
const CANCEL_STUCK_AFTER_SECS: u64 = 3 * 60;
const CANCEL_WATCHDOG_TICK: Duration = Duration::from_secs(10);

fn is_cancel_stuck(runtime: &PackageTaskRuntime, now_unix_secs: u64) -> bool {
    runtime.status == PackageTaskStatus::Canceling
        && runtime
            .canceling_at_unix_secs
            .is_some_and(|started| now_unix_secs.saturating_sub(started) >= CANCEL_STUCK_AFTER_SECS)
}

async fn watch_cancel_timeouts(state: AppState) {
    loop {
        sleep(CANCEL_WATCHDOG_TICK).await;
        if !state.is_build_in_progress().await {
            break;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        for runtime in state.all_task_runtimes().await {
            if !is_cancel_stuck(&runtime, now) {
                continue;
            }
            error!(task_id = %runtime.task_id, "停止请求超时，强制标记失败");
            let _ = state
                .update_task_runtime(&runtime.task_id, RuntimeFlushMode::Immediate, |runtime| {
                    runtime.status = PackageTaskStatus::Failed;
                    runtime.step_label = "停止超时，已强制结束".to_owned();
                    runtime.last_error = Some(
                        "停止请求超过 3 分钟未完成，已强制标记为失败（子进程可能无法终止）"
                            .to_owned(),
                    );
                    runtime.finished_at = Some(super::logging::now_string());
                })
                .await;
        }
    }
}

pub async fn start_build(state: AppState, request: BuildStartRequest) -> Result<(), AppError> {
    state.try_start_build().await?;
    let tasks = match collect_requested_tasks(&state, &request).await {
        Ok(tasks) => tasks,
        Err(error) => {
            state.finish_build().await;
            return Err(error);
        }
    };
    if let Err(error) = validate_package_tasks(&state, &tasks).await {
        state.finish_build().await;
        return Err(error);
    }
    if let Err(error) = state.prepare_tasks_for_build(&request.task_ids).await {
        state.finish_build().await;
        return Err(error);
    }

    let task_ids = request.task_ids;
    tokio::spawn(async move {
        let _restart_guard = state.restart_guard("打包任务队列");
        let watchdog_state = state.clone();
        tokio::spawn(watch_cancel_timeouts(watchdog_state));
        if let Err(error) = run_build_queue(state.clone(), task_ids).await {
            error!(error = %error, "构建队列执行失败");
        }
        state.finish_build().await;
    });

    Ok(())
}

async fn run_build_queue(state: AppState, task_ids: Vec<String>) -> Result<(), AppError> {
    let queue_started_at = chrono::Local::now();
    let cleanup_targets = collect_cleanup_targets(&state, &task_ids).await;
    let task_snapshots = collect_task_snapshots(&state, &task_ids).await;
    let settings = state.get_settings().await;
    let start_notify_logs = format_notification_logs(
        "整轮开始通知",
        &feishu::send_build_started(
            &settings.feishu_bots,
            &task_snapshots
                .iter()
                .map(|task| task.task_name.clone())
                .collect::<Vec<_>>(),
            queue_started_at,
        )
        .await,
    );
    let mut success_tasks = Vec::new();
    let mut failed_tasks = Vec::new();
    info!(task_count = task_ids.len(), "构建队列开始执行");

    for (index, task_id) in task_ids.iter().enumerate() {
        if cancel_requested(&state).await {
            cancel_remaining_tasks(&state, &task_ids[index..]).await;
            break;
        }
        state.set_active_task(Some(task_id.clone())).await;
        match run_single_task(&state, task_id, &start_notify_logs).await {
            Ok(()) => {
                if let Some(task) = task_snapshots.iter().find(|task| task.task_id == *task_id) {
                    success_tasks.push(FinishedTaskSummary {
                        task_name: task.task_name.clone(),
                        project_name: task.project_name.clone(),
                        error: None,
                    });
                }
            }
            Err(error)
                if matches!(error, AppError::Canceled(_)) || cancel_requested(&state).await =>
            {
                let _ = state
                    .update_task_runtime(task_id, RuntimeFlushMode::Immediate, |runtime| {
                        runtime.status = PackageTaskStatus::Canceled;
                        runtime.step_label = "构建已取消".to_owned();
                        runtime.finished_at = Some(super::logging::now_string());
                    })
                    .await;
                cancel_remaining_tasks(&state, &task_ids[index + 1..]).await;
                break;
            }
            Err(error) => {
                handle_task_failure(&error);
                let updated_runtime = state
                    .update_task_runtime(task_id, RuntimeFlushMode::Immediate, |runtime| {
                        runtime.status = PackageTaskStatus::Failed;
                        if runtime.progress == 0 {
                            runtime.progress = 5;
                        }
                        runtime.last_error = Some(error.to_string());
                        runtime.finished_at = Some(super::logging::now_string());
                    })
                    .await;

                let (task_name, project_name, branch, log_path) = if let Some(snapshot) =
                    task_snapshots.iter().find(|task| task.task_id == *task_id)
                {
                    let log_path = updated_runtime
                        .as_ref()
                        .ok()
                        .and_then(|runtime| runtime.last_log_path.clone());
                    (
                        snapshot.task_name.clone(),
                        snapshot.project_name.clone(),
                        snapshot.branch.clone(),
                        log_path,
                    )
                } else {
                    (
                        task_id.clone(),
                        "未知项目".to_string(),
                        "未知分支".to_string(),
                        None,
                    )
                };

                failed_tasks.push(FinishedTaskSummary {
                    task_name: task_name.clone(),
                    project_name: project_name.clone(),
                    error: Some(error.to_string()),
                });

                let mut fail_logs = vec![format!("任务失败原因: {error}")];
                fail_logs.extend(format_notification_logs(
                    "任务失败通知",
                    &feishu::send_task_failed(
                        &settings.feishu_bots,
                        &FailedTaskNotification {
                            task_name,
                            project_name,
                            branch,
                            failed_at: super::logging::now_string(),
                            error: error.to_string(),
                            log_path,
                        },
                    )
                    .await,
                ));
                let _ = append_task_logs(&state, task_id, &fail_logs).await;
            }
        }
        state.set_active_task(None).await;
    }

    let queue_finished_at = chrono::Local::now();
    let finish_notify_logs = format_notification_logs(
        "整轮完成通知",
        &feishu::send_build_finished(
            &settings.feishu_bots,
            &BuildFinishedNotification {
                started_at: queue_started_at,
                finished_at: queue_finished_at,
                success_tasks: success_tasks.clone(),
                failed_tasks: failed_tasks.clone(),
            },
        )
        .await,
    );
    for task in &task_snapshots {
        let _ = append_task_logs(&state, &task.task_id, &finish_notify_logs).await;
    }

    finalize_main_repo_cleanup(&state, &cleanup_targets).await;
    info!(
        success_count = success_tasks.len(),
        failed_count = failed_tasks.len(),
        "构建队列执行结束"
    );

    Ok(())
}

async fn cancel_requested(state: &AppState) -> bool {
    state
        .cancellation_receiver()
        .await
        .is_some_and(|receiver| *receiver.borrow())
}

async fn cancel_remaining_tasks(state: &AppState, task_ids: &[String]) {
    for task_id in task_ids {
        let _ = state
            .update_task_runtime(task_id, RuntimeFlushMode::Immediate, |runtime| {
                runtime.status = PackageTaskStatus::Canceled;
                runtime.step_label = "队列已取消".to_owned();
                runtime.finished_at = Some(super::logging::now_string());
            })
            .await;
    }
    state.set_active_task(None).await;
}

async fn collect_cleanup_targets(state: &AppState, task_ids: &[String]) -> Vec<QueueCleanupTarget> {
    let mut targets = Vec::new();

    for task_id in task_ids {
        let Ok(task) = state.find_task(task_id).await else {
            continue;
        };
        let Ok((project, _)) = state.resolve_task_context(&task).await else {
            continue;
        };
        targets.push(QueueCleanupTarget {
            task_id: task_id.clone(),
            project_id: project.id,
        });
    }

    targets
}

async fn finalize_main_repo_cleanup(state: &AppState, targets: &[QueueCleanupTarget]) {
    let mut grouped: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for target in targets {
        grouped
            .entry(&target.project_id)
            .or_default()
            .push(&target.task_id);
    }

    for (project_id, task_ids) in grouped {
        let Ok(project) = state.find_project(project_id).await else {
            let message = format!("全部任务结束后清理主工程仓库时跳过：未找到项目 {project_id}");
            for task_id in task_ids {
                let _ = super::logging::append_task_log(state, task_id, &message).await;
            }
            continue;
        };

        let repo_dir = state.workspace_main_repo_dir(&project);
        let message = if !repo_dir.exists() {
            format!(
                "全部任务结束后清理主工程仓库时跳过：仓库不存在 {}",
                repo_dir.display()
            )
        } else {
            match crate::git::cleanup_all_changes(&repo_dir).await {
                Ok(result) => format!(
                    "全部任务结束后已清理主工程仓库: path={}, staged={}, unstaged={}, untracked={}",
                    repo_dir.display(),
                    result.had_staged_changes,
                    result.had_unstaged_changes,
                    result.had_untracked_files
                ),
                Err(error) => format!(
                    "全部任务结束后清理主工程仓库失败: path={}, error={}",
                    repo_dir.display(),
                    error
                ),
            }
        };

        for task_id in task_ids {
            let _ = super::logging::append_task_log(state, task_id, &message).await;
        }
    }
}

async fn collect_task_snapshots(state: &AppState, task_ids: &[String]) -> Vec<QueueTaskSnapshot> {
    let mut snapshots = Vec::new();
    for task_id in task_ids {
        let Ok(task) = state.find_task(task_id).await else {
            continue;
        };
        let Ok((bound_project, group)) = state.resolve_task_context(&task).await else {
            continue;
        };
        snapshots.push(QueueTaskSnapshot {
            task_id: task_id.clone(),
            task_name: task.name,
            project_name: bound_project.name,
            branch: group.branch,
        });
    }
    snapshots
}

fn format_notification_logs(scene: &str, results: &[feishu::NotificationResult]) -> Vec<String> {
    results
        .iter()
        .map(|result| {
            format!(
                "{}: 机器人={}, success={}, detail={}",
                scene, result.bot_name, result.success, result.detail
            )
        })
        .collect()
}
