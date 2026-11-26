use axum::{Json, Router, extract::State, routing::get};

use crate::{
    error::AppError,
    models::{PublicSettings, PublicSettingsUpdate},
    services,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/parameters/placeholders", get(get_placeholders))
        .route("/api/settings", get(get_settings).put(save_settings))
}

async fn get_placeholders(
    State(state): State<AppState>,
) -> Json<Vec<crate::models::PlaceholderInfo>> {
    Json(services::parameters::placeholder_catalog(
        &state.get_settings().await.param_definitions,
    ))
}

async fn get_settings(State(state): State<AppState>) -> Result<Json<PublicSettings>, AppError> {
    Ok(Json(PublicSettings::from(&state.get_settings().await)))
}

async fn save_settings(
    State(state): State<AppState>,
    Json(payload): Json<PublicSettingsUpdate>,
) -> Result<Json<PublicSettings>, AppError> {
    let settings = services::settings::save_public_settings(&state, payload).await?;
    Ok(Json(PublicSettings::from(&settings)))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::*;
    use crate::models::{AppSettings, GitConfig, PackageTask};

    #[tokio::test]
    async fn public_settings_reject_engine_and_credential_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::load(dir.path().to_path_buf()).await;
        for field in ["engines", "gitConfig", "workspaceRoot"] {
            let payload = serde_json::json!({field: []});
            let response = router()
                .with_state(state.clone())
                .oneshot(
                    Request::builder()
                        .uri("/api/settings")
                        .method("PUT")
                        .header("content-type", "application/json")
                        .body(Body::from(payload.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }
    }

    #[tokio::test]
    async fn public_settings_never_expose_git_credentials() {
        let data_dir = std::env::temp_dir().join(format!(
            "cocos_build_public_settings_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let state = AppState::load(data_dir.clone()).await;
        state
            .set_git_config(GitConfig {
                username: "builder-bot".to_owned(),
                password: "secret-token".to_owned(),
            })
            .await
            .expect("set credential");
        state
            .save_settings(AppSettings::default())
            .await
            .expect("save settings");
        let app: Router = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/settings")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let text = String::from_utf8(body.to_vec()).expect("utf8");
        assert!(text.contains("gitCredentialsConfigured"));
        assert!(!text.contains("builder-bot"));
        assert!(!text.contains("secret-token"));
        assert!(
            !std::fs::read_to_string(data_dir.join("settings.json"))
                .expect("settings file")
                .contains("secret-token")
        );

        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn public_settings_update_preserves_tasks_and_git_credentials() {
        let data_dir = std::env::temp_dir().join(format!(
            "cocos_build_public_settings_update_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let state = AppState::load(data_dir.clone()).await;
        state
            .save_settings(AppSettings {
                package_tasks: vec![PackageTask {
                    id: "task_1".to_owned(),
                    name: "保留任务".to_owned(),
                    build_args_json: "{}".to_owned(),
                    ..PackageTask::default()
                }],
                ..AppSettings::default()
            })
            .await
            .unwrap();
        state
            .set_git_config(GitConfig {
                username: "builder".to_owned(),
                password: "secret".to_owned(),
            })
            .await
            .unwrap();
        let app = router().with_state(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/settings")
                    .method("PUT")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "projects": [],
                            "feishuBots": [],
                            "paramDefinitions": [services::parameters::builtin_version()]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let settings = state.get_settings().await;
        assert_eq!(settings.package_tasks.len(), 1);
        assert_eq!(settings.git_config.username, "builder");
        assert_eq!(settings.git_config.password, "secret");

        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }
}
