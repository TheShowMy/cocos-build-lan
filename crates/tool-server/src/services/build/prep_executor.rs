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
    let params_text = prep_input_params_text(&prep_project, &target.params, placeholder_context)?;
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

fn prep_input_params_text(
    prep_project: &crate::models::PrepProject,
    params: &std::collections::HashMap<String, serde_json::Value>,
    context: &PlaceholderContext,
) -> Result<String, AppError> {
    let args =
        preps::build_prep_args(prep_project, params, context).map_err(AppError::validation)?;
    let inputs = args
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| (pair[0].trim_start_matches("--"), pair[1].as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    serde_json::to_string_pretty(&inputs).map_err(|error| AppError::internal(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::models::{
        BuildMode, Engine, PrepParam, PrepParamType, PrepParamValueSource, PrepProject, Project,
    };

    #[test]
    fn input_log_should_include_resolved_fixed_runtime_and_system_params() {
        let param = |name: &str, fixed_value: Option<serde_json::Value>| PrepParam {
            name: name.to_string(),
            param_type: PrepParamType::Str,
            value_source: if fixed_value.is_some() {
                PrepParamValueSource::Fixed
            } else {
                PrepParamValueSource::Runtime
            },
            optional: true,
            options: Vec::new(),
            fixed_value,
        };
        let prep_project = PrepProject {
            id: "prep".to_string(),
            name: "配置拉取".to_string(),
            description: String::new(),
            path: String::new(),
            create_time: String::new(),
            params: vec![
                param("build_mode", Some(json!("${build_mode}"))),
                param("censor_switch", Some(json!("${review_mode}"))),
                param("test_branch", None),
                param("optional_branch", None),
            ],
        };
        let context = PlaceholderContext::new_with_params(
            &Project {
                build_mode: BuildMode::Test,
                review_mode: true,
                ..Project::default()
            },
            &Engine::default(),
            "master".to_string(),
            "D:/project".to_string(),
            String::new(),
            String::new(),
            &std::collections::BTreeMap::from([
                ("build_mode".into(), json!("test")),
                ("review_mode".into(), json!(true)),
            ]),
        );
        let params = HashMap::from([
            ("test_branch".to_string(), json!("master")),
            ("build_mode".to_string(), json!("override")),
            ("project_path".to_string(), json!("override")),
            ("unknown".to_string(), json!("ignored")),
        ]);
        let text = prep_input_params_text(&prep_project, &params, &context).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap(),
            json!({
                "build_mode": "test",
                "censor_switch": "true",
                "test_branch": "master",
                "project_path": "D:/project"
            })
        );
    }
}
