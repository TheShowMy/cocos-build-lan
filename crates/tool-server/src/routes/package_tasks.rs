use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{post, put},
};

use crate::{
    error::AppError,
    models::{
        PackageTask, PackageTaskReorderRequest, PackageTaskRequest, PackageTaskStatus,
        TaskProjectConfig,
    },
    services,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/package-tasks", post(create_task))
        .route("/api/package-tasks/reorder", put(reorder_tasks))
        .route(
            "/api/package-tasks/{task_id}",
            put(update_task).delete(delete_task),
        )
        .route(
            "/api/package-tasks/{task_id}/duplicate",
            post(duplicate_task),
        )
        .route(
            "/api/package-tasks/{task_id}/run-obfuscation",
            post(run_obfuscation),
        )
        .route(
            "/api/package-tasks/{task_id}/cleanup-private-repos",
            post(cleanup_private_repos),
        )
}

async fn create_task(
    State(state): State<AppState>,
    Json(request): Json<PackageTaskRequest>,
) -> Result<Json<PackageTask>, AppError> {
    validate_task_request(&state, &request, None).await?;
    let mut settings = state.get_settings().await;
    let group = settings
        .task_groups
        .iter()
        .find(|group| group.id == request.task_group_id)
        .cloned()
        .ok_or_else(|| AppError::not_found("任务组不存在"))?;
    let order = settings
        .package_tasks
        .iter()
        .filter(|task| task.task_group_id == group.id)
        .map(|task| task.order)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let task = task_from_request(
        format!("task_{}", uuid::Uuid::new_v4().simple()),
        order,
        &group,
        request,
    );
    services::build::validate_package_tasks(&state, std::slice::from_ref(&task)).await?;
    settings.package_tasks.push(task.clone());
    state.save_settings(settings).await?;
    Ok(Json(task))
}

async fn update_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(request): Json<PackageTaskRequest>,
) -> Result<Json<PackageTask>, AppError> {
    validate_task_request(&state, &request, Some(&task_id)).await?;
    let runtime = state.get_task_runtime(&task_id).await?;
    if runtime.status == PackageTaskStatus::Running
        || runtime.status == PackageTaskStatus::Canceling
    {
        return Err(AppError::conflict("任务正在执行中，无法修改"));
    }
    let mut settings = state.get_settings().await;
    let current = settings
        .package_tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| AppError::not_found(format!("未找到任务 {task_id}")))?;
    let group = settings
        .task_groups
        .iter()
        .find(|group| group.id == request.task_group_id)
        .cloned()
        .ok_or_else(|| AppError::not_found("任务组不存在"))?;
    let order = if current.task_group_id == group.id {
        current.order
    } else {
        settings
            .package_tasks
            .iter()
            .filter(|task| task.task_group_id == group.id)
            .map(|task| task.order)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    };
    let task = task_from_request(task_id.clone(), order, &group, request);
    services::build::validate_package_tasks(&state, std::slice::from_ref(&task)).await?;
    let slot = settings
        .package_tasks
        .iter_mut()
        .find(|item| item.id == task_id)
        .expect("任务已检查存在");
    *slot = task.clone();
    state.save_settings(settings).await?;
    Ok(Json(task))
}

async fn reorder_tasks(
    State(state): State<AppState>,
    Json(request): Json<PackageTaskReorderRequest>,
) -> Result<Json<Vec<PackageTask>>, AppError> {
    state.find_task_group(request.task_group_id.trim()).await?;
    let mut settings = state.get_settings().await;
    let group_task_ids = settings
        .package_tasks
        .iter()
        .filter(|task| task.task_group_id == request.task_group_id)
        .map(|task| task.id.clone())
        .collect::<std::collections::HashSet<_>>();
    let requested = request
        .task_ids
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    if requested.len() != request.task_ids.len() || requested != group_task_ids {
        return Err(AppError::validation(
            "排序列表必须且只能包含任务组内的全部任务",
        ));
    }
    for (order, task_id) in request.task_ids.iter().enumerate() {
        let task = settings
            .package_tasks
            .iter_mut()
            .find(|task| task.id == *task_id)
            .expect("排序任务已校验");
        task.order = order as u32;
    }
    let mut result = settings
        .package_tasks
        .iter()
        .filter(|task| task.task_group_id == request.task_group_id)
        .cloned()
        .collect::<Vec<_>>();
    result.sort_by_key(|task| task.order);
    state.save_settings(settings).await?;
    Ok(Json(result))
}

