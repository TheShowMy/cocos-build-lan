use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};

use crate::{
    error::AppError,
    models::{
        BuildStartRequest, BuildStartResponse, BuildStatusResponse, BuildStopRequest,
        BuildStopResponse,
    },
    services,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/build/start", post(start_build))
        .route("/api/build/stop", post(stop_build))
        .route("/api/build/status", get(get_build_status))
}

async fn stop_build(
    State(state): State<AppState>,
    Json(payload): Json<BuildStopRequest>,
) -> Result<Json<BuildStopResponse>, AppError> {
    state.cancel_active_build(&payload.task_id).await?;
    Ok(Json(BuildStopResponse {
        message: "已请求终止当前构建，剩余队列将标记为已取消".to_owned(),
    }))
}

async fn start_build(
    State(state): State<AppState>,
    Json(payload): Json<BuildStartRequest>,
) -> Result<Json<BuildStartResponse>, AppError> {
    services::build::start_build(state, payload).await?;

    Ok(Json(BuildStartResponse {
        message: "打包任务已开始".to_string(),
    }))
}

async fn get_build_status(
    State(state): State<AppState>,
) -> Result<Json<BuildStatusResponse>, AppError> {
    Ok(Json(services::build::get_build_status(&state).await))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::models::{AppSettings, PackageTask};

    fn temp_data_dir(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        std::env::temp_dir().join(format!("cocos_build_route_test_{name}_{unique}"))
    }

    async fn app_with_single_task() -> (Router, AppState, std::path::PathBuf) {
        let data_dir = temp_data_dir("build");
        tokio::fs::create_dir_all(&data_dir)
            .await
            .expect("create temp data dir");
        let state = AppState::load(data_dir.clone()).await;
        state
            .save_settings(AppSettings {
                package_tasks: vec![PackageTask {
                    id: "task_1".to_string(),
                    name: "demo".to_string(),
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
    async fn get_build_status_should_only_return_runtime_fields() {
        let (app, state, data_dir) = app_with_single_task().await;
        state
            .update_task_runtime(
                "task_1",
                crate::state::RuntimeFlushMode::Immediate,
                |runtime| {
                    runtime.progress = 55;
                    runtime.status = crate::models::PackageTaskStatus::Running;
                    runtime.last_log_path = Some("/tmp/build.log".to_string());
                },
            )
            .await
            .expect("update runtime");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/build/status")
                    .method("GET")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("execute request");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let value: Value = serde_json::from_slice(&body).expect("parse body");
        assert_eq!(
            value,
            json!({
                "buildInProgress": false, "activeTaskId": null, "queuedTaskIds": [], "cancelRequested": false,
                "packageTasks": [
                    {
                        "taskId": "task_1",
                        "progress": 55,
                        "stepLabel": "",
                        "status": "running",
                        "lastError": null,
                        "startedAt": null,
                        "finishedAt": null
                    }
                ]
            })
        );

        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn start_build_should_return_conflict_when_build_already_running() {
        let (app, state, data_dir) = app_with_single_task().await;
        state.try_start_build().await.expect("lock build");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/build/start")
                    .method("POST")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&BuildStartRequest {
                            task_ids: vec!["task_1".to_string()],
                        })
                        .expect("serialize payload"),
                    ))
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
            "已有打包任务在进行中"
        );

        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }
    #[tokio::test]
    async fn queue_status_tracks_waiting_active_cancel_and_cleanup() {
        let (_app, state, data_dir) = app_with_single_task().await;
        state.try_start_build().await.unwrap();
        state
            .prepare_tasks_for_build(&["task_1".into()])
            .await
            .unwrap();
        let waiting = state.get_build_status().await;
        assert!(waiting.build_in_progress);
        assert_eq!(waiting.queued_task_ids, vec!["task_1"]);
        state.set_active_task(Some("task_1".into())).await;
        assert!(state.get_build_status().await.queued_task_ids.is_empty());
        state.cancel_active_build("task_1").await.unwrap();
        assert!(state.get_build_status().await.cancel_requested);
        state.finish_build().await;
        let finished = state.get_build_status().await;
        assert!(!finished.build_in_progress);
        assert!(!finished.cancel_requested);
        assert!(finished.active_task_id.is_none());
        assert!(finished.queued_task_ids.is_empty());
        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn cancellation_drops_work_even_outside_the_cocos_phase() {
        let (_app, state, data_dir) = app_with_single_task().await;
        state.try_start_build().await.unwrap();
        state.set_active_task(Some("task_1".into())).await;
        let (started, ready) = tokio::sync::oneshot::channel();
        let (dropped, drop_result) = tokio::sync::oneshot::channel::<()>();
        let task_state = state.clone();
        let worker = tokio::spawn(async move {
            crate::services::build::cancel_on_request(&task_state, async move {
                let _drop_guard = dropped;
                let _ = started.send(());
                std::future::pending::<Result<(), AppError>>().await
            })
            .await
        });
        ready.await.unwrap();
        state.cancel_active_build("task_1").await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, Err(AppError::Canceled(_))));
        assert!(drop_result.await.is_err());
        state.finish_build().await;
        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }
}
