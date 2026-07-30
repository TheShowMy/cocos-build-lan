use std::path::{Path, PathBuf};

use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
};
use tracing::{info, warn};

use crate::{
    error::AppError,
    git,
    models::{BuildStartRequest, ObfuscationMode, PackageTask, PackageTaskStatus, RepoSyncResult},
    services::{ast_obfuscation, obfuscation, placeholders::PlaceholderContext},
    state::{AppState, RuntimeFlushMode},
};

use super::{
    cocos::execute_cocos_build,
    logging::{now_string, write_log_line},
    prep_executor::{execute_prep_actions, validate_task_prep_actions},
};

pub(crate) struct ObfuscationExecutionContext<'a> {
    pub task: &'a PackageTask,
    pub project: &'a crate::models::Project,
    pub main_repo_path: &'a Path,
    pub code_repo_result: Option<&'a RepoSyncResult>,
    pub task_dir: &'a Path,
    pub log_file: &'a mut File,
    pub git_config: &'a crate::models::GitConfig,
}

pub async fn validate_package_tasks(
    state: &AppState,
    tasks: &[PackageTask],
) -> Result<(), AppError> {
    if tasks.is_empty() {
        return Err(AppError::validation("请至少选择一个任务"));
    }

    for task in tasks {
        state.resolve_task_context(task).await?;
        validate_task_prep_actions(
            state,
            &format!("任务 {} 的打包前准备", task.name),
            &task.pre_build_actions,
        )
        .await?;
        validate_task_prep_actions(
            state,
            &format!("任务 {} 的打包后准备", task.name),
            &task.post_build_actions,
        )
        .await?;
    }

    Ok(())
}

pub async fn collect_requested_tasks(
    state: &AppState,
    request: &BuildStartRequest,
) -> Result<Vec<PackageTask>, AppError> {
    if request.task_ids.is_empty() {
        return Err(AppError::validation("请至少选择一个任务"));
    }

    let mut tasks = Vec::with_capacity(request.task_ids.len());
    for task_id in &request.task_ids {
        tasks.push(state.find_task(task_id).await?);
    }
    Ok(tasks)
}