async fn validate_task_request(
    state: &AppState,
    request: &PackageTaskRequest,
    current_task_id: Option<&str>,
) -> Result<(), AppError> {
    if request.name.trim().is_empty() {
        return Err(AppError::validation("任务名称不能为空"));
    }
    state.find_task_group(request.task_group_id.trim()).await?;
    serde_json::from_str::<serde_json::Value>(&request.build_args_json)
        .map_err(|error| AppError::validation(format!("构建参数 JSON 无效: {error}")))?;
    if request.enable_dead_code_injection && request.dead_code_injection_count == 0 {
        return Err(AppError::validation("死代码注入数量必须大于 0"));
    }
    let duplicate = state
        .get_settings()
        .await
        .package_tasks
        .into_iter()
        .any(|task| {
            Some(task.id.as_str()) != current_task_id
                && task.task_group_id == request.task_group_id
                && task.name.eq_ignore_ascii_case(request.name.trim())
        });
    if duplicate {
        return Err(AppError::conflict("同一任务组内的任务名称不能重复"));
    }
    Ok(())
}

fn task_from_request(
    id: String,
    order: u32,
    group: &crate::models::TaskGroup,
    request: PackageTaskRequest,
) -> PackageTask {
    PackageTask {
        id,
        name: request.name.trim().to_owned(),
        group: group.name.clone(),
        task_group_id: group.id.clone(),
        order,
        project: Some(TaskProjectConfig {
            project_id: group.project_id.clone(),
            branch: group.branch.clone(),
        }),
        code_repo_url: request.code_repo_url.trim().to_owned(),
        asset_repo_url: request.asset_repo_url.trim().to_owned(),
        build_args_json: request.build_args_json,
        enable_obfuscation: request.enable_obfuscation,
        obfuscation_mode: request.obfuscation_mode,
        obfuscation_seed: request.obfuscation_seed,
        enable_dead_code_injection: request.enable_dead_code_injection,
        dead_code_injection_count: request.dead_code_injection_count,
        pre_build_actions: request.pre_build_actions,
        post_build_actions: request.post_build_actions,
        ..PackageTask::default()
    }
}

async fn duplicate_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<axum::Json<crate::models::PackageTask>, AppError> {
    let mut settings = state.get_settings().await;
    let source = settings
        .package_tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| AppError::not_found(format!("未找到任务 {task_id}")))?;
    let mut duplicate = source;
    duplicate.id = format!("task_{}", uuid::Uuid::new_v4().simple());
    duplicate.name = format!("{}副本", duplicate.name);
    duplicate.progress = 0;
    duplicate.status = crate::models::PackageTaskStatus::Pending;
    duplicate.last_log_path = None;
    duplicate.last_error = None;
    duplicate.started_at = None;
    duplicate.finished_at = None;
    duplicate.order = settings
        .package_tasks
        .iter()
        .filter(|task| task.task_group_id == duplicate.task_group_id)
        .map(|task| task.order)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    settings.package_tasks.push(duplicate.clone());
    state.save_settings(settings).await?;
    Ok(axum::Json(duplicate))
}

