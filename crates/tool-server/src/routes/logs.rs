use std::path::{Path, PathBuf};

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::{delete, get},
};
use serde::Deserialize;
use tokio::fs;

use crate::{error::AppError, models::LogFileInfo, state::AppState};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogPathQuery {
    path: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/logs", get(get_logs).delete(clear_logs))
        .route("/api/log-content", get(read_log_content))
        .route("/api/log", delete(delete_log))
}

async fn get_logs(State(state): State<AppState>) -> Result<Json<Vec<LogFileInfo>>, AppError> {
    let log_dir = state.logs_dir();
    if !log_dir.exists() {
        return Ok(Json(Vec::new()));
    }

    let mut entries = fs::read_dir(&log_dir).await.map_err(AppError::from)?;
    let mut logs = Vec::new();

    while let Some(entry) = entries.next_entry().await.map_err(AppError::from)? {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let metadata = entry.metadata().await.map_err(AppError::from)?;
        let created = metadata
            .created()
            .or_else(|_| metadata.modified())
            .unwrap_or(std::time::SystemTime::now());
        let created_at = chrono::DateTime::<chrono::Local>::from(created)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();

        logs.push(LogFileInfo {
            name: entry.file_name().to_string_lossy().to_string(),
            path: path.to_string_lossy().to_string(),
            size: metadata.len(),
            created_at,
        });
    }

    logs.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(Json(logs))
}

async fn read_log_content(
    State(state): State<AppState>,
    Query(query): Query<LogPathQuery>,
) -> Result<String, AppError> {
    let path = ensure_log_path(state.data_dir(), &query.path).await?;
    Ok(fs::read_to_string(path).await?)
}

async fn delete_log(
    State(state): State<AppState>,
    Query(query): Query<LogPathQuery>,
) -> Result<axum::http::StatusCode, AppError> {
    let path = ensure_log_path(state.data_dir(), &query.path).await?;
    fs::remove_file(path).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn clear_logs(State(state): State<AppState>) -> Result<axum::http::StatusCode, AppError> {
    let log_dir = state.logs_dir();
    if log_dir.exists() {
        let mut entries = fs::read_dir(&log_dir).await?;
        while let Some(entry) = entries.next_entry().await.map_err(AppError::from)? {
            let path = entry.path();
            if path.is_file() {
                fs::remove_file(path).await?;
            }
        }
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn ensure_log_path(data_dir: &Path, raw_path: &str) -> Result<PathBuf, AppError> {
    let log_dir = data_dir.join("logs");
    if !log_dir.exists() {
        return Err(AppError::not_found("日志目录不存在"));
    }

    let canonical_log_dir = fs::canonicalize(&log_dir).await?;
    let target_path = PathBuf::from(raw_path);
    let canonical_target = fs::canonicalize(&target_path)
        .await
        .map_err(|_| AppError::not_found("日志文件不存在"))?;

    if !canonical_target.starts_with(&canonical_log_dir) {
        return Err(AppError::forbidden("非法的日志路径"));
    }

    Ok(canonical_target)
}