pub async fn run_single_task(
    state: &AppState,
    task_id: &str,
    start_notify_logs: &[String],
) -> Result<(), AppError> {
    let task = state.find_task(task_id).await?;
    let (project, task_group) = state.resolve_task_context(&task).await?;
    let engine = state.find_engine(&project.engine_name).await?;
    let version_prefix = project
        .version
        .split('.')
        .take(3)
        .collect::<Vec<_>>()
        .join(".");
    let minor_version = project.minor_version.to_string();

    if task.build_args_json.trim().is_empty() {
        return Err(AppError::validation(format!(
            "任务 {} 的构建参数为空",
            task.name
        )));
    }

    let workspace_dir = state.workspace_project_dir(&project);
    let main_repo_dir = workspace_dir.join("main-repo");
    let task_dir = workspace_dir.join("tasks").join(&task.id);
    let code_repo_dir = code_package_work_dir(&task_dir);
    let asset_repo_dir = task_dir.join("asset-repo");
    let temp_dir = task_dir.join("temp");

    fs::create_dir_all(&temp_dir)
        .await
        .map_err(|error| AppError::internal(format!("创建任务目录失败: {error}")))?;

    let log_path = state.create_log_path(&task.name).await?;
    let mut log_file = File::create(&log_path)
        .await
        .map_err(|error| AppError::internal(format!("创建日志文件失败: {error}")))?;

    state
        .update_task_runtime(task_id, RuntimeFlushMode::Immediate, |runtime| {
            runtime.status = PackageTaskStatus::Running;
            runtime.progress = 5;
            runtime.step_label = "准备工作区".to_owned();
            runtime.started_at = Some(now_string());
            runtime.finished_at = None;
            runtime.last_error = None;
            runtime.last_log_path = Some(log_path.to_string_lossy().to_string());
        })
        .await?;

    write_log_line(&mut log_file, &format!("任务开始: {}", task.name)).await?;
    for line in start_notify_logs {
        write_log_line(&mut log_file, line).await?;
    }
    info!(task_id, task_name = %task.name, project_name = %project.name, "单任务开始执行");

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
    state
        .update_task_runtime(task_id, RuntimeFlushMode::Deferred, |runtime| {
            runtime.progress = 20;
            runtime.step_label = "同步主工程".to_owned();
        })
        .await?;
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

    let code_repo = if task.code_repo_url.trim().is_empty() {
        ensure_code_package_work_dir(&code_repo_dir, &mut log_file).await?;
        None
    } else {
        sync_optional_repo(
            &task.code_repo_url,
            &code_repo_dir,
            &settings.git_config,
            "代码包仓库",
            &mut log_file,
        )
        .await?
    };
    if let Some(code_repo_result) = code_repo.as_ref() {
        let prepare_result = git::prepare_branch_from_current(
            &code_repo_result.path,
            &task.code_repo_url,
            &settings.git_config,
            &project.version,
        )
        .await
        .map_err(AppError::internal)?;
        log_branch_prepare(&mut log_file, "代码包仓库", &prepare_result).await?;
    }

    let asset_repo = sync_optional_repo(
        &task.asset_repo_url,
        &asset_repo_dir,
        &settings.git_config,
        "资源包仓库",
        &mut log_file,
    )
    .await?;
    if let Some(asset_repo_result) = asset_repo.as_ref() {
        let prepare_result = git::prepare_branch_from_base(
            &asset_repo_result.path,
            &task.asset_repo_url,
            &settings.git_config,
            &version_prefix,
            "master",
        )
        .await
        .map_err(AppError::internal)?;
        log_branch_prepare(&mut log_file, "资源包仓库", &prepare_result).await?;
    }
    state
        .update_task_runtime(task_id, RuntimeFlushMode::Deferred, |runtime| {
            runtime.progress = 35;
            runtime.step_label = "准备代码与资源仓库".to_owned();
        })
        .await?;
    write_log_line(&mut log_file, "代码包/资源包分支准备完成").await?;

    let placeholder_context = PlaceholderContext::new_with_params(
        &project,
        &engine,
        main_repo.branch.clone(),
        main_repo.path.to_string_lossy().to_string(),
        code_repo_dir.to_string_lossy().to_string(),
        asset_repo
            .as_ref()
            .map(|repo| repo.path.to_string_lossy().to_string())
            .unwrap_or_default(),
        &task_group.params,
    );

    execute_prep_actions(
        state,
        task_id,
        "打包前准备",
        &task.pre_build_actions,
        &placeholder_context,
        &mut log_file,
    )
    .await?;

    let rendered_config = render_build_config(&task, &placeholder_context);
    let temp_config_path = temp_dir.join("build-config.json");
    let mut config_file = File::create(&temp_config_path)
        .await
        .map_err(|error| AppError::internal(format!("写入构建配置失败: {error}")))?;
    config_file
        .write_all(rendered_config.as_bytes())
        .await
        .map_err(|error| AppError::internal(format!("写入构建配置失败: {error}")))?;
    state
        .update_task_runtime(task_id, RuntimeFlushMode::Deferred, |runtime| {
            runtime.progress = 40;
            runtime.step_label = "生成构建配置".to_owned();
        })
        .await?;
    write_log_line(
        &mut log_file,
        &format!("构建配置已生成: {}", temp_config_path.display()),
    )
    .await?;

    execute_cocos_build(
        state,
        task_id,
        &task.name,
        &engine,
        &main_repo.path,
        &temp_config_path,
        &mut log_file,
    )
    .await?;

    execute_prep_actions(
        state,
        task_id,
        "打包后准备",
        &task.post_build_actions,
        &placeholder_context,
        &mut log_file,
    )
    .await?;

    if let Some(code_repo_result) = code_repo.as_ref() {
        let finalize_result = git::finalize_repo_changes(
            &code_repo_result.path,
            &task.code_repo_url,
            &settings.git_config,
            &minor_version,
            project.is_hot_update,
        )
        .await
        .map_err(AppError::internal)?;
        log_commit_push(&mut log_file, "代码包仓库", &finalize_result).await?;
        state
            .update_task_runtime(task_id, RuntimeFlushMode::Deferred, |runtime| {
                runtime.progress = 97;
            })
            .await?;

        if task.enable_obfuscation && !project.is_hot_update {
            run_task_obfuscation(ObfuscationExecutionContext {
                task: &task,
                project: &project,
                main_repo_path: &main_repo.path,
                code_repo_result: Some(code_repo_result),
                task_dir: &task_dir,
                log_file: &mut log_file,
                git_config: &settings.git_config,
            })
            .await?;

            state
                .update_task_runtime(task_id, RuntimeFlushMode::Deferred, |runtime| {
                    runtime.progress = 98;
                })
                .await?;
        } else if task.enable_obfuscation {
            write_log_line(
                &mut log_file,
                "任务已开启混淆，但项目为热更新模式，跳过混淆阶段",
            )
            .await?;
        } else {
            write_log_line(&mut log_file, "任务未开启混淆，跳过混淆阶段").await?;
        }
    } else if task.enable_obfuscation && !project.is_hot_update {
        return Err(AppError::validation(
            "任务已开启混淆，但未配置代码包 Git 地址，无法执行混淆",
        ));
    }

    if let Some(asset_repo_result) = asset_repo.as_ref() {
        let finalize_result = git::finalize_repo_changes(
            &asset_repo_result.path,
            &task.asset_repo_url,
            &settings.git_config,
            &minor_version,
            false,
        )
        .await
        .map_err(AppError::internal)?;
        log_commit_push(&mut log_file, "资源包仓库", &finalize_result).await?;
        state
            .update_task_runtime(task_id, RuntimeFlushMode::Deferred, |runtime| {
                runtime.progress = 99;
            })
            .await?;
    }

    state
        .update_task_runtime(task_id, RuntimeFlushMode::Immediate, |runtime| {
            runtime.status = PackageTaskStatus::Success;
            runtime.progress = 100;
            runtime.step_label = "已完成".to_owned();
            runtime.finished_at = Some(now_string());
            runtime.last_error = None;
        })
        .await?;
    write_log_line(&mut log_file, "任务执行完成").await?;
    info!(task_id, task_name = %task.name, "单任务执行完成");

    Ok(())
}

