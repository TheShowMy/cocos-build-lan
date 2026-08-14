use std::collections::{BTreeMap, HashSet};

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, put},
};
use serde_json::Value;

use crate::{
    error::AppError,
    models::{
        GroupParamPreset, ParamDefinition, ParamKind, TaskGroup, TaskGroupParamsRequest,
        TaskGroupRequest,
    },
    services::settings::normalize_integral_number,
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
        presets: normalize_presets(
            &settings.param_definitions,
            request.presets.unwrap_or_default(),
        )?,
        hidden_params: normalize_hidden_params(
            &settings.param_definitions,
            request.hidden_params.unwrap_or_default(),
        )?,
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
    if let Some(presets) = request.presets {
        group.presets = normalize_presets(&definitions, presets)?;
    }
    if let Some(hidden) = request.hidden_params {
        group.hidden_params = normalize_hidden_params(&definitions, hidden)?;
    }
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
        let mut value = params
            .get(&definition.key)
            .cloned()
            .unwrap_or_else(|| definition.default_value.clone());
        if definition.required && is_empty(&value) {
            return Err(AppError::validation(format!(
                "参数 {} 不能为空",
                definition.label
            )));
        }
        check_param_kind(definition, &mut value)?;
        normalized.insert(definition.key.clone(), value);
    }
    Ok(normalized)
}

fn check_param_kind(definition: &ParamDefinition, value: &mut Value) -> Result<(), AppError> {
    if definition.kind == ParamKind::Number {
        normalize_integral_number(value);
        if !value.is_number() {
            return Err(AppError::validation(format!(
                "参数 {} 必须是数字",
                definition.label
            )));
        }
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
    Ok(())
}

fn normalize_presets(
    definitions: &[ParamDefinition],
    presets: Vec<GroupParamPreset>,
) -> Result<Vec<GroupParamPreset>, AppError> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(presets.len());
    for preset in presets {
        let name = preset.name.trim().to_owned();
        if name.is_empty() {
            return Err(AppError::validation("预设名称不能为空"));
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(AppError::conflict(format!("预设名称不能重复：{name}")));
        }
        let mut params = BTreeMap::new();
        for (key, mut value) in preset.params {
            let definition = definitions
                .iter()
                .find(|definition| definition.key == key)
                .ok_or_else(|| {
                    AppError::validation(format!("预设 {name} 引用了未知任务参数 {key}"))
                })?;
            check_param_kind(definition, &mut value)?;
            params.insert(key, value);
        }
        let id = preset.id.trim();
        let id = if id.is_empty() {
            format!("preset_{}", uuid::Uuid::new_v4().simple())
        } else {
            id.to_owned()
        };
        normalized.push(GroupParamPreset { id, name, params });
    }
    Ok(normalized)
}

