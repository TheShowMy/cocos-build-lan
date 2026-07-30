use tokio::fs::File;
use tracing::info;

use crate::{
    error::AppError,
    models::{TaskPrepAction, TaskPrepTarget},
    services::{placeholders::PlaceholderContext, preps},
    state::{AppState, RuntimeFlushMode},
};

use super::logging::write_log_line;

pub async fn execute_prep_actions(
    state: &AppState,
    task_id: &str,
    stage_label: &str,
    actions: &[TaskPrepAction],
    placeholder_context: &PlaceholderContext,
    log_file: &mut File,
) -> Result<(), AppError> {
    if actions.is_empty() {
        write_log_line(log_file, &format!("{stage_label}为空，跳过执行")).await?;
        return Ok(());
    }

    for (index, action) in actions.iter().enumerate() {
        match action {
            TaskPrepAction::Single {
                prep_project_id,
                params,
            } => {
                let target = TaskPrepTarget {
                    prep_project_id: prep_project_id.clone(),
                    params: params.clone(),
                };
                execute_prep_target(
                    state,
                    task_id,
                    stage_label,
                    index + 1,
                    &target,
                    None,
                    None,
                    placeholder_context,
                    log_file,
                )
                .await?;
            }
            TaskPrepAction::Conditional {
                condition_source, ..
            } => {
                let selection = select_conditional_targets(action, placeholder_context)?;
                write_log_line(
                    log_file,
                    &format!("{stage_label}第 {} 项开始执行条件步骤", index + 1),
                )
                .await?;
                write_log_line(log_file, &format!("条件来源: {}", condition_source.trim())).await?;
                write_log_line(
                    log_file,
                    &format!("解析结果: {}", selection.rendered_source),
                )
                .await?;
                write_log_line(log_file, &format!("比较值: {}", selection.compare_value)).await?;
                write_log_line(
                    log_file,
                    &format!(
                        "条件结果: {}",
                        if selection.matched {
                            "命中"
                        } else {
                            "未命中"
                        }
                    ),
                )
                .await?;

                let branch_label = if selection.matched {
                    "条件成立分支"
                } else {
                    "条件不成立分支"
                };
                write_log_line(log_file, &format!("进入{branch_label}")).await?;

                for (target_index, target) in selection.targets.iter().enumerate() {
                    execute_prep_target(
                        state,
                        task_id,
                        stage_label,
                        index + 1,
                        target,
                        Some(branch_label),
                        Some(target_index + 1),
                        placeholder_context,
                        log_file,
                    )
                    .await?;
                }
            }
        }
    }

    Ok(())
}

pub async fn validate_task_prep_actions(
    state: &AppState,
    stage_label: &str,
    actions: &[TaskPrepAction],
) -> Result<(), AppError> {
    for (index, action) in actions.iter().enumerate() {
        validate_prep_action_structure(stage_label, index + 1, action)?;

        match action {
            TaskPrepAction::Single {
                prep_project_id,
                params,
            } => {
                let target = TaskPrepTarget {
                    prep_project_id: prep_project_id.clone(),
                    params: params.clone(),
                };
                validate_task_prep_target(
                    state,
                    &format!("{stage_label}第 {} 项", index + 1),
                    &target,
                )
                .await?;
            }
            TaskPrepAction::Conditional {
                on_match_targets,
                on_mismatch_targets,
                ..
            } => {
                for (target_index, target) in on_match_targets.iter().enumerate() {
                    validate_task_prep_target(
                        state,
                        &format!(
                            "{stage_label}第 {} 项条件成立分支第 {} 个准备项目",
                            index + 1,
                            target_index + 1
                        ),
                        target,
                    )
                    .await?;
                }

                for (target_index, target) in on_mismatch_targets.iter().enumerate() {
                    validate_task_prep_target(
                        state,
                        &format!(
                            "{stage_label}第 {} 项条件不成立分支第 {} 个准备项目",
                            index + 1,
                            target_index + 1
                        ),
                        target,
                    )
                    .await?;
                }
            }
        }
    }

    Ok(())
}

fn validate_prep_action_structure(
    stage_label: &str,
    step_index: usize,
    action: &TaskPrepAction,
) -> Result<(), AppError> {
    match action {
        TaskPrepAction::Single {
            prep_project_id, ..
        } => {
            if prep_project_id.trim().is_empty() {
                return Err(AppError::validation(format!(
                    "{stage_label}第 {step_index} 项未选择准备项目"
                )));
            }
        }
        TaskPrepAction::Conditional {
            condition_source,
            condition_equals,
            on_match_targets,
            on_mismatch_targets,
        } => {
            if condition_source.trim().is_empty() {
                return Err(AppError::validation(format!(
                    "{stage_label}第 {step_index} 项缺少判断来源"
                )));
            }
            if condition_equals.trim().is_empty() {
                return Err(AppError::validation(format!(
                    "{stage_label}第 {step_index} 项缺少等于值"
                )));
            }
            if on_match_targets.is_empty() {
                return Err(AppError::validation(format!(
                    "{stage_label}第 {step_index} 项缺少条件成立执行项目"
                )));
            }
            if on_mismatch_targets.is_empty() {
                return Err(AppError::validation(format!(
                    "{stage_label}第 {step_index} 项缺少条件不成立执行项目"
                )));
            }

            for (target_index, target) in on_match_targets.iter().enumerate() {
                if target.prep_project_id.trim().is_empty() {
                    return Err(AppError::validation(format!(
                        "{stage_label}第 {step_index} 项条件成立分支第 {} 个准备项目未选择",
                        target_index + 1
                    )));
                }
            }

            for (target_index, target) in on_mismatch_targets.iter().enumerate() {
                if target.prep_project_id.trim().is_empty() {
                    return Err(AppError::validation(format!(
                        "{stage_label}第 {step_index} 项条件不成立分支第 {} 个准备项目未选择",
                        target_index + 1
                    )));
                }
            }
        }
    }

    Ok(())
}