pub(crate) async fn run_task_obfuscation(
    context: ObfuscationExecutionContext<'_>,
) -> Result<(), AppError> {
    let task = context.task;
    let project = context.project;
    let code_repo_result = context.code_repo_result.ok_or_else(|| {
        AppError::validation("任务已开启混淆，但未配置代码包 Git 地址，无法执行混淆")
    })?;

    if !task.enable_obfuscation {
        write_log_line(context.log_file, "任务未开启混淆，跳过混淆阶段").await?;
        return Ok(());
    }

    if project.is_hot_update {
        write_log_line(
            context.log_file,
            "任务已开启混淆，但项目为热更新模式，跳过混淆阶段",
        )
        .await?;
        return Ok(());
    }

    match task.obfuscation_mode {
        ObfuscationMode::Classic => {
            let obfuscation_result = obfuscation::run_code_package_obfuscation(
                context.main_repo_path,
                &code_repo_result.path,
                context.task_dir,
                task.obfuscation_seed,
            )
            .await
            .map_err(AppError::internal)?;
            write_log_line(
                context.log_file,
                &format!(
                    "代码包普通混淆完成: work_dir={}, target_input={}, target_output={}, whitelist_path={}, whitelist_keyword_count={}, file_count={}, replaced_word_count={}, mapping_path={}",
                    obfuscation_result.work_dir.display(),
                    obfuscation_result.target_input_path.display(),
                    obfuscation_result.target_output_path.display(),
                    obfuscation_result.whitelist_path.display(),
                    obfuscation_result.whitelist_keyword_count,
                    obfuscation_result.file_count,
                    obfuscation_result.replaced_word_count,
                    obfuscation_result
                        .mapping_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "无".to_string()),
                ),
            )
            .await?;

            if obfuscation_result.copied_back {
                let finalize_result = git::finalize_repo_changes(
                    &code_repo_result.path,
                    &task.code_repo_url,
                    context.git_config,
                    "obfuscation",
                    false,
                )
                .await
                .map_err(AppError::internal)?;
                log_commit_push(context.log_file, "代码包仓库混淆后处理", &finalize_result).await?;
            } else {
                write_log_line(
                    context.log_file,
                    "普通混淆结果未改变目标 game.js，跳过 obfuscation 提交",
                )
                .await?;
            }
        }
        ObfuscationMode::Ast => {
            write_log_line(
                context.log_file,
                "AST 模式开始：先执行普通混淆，再执行 AST 混淆",
            )
            .await?;

            let classic_result = obfuscation::run_code_package_obfuscation(
                context.main_repo_path,
                &code_repo_result.path,
                context.task_dir,
                task.obfuscation_seed,
            )
            .await
            .map_err(AppError::internal)?;
            write_log_line(
                context.log_file,
                &format!(
                    "代码包普通混淆完成（AST 前置步骤）: work_dir={}, target_input={}, target_output={}, whitelist_path={}, whitelist_keyword_count={}, file_count={}, replaced_word_count={}, mapping_path={}",
                    classic_result.work_dir.display(),
                    classic_result.target_input_path.display(),
                    classic_result.target_output_path.display(),
                    classic_result.whitelist_path.display(),
                    classic_result.whitelist_keyword_count,
                    classic_result.file_count,
                    classic_result.replaced_word_count,
                    classic_result
                        .mapping_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "无".to_string()),
                ),
            )
            .await?;

            let ast_result = ast_obfuscation::run_code_package_ast_obfuscation(
                context.main_repo_path,
                &code_repo_result.path,
                context.task_dir,
                task.obfuscation_seed,
                task.enable_dead_code_injection,
                task.dead_code_injection_count,
            )
            .await
            .map_err(AppError::internal)?;
            write_log_line(
                context.log_file,
                &format!(
                    "代码包 AST 混淆完成: work_dir={}, target_input={}, target_output={}, whitelist_path={}, whitelist_keyword_count={}, renamed_binding_count={}, rewritten_expression_count={}, rewritten_literal_count={}, dead_code_target_count={}, dead_code_actual_count={}, dead_code_block_count={}, candidate_function_count={}, dead_code_shortage_reason={}",
                    ast_result.work_dir.display(),
                    ast_result.target_input_path.display(),
                    ast_result.target_output_path.display(),
                    ast_result.whitelist_path.display(),
                    ast_result.whitelist_keyword_count,
                    ast_result.renamed_binding_count,
                    ast_result.rewritten_expression_count,
                    ast_result.rewritten_literal_count,
                    ast_result.dead_code_target_count,
                    ast_result.dead_code_actual_count,
                    ast_result.dead_code_block_count,
                    ast_result.candidate_function_count,
                    ast_result.dead_code_shortage_reason.as_deref().unwrap_or("无"),
                ),
            )
            .await?;

            if classic_result.copied_back || ast_result.copied_back {
                let finalize_result = git::finalize_repo_changes(
                    &code_repo_result.path,
                    &task.code_repo_url,
                    context.git_config,
                    "obfuscation",
                    false,
                )
                .await
                .map_err(AppError::internal)?;
                log_commit_push(
                    context.log_file,
                    "代码包仓库 classic-before-ast 混淆后处理",
                    &finalize_result,
                )
                .await?;
            } else {
                write_log_line(
                    context.log_file,
                    "classic-before-ast 两步混淆都未改变目标 game.js，跳过 obfuscation 提交",
                )
                .await?;
            }
        }
    }

    Ok(())
}

