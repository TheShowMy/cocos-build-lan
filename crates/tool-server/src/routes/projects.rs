use axum::{
    Json, Router,
    extract::{Query, State},
    routing::{get, post},
};
use serde::Deserialize;

use crate::{
    error::AppError,
    git,
    models::{
        ProjectBranchesResponse, ProjectWorkspaceInitializeRequest, ProjectWorkspaceStatus,
        ProjectWorktreeCleanupRequest, ProjectWorktreeCleanupResponse,
    },
    services,
    state::AppState,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectBranchesQuery {
    #[serde(alias = "projectName")]
    project_id: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/project-branches", get(get_project_branches))
        .route(
            "/api/projects/init-statuses",
            get(list_project_init_statuses),
        )
        .route("/api/projects/initialize", post(initialize_project))
        .route(
            "/api/projects/cleanup-worktree",
            post(cleanup_project_worktree),
        )
}

async fn get_project_branches(
    State(state): State<AppState>,
    Query(query): Query<ProjectBranchesQuery>,
) -> Result<Json<ProjectBranchesResponse>, AppError> {
    let settings = state.get_settings().await;
    let project = state.find_project(&query.project_id).await?;

    let branches = git::list_remote_branches(&project.git_url, &settings.git_config).await?;

    Ok(Json(ProjectBranchesResponse {
        project_id: project.id,
        project_name: project.name,
        branches,
    }))
}

async fn list_project_init_statuses(
    State(state): State<AppState>,
) -> Result<Json<Vec<ProjectWorkspaceStatus>>, AppError> {
    let statuses = services::projects::list_project_workspace_statuses(&state).await?;
    Ok(Json(statuses))
}

async fn initialize_project(
    State(state): State<AppState>,
    Json(payload): Json<ProjectWorkspaceInitializeRequest>,
) -> Result<Json<ProjectWorkspaceStatus>, AppError> {
    let status =
        services::projects::initialize_project_workspace(&state, &payload.project_id).await?;
    Ok(Json(status))
}

async fn cleanup_project_worktree(
    State(state): State<AppState>,
    Json(payload): Json<ProjectWorktreeCleanupRequest>,
) -> Result<Json<ProjectWorktreeCleanupResponse>, AppError> {
    let result = services::projects::cleanup_project_worktree(&state, &payload.project_id).await?;
    Ok(Json(result))
}
