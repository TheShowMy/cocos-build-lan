use tokio::fs::{self, File};
use tracing::{error, info};

use crate::{
    error::AppError,
    git,
    models::{PackageTaskStatus, TaskPrivateReposCleanupResponse},
    services::build::{
        ObfuscationExecutionContext, append_task_log, now_string, run_task_obfuscation,
        write_log_line,
    },
    state::{AppState, RuntimeFlushMode},
};

pub async fn delete_task(state: &AppState, task_id: &str) -> Result<(), AppError> {
    let task = state.find_task(task_id).await?;
    let runtime = state.get_task_runtime(task_id).await?;
    if runtime.status == PackageTaskStatus::Running
        || runtime.status == PackageTaskStatus::Canceling
    {
        return Err(AppError::conflict("任务正在执行中，无法删除"));
    }

    if let Ok((project, _)) = state.resolve_task_context(&task).await {
        let task_dir = state
            .workspace_project_dir(&project)
            .join("tasks")
            .join(task_id);
        if task_dir.exists() {
            fs::remove_dir_all(&task_dir)
                .await
                .map_err(|error| AppError::internal(format!("删除任务目录失败: {error}")))?;
        }
    }

    let mut settings = state.get_settings().await;
    settings.package_tasks.retain(|item| item.id != task_id);
    state.save_settings(settings).await?;
    Ok(())
}

pub async fn cleanup_task_private_repos(
    state: &AppState,
    task_id: &str,
) -> Result<TaskPrivateReposCleanupResponse, AppError> {
    let task = state.find_task(task_id).await?;
    let runtime = state.get_task_runtime(task_id).await?;
    if runtime.status == PackageTaskStatus::Running
        || runtime.status == PackageTaskStatus::Canceling
    {
        return Err(AppError::conflict("任务正在执行中，无法清理私有仓库"));
    }

    let (project, _) = state.resolve_task_context(&task).await?;
    let task_dir = state
        .workspace_project_dir(&project)
        .join("tasks")
        .join(task_id);
    let code_repo_dir = task_dir.join("code-repo");
    let asset_repo_dir = task_dir.join("asset-repo");

    let code_repo_git_repo = code_repo_dir.join(".git").exists();
    let asset_repo_git_repo = asset_repo_dir.join(".git").exists();

    let code_repo_cleanup = if code_repo_git_repo {
        Some(git::cleanup_all_changes(&code_repo_dir).await?)
    } else {
        None
    };
    let asset_repo_cleanup = if asset_repo_git_repo {
        Some(git::cleanup_all_changes(&asset_repo_dir).await?)
    } else {
        None
    };

    Ok(TaskPrivateReposCleanupResponse {
        task_id: task.id,
        task_name: task.name,
        code_repo_path: code_repo_dir.to_string_lossy().to_string(),
        asset_repo_path: asset_repo_dir.to_string_lossy().to_string(),
        code_repo_git_repo,
        asset_repo_git_repo,
        code_repo_cleaned: code_repo_cleanup.is_some(),
        asset_repo_cleaned: asset_repo_cleanup.is_some(),
        code_repo_had_staged_changes: code_repo_cleanup
            .as_ref()
            .map(|result| result.had_staged_changes)
            .unwrap_or(false),
        code_repo_had_unstaged_changes: code_repo_cleanup
            .as_ref()
            .map(|result| result.had_unstaged_changes)
            .unwrap_or(false),
        code_repo_had_untracked_files: code_repo_cleanup
            .as_ref()
            .map(|result| result.had_untracked_files)
            .unwrap_or(false),
        asset_repo_had_staged_changes: asset_repo_cleanup
            .as_ref()
            .map(|result| result.had_staged_changes)
            .unwrap_or(false),
        asset_repo_had_unstaged_changes: asset_repo_cleanup
            .as_ref()
            .map(|result| result.had_unstaged_changes)
            .unwrap_or(false),
        asset_repo_had_untracked_files: asset_repo_cleanup
            .as_ref()
            .map(|result| result.had_untracked_files)
            .unwrap_or(false),
    })
}