async fn sync_optional_repo(
    git_url: &str,
    repo_dir: &std::path::Path,
    git_config: &crate::models::GitConfig,
    label: &str,
    log_file: &mut File,
) -> Result<Option<RepoSyncResult>, AppError> {
    if git_url.trim().is_empty() {
        write_log_line(log_file, &format!("{label}未配置，跳过同步")).await?;
        return Ok(None);
    }

    let result = git::ensure_repo_synced(repo_dir, git_url, git_config, None)
        .await
        .map_err(AppError::internal)?;
    write_log_line(
        log_file,
        &format!(
            "{label}同步完成: path={}, branch={}, commit={}",
            result.path.display(),
            result.branch,
            result.commit
        ),
    )
    .await?;
    Ok(Some(result))
}

fn code_package_work_dir(task_dir: &Path) -> PathBuf {
    task_dir.join("code-repo")
}

async fn ensure_code_package_work_dir(
    code_repo_dir: &Path,
    log_file: &mut File,
) -> Result<(), AppError> {
    fs::create_dir_all(code_repo_dir)
        .await
        .map_err(|error| AppError::internal(format!("创建代码包工作目录失败: {error}")))?;
    write_log_line(
        log_file,
        &format!(
            "代码包 Git 未配置，使用本地固定目录: path={}",
            code_repo_dir.display()
        ),
    )
    .await?;
    Ok(())
}