fn normalize_hidden_params(
    definitions: &[ParamDefinition],
    hidden: Vec<String>,
) -> Result<Vec<String>, AppError> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for key in hidden {
        let key = key.trim().to_owned();
        if key.is_empty() {
            continue;
        }
        if !definitions.iter().any(|definition| definition.key == key) {
            return Err(AppError::validation(format!(
                "隐藏参数引用了未知任务参数 {key}"
            )));
        }
        if seen.insert(key.clone()) {
            normalized.push(key);
        }
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
    use crate::models::{AppSettings, GroupParamPreset, PackageTask, Project};

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

    #[test]
    fn changing_number_preserves_existing_select_value() {
        let definitions = vec![
            ParamDefinition {
                key: "minor_version".to_owned(),
                kind: ParamKind::Number,
                default_value: serde_json::json!(0),
                required: true,
                ..ParamDefinition::default()
            },
            ParamDefinition {
                key: "build_mode".to_owned(),
                kind: ParamKind::Select,
                options: vec!["release".to_owned(), "test".to_owned()],
                default_value: serde_json::json!("release"),
                required: true,
                ..ParamDefinition::default()
            },
        ];
        let params = BTreeMap::from([
            ("minor_version".to_owned(), serde_json::json!(7.0)),
            ("build_mode".to_owned(), serde_json::json!("test")),
        ]);

        let normalized = normalize_params(&definitions, params).expect("params should be valid");

        assert_eq!(normalized.get("minor_version"), Some(&serde_json::json!(7)));
        assert_eq!(
            normalized.get("build_mode"),
            Some(&serde_json::json!("test"))
        );
    }

    #[test]
    fn presets_validate_names_params_and_assign_ids() {
        let definitions = vec![
            ParamDefinition {
                key: "hot_update".to_owned(),
                kind: ParamKind::Switch,
                default_value: Value::Bool(false),
                ..ParamDefinition::default()
            },
            ParamDefinition {
                key: "env".to_owned(),
                kind: ParamKind::Select,
                options: vec!["dev".to_owned(), "pre".to_owned(), "release".to_owned()],
                default_value: Value::String("dev".to_owned()),
                ..ParamDefinition::default()
            },
            ParamDefinition {
                key: "count".to_owned(),
                kind: ParamKind::Number,
                default_value: serde_json::json!(0),
                ..ParamDefinition::default()
            },
        ];

        let presets = normalize_presets(
            &definitions,
            vec![GroupParamPreset {
                id: String::new(),
                name: "首提审包".to_owned(),
                params: BTreeMap::from([
                    ("hot_update".to_owned(), Value::Bool(false)),
                    ("env".to_owned(), Value::String("dev".to_owned())),
                    ("count".to_owned(), serde_json::json!(7.0)),
                ]),
            }],
        )
        .expect("partial preset should be valid");

        assert_eq!(presets.len(), 1);
        assert!(!presets[0].id.is_empty());
        assert_eq!(presets[0].name, "首提审包");
        assert_eq!(presets[0].params["count"], serde_json::json!(7));
        assert_eq!(presets[0].params["hot_update"], Value::Bool(false));
        assert_eq!(presets[0].params["env"], Value::String("dev".to_owned()));
    }

    #[test]
    fn presets_reject_unknown_params_duplicate_names_and_bad_kinds() {
        let definitions = vec![ParamDefinition {
            key: "env".to_owned(),
            kind: ParamKind::Select,
            options: vec!["dev".to_owned()],
            default_value: Value::String("dev".to_owned()),
            ..ParamDefinition::default()
        }];
        let unknown = normalize_presets(
            &definitions,
            vec![GroupParamPreset {
                name: "预设".to_owned(),
                params: BTreeMap::from([("nope".to_owned(), serde_json::json!(1))]),
                ..GroupParamPreset::default()
            }],
        );
        assert!(unknown.is_err());
        let bad_value = normalize_presets(
            &definitions,
            vec![GroupParamPreset {
                name: "预设".to_owned(),
                params: BTreeMap::from([("env".to_owned(), serde_json::json!("bad"))]),
                ..GroupParamPreset::default()
            }],
        );
        assert!(bad_value.is_err());
        let duplicated = normalize_presets(
            &definitions,
            vec![
                GroupParamPreset {
                    name: "线上".to_owned(),
                    ..GroupParamPreset::default()
                },
                GroupParamPreset {
                    name: "线上".to_owned(),
                    ..GroupParamPreset::default()
                },
            ],
        );
        assert!(duplicated.is_err());
        let empty_name = normalize_presets(
            &definitions,
            vec![GroupParamPreset {
                name: "  ".to_owned(),
                ..GroupParamPreset::default()
            }],
        );
        assert!(empty_name.is_err());
    }

    #[test]
    fn hidden_params_reject_unknown_keys_and_deduplicate() {
        let definitions = vec![ParamDefinition {
            key: "env".to_owned(),
            ..ParamDefinition::default()
        }];
        let unknown =
            normalize_hidden_params(&definitions, vec!["env".to_owned(), "nope".to_owned()]);
        assert!(unknown.is_err());

        let hidden = normalize_hidden_params(
            &definitions,
            vec![
                "env".to_owned(),
                " env ".to_owned(),
                "env".to_owned(),
                "".to_owned(),
            ],
        )
        .expect("valid hidden params");
        assert_eq!(hidden, vec!["env".to_owned()]);
    }

    #[tokio::test]
    async fn updating_group_keeps_omitted_presets_and_replaces_sent_ones() {
        let data_dir = std::env::temp_dir().join(format!(
            "cocos_build_group_presets_{}",
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
                    key: "env".to_owned(),
                    kind: ParamKind::Select,
                    options: vec!["dev".to_owned(), "pre".to_owned(), "release".to_owned()],
                    default_value: Value::String("dev".to_owned()),
                    ..ParamDefinition::default()
                }],
                task_groups: vec![TaskGroup {
                    id: "group_1".to_owned(),
                    project_id: "project_1".to_owned(),
                    name: "主分组".to_owned(),
                    branch: "main".to_owned(),
                    presets: vec![GroupParamPreset {
                        id: "preset_old".to_owned(),
                        name: "旧预设".to_owned(),
                        params: BTreeMap::new(),
                    }],
                    hidden_params: vec!["env".to_owned()],
                    ..TaskGroup::default()
                }],
                ..AppSettings::default()
            })
            .await
            .expect("settings");
        let app: Router = router().with_state(state.clone());

        let update_without_config = Request::builder()
            .method("PUT")
            .uri("/api/task-groups/group_1")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "projectId": "project_1",
                    "name": "主分组",
                    "description": "",
                    "branch": "release",
                    "params": {}
                })
                .to_string(),
            ))
            .unwrap();
        let response = app.clone().oneshot(update_without_config).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let group = state.get_settings().await.task_groups[0].clone();
        assert_eq!(group.presets[0].id, "preset_old");
        assert_eq!(group.hidden_params, vec!["env".to_owned()]);
        assert_eq!(group.branch, "release");

        let update_with_config = Request::builder()
            .method("PUT")
            .uri("/api/task-groups/group_1")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "projectId": "project_1",
                    "name": "主分组",
                    "description": "",
                    "branch": "release",
                    "params": {},
                    "presets": [{
                        "id": "",
                        "name": "线上热更新",
                        "params": { "env": "release" }
                    }],
                    "hiddenParams": ["env"]
                })
                .to_string(),
            ))
            .unwrap();
        let response = app.oneshot(update_with_config).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let group = state.get_settings().await.task_groups[0].clone();
        assert_eq!(group.presets.len(), 1);
        assert_eq!(group.presets[0].name, "线上热更新");
        assert!(!group.presets[0].id.is_empty());
        assert_eq!(
            group.presets[0].params["env"],
            Value::String("release".to_owned())
        );

        let _ = tokio::fs::remove_dir_all(data_dir).await;
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
                    presets: Vec::new(),
                    hidden_params: Vec::new(),
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
