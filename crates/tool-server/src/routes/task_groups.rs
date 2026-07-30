use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, put},
};
use serde_json::Value;

use crate::{
    error::AppError,
    models::{ParamDefinition, ParamKind, TaskGroup, TaskGroupParamsRequest, TaskGroupRequest},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/task-groups",
            get(list_task_groups).post(create_task_group),
        )
        .route(
            "/api/task-groups/{group_id}",
            put(update_task_group).delete(delete_task_group),
        )
        .route(
            "/api/task-groups/{group_id}/params",
            put(update_task_group_params),
        )
}

async fn list_task_groups(State(state): State<AppState>) -> Json<Vec<TaskGroup>> {
    let mut groups = state.get_settings().await.task_groups;
    groups.sort_by_key(|group| group.order);
    Json(groups)
}

async fn create_task_group(
    State(state): State<AppState>,
    Json(request): Json<TaskGroupRequest>,
) -> Result<Json<TaskGroup>, AppError> {
    validate_group_request(&state, &request, None).await?;
    let mut settings = state.get_settings().await;
    let order = settings
        .task_groups
        .iter()
        .map(|group| group.order)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let params = if let Some(source_id) = request
        .copy_from_group_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
    {
        settings
            .task_groups
            .iter()
            .find(|group| group.id == source_id)
            .map(|group| group.params.clone())
            .ok_or_else(|| AppError::not_found(format!("未找到源任务组 {source_id}")))?
    } else {
        request.params
    };
    let group = TaskGroup {
        id: format!("group_{}", uuid::Uuid::new_v4().simple()),
        project_id: request.project_id.trim().to_owned(),
        name: request.name.trim().to_owned(),
        description: request.description.trim().to_owned(),
        branch: request.branch.trim().to_owned(),
        params: normalize_params(&settings.param_definitions, params)?,
        order,
    };
    settings.task_groups.push(group.clone());
    state.save_settings(settings).await?;
    Ok(Json(group))
}

async fn update_task_group(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
    Json(request): Json<TaskGroupRequest>,
) -> Result<Json<TaskGroup>, AppError> {
    validate_group_request(&state, &request, Some(&group_id)).await?;
    let mut settings = state.get_settings().await;
    let definitions = settings.param_definitions.clone();
    let group = settings
        .task_groups
        .iter_mut()
        .find(|group| group.id == group_id)
        .ok_or_else(|| AppError::not_found(format!("未找到任务组 {group_id}")))?;
    group.project_id = request.project_id.trim().to_owned();
    group.name = request.name.trim().to_owned();
    group.description = request.description.trim().to_owned();
    group.branch = request.branch.trim().to_owned();
    group.params = normalize_params(&definitions, request.params)?;
    let result = group.clone();
    for task in settings
        .package_tasks
        .iter_mut()
        .filter(|task| task.task_group_id == group_id)
    {
        task.group = result.name.clone();
        task.project = Some(crate::models::TaskProjectConfig {
            project_id: result.project_id.clone(),
            branch: result.branch.clone(),
        });
    }
    state.save_settings(settings).await?;
    Ok(Json(result))
}

async fn update_task_group_params(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
    Json(request): Json<TaskGroupParamsRequest>,
) -> Result<Json<TaskGroup>, AppError> {
    if request.branch.trim().is_empty() {
        return Err(AppError::validation("任务组分支不能为空"));
    }
    let mut settings = state.get_settings().await;
    let definitions = settings.param_definitions.clone();
    let group = settings
        .task_groups
        .iter_mut()
        .find(|group| group.id == group_id)
        .ok_or_else(|| AppError::not_found(format!("未找到任务组 {group_id}")))?;
    group.branch = request.branch.trim().to_owned();
    group.params = normalize_params(&definitions, request.params)?;
    let result = group.clone();
    for task in settings
        .package_tasks
        .iter_mut()
        .filter(|task| task.task_group_id == group_id)
    {
        task.project = Some(crate::models::TaskProjectConfig {
            project_id: result.project_id.clone(),
            branch: result.branch.clone(),
        });
    }
    state.save_settings(settings).await?;
    Ok(Json(result))
}

