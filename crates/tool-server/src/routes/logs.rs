use std::path::{Path, PathBuf};

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::{delete, get},
};
use serde::Deserialize;
use tokio::fs;

use crate::{
    error::AppError,
    models::{LogFileInfo, LogPageResponse},
    state::AppState,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogPathQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogListQuery {
    #[serde(default = "default_page")]
    page: usize,
    #[serde(default = "default_page_size")]
    page_size: usize,
    #[serde(default)]
    query: String,
    #[serde(default = "default_sort_by")]
    sort_by: String,
    #[serde(default = "default_sort_order")]
    sort_order: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/logs", get(get_logs).delete(clear_logs))
        .route("/api/log-content", get(read_log_content))
        .route("/api/log", delete(delete_log))
}

async fn get_logs(
    State(state): State<AppState>,
    Query(query): Query<LogListQuery>,
) -> Result<Json<LogPageResponse>, AppError> {
    if query.page == 0 || !(1..=100).contains(&query.page_size) {
        return Err(AppError::validation(
            "日志页码必须大于 0，pageSize 必须在 1 到 100 之间",
        ));
    }
    let log_dir = state.logs_dir();
    if !log_dir.exists() {
        return Ok(Json(LogPageResponse {
            items: Vec::new(),
            total: 0,
            page: query.page,
            page_size: query.page_size,
        }));
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

    let needle = query.query.trim().to_lowercase();
    if !needle.is_empty() {
        logs.retain(|log| log.name.to_lowercase().contains(&needle));
    }
    match query.sort_by.as_str() {
        "name" => logs.sort_by(|left, right| left.name.cmp(&right.name)),
        "size" => logs.sort_by_key(|log| log.size),
        "createdAt" => logs.sort_by(|left, right| left.created_at.cmp(&right.created_at)),
        _ => {
            return Err(AppError::validation(
                "sortBy 只支持 name、createdAt 或 size",
            ));
        }
    }
    match query.sort_order.as_str() {
        "asc" => {}
        "desc" => logs.reverse(),
        _ => return Err(AppError::validation("sortOrder 只支持 asc 或 desc")),
    }
    let total = logs.len();
    let start = (query.page - 1).saturating_mul(query.page_size);
    let items = logs.into_iter().skip(start).take(query.page_size).collect();
    Ok(Json(LogPageResponse {
        items,
        total,
        page: query.page,
        page_size: query.page_size,
    }))
}

fn default_page() -> usize {
    1
}

fn default_page_size() -> usize {
    20
}

fn default_sort_by() -> String {
    "createdAt".to_owned()
}

fn default_sort_order() -> String {
    "desc".to_owned()
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

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn log_list_supports_filter_sort_and_pagination() {
        let data_dir = std::env::temp_dir().join(format!(
            "cocos_build_log_page_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let log_dir = data_dir.join("logs");
        tokio::fs::create_dir_all(&log_dir).await.unwrap();
        tokio::fs::write(log_dir.join("build_a.log"), "a")
            .await
            .unwrap();
        tokio::fs::write(log_dir.join("build_b.log"), "bbbb")
            .await
            .unwrap();
        tokio::fs::write(log_dir.join("other.log"), "other")
            .await
            .unwrap();
        let state = AppState::load(data_dir.clone()).await;
        let app = router().with_state(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/logs?page=1&pageSize=1&query=build&sortBy=name&sortOrder=asc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["total"], 2);
        assert_eq!(value["pageSize"], 1);
        assert_eq!(value["items"][0]["name"], "build_a.log");

        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }
}