async fn delete_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<axum::http::StatusCode, AppError> {
    services::package_tasks::delete_task(&state, &task_id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn cleanup_private_repos(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<axum::Json<crate::models::TaskPrivateReposCleanupResponse>, AppError> {
    let result = services::package_tasks::cleanup_task_private_repos(&state, &task_id).await?;
    Ok(axum::Json(result))
}

async fn run_obfuscation(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<axum::http::StatusCode, AppError> {
    services::package_tasks::start_task_obfuscation(state, task_id).await?;
    Ok(axum::http::StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::*;
    use crate::{
        models::{
            AppSettings, Engine, PackageTask, PackageTaskStatus, Project, TaskGroup,
            TaskProjectConfig,
        },
        state::RuntimeFlushMode,
    };

    static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_data_dir(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("cocos_build_route_test_{name}_{unique}_{counter}"))
    }

    async fn app_with_task() -> (Router, AppState, std::path::PathBuf) {
        let data_dir = temp_data_dir("package_task_delete");
        tokio::fs::create_dir_all(&data_dir)
            .await
            .expect("create temp data dir");
        let state = AppState::load(data_dir.clone()).await;
        state
            .save_settings(AppSettings {
                engines: vec![Engine {
                    name: "cocos".to_string(),
                    path: "/engine".to_string(),
                }],
                projects: vec![Project {
                    id: "project_1".to_string(),
                    workspace_dir_key: "workspace_1".to_string(),
                    name: "演示项目".to_string(),
                    git_url: "https://example.com/demo.git".to_string(),
                    engine_name: "cocos".to_string(),
                    ..Project::default()
                }],
                package_tasks: vec![PackageTask {
                    id: "task_1".to_string(),
                    name: "任务1".to_string(),
                    group: "主组".to_string(),
                    task_group_id: "group_1".to_string(),
                    project: Some(TaskProjectConfig {
                        project_id: "演示项目".to_string(),
                        branch: "main".to_string(),
                    }),
                    build_args_json: "{}".to_string(),
                    ..PackageTask::default()
                }],
                task_groups: vec![TaskGroup {
                    id: "group_1".to_string(),
                    project_id: "project_1".to_string(),
                    name: "主组".to_string(),
                    branch: "main".to_string(),
                    ..TaskGroup::default()
                }],
                ..AppSettings::default()
            })
            .await
            .expect("save settings");
        (router().with_state(state.clone()), state, data_dir)
    }

    #[tokio::test]
    async fn delete_task_should_remove_settings_runtime_and_task_dir() {
        let (app, state, data_dir) = app_with_task().await;
        let project = state.find_project("演示项目").await.expect("find project");
        let task_dir = state
            .workspace_project_dir(&project)
            .join("tasks")
            .join("task_1");
        tokio::fs::create_dir_all(task_dir.join("temp"))
            .await
            .expect("create task dir");
        state
            .update_task_runtime("task_1", RuntimeFlushMode::Immediate, |runtime| {
                runtime.status = PackageTaskStatus::Success;
            })
            .await
            .expect("update runtime");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/package-tasks/task_1")
                    .method("DELETE")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("execute request");

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(!task_dir.exists());

        let settings_text = tokio::fs::read_to_string(data_dir.join("settings.json"))
            .await
            .expect("read settings file");
        assert!(!settings_text.contains("\"task_1\""));

        let runtime_text = tokio::fs::read_to_string(data_dir.join("runtime_state.json"))
            .await
            .expect("read runtime file");
        assert!(!runtime_text.contains("\"task_1\""));

        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn delete_task_should_reject_running_task() {
        let (app, state, data_dir) = app_with_task().await;
        state
            .update_task_runtime("task_1", RuntimeFlushMode::Immediate, |runtime| {
                runtime.status = PackageTaskStatus::Running;
            })
            .await
            .expect("update runtime");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/package-tasks/task_1")
                    .method("DELETE")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("execute request");

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        assert_eq!(
            String::from_utf8(body.to_vec()).expect("utf8 body"),
            "任务正在执行中，无法删除"
        );

        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn delete_task_should_allow_missing_task_dir() {
        let (app, _state, data_dir) = app_with_task().await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/package-tasks/task_1")
                    .method("DELETE")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("execute request");

        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let settings_text = tokio::fs::read_to_string(data_dir.join("settings.json"))
            .await
            .expect("read settings file");
        assert!(!settings_text.contains("\"task_1\""));

        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn run_obfuscation_should_reject_task_without_obfuscation_enabled() {
        let (app, _state, data_dir) = app_with_task().await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/package-tasks/task_1/run-obfuscation")
                    .method("POST")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("execute request");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        assert_eq!(
            String::from_utf8(body.to_vec()).expect("utf8 body"),
            "任务未开启混淆，无法单独执行混淆"
        );

        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn delete_task_without_project_should_only_remove_settings_entry() {
        let (app, state, data_dir) = app_with_task().await;
        let mut settings = state.get_settings().await;
        settings.package_tasks[0].project = None;
        state
            .save_settings(settings)
            .await
            .expect("save settings without project");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/package-tasks/task_1")
                    .method("DELETE")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("execute request");

        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let settings_text = tokio::fs::read_to_string(data_dir.join("settings.json"))
            .await
            .expect("read settings file");
        assert!(!settings_text.contains("\"task_1\""));

        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn create_update_and_reorder_tasks() {
        let (app, state, data_dir) = app_with_task().await;
        let create = serde_json::json!({
            "name": "任务2",
            "taskGroupId": "group_1",
            "buildArgsJson": "{}"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/package-tasks")
                    .method("POST")
                    .header("content-type", "application/json")
                    .body(Body::from(create.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let created: PackageTask = serde_json::from_slice(&body).unwrap();

        let update = serde_json::json!({
            "name": "任务2修改",
            "taskGroupId": "group_1",
            "buildArgsJson": "{\"platform\":\"web-mobile\"}"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/package-tasks/{}", created.id))
                    .method("PUT")
                    .header("content-type", "application/json")
                    .body(Body::from(update.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let reorder = serde_json::json!({
            "taskGroupId": "group_1",
            "taskIds": [created.id, "task_1"]
        });
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/package-tasks/reorder")
                    .method("PUT")
                    .header("content-type", "application/json")
                    .body(Body::from(reorder.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut tasks = state.get_settings().await.package_tasks;
        tasks.sort_by_key(|task| task.order);
        assert_eq!(tasks[0].name, "任务2修改");
        assert_eq!(tasks[1].id, "task_1");

        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }
}