pub async fn start_task_obfuscation(state: AppState, task_id: String) -> Result<(), AppError> {
    let task = state.find_task(&task_id).await?;
    let runtime = state.get_task_runtime(&task_id).await?;
    if runtime.status == PackageTaskStatus::Running
        || runtime.status == PackageTaskStatus::Canceling
    {
        return Err(AppError::conflict("任务正在执行中，无法单独执行混淆"));
    }
    if !task.enable_obfuscation {
        return Err(AppError::validation("任务未开启混淆，无法单独执行混淆"));
    }

    let (project, _) = state.resolve_task_context(&task).await?;
    if project.is_hot_update {
        return Err(AppError::validation("热更新项目不支持单独执行混淆"));
    }
    if task.code_repo_url.trim().is_empty() {
        return Err(AppError::validation(
            "任务未配置代码包 Git 地址，无法执行混淆",
        ));
    }

    state.try_start_build().await?;
    if let Err(error) = state
        .prepare_tasks_for_build(std::slice::from_ref(&task_id))
        .await
    {
        state.finish_build().await;
        return Err(error);
    }

    tokio::spawn(async move {
        let _restart_guard = state.restart_guard("单独混淆任务");
        if let Err(error) = run_task_obfuscation_job(state.clone(), &task_id).await {
            error!(task_id, error = %error, "单独混淆执行失败");
            let updated_runtime = state
                .update_task_runtime(&task_id, RuntimeFlushMode::Immediate, |runtime| {
                    runtime.status = PackageTaskStatus::Failed;
                    if runtime.progress == 0 {
                        runtime.progress = 5;
                    }
                    runtime.last_error = Some(error.to_string());
                    runtime.finished_at = Some(now_string());
                })
                .await;
            let log_path = updated_runtime
                .ok()
                .and_then(|runtime| runtime.last_log_path);
            let log_message = format!(
                "单独混淆执行失败: error={}, log_path={}",
                error,
                log_path.unwrap_or_else(|| "无".to_string())
            );
            let _ = append_task_log(&state, &task_id, &log_message).await;
        }
        state.finish_build().await;
    });

    Ok(())
}