async fn log_branch_prepare(
    log_file: &mut File,
    label: &str,
    result: &git::BranchPrepareResult,
) -> Result<(), AppError> {
    write_log_line(
        log_file,
        &format!(
            "{label}分支准备完成: previous_branch={}, target_branch={}, source_branch={}, remote_exists={}, created={}, pushed={}",
            result.previous_branch,
            result.branch,
            result.source_branch,
            result.remote_exists,
            result.created,
            result.pushed
        ),
    )
    .await
}

async fn log_commit_push(
    log_file: &mut File,
    label: &str,
    result: &git::CommitPushResult,
) -> Result<(), AppError> {
    write_log_line(
        log_file,
        &format!(
            "{label}后处理完成: branch={}, had_changes={}, discarded_changes={}, commit_sha={}, had_unpushed_commits={}, pushed={}",
            result.branch,
            result.had_changes,
            result.discarded_changes,
            result.commit_sha.as_deref().unwrap_or("无"),
            result.had_unpushed_commits,
            result.pushed
        ),
    )
    .await
}

fn render_build_config(task: &PackageTask, placeholder_context: &PlaceholderContext) -> String {
    placeholder_context.replace_text(&task.build_args_json)
}

pub fn handle_task_failure(error: &AppError) {
    warn!(error = %error, "单任务执行失败");
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_test_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        std::env::temp_dir().join(format!("cocos_build_task_test_{name}_{unique}"))
    }

    #[test]
    fn code_package_work_dir_should_use_fixed_code_repo_name() {
        let task_dir = PathBuf::from("/tmp/demo-task");
        assert_eq!(code_package_work_dir(&task_dir), task_dir.join("code-repo"));
    }

    #[tokio::test]
    async fn ensure_code_package_work_dir_should_create_dir_and_log_path() {
        let temp_dir = temp_test_dir("code_package_dir");
        let log_path = temp_dir.join("task.log");
        fs::create_dir_all(&temp_dir)
            .await
            .expect("create temporary directory");
        let code_repo_dir = code_package_work_dir(&temp_dir.join("task"));
        let mut log_file = File::create(&log_path).await.expect("create log file");

        ensure_code_package_work_dir(&code_repo_dir, &mut log_file)
            .await
            .expect("ensure code package dir");

        assert!(code_repo_dir.exists());
        let log_content = fs::read_to_string(&log_path)
            .await
            .expect("read log content");
        assert!(log_content.contains("代码包 Git 未配置，使用本地固定目录"));
        assert!(log_content.contains(&code_repo_dir.display().to_string()));

        let _ = fs::remove_dir_all(temp_dir).await;
    }
}