async fn validate_task_prep_target(
    state: &AppState,
    target_label: &str,
    target: &TaskPrepTarget,
) -> Result<(), AppError> {
    let prep_project = preps::load_prep_project(state, &target.prep_project_id)
        .await
        .map_err(|error| {
            AppError::validation(format!("{target_label}引用的准备项目不存在: {error}"))
        })?;
    preps::validate_prep_target_params(&prep_project, &target.params)
        .map_err(|error| AppError::validation(format!("{target_label}参数无效: {error}")))
}

#[derive(Debug)]
struct ConditionalTargetSelection<'a> {
    matched: bool,
    rendered_source: String,
    compare_value: String,
    targets: &'a [TaskPrepTarget],
}

fn select_conditional_targets<'a>(
    action: &'a TaskPrepAction,
    placeholder_context: &PlaceholderContext,
) -> Result<ConditionalTargetSelection<'a>, AppError> {
    let TaskPrepAction::Conditional {
        condition_source,
        condition_equals,
        on_match_targets,
        on_mismatch_targets,
    } = action
    else {
        return Err(AppError::validation("当前步骤不是条件步骤"));
    };

    let condition_source = condition_source.trim();
    if condition_source.is_empty() {
        return Err(AppError::validation("条件步骤缺少判断来源"));
    }

    let compare_value = condition_equals.trim().to_string();
    if compare_value.is_empty() {
        return Err(AppError::validation("条件步骤缺少等于值"));
    }

    if on_match_targets.is_empty() {
        return Err(AppError::validation("条件步骤缺少条件成立执行项目"));
    }
    if on_mismatch_targets.is_empty() {
        return Err(AppError::validation("条件步骤缺少条件不成立执行项目"));
    }

    let rendered_source = placeholder_context
        .replace_text(condition_source)
        .trim()
        .to_string();
    let matched = rendered_source == compare_value;
    let targets = if matched {
        on_match_targets.as_slice()
    } else {
        on_mismatch_targets.as_slice()
    };

    Ok(ConditionalTargetSelection {
        matched,
        rendered_source,
        compare_value,
        targets,
    })
}

#[allow(clippy::too_many_arguments)]
async fn execute_prep_target(
    state: &AppState,
    task_id: &str,
    stage_label: &str,
    step_index: usize,
    target: &TaskPrepTarget,
    branch_label: Option<&str>,
    branch_target_index: Option<usize>,
    placeholder_context: &PlaceholderContext,
    log_file: &mut File,
) -> Result<(), AppError> {
    if target.prep_project_id.trim().is_empty() {
        return Err(AppError::validation(
            match (branch_label, branch_target_index) {
                (Some(branch), Some(index)) => {
                    format!("{stage_label}第 {step_index} 项{branch}第 {index} 个准备项目未选择")
                }
                _ => format!("{stage_label}第 {step_index} 项未选择准备项目"),
            },
        ));
    }

    let prep_project = preps::load_prep_project(state, &target.prep_project_id)
        .await
        .map_err(AppError::internal)?;
    let params_text =
        serde_json::to_string_pretty(&target.params).unwrap_or_else(|_| "{}".to_string());
    let step_prefix = match (branch_label, branch_target_index) {
        (Some(branch), Some(index)) => {
            format!("{stage_label}第 {step_index} 项{branch}第 {index} 个准备项目")
        }
        _ => format!("{stage_label}第 {step_index} 项"),
    };

    write_log_line(
        log_file,
        &format!(
            "{step_prefix}开始执行: {} ({})",
            prep_project.name, prep_project.id
        ),
    )
    .await?;
    write_log_line(log_file, &format!("输入参数: {params_text}")).await?;

    let result = preps::run_prep_project(&prep_project, target.params.clone(), placeholder_context)
        .await
        .map_err(AppError::internal)?;

    write_log_line(log_file, &format!("执行命令: {}", result.command)).await?;
    if !result.stdout.trim().is_empty() {
        write_log_line(log_file, "stdout >>>").await?;
        for line in result.stdout.lines() {
            write_log_line(log_file, line).await?;
        }
    }
    if !result.stderr.trim().is_empty() {
        write_log_line(log_file, "stderr >>>").await?;
        for line in result.stderr.lines() {
            write_log_line(log_file, line).await?;
        }
    }

    if !result.success {
        return Err(AppError::internal(format!(
            "{step_prefix}执行失败: {}，退出码 {}",
            prep_project.name, result.exit_code
        )));
    }

    state
        .update_task_runtime(task_id, RuntimeFlushMode::Deferred, |task| {
            if task.progress < 40 {
                task.progress = task.progress.saturating_add(1).min(40);
            }
        })
        .await?;
    info!(task_id, stage_label, step_index, prep_project_id = %prep_project.id, "准备步骤执行成功");
    write_log_line(
        log_file,
        &format!("{step_prefix}执行成功: {}", prep_project.name),
    )
    .await?;

    Ok(())
}