async fn run_task_obfuscation_job(state: AppState, task_id: &str) -> Result<(), AppError> {
    let task = state.find_task(task_id).await?;
    let (project, task_group) = state.resolve_task_context(&task).await?;
    let workspace_dir = state.workspace_project_dir(&project);
    let main_repo_dir = workspace_dir.join("main-repo");
    let task_dir = workspace_dir.join("tasks").join(&task.id);
    let code_repo_dir = task_dir.join("code-repo");
    let temp_dir = task_dir.join("temp");
    fs::create_dir_all(&temp_dir)
        .await
        .map_err(|error| AppError::internal(format!("创建任务目录失败: {error}")))?;

    let log_path = state
        .create_log_path(&format!("{}_obfuscation", task.name))
        .await?;
    let mut log_file = File::create(&log_path)
        .await
        .map_err(|error| AppError::internal(format!("创建日志文件失败: {error}")))?;

    state
        .update_task_runtime(task_id, RuntimeFlushMode::Immediate, |runtime| {
            runtime.status = PackageTaskStatus::Running;
            runtime.progress = 5;
            runtime.started_at = Some(now_string());
            runtime.finished_at = None;
            runtime.last_error = None;
            runtime.last_log_path = Some(log_path.to_string_lossy().to_string());
        })
        .await?;

    write_log_line(&mut log_file, &format!("任务开始单独混淆: {}", task.name)).await?;
    let settings = state.get_settings().await;

    if main_repo_dir.exists() {
        let main_repo_cleanup = git::cleanup_all_changes(&main_repo_dir)
            .await
            .map_err(AppError::internal)?;
        write_log_line(
            &mut log_file,
            &format!(
                "主工程仓库预处理完成: 清理已暂存改动={}, 清理未暂存改动={}, 清理未跟踪文件={}",
                main_repo_cleanup.had_staged_changes,
                main_repo_cleanup.had_unstaged_changes,
                main_repo_cleanup.had_untracked_files,
            ),
        )
        .await?;
    } else {
        write_log_line(&mut log_file, "主工程仓库首次同步，跳过未暂存改动清理").await?;
    }

    let main_repo = git::ensure_repo_synced(
        &main_repo_dir,
        &project.git_url,
        &settings.git_config,
        Some(&task_group.branch),
    )
    .await
    .map_err(AppError::internal)?;
    write_log_line(
        &mut log_file,
        &format!(
            "主工程仓库同步完成: path={}, branch={}, commit={}",
            main_repo.path.display(),
            main_repo.branch,
            main_repo.commit
        ),
    )
    .await?;
    state
        .update_task_runtime(task_id, RuntimeFlushMode::Deferred, |runtime| {
            runtime.progress = 35;
        })
        .await?;

    let code_repo = git::ensure_repo_synced(
        &code_repo_dir,
        &task.code_repo_url,
        &settings.git_config,
        None,
    )
    .await
    .map_err(AppError::internal)?;
    write_log_line(
        &mut log_file,
        &format!(
            "代码包仓库同步完成: path={}, branch={}, commit={}",
            code_repo.path.display(),
            code_repo.branch,
            code_repo.commit
        ),
    )
    .await?;

    let prepare_result = git::prepare_branch_from_current(
        &code_repo.path,
        &task.code_repo_url,
        &settings.git_config,
        &project.version,
    )
    .await
    .map_err(AppError::internal)?;
    write_log_line(
        &mut log_file,
        &format!(
            "代码包仓库分支准备完成: previous_branch={}, target_branch={}, source_branch={}, remote_exists={}, created={}, pushed={}",
            prepare_result.previous_branch,
            prepare_result.branch,
            prepare_result.source_branch,
            prepare_result.remote_exists,
            prepare_result.created,
            prepare_result.pushed
        ),
    )
    .await?;
    state
        .update_task_runtime(task_id, RuntimeFlushMode::Deferred, |runtime| {
            runtime.progress = 55;
        })
        .await?;

    run_task_obfuscation(ObfuscationExecutionContext {
        task: &task,
        project: &project,
        main_repo_path: &main_repo.path,
        code_repo_result: Some(&code_repo),
        task_dir: &task_dir,
        log_file: &mut log_file,
        git_config: &settings.git_config,
    })
    .await?;
    state
        .update_task_runtime(task_id, RuntimeFlushMode::Deferred, |runtime| {
            runtime.progress = 95;
        })
        .await?;

    state
        .update_task_runtime(task_id, RuntimeFlushMode::Immediate, |runtime| {
            runtime.status = PackageTaskStatus::Success;
            runtime.progress = 100;
            runtime.finished_at = Some(now_string());
            runtime.last_error = None;
        })
        .await?;
    write_log_line(&mut log_file, "任务单独混淆完成").await?;
    info!(task_id, task_name = %task.name, "单独混淆执行完成");

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use tokio::{fs, process::Command};

    use super::*;
    use crate::{
        models::{AppSettings, Engine, PackageTask, Project, TaskProjectConfig},
        state::RuntimeFlushMode,
    };

    fn temp_test_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        std::env::temp_dir().join(format!("cocos_build_package_tasks_test_{name}_{unique}"))
    }

    async fn create_state_with_task(name: &str) -> (AppState, Project, PathBuf) {
        let data_dir = temp_test_dir(name);
        fs::create_dir_all(&data_dir)
            .await
            .expect("create temp data dir");
        let state = AppState::load(data_dir.clone()).await;
        let project = Project {
            id: "project_1".to_string(),
            name: "演示项目".to_string(),
            workspace_dir_key: "workspace_1".to_string(),
            git_url: "https://example.com/demo.git".to_string(),
            engine_name: "cocos".to_string(),
            ..Project::default()
        };
        state
            .save_settings(AppSettings {
                engines: vec![Engine {
                    name: "cocos".to_string(),
                    path: "/engine".to_string(),
                }],
                projects: vec![project.clone()],
                package_tasks: vec![PackageTask {
                    id: "task_1".to_string(),
                    name: "任务1".to_string(),
                    project: Some(TaskProjectConfig {
                        project_id: project.id.clone(),
                        branch: "main".to_string(),
                    }),
                    build_args_json: "{}".to_string(),
                    ..PackageTask::default()
                }],
                ..AppSettings::default()
            })
            .await
            .expect("save settings");

        (state, project, data_dir)
    }

    async fn git(repo_dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo_dir)
            .args(args)
            .status()
            .await
            .expect("run git command");
        assert!(status.success(), "git command failed: {:?}", args);
    }

    async fn init_repo(repo_dir: &std::path::Path, tracked_file_name: &str) {
        fs::create_dir_all(repo_dir).await.expect("create repo dir");
        git(repo_dir, &["init"]).await;
        git(repo_dir, &["config", "user.name", "tester"]).await;
        git(repo_dir, &["config", "user.email", "tester@example.com"]).await;
        fs::write(repo_dir.join(tracked_file_name), "base\n")
            .await
            .expect("write tracked file");
        git(repo_dir, &["add", tracked_file_name]).await;
        git(repo_dir, &["commit", "-m", "init"]).await;
    }

    #[tokio::test]
    async fn cleanup_task_private_repos_should_skip_non_git_code_repo_and_clean_asset_repo() {
        let (state, project, data_dir) = create_state_with_task("cleanup_private_repos").await;
        let task_dir = state
            .workspace_project_dir(&project)
            .join("tasks")
            .join("task_1");
        let code_repo_dir = task_dir.join("code-repo");
        let asset_repo_dir = task_dir.join("asset-repo");

        fs::create_dir_all(&code_repo_dir)
            .await
            .expect("create code repo dir");
        fs::write(code_repo_dir.join("local.txt"), "non git\n")
            .await
            .expect("write non git file");

        init_repo(&asset_repo_dir, "tracked.txt").await;
        fs::write(asset_repo_dir.join("tracked.txt"), "changed\n")
            .await
            .expect("modify tracked file");
        git(&asset_repo_dir, &["add", "tracked.txt"]).await;
        fs::write(asset_repo_dir.join("tracked.txt"), "changed again\n")
            .await
            .expect("modify tracked file twice");
        fs::write(asset_repo_dir.join("new.txt"), "untracked\n")
            .await
            .expect("write untracked file");

        let result = cleanup_task_private_repos(&state, "task_1")
            .await
            .expect("cleanup private repos");

        assert!(!result.code_repo_git_repo);
        assert!(!result.code_repo_cleaned);
        assert!(result.asset_repo_git_repo);
        assert!(result.asset_repo_cleaned);
        assert!(result.asset_repo_had_staged_changes);
        assert!(result.asset_repo_had_unstaged_changes);
        assert!(result.asset_repo_had_untracked_files);
        assert!(code_repo_dir.join("local.txt").exists());
        assert!(!asset_repo_dir.join("new.txt").exists());
        assert_eq!(
            fs::read_to_string(asset_repo_dir.join("tracked.txt"))
                .await
                .expect("read tracked file"),
            "base\n"
        );

        let _ = fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn cleanup_task_private_repos_should_reject_running_task() {
        let (state, _project, data_dir) =
            create_state_with_task("cleanup_private_repos_running").await;
        state
            .update_task_runtime("task_1", RuntimeFlushMode::Immediate, |runtime| {
                runtime.status = PackageTaskStatus::Running;
            })
            .await
            .expect("update runtime");

        let error = cleanup_task_private_repos(&state, "task_1")
            .await
            .expect_err("should reject running task");

        assert_eq!(error.message(), "任务正在执行中，无法清理私有仓库");

        let _ = fs::remove_dir_all(data_dir).await;
    }
}
