use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post, put},
};

use crate::{
    error::AppError,
    models::{
        CreatePrepProjectRequest, PrepProject, PrepProjectExportPayload, PrepProjectImportRequest,
        PrepProjectRunForTasksRequest, PrepProjectRunForTasksResponse, PrepProjectRunRequest,
        PrepProjectRunResponse, UpdatePrepProjectRequest,
    },
    services,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/prep-projects",
            get(list_prep_projects).post(create_prep_project),
        )
        .route(
            "/api/prep-projects/{id}",
            put(update_prep_project).delete(delete_prep_project),
        )
        .route("/api/prep-projects/{id}/export", get(export_prep_project))
        .route("/api/prep-projects/{id}/run", post(run_prep_project))
        .route(
            "/api/prep-projects/{id}/run-for-tasks",
            post(run_prep_project_for_tasks),
        )
        .route("/api/prep-projects/import", post(import_prep_project))
}

async fn list_prep_projects(
    State(state): State<AppState>,
) -> Result<Json<Vec<PrepProject>>, AppError> {
    let projects = services::preps::list_prep_projects(&state).await?;
    Ok(Json(projects))
}

async fn create_prep_project(
    State(state): State<AppState>,
    Json(payload): Json<CreatePrepProjectRequest>,
) -> Result<Json<PrepProject>, AppError> {
    let project = services::preps::create_prep_project(&state, payload).await?;
    Ok(Json(project))
}

async fn update_prep_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdatePrepProjectRequest>,
) -> Result<Json<PrepProject>, AppError> {
    let project = services::preps::update_prep_project(&state, &id, payload).await?;
    Ok(Json(project))
}

async fn delete_prep_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode, AppError> {
    services::preps::delete_prep_project(&state, &id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn run_prep_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<PrepProjectRunRequest>,
) -> Result<Json<PrepProjectRunResponse>, AppError> {
    let result = services::preps::run_prep_project_for_manual(&state, &id, payload).await?;
    Ok(Json(result))
}

async fn run_prep_project_for_tasks(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<PrepProjectRunForTasksRequest>,
) -> Result<Json<PrepProjectRunForTasksResponse>, AppError> {
    let result = services::preps::run_prep_project_for_tasks(&state, &id, payload).await?;
    Ok(Json(result))
}

async fn export_prep_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PrepProjectExportPayload>, AppError> {
    let payload = services::preps::export_prep_project(&state, &id).await?;
    Ok(Json(payload))
}

async fn import_prep_project(
    State(state): State<AppState>,
    Json(payload): Json<PrepProjectImportRequest>,
) -> Result<Json<PrepProject>, AppError> {
    let project = services::preps::import_prep_project(&state, payload).await?;
    Ok(Json(project))
}
