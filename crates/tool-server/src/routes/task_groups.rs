use std::collections::{BTreeMap, HashSet};

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post, put},
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
            "/api/task-groups/{group_id}/schemes/manage",
            put(manage_schemes),
        )
        .route(
            "/api/task-groups/{group_id}/schemes/{scheme_id}/tasks",
            put(save_scheme_tasks),
        )
        .route("/api/task-groups/{group_id}/schemes", post(create_scheme))
        .route(
            "/api/task-groups/{group_id}/schemes/{scheme_id}/name",
            put(rename_scheme),
        )
        .route(
            "/api/task-groups/{group_id}/schemes/{scheme_id}",
            put(save_scheme).delete(delete_scheme),
        )
        .route(
            "/api/task-groups/{group_id}/schemes/{scheme_id}/apply",
            post(apply_scheme),
        )
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

fn scheme_group<'a>(
    settings: &'a mut crate::models::AppSettings,
    id: &str,
) -> Result<&'a mut TaskGroup, AppError> {
    settings
        .task_groups
        .iter_mut()
        .find(|g| g.id == id)
        .ok_or_else(|| AppError::not_found("任务组不存在"))
}

fn prepare_scheme(
    settings: &crate::models::AppSettings,
    group_id: &str,
    mut scheme: crate::models::BuildScheme,
    current_id: Option<&str>,
) -> Result<crate::models::BuildScheme, AppError> {
    let group = settings
        .task_groups
        .iter()
        .find(|g| g.id == group_id)
        .ok_or_else(|| AppError::not_found("任务组不存在"))?;
    let schemes = group.schemes.as_deref().unwrap_or_default();
    if current_id.is_none() && schemes.len() >= 5 {
        return Err(AppError::validation("每个任务组最多保存 5 个构建方案"));
    }
    if let Some(id) = current_id {
        if !schemes.iter().any(|s| s.id == id) {
            return Err(AppError::not_found("构建方案不存在"));
        }
        scheme.id = id.to_owned();
    } else {
        scheme.id = format!("scheme_{}", uuid::Uuid::new_v4().simple());
    }
    scheme.name = scheme.name.trim().to_owned();
    if scheme.name.is_empty() {
        return Err(AppError::validation("方案名称不能为空"));
    }
    if schemes.iter().any(|s| {
        Some(s.id.as_str()) != current_id && s.name.to_lowercase() == scheme.name.to_lowercase()
    }) {
        return Err(AppError::validation("方案名称不能重复（忽略大小写）"));
    }
    scheme.branch = scheme.branch.trim().to_owned();
    if scheme.branch.is_empty() {
        return Err(AppError::validation("分支不能为空"));
    }
    scheme.params = normalize_params(&settings.param_definitions, scheme.params)?;
    let mut seen = HashSet::new();
    for id in &scheme.task_ids {
        if !settings
            .package_tasks
            .iter()
            .any(|task| task.id == *id && task.task_group_id == group_id)
        {
            return Err(AppError::validation(format!(
                "任务 {id} 不属于当前组或已删除"
            )));
        }
    }
    scheme.task_ids.retain(|id| seen.insert(id.clone()));
    Ok(scheme)
}

fn apply_scheme_values(
    settings: &mut crate::models::AppSettings,
    group_id: &str,
    scheme: &crate::models::BuildScheme,
) -> Result<(), AppError> {
    let group = scheme_group(settings, group_id)?;
    group.branch = scheme.branch.clone();
    group.params = scheme.params.clone();
    let project_id = group.project_id.clone();
    for task in settings
        .package_tasks
        .iter_mut()
        .filter(|t| t.task_group_id == group_id)
    {
        task.project = Some(crate::models::TaskProjectConfig {
            project_id: project_id.clone(),
            branch: scheme.branch.clone(),
        });
    }
    Ok(())
}