async fn delete_task_group(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
) -> Result<axum::http::StatusCode, AppError> {
    let settings = state.get_settings().await;
    if !settings
        .task_groups
        .iter()
        .any(|group| group.id == group_id)
    {
        return Err(AppError::not_found(format!("未找到任务组 {group_id}")));
    }
    let task_ids = settings
        .package_tasks
        .iter()
        .filter(|task| task.task_group_id == group_id)
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    for task_id in &task_ids {
        let runtime = state.get_task_runtime(task_id).await?;
        if runtime.status == crate::models::PackageTaskStatus::Running
            || runtime.status == crate::models::PackageTaskStatus::Canceling
        {
            return Err(AppError::conflict("任务组内存在运行中任务，无法删除"));
        }
    }
    for task_id in task_ids {
        crate::services::package_tasks::delete_task(&state, &task_id).await?;
    }
    let mut settings = state.get_settings().await;
    settings.task_groups.retain(|group| group.id != group_id);
    state.save_settings(settings).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn validate_group_request(
    state: &AppState,
    request: &TaskGroupRequest,
    current_group_id: Option<&str>,
) -> Result<(), AppError> {
    if request.name.trim().is_empty() {
        return Err(AppError::validation("任务组名称不能为空"));
    }
    if request.branch.trim().is_empty() {
        return Err(AppError::validation("任务组分支不能为空"));
    }
    state.find_project(request.project_id.trim()).await?;
    let duplicate = state
        .get_settings()
        .await
        .task_groups
        .into_iter()
        .any(|group| {
            Some(group.id.as_str()) != current_group_id
                && group.project_id == request.project_id.trim()
                && group.name.eq_ignore_ascii_case(request.name.trim())
        });
    if duplicate {
        return Err(AppError::conflict("同一项目下的任务组名称不能重复"));
    }
    Ok(())
}

fn normalize_params(
    definitions: &[ParamDefinition],
    params: BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, AppError> {
    for key in params.keys() {
        if !definitions.iter().any(|definition| definition.key == *key) {
            return Err(AppError::validation(format!("未知任务参数 {key}")));
        }
    }

    let mut normalized = BTreeMap::new();
    for definition in definitions {
        let value = params
            .get(&definition.key)
            .cloned()
            .unwrap_or_else(|| definition.default_value.clone());
        if definition.required && is_empty(&value) {
            return Err(AppError::validation(format!(
                "参数 {} 不能为空",
                definition.label
            )));
        }
        if definition.kind == ParamKind::Select {
            let Some(value) = value.as_str() else {
                return Err(AppError::validation(format!(
                    "参数 {} 必须是选项值",
                    definition.label
                )));
            };
            if !definition.options.iter().any(|option| option == value) {
                return Err(AppError::validation(format!(
                    "参数 {} 的选项无效",
                    definition.label
                )));
            }
        }
        if definition.kind == ParamKind::Switch && !value.is_boolean() {
            return Err(AppError::validation(format!(
                "参数 {} 必须是开关值",
                definition.label
            )));
        }
        if definition.kind == ParamKind::Number && !value.is_number() {
            return Err(AppError::validation(format!(
                "参数 {} 必须是数字",
                definition.label
            )));
        }
        normalized.insert(definition.key.clone(), value);
    }
    Ok(normalized)
}

fn is_empty(value: &Value) -> bool {
    value.is_null() || value.as_str().is_some_and(|value| value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::*;
    use crate::models::{AppSettings, PackageTask, Project};

    #[test]
    fn parameter_validation_rejects_unknown_and_invalid_select_values() {
        let definitions = vec![ParamDefinition {
            key: "channel".to_owned(),
            label: "渠道".to_owned(),
            kind: ParamKind::Select,
            options: vec!["official".to_owned()],
            default_value: Value::String("official".to_owned()),
            required: true,
            ..ParamDefinition::default()
        }];
        assert!(
            normalize_params(
                &definitions,
                BTreeMap::from([("other".to_owned(), Value::Null)])
            )
            .is_err()
        );
        assert!(
            normalize_params(
                &definitions,
                BTreeMap::from([("channel".to_owned(), Value::String("bad".to_owned()))])
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn deleting_group_cascades_its_tasks() {
        let data_dir = std::env::temp_dir().join(format!(
            "cocos_build_group_delete_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let state = AppState::load(data_dir.clone()).await;
        state
            .save_settings(AppSettings {
                projects: vec![Project {
                    id: "project_1".to_owned(),
                    workspace_dir_key: "workspace_1".to_owned(),
                    name: "项目".to_owned(),
                    ..Project::default()
                }],
                task_groups: vec![TaskGroup {
                    id: "group_1".to_owned(),
                    project_id: "project_1".to_owned(),
                    name: "主分组".to_owned(),
                    branch: "main".to_owned(),
                    ..TaskGroup::default()
                }],
                package_tasks: vec![PackageTask {
                    id: "task_1".to_owned(),
                    name: "任务".to_owned(),
                    task_group_id: "group_1".to_owned(),
                    build_args_json: "{}".to_owned(),
                    ..PackageTask::default()
                }],
                ..AppSettings::default()
            })
            .await
            .expect("settings");
        let app: Router = router().with_state(state.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/task-groups/group_1")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let settings = state.get_settings().await;
        assert!(settings.task_groups.is_empty());
        assert!(settings.package_tasks.is_empty());

        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn creating_group_can_copy_only_source_params() {
        let data_dir = std::env::temp_dir().join(format!(
            "cocos_build_group_copy_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let state = AppState::load(data_dir.clone()).await;
        state
            .save_settings(AppSettings {
                projects: vec![Project {
                    id: "project_1".to_owned(),
                    workspace_dir_key: "workspace_1".to_owned(),
                    name: "项目".to_owned(),
                    ..Project::default()
                }],
                param_definitions: vec![ParamDefinition {
                    key: "channel".to_owned(),
                    label: "渠道".to_owned(),
                    default_value: Value::String("default".to_owned()),
                    ..ParamDefinition::default()
                }],
                task_groups: vec![TaskGroup {
                    id: "group_source".to_owned(),
                    project_id: "project_1".to_owned(),
                    name: "源组".to_owned(),
                    description: "源描述".to_owned(),
                    branch: "source".to_owned(),
                    params: BTreeMap::from([(
                        "channel".to_owned(),
                        Value::String("copied".to_owned()),
                    )]),
                    order: 0,
                }],
                ..AppSettings::default()
            })
            .await
            .unwrap();
        let app: Router = router().with_state(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/task-groups")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "projectId": "project_1",
                            "name": "目标组",
                            "description": "目标描述",
                            "branch": "target",
                            "copyFromGroupId": "group_source"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let group: TaskGroup = serde_json::from_slice(&body).unwrap();
        assert_eq!(group.description, "目标描述");
        assert_eq!(group.branch, "target");
        assert_eq!(group.params["channel"], Value::String("copied".to_owned()));

        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }
}
