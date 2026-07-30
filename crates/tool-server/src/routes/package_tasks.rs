use axum::{
    Router,
    extract::{Path, State},
    routing::{delete, post},
};

use crate::{error::AppError, services, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/package-tasks/{task_id}", delete(delete_task))
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
        models::{AppSettings, Engine, PackageTask, PackageTaskStatus, Project, TaskProjectConfig},
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
                    name: "演示项目".to_string(),
                    git_url: "https://example.com/demo.git".to_string(),
                    engine_name: "cocos".to_string(),
                    ..Project::default()
                }],
                package_tasks: vec![PackageTask {
                    id: "task_1".to_string(),
                    name: "任务1".to_string(),
                    project: Some(TaskProjectConfig {
                        project_id: "演示项目".to_string(),
                        branch: "main".to_string(),
                    }),
                    build_args_json: "{}".to_string(),
                    ..PackageTask::default()
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
}