async fn store_scheme(
    state: AppState,
    group_id: String,
    current_id: Option<String>,
    scheme: crate::models::BuildScheme,
) -> Result<Json<TaskGroup>, AppError> {
    let _edit = state.settings_edit.lock().await;
    let _queue = state.idle_queue().await?;
    let mut settings = state.get_settings().await;
    let scheme = prepare_scheme(&settings, &group_id, scheme, current_id.as_deref())?;
    let schemes = scheme_group(&mut settings, &group_id)?
        .schemes
        .get_or_insert_with(Vec::new);
    if let Some(index) = schemes.iter().position(|s| s.id == scheme.id) {
        schemes[index] = scheme.clone();
    } else {
        schemes.push(scheme.clone());
    }
    apply_scheme_values(&mut settings, &group_id, &scheme)?;
    state.save_settings(settings).await?;
    Ok(Json(state.find_task_group(&group_id).await?))
}
async fn create_scheme(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
    Json(scheme): Json<crate::models::BuildScheme>,
) -> Result<Json<TaskGroup>, AppError> {
    store_scheme(state, group_id, None, scheme).await
}
async fn save_scheme(
    State(state): State<AppState>,
    Path((group_id, scheme_id)): Path<(String, String)>,
    Json(scheme): Json<crate::models::BuildScheme>,
) -> Result<Json<TaskGroup>, AppError> {
    store_scheme(state, group_id, Some(scheme_id), scheme).await
}
async fn delete_scheme(
    State(state): State<AppState>,
    Path((group_id, scheme_id)): Path<(String, String)>,
) -> Result<Json<TaskGroup>, AppError> {
    let _edit = state.settings_edit.lock().await;
    let _queue = state.idle_queue().await?;
    let mut settings = state.get_settings().await;
    let schemes = scheme_group(&mut settings, &group_id)?
        .schemes
        .get_or_insert_with(Vec::new);
    if !schemes.iter().any(|s| s.id == scheme_id) {
        return Err(AppError::not_found("构建方案不存在"));
    }
    if schemes.len() <= 1 {
        return Err(AppError::validation("至少保留一个构建方案"));
    }
    schemes.retain(|s| s.id != scheme_id);
    state.save_settings(settings).await?;
    Ok(Json(state.find_task_group(&group_id).await?))
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemeName {
    name: String,
}
async fn rename_scheme(
    State(state): State<AppState>,
    Path((group_id, scheme_id)): Path<(String, String)>,
    Json(request): Json<SchemeName>,
) -> Result<Json<TaskGroup>, AppError> {
    let _edit = state.settings_edit.lock().await;
    let _queue = state.idle_queue().await?;
    let mut settings = state.get_settings().await;
    let schemes = scheme_group(&mut settings, &group_id)?
        .schemes
        .get_or_insert_with(Vec::new);
    let name = request.name.trim().to_owned();
    if name.is_empty()
        || schemes
            .iter()
            .any(|s| s.id != scheme_id && s.name.to_lowercase() == name.to_lowercase())
    {
        return Err(AppError::validation("方案名称不能为空或重复（忽略大小写）"));
    }
    let scheme = schemes
        .iter_mut()
        .find(|s| s.id == scheme_id)
        .ok_or_else(|| AppError::not_found("构建方案不存在"))?;
    scheme.name = name;
    state.save_settings(settings).await?;
    Ok(Json(state.find_task_group(&group_id).await?))
}

async fn apply_scheme(
    State(state): State<AppState>,
    Path((group_id, scheme_id)): Path<(String, String)>,
) -> Result<Json<crate::models::SchemeApplied>, AppError> {
    let _edit = state.settings_edit.lock().await;
    let _queue = state.idle_queue().await?;
    let mut settings = state.get_settings().await;
    let scheme = scheme_group(&mut settings, &group_id)?
        .schemes
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|s| s.id == scheme_id)
        .cloned()
        .ok_or_else(|| AppError::not_found("构建方案不存在"))?;
    let scheme = prepare_scheme(&settings, &group_id, scheme, Some(&scheme_id))?;
    apply_scheme_values(&mut settings, &group_id, &scheme)?;
    state.save_settings(settings).await?;
    Ok(Json(crate::models::SchemeApplied {
        group: state.find_task_group(&group_id).await?,
        scheme_id,
        task_ids: scheme.task_ids,
    }))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SchemeTasks {
    task_ids: Vec<String>,
}

async fn save_scheme_tasks(
    State(state): State<AppState>,
    Path((group_id, scheme_id)): Path<(String, String)>,
    Json(request): Json<SchemeTasks>,
) -> Result<Json<Vec<String>>, AppError> {
    let _edit = state.settings_edit.lock().await;
    let mut settings = state.get_settings().await;
    let mut ids = request.task_ids;
    if ids.iter().any(|id| {
        !settings
            .package_tasks
            .iter()
            .any(|t| t.id == *id && t.task_group_id == group_id)
    }) {
        return Err(AppError::validation("勾选任务必须属于当前任务组"));
    }
    let mut seen = HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    let scheme = scheme_group(&mut settings, &group_id)?
        .schemes
        .iter_mut()
        .flatten()
        .find(|s| s.id == scheme_id)
        .ok_or_else(|| AppError::not_found("构建方案不存在"))?;
    scheme.task_ids = ids.clone();
    state.save_settings(settings).await?;
    Ok(Json(ids))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SchemeManagement {
    #[serde(default)]
    template: Option<crate::models::BuildScheme>,
    current_id: String,
    schemes: Vec<SchemeEntry>,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemeEntry {
    id: Option<String>,
    name: String,
}

async fn manage_schemes(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
    Json(request): Json<SchemeManagement>,
) -> Result<Json<crate::models::SchemeApplied>, AppError> {
    let _edit = state.settings_edit.lock().await;
    let _queue = state.idle_queue().await?;
    let mut settings = state.get_settings().await;
    if !(1..=5).contains(&request.schemes.len()) {
        return Err(AppError::validation("每组必须保留 1～5 个方案"));
    }
    let group = scheme_group(&mut settings, &group_id)?;
    let old = group.schemes.clone().unwrap_or_default();
    let source = old
        .iter()
        .find(|s| s.id == request.current_id)
        .ok_or_else(|| AppError::not_found("当前构建方案不存在"))?
        .clone();
    let template = if let Some(mut template) = request.template {
        template.name = source.name.clone();
        prepare_scheme(&settings, &group_id, template, Some(&source.id))?
    } else {
        source.clone()
    };
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    let mut schemes = Vec::new();
    for entry in request.schemes {
        let name = entry.name.trim().to_owned();
        if name.is_empty() || !names.insert(name.to_lowercase()) {
            return Err(AppError::validation("方案名称不能为空或重复（忽略大小写）"));
        }
        let mut scheme = if let Some(id) = entry.id {
            if !ids.insert(id.clone()) {
                return Err(AppError::validation("方案 ID 重复"));
            }
            old.iter()
                .find(|s| s.id == id)
                .ok_or_else(|| AppError::not_found("构建方案不存在"))?
                .clone()
        } else {
            let mut new = template.clone();
            new.id = format!("scheme_{}", uuid::Uuid::new_v4().simple());
            new
        };
        scheme.name = name;
        schemes.push(scheme);
    }
    let active = schemes
        .iter()
        .find(|s| s.id == request.current_id)
        .unwrap_or(&schemes[0])
        .clone();
    scheme_group(&mut settings, &group_id)?.schemes = Some(schemes);
    apply_scheme_values(&mut settings, &group_id, &active)?;
    state.save_settings(settings).await?;
    Ok(Json(crate::models::SchemeApplied {
        group: state.find_task_group(&group_id).await?,
        scheme_id: active.id,
        task_ids: active.task_ids,
    }))
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
    let _edit = state.settings_edit.lock().await;
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
        schemes: None,
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
    let id = group.id.clone();
    state.save_settings(settings).await?;
    Ok(Json(state.find_task_group(&id).await?))
}

async fn update_task_group(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
    Json(request): Json<TaskGroupRequest>,
) -> Result<Json<TaskGroup>, AppError> {
    let _edit = state.settings_edit.lock().await;
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
    let _edit = state.settings_edit.lock().await;
    let _queue = state.idle_queue().await?;
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
    if let Some(id) = request.scheme_id {
        let scheme = group
            .schemes
            .iter_mut()
            .flatten()
            .find(|s| s.id == id)
            .ok_or_else(|| AppError::not_found("构建方案不存在"))?;
        scheme.branch = group.branch.clone();
        scheme.params = group.params.clone();
    }
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
    let _edit = state.settings_edit.lock().await;
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

pub(crate) fn normalize_params(
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

pub(crate) fn check_param_kind(
    definition: &ParamDefinition,
    value: &mut Value,
) -> Result<(), AppError> {
    crate::services::parameters::normalize_identifier(definition, value)?;
    if definition.required && is_empty(value) {
        return Err(AppError::validation(format!(
            "参数 {} 不能为空",
            definition.key
        )));
    }
    if definition.kind == ParamKind::Text && !value.is_string() && !value.is_null() {
        return Err(AppError::validation(format!(
            "参数 {} 必须是文本",
            definition.key
        )));
    }
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

    async fn scheme_fixture() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::load(dir.path().to_path_buf()).await;
        state
            .save_settings(AppSettings {
                projects: vec![Project {
                    id: "p".into(),
                    name: "P".into(),
                    workspace_dir_key: "p".into(),
                    ..Default::default()
                }],
                task_groups: vec![
                    TaskGroup {
                        id: "g".into(),
                        project_id: "p".into(),
                        branch: "main".into(),
                        name: "G".into(),
                        ..Default::default()
                    },
                    TaskGroup {
                        id: "other".into(),
                        project_id: "p".into(),
                        branch: "main".into(),
                        name: "Other".into(),
                        ..Default::default()
                    },
                ],
                package_tasks: vec![
                    PackageTask {
                        id: "t".into(),
                        task_group_id: "g".into(),
                        ..Default::default()
                    },
                    PackageTask {
                        id: "foreign".into(),
                        task_group_id: "other".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            })
            .await
            .unwrap();
        (dir, state)
    }
    fn snapshot(name: &str) -> crate::models::BuildScheme {
        crate::models::BuildScheme {
            name: name.into(),
            branch: "release".into(),
            params: BTreeMap::from([("version".into(), serde_json::json!("01.2.3"))]),
            task_ids: vec!["t".into()],
            ..Default::default()
        }
    }
    #[tokio::test]
    async fn scheme_limits_names_and_deleted_defaults_persist() {
        let (dir, state) = scheme_fixture().await;
        let group = state.find_task_group("g").await.unwrap();
        let defaults = group.schemes.unwrap();
        assert_eq!(
            defaults.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["pro", "test"]
        );
        assert!(defaults.iter().all(|s| s.task_ids.is_empty()));
        assert!(
            store_scheme(state.clone(), "g".into(), None, snapshot(" PRO "))
                .await
                .is_err()
        );
        for name in ["a", "b", "c"] {
            let _ = store_scheme(state.clone(), "g".into(), None, snapshot(name))
                .await
                .unwrap();
        }
        assert!(
            store_scheme(state.clone(), "g".into(), None, snapshot("d"))
                .await
                .is_err()
        );
        let group = state.find_task_group("g").await.unwrap();
        for scheme in group.schemes.unwrap().into_iter().take(4) {
            let _ = delete_scheme(State(state.clone()), Path(("g".into(), scheme.id)))
                .await
                .unwrap();
        }
        let reloaded = AppState::load(dir.path().to_path_buf()).await;
        let remaining = reloaded
            .find_task_group("g")
            .await
            .unwrap()
            .schemes
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "c");
        assert!(
            delete_scheme(
                State(reloaded.clone()),
                Path(("g".into(), remaining[0].id.clone()))
            )
            .await
            .is_err()
        );
        let _ = store_scheme(reloaded, "g".into(), None, snapshot("new"))
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn schemes_validate_atomically_apply_and_clean_moved_tasks() {
        let (_dir, state) = scheme_fixture().await;
        let original = state.find_task_group("g").await.unwrap();
        for invalid in [
            crate::models::BuildScheme {
                task_ids: vec!["foreign".into()],
                ..snapshot("bad")
            },
            crate::models::BuildScheme {
                params: BTreeMap::from([("version".into(), serde_json::json!("1..2"))]),
                ..snapshot("bad")
            },
        ] {
            assert!(
                store_scheme(state.clone(), "g".into(), None, invalid)
                    .await
                    .is_err()
            );
            assert_eq!(state.find_task_group("g").await.unwrap(), original);
        }
        let saved = store_scheme(state.clone(), "g".into(), None, snapshot("new"))
            .await
            .unwrap()
            .0;
        let scheme = saved
            .schemes
            .unwrap()
            .into_iter()
            .find(|s| s.name == "new")
            .unwrap();
        let applied = apply_scheme(State(state.clone()), Path(("g".into(), scheme.id.clone())))
            .await
            .unwrap()
            .0;
        assert_eq!(applied.task_ids, vec!["t"]);
        assert_eq!(applied.group.params["version"], "01.2.3");
        state.try_start_build().await.unwrap();
        assert!(
            apply_scheme(State(state.clone()), Path(("g".into(), scheme.id.clone())))
                .await
                .is_err()
        );
        assert_eq!(state.find_task_group("g").await.unwrap(), applied.group);
        state.finish_build().await;
        let mut settings = state.get_settings().await;
        settings
            .package_tasks
            .iter_mut()
            .find(|t| t.id == "t")
            .unwrap()
            .task_group_id = "other".into();
        state.save_settings(settings).await.unwrap();
        let group = state.find_task_group("g").await.unwrap();
        assert!(group.schemes.unwrap().iter().all(|s| s.task_ids.is_empty()));
    }
    #[tokio::test]
    async fn scheme_disk_failure_leaves_group_and_scheme_unchanged() {
        let (dir, state) = scheme_fixture().await;
        let original = state.find_task_group("g").await.unwrap();
        let path = dir.path().join("settings.json");
        tokio::fs::remove_file(&path).await.unwrap();
        tokio::fs::create_dir(&path).await.unwrap();
        assert!(
            store_scheme(state.clone(), "g".into(), None, snapshot("new"))
                .await
                .is_err()
        );
        assert_eq!(state.find_task_group("g").await.unwrap(), original);
        let id = original.schemes.unwrap()[0].id.clone();
        assert!(
            apply_scheme(State(state.clone()), Path(("g".into(), id)))
                .await
                .is_err()
        );
        assert_eq!(state.find_task_group("g").await.unwrap().branch, "main");
    }

    #[tokio::test]
    async fn selection_only_save_is_queue_safe_and_rejects_foreign_tasks() {
        let (_dir, state) = scheme_fixture().await;
        let original = state.find_task_group("g").await.unwrap();
        let id = original.schemes.as_ref().unwrap()[0].id.clone();
        state.try_start_build().await.unwrap();
        let _ = save_scheme_tasks(
            State(state.clone()),
            Path(("g".into(), id.clone())),
            Json(SchemeTasks {
                task_ids: vec!["t".into(), "t".into()],
            }),
        )
        .await
        .unwrap();
        let saved = state.find_task_group("g").await.unwrap();
        assert_eq!(saved.params, original.params);
        assert_eq!(saved.branch, original.branch);
        assert_eq!(
            saved.schemes.as_ref().unwrap()[0].params,
            original.schemes.as_ref().unwrap()[0].params
        );
        assert_eq!(saved.schemes.as_ref().unwrap()[0].task_ids, vec!["t"]);
        assert!(state.is_build_in_progress().await);
        assert!(
            save_scheme_tasks(
                State(state.clone()),
                Path(("g".into(), id)),
                Json(SchemeTasks {
                    task_ids: vec!["foreign".into()]
                })
            )
            .await
            .is_err()
        );
        assert_eq!(state.find_task_group("g").await.unwrap(), saved);
        state.finish_build().await;
    }

    #[tokio::test]
    async fn parameters_update_scheme_atomically_without_changing_selection() {
        let (_dir, state) = scheme_fixture().await;
        let original = state.find_task_group("g").await.unwrap();
        let id = original.schemes.as_ref().unwrap()[0].id.clone();
        for (scheme_id, version) in [(id.clone(), "bad"), ("missing".into(), "2.3")] {
            assert!(
                update_task_group_params(
                    State(state.clone()),
                    Path("g".into()),
                    Json(TaskGroupParamsRequest {
                        scheme_id: Some(scheme_id),
                        branch: "test".into(),
                        params: BTreeMap::from([("version".into(), serde_json::json!(version))])
                    })
                )
                .await
                .is_err()
            );
            assert_eq!(state.find_task_group("g").await.unwrap(), original);
        }
        let saved = update_task_group_params(
            State(state.clone()),
            Path("g".into()),
            Json(TaskGroupParamsRequest {
                scheme_id: Some(id),
                branch: "test".into(),
                params: BTreeMap::from([("version".into(), serde_json::json!("02.3"))]),
            }),
        )
        .await
        .unwrap()
        .0;
        let scheme = &saved.schemes.as_ref().unwrap()[0];
        assert_eq!(saved.params, scheme.params);
        assert_eq!(saved.branch, scheme.branch);
        assert!(scheme.task_ids.is_empty());
        assert_eq!(
            saved.schemes.as_ref().unwrap()[1],
            original.schemes.as_ref().unwrap()[1]
        );
    }

    #[tokio::test]
    async fn management_preserves_snapshots_and_applies_active_replacement() {
        let (_dir, state) = scheme_fixture().await;
        let original = state.find_task_group("g").await.unwrap();
        let schemes = original.schemes.clone().unwrap();
        let first = schemes[0].id.clone();
        for entries in [
            vec![],
            vec![
                SchemeEntry {
                    id: Some(first.clone()),
                    name: "x".into(),
                },
                SchemeEntry {
                    id: None,
                    name: " X ".into(),
                },
            ],
            (0..6)
                .map(|i| SchemeEntry {
                    id: None,
                    name: i.to_string(),
                })
                .collect(),
        ] {
            assert!(
                manage_schemes(
                    State(state.clone()),
                    Path("g".into()),
                    Json(SchemeManagement {
                        template: None,
                        current_id: first.clone(),
                        schemes: entries
                    })
                )
                .await
                .is_err()
            );
            assert_eq!(state.find_task_group("g").await.unwrap(), original);
        }
        let _ = save_scheme_tasks(
            State(state.clone()),
            Path(("g".into(), first.clone())),
            Json(SchemeTasks {
                task_ids: vec!["t".into()],
            }),
        )
        .await
        .unwrap();
        let saved = manage_schemes(
            State(state.clone()),
            Path("g".into()),
            Json(SchemeManagement {
                template: Some(snapshot("ignored")),
                current_id: first.clone(),
                schemes: vec![
                    SchemeEntry {
                        id: Some(first.clone()),
                        name: " renamed ".into(),
                    },
                    SchemeEntry {
                        id: None,
                        name: "new".into(),
                    },
                ],
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(saved.scheme_id, first);
        let list = saved.group.schemes.unwrap();
        assert_eq!(list[0].name, "renamed");
        assert_eq!(list[0].task_ids, vec!["t"]);
        assert_eq!(list[0].params, schemes[0].params);
        assert_eq!(list[1].task_ids, vec!["t"]);
        assert_eq!(list[1].params["version"], "01.2.3");
        assert_eq!(list[1].branch, "release");
        let replacement = manage_schemes(
            State(state.clone()),
            Path("g".into()),
            Json(SchemeManagement {
                template: None,
                current_id: first,
                schemes: vec![SchemeEntry {
                    id: Some(list[1].id.clone()),
                    name: "new".into(),
                }],
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(replacement.scheme_id, list[1].id);
        assert_eq!(replacement.task_ids, vec!["t"]);
        assert_eq!(replacement.group.params, list[1].params);
        state.try_start_build().await.unwrap();
        assert!(
            manage_schemes(
                State(state.clone()),
                Path("g".into()),
                Json(SchemeManagement {
                    template: None,
                    current_id: replacement.scheme_id.clone(),
                    schemes: vec![SchemeEntry {
                        id: Some(replacement.scheme_id),
                        name: "blocked".into()
                    }]
                })
            )
            .await
            .is_err()
        );
        state.finish_build().await;
    }

    #[tokio::test]
    async fn explicit_empty_schemes_create_only_pro_and_new_writes_are_atomic() {
        let (dir, state) = scheme_fixture().await;
        let mut settings = state.get_settings().await;
        settings
            .task_groups
            .iter_mut()
            .find(|g| g.id == "g")
            .unwrap()
            .schemes = Some(vec![]);
        state.save_settings(settings).await.unwrap();
        let group = state.find_task_group("g").await.unwrap();
        let schemes = group.schemes.as_ref().unwrap();
        assert_eq!(schemes.len(), 1);
        assert_eq!(schemes[0].name, "pro");
        let path = dir.path().join("settings.json");
        tokio::fs::remove_file(&path).await.unwrap();
        tokio::fs::create_dir(&path).await.unwrap();
        let id = schemes[0].id.clone();
        assert!(
            save_scheme_tasks(
                State(state.clone()),
                Path(("g".into(), id.clone())),
                Json(SchemeTasks {
                    task_ids: vec!["t".into()]
                })
            )
            .await
            .is_err()
        );
        assert!(
            manage_schemes(
                State(state.clone()),
                Path("g".into()),
                Json(SchemeManagement {
                    template: None,
                    current_id: id.clone(),
                    schemes: vec![SchemeEntry {
                        id: Some(id.clone()),
                        name: "changed".into()
                    }]
                })
            )
            .await
            .is_err()
        );
        assert!(
            update_task_group_params(
                State(state.clone()),
                Path("g".into()),
                Json(TaskGroupParamsRequest {
                    scheme_id: Some(id),
                    branch: "other".into(),
                    params: group.params.clone()
                })
            )
            .await
            .is_err()
        );
        assert_eq!(state.find_task_group("g").await.unwrap(), group);
    }

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
                    schemes: None,
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
