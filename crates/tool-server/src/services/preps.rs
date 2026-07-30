use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::Local;
use serde_json::Value;
use tokio::{fs, process::Command};
use walkdir::WalkDir;

use crate::{
    error::AppError,
    git,
    models::{
        CreatePrepProjectRequest, Engine, PackageTask, PrepParam, PrepParamOption, PrepParamType,
        PrepParamValueSource, PrepProject, PrepProjectExportFile, PrepProjectExportMeta,
        PrepProjectExportPayload, PrepProjectImportMode, PrepProjectImportRequest,
        PrepProjectRunForTasksRequest, PrepProjectRunForTasksResponse, PrepProjectRunRequest,
        PrepProjectRunResponse, PrepProjectTaskRunItem, Project, UpdatePrepProjectRequest,
    },
    state::AppState,
};

use super::placeholders::PlaceholderContext;

const EXPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
struct TaskPrepExecutionContext {
    task: PackageTask,
    project: Project,
    placeholder_context: PlaceholderContext,
}

pub async fn list_prep_projects(state: &AppState) -> Result<Vec<PrepProject>, AppError> {
    let preps_dir = state.preps_dir();
    if !preps_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = fs::read_dir(&preps_dir)
        .await
        .map_err(|error| AppError::internal(format!("读取准备项目目录失败: {error}")))?;
    let mut projects = Vec::new();

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| AppError::internal(format!("读取准备项目目录项失败: {error}")))?
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|file_name| file_name.to_str()) else {
            continue;
        };
        if !name.starts_with("prep_") || !name.ends_with(".json") {
            continue;
        }

        let content = fs::read_to_string(&path)
            .await
            .map_err(|error| AppError::internal(format!("读取准备项目元数据失败: {error}")))?;
        let project: PrepProject = serde_json::from_str(&content)
            .map_err(|error| AppError::internal(format!("解析准备项目元数据失败: {error}")))?;
        projects.push(project);
    }

    projects.sort_by(|left, right| right.create_time.cmp(&left.create_time));
    Ok(projects)
}

pub async fn load_prep_project(
    state: &AppState,
    prep_project_id: &str,
) -> Result<PrepProject, AppError> {
    let meta_path = prep_meta_path(state, prep_project_id);
    let content = fs::read_to_string(&meta_path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AppError::not_found(format!("未找到准备项目 {}", prep_project_id))
        } else {
            AppError::internal(format!("读取准备项目失败: {error}"))
        }
    })?;
    serde_json::from_str(&content)
        .map_err(|error| AppError::internal(format!("解析准备项目失败: {error}")))
}

pub async fn create_prep_project(
    state: &AppState,
    request: CreatePrepProjectRequest,
) -> Result<PrepProject, AppError> {
    validate_prep_params(&request.params).map_err(AppError::validation)?;

    let id = generate_id();
    let project_dir = prep_project_dir(state, &id);
    fs::create_dir_all(&project_dir)
        .await
        .map_err(|error| AppError::internal(format!("创建准备项目目录失败: {error}")))?;
    init_uv_project(&project_dir)
        .await
        .map_err(AppError::internal)?;

    let params = normalized_prep_params(&request.params);
    write_prep_templates(&project_dir, &params, true)
        .await
        .map_err(AppError::internal)?;

    let project = PrepProject {
        id: id.clone(),
        name: request.name.trim().to_string(),
        path: project_dir.to_string_lossy().to_string(),
        description: request.description.trim().to_string(),
        create_time: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        params,
    };

    save_prep_project_meta(state, &project)
        .await
        .map_err(AppError::internal)?;
    Ok(project)
}

pub async fn update_prep_project(
    state: &AppState,
    prep_project_id: &str,
    request: UpdatePrepProjectRequest,
) -> Result<PrepProject, AppError> {
    validate_prep_params(&request.params).map_err(AppError::validation)?;

    let mut existing = load_prep_project(state, prep_project_id).await?;
    let params = normalized_prep_params(&request.params);
    let project_dir = PathBuf::from(&existing.path);
    if !project_dir.exists() {
        return Err(AppError::not_found(format!(
            "准备项目目录不存在: {}",
            project_dir.display()
        )));
    }

    write_prep_templates(&project_dir, &params, false)
        .await
        .map_err(AppError::internal)?;

    existing.name = request.name.trim().to_string();
    existing.description = request.description.trim().to_string();
    existing.params = params;

    save_prep_project_meta(state, &existing)
        .await
        .map_err(AppError::internal)?;
    Ok(existing)
}

pub async fn export_prep_project(
    state: &AppState,
    prep_project_id: &str,
) -> Result<PrepProjectExportPayload, AppError> {
    let project = load_prep_project(state, prep_project_id).await?;
    let project_dir = PathBuf::from(&project.path);
    if !project_dir.exists() {
        return Err(AppError::not_found(format!(
            "准备项目目录不存在: {}",
            project_dir.display()
        )));
    }

    let files = collect_export_files(&project_dir)
        .await
        .map_err(AppError::internal)?;
    Ok(PrepProjectExportPayload {
        schema_version: EXPORT_SCHEMA_VERSION,
        prep: PrepProjectExportMeta {
            name: project.name,
            description: project.description,
            params: project
                .params
                .into_iter()
                .filter(|param| param.name != "project_path")
                .collect(),
        },
        files,
    })
}

pub async fn import_prep_project(
    state: &AppState,
    request: PrepProjectImportRequest,
) -> Result<PrepProject, AppError> {
    let payload: PrepProjectExportPayload = serde_json::from_str(request.raw_text.trim())
        .map_err(|error| AppError::validation(format!("导入内容不是合法 JSON: {error}")))?;
    validate_import_payload(&payload).map_err(AppError::validation)?;

    match request.mode {
        PrepProjectImportMode::Create => import_prep_project_as_new(state, payload)
            .await
            .map_err(AppError::internal),
        PrepProjectImportMode::Update => {
            let target_id = request
                .target_prep_project_id
                .as_deref()
                .ok_or_else(|| AppError::validation("更新已有准备项目时必须选择目标项目"))?;
            import_prep_project_as_update(state, target_id, payload)
                .await
                .map_err(AppError::internal)
        }
    }
}

pub async fn delete_prep_project(state: &AppState, prep_project_id: &str) -> Result<(), AppError> {
    let project = load_prep_project(state, prep_project_id).await?;
    let project_dir = PathBuf::from(&project.path);
    if project_dir.exists() {
        fs::remove_dir_all(&project_dir)
            .await
            .map_err(|error| AppError::internal(format!("删除准备项目目录失败: {error}")))?;
    }

    let meta_path = prep_meta_path(state, prep_project_id);
    if meta_path.exists() {
        fs::remove_file(meta_path)
            .await
            .map_err(|error| AppError::internal(format!("删除准备项目元数据失败: {error}")))?;
    }

    Ok(())
}

pub async fn run_prep_project_for_manual(
    state: &AppState,
    prep_project_id: &str,
    request: PrepProjectRunRequest,
) -> Result<PrepProjectRunResponse, AppError> {
    let prep_project = load_prep_project(state, prep_project_id).await?;
    let project = state.find_project(&request.project_id).await?;
    let engine = state.find_engine(&project.engine_name).await?;
    let project_path = state.workspace_main_repo_dir(&project);
    if !state.is_project_initialized(&project) {
        return Err(AppError::validation(format!(
            "项目 {} 未初始化，请先到引擎与项目页初始化",
            project.name
        )));
    }

    let project_branch = if project_path.join(".git").exists() {
        git::current_branch(&project_path).await.unwrap_or_default()
    } else {
        String::new()
    };
    let context = PlaceholderContext::new(
        &project,
        &engine,
        project_branch,
        project_path.to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );

    run_prep_project(&prep_project, request.params, &context)
        .await
        .map_err(AppError::internal)
}

pub async fn run_prep_project_for_tasks(
    state: &AppState,
    prep_project_id: &str,
    request: PrepProjectRunForTasksRequest,
) -> Result<PrepProjectRunForTasksResponse, AppError> {
    if request.task_ids.is_empty() {
        return Err(AppError::validation("请至少选择一个任务"));
    }

    let prep_project = load_prep_project(state, prep_project_id).await?;
    let mut results = Vec::with_capacity(request.task_ids.len());

    for task_id in request.task_ids {
        let execution_context = build_task_prep_execution_context(state, &task_id).await;
        let item = match execution_context {
            Ok(context) => {
                match run_prep_project(
                    &prep_project,
                    request.params.clone(),
                    &context.placeholder_context,
                )
                .await
                {
                    Ok(result) => PrepProjectTaskRunItem {
                        task_id: context.task.id,
                        task_name: context.task.name,
                        project_name: context.project.name,
                        success: result.success,
                        exit_code: result.exit_code,
                        command: result.command,
                        project_path: result.project_path,
                        stdout: result.stdout,
                        stderr: result.stderr,
                        error_message: if result.success {
                            None
                        } else {
                            Some(format!("准备项目执行失败，退出码 {}", result.exit_code))
                        },
                    },
                    Err(error) => failed_task_run_item(
                        &context.task.id,
                        &context.task.name,
                        &context.project.name,
                        context.placeholder_context.project_path.as_str(),
                        error.message(),
                    ),
                }
            }
            Err(error) => match state.find_task(&task_id).await {
                Ok(task) => failed_task_run_item(
                    &task.id,
                    &task.name,
                    task.project
                        .as_ref()
                        .map(|project| project.project_id.as_str())
                        .unwrap_or_default(),
                    "",
                    error.message(),
                ),
                Err(_) => failed_task_run_item(&task_id, &task_id, "", "", error.message()),
            },
        };
        results.push(item);
    }

    let success_count = results.iter().filter(|item| item.success).count();
    let total_count = results.len();

    Ok(PrepProjectRunForTasksResponse {
        total_count,
        success_count,
        failed_count: total_count.saturating_sub(success_count),
        results,
    })
}

pub async fn run_prep_project(
    prep_project: &PrepProject,
    params: HashMap<String, Value>,
    context: &PlaceholderContext,
) -> Result<PrepProjectRunResponse, AppError> {
    let project_dir = PathBuf::from(&prep_project.path);
    if !project_dir.exists() {
        return Err(AppError::not_found(format!(
            "准备项目目录不存在: {}",
            project_dir.display()
        )));
    }

    let command_args =
        build_prep_args(prep_project, &params, context).map_err(AppError::validation)?;
    let mut command = Command::new("uv");
    command
        .arg("run")
        .arg("--python")
        .arg("3.14")
        .arg("main.py");
    for arg in &command_args {
        command.arg(arg);
    }
    command.current_dir(&project_dir);

    let output = command.output().await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AppError::not_found("未检测到 uv 命令，请先安装 uv")
        } else {
            AppError::internal(format!("执行准备项目失败: {error}"))
        }
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);
    let rendered_command = format!("uv run --python 3.14 main.py {}", command_args.join(" "));

    Ok(PrepProjectRunResponse {
        success: output.status.success(),
        exit_code,
        stdout,
        stderr,
        command: rendered_command,
        project_path: context.project_path.clone(),
    })
}

async fn build_task_prep_execution_context(
    state: &AppState,
    task_id: &str,
) -> Result<TaskPrepExecutionContext, AppError> {
    let task = state.find_task(task_id).await?;
    let (project, task_group) = state.resolve_task_context(&task).await?;
    if !state.is_project_initialized(&project) {
        return Err(AppError::validation(format!(
            "任务 {} 关联项目 {} 未初始化，请先到引擎与项目页初始化",
            task.name, project.name
        )));
    }

    let engine = state.find_engine(&project.engine_name).await?;
    let project_path = state.workspace_main_repo_dir(&project);
    let task_dir = state
        .workspace_project_dir(&project)
        .join("tasks")
        .join(&task.id);
    let code_package_path = task_dir.join("code-repo");
    let remote_package_path = task_dir.join("asset-repo");
    fs::create_dir_all(&code_package_path)
        .await
        .map_err(|error| AppError::internal(format!("创建代码包工作目录失败: {error}")))?;
    fs::create_dir_all(&remote_package_path)
        .await
        .map_err(|error| AppError::internal(format!("创建远程包工作目录失败: {error}")))?;

    let placeholder_context = build_task_placeholder_context(
        &project,
        &engine,
        &task_group,
        &project_path,
        &code_package_path,
        &remote_package_path,
    )
    .await;

    Ok(TaskPrepExecutionContext {
        task,
        project,
        placeholder_context,
    })
}

async fn build_task_placeholder_context(
    project: &Project,
    engine: &Engine,
    task_group: &crate::models::TaskGroup,
    project_path: &Path,
    code_package_path: &Path,
    remote_package_path: &Path,
) -> PlaceholderContext {
    let fallback_branch = task_group.branch.clone();
    let project_branch = if project_path.join(".git").exists() {
        git::current_branch(project_path)
            .await
            .unwrap_or(fallback_branch)
    } else {
        fallback_branch
    };

    PlaceholderContext::new_with_params(
        project,
        engine,
        project_branch,
        project_path.to_string_lossy().to_string(),
        code_package_path.to_string_lossy().to_string(),
        remote_package_path.to_string_lossy().to_string(),
        &task_group.params,
    )
}

fn failed_task_run_item(
    task_id: &str,
    task_name: &str,
    project_name: &str,
    project_path: &str,
    error_message: &str,
) -> PrepProjectTaskRunItem {
    PrepProjectTaskRunItem {
        task_id: task_id.to_string(),
        task_name: task_name.to_string(),
        project_name: project_name.to_string(),
        success: false,
        exit_code: -1,
        command: String::new(),
        project_path: project_path.to_string(),
        stdout: String::new(),
        stderr: String::new(),
        error_message: Some(error_message.to_string()),
    }
}

pub fn validate_prep_target_params(
    prep_project: &PrepProject,
    params: &HashMap<String, Value>,
) -> Result<(), AppError> {
    for param in prep_project
        .params
        .iter()
        .filter(|param| param.name != "project_path")
    {
        let Some(value) = resolve_effective_param_value(param, params) else {
            if param.value_source == PrepParamValueSource::Fixed || !param.optional {
                return Err(AppError::validation(format!("缺少必填参数 {}", param.name)));
            }
            continue;
        };

        validate_prep_param_value(param, &value).map_err(AppError::validation)?;
    }

    Ok(())
}

fn validate_prep_params(params: &[PrepParam]) -> Result<(), String> {
    for param in params {
        let param_name = param.name.trim();
        if param_name.is_empty() {
            return Err("准备项目参数名不能为空".to_string());
        }
        if param_name == "project_path" {
            return Err("project_path 为系统保留参数，不需要手动配置".to_string());
        }

        if param.param_type == PrepParamType::Select {
            if param.options.is_empty() {
                return Err(format!("参数 {} 至少需要配置一个选项", param_name));
            }

            let mut labels = HashSet::new();
            let mut values = HashSet::new();
            for option in &param.options {
                let label = option.label.trim();
                if label.is_empty() {
                    return Err(format!("参数 {} 的选项显示文案不能为空", param_name));
                }
                if !labels.insert(label.to_string()) {
                    return Err(format!("参数 {} 的选项显示文案不能重复", param_name));
                }

                let value = option.value.trim();
                if value.is_empty() {
                    return Err(format!("参数 {} 的选项值不能为空", param_name));
                }
                if !values.insert(value.to_string()) {
                    return Err(format!("参数 {} 的选项值不能重复", param_name));
                }
            }
        }

        if param.value_source == PrepParamValueSource::Fixed {
            let fixed_value = param
                .fixed_value
                .as_ref()
                .ok_or_else(|| format!("参数 {} 必须填写固定值", param_name))?;
            validate_prep_param_value(param, fixed_value)?;
        }
    }
    Ok(())
}

fn normalized_prep_params(params: &[PrepParam]) -> Vec<PrepParam> {
    let mut final_params = vec![PrepParam {
        name: "project_path".to_string(),
        param_type: PrepParamType::Str,
        value_source: PrepParamValueSource::Runtime,
        optional: false,
        options: Vec::new(),
        fixed_value: None,
    }];

    for param in params {
        if param.name.trim() != "project_path" {
            final_params.push(normalize_prep_param(param));
        }
    }

    final_params
}

fn normalize_prep_param(param: &PrepParam) -> PrepParam {
    let name = param.name.trim().to_string();
    let value_source = param.value_source.clone();
    let options = if param.param_type == PrepParamType::Select {
        param
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| normalize_prep_param_option(&name, index, option))
            .collect()
    } else {
        Vec::new()
    };

    PrepParam {
        name,
        param_type: param.param_type.clone(),
        value_source: value_source.clone(),
        optional: param.optional,
        options,
        fixed_value: normalize_fixed_value(param, &value_source),
    }
}

fn normalize_fixed_value(param: &PrepParam, value_source: &PrepParamValueSource) -> Option<Value> {
    if *value_source != PrepParamValueSource::Fixed {
        return None;
    }

    let value = param.fixed_value.as_ref()?;
    Some(match (&param.param_type, value) {
        (PrepParamType::Str, Value::String(text))
        | (PrepParamType::Select, Value::String(text)) => Value::String(text.trim().to_string()),
        (PrepParamType::Int, Value::String(text)) => match text.trim().parse::<i64>() {
            Ok(number) => Value::Number(number.into()),
            Err(_) => Value::String(text.trim().to_string()),
        },
        (PrepParamType::Bool, Value::String(text)) => match text.trim() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => Value::String(text.trim().to_string()),
        },
        _ => value.clone(),
    })
}

fn validate_prep_param_value(param: &PrepParam, value: &Value) -> Result<(), String> {
    match param.param_type {
        PrepParamType::Str => {
            let rendered = match value {
                Value::String(text) => text.trim().to_string(),
                other => other.to_string(),
            };
            if rendered.is_empty() {
                return Err(format!("参数 {} 必须填写有效值", param.name));
            }
        }
        PrepParamType::Select => {
            let option_value = match value {
                Value::String(text) => text.trim().to_string(),
                other => other.to_string(),
            };
            if option_value.is_empty() {
                return Err(format!("参数 {} 必须填写有效值", param.name));
            }
            if !param
                .options
                .iter()
                .any(|option| option.value == option_value)
            {
                return Err(format!(
                    "参数 {} 的选项值不合法: {}",
                    param.name, option_value
                ));
            }
        }
        PrepParamType::Int => match value {
            Value::Number(number) if number.is_i64() || number.is_u64() || number.is_f64() => {}
            Value::String(text) if text.trim().parse::<i64>().is_ok() => {}
            _ => return Err(format!("参数 {} 必须为数字", param.name)),
        },
        PrepParamType::Bool => match value {
            Value::Bool(_) => {}
            Value::String(text) if matches!(text.trim(), "true" | "false") => {}
            _ => return Err(format!("参数 {} 必须为布尔值", param.name)),
        },
    }

    Ok(())
}

fn resolve_effective_param_value(
    param: &PrepParam,
    params: &HashMap<String, Value>,
) -> Option<Value> {
    if param.value_source == PrepParamValueSource::Fixed {
        return param.fixed_value.clone();
    }

    params.get(&param.name).cloned()
}

fn normalize_prep_param_option(
    param_name: &str,
    index: usize,
    option: &PrepParamOption,
) -> PrepParamOption {
    let label = option.label.trim().to_string();
    let value = option.value.trim().to_string();
    let id = if option.id.trim().is_empty() {
        build_prep_option_id(param_name, index, &value)
    } else {
        option.id.trim().to_string()
    };

    PrepParamOption { id, label, value }
}

async fn import_prep_project_as_new(
    state: &AppState,
    payload: PrepProjectExportPayload,
) -> Result<PrepProject, String> {
    let name = unique_import_name(state, payload.prep.name.trim())
        .await
        .map_err(|error| error.to_string())?;
    let request = CreatePrepProjectRequest {
        name,
        description: payload.prep.description,
        params: payload.prep.params,
    };
    let project = create_prep_project(state, request)
        .await
        .map_err(|error| error.to_string())?;
    restore_imported_files(Path::new(&project.path), &payload.files, false).await?;
    Ok(project)
}

async fn import_prep_project_as_update(
    state: &AppState,
    target_prep_project_id: &str,
    payload: PrepProjectExportPayload,
) -> Result<PrepProject, String> {
    let mut existing = load_prep_project(state, target_prep_project_id).await?;
    validate_prep_params(&payload.prep.params)?;

    let params = normalized_prep_params(&payload.prep.params);
    let project_dir = PathBuf::from(&existing.path);
    if !project_dir.exists() {
        return Err(format!("准备项目目录不存在: {}", project_dir.display()));
    }

    write_prep_templates(&project_dir, &params, false).await?;
    restore_imported_files(&project_dir, &payload.files, true).await?;

    existing.name = payload.prep.name.trim().to_string();
    existing.description = payload.prep.description.trim().to_string();
    existing.params = params;
    save_prep_project_meta(state, &existing).await?;
    Ok(existing)
}

async fn unique_import_name(state: &AppState, base_name: &str) -> Result<String, String> {
    let normalized = base_name.trim();
    let candidate = if normalized.is_empty() {
        "导入准备项目"
    } else {
        normalized
    };

    let existing_names = list_prep_projects(state)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|project| project.name)
        .collect::<HashSet<_>>();
    if !existing_names.contains(candidate) {
        return Ok(candidate.to_string());
    }

    let imported = format!("{candidate}（导入）");
    if !existing_names.contains(&imported) {
        return Ok(imported);
    }

    let mut index = 2;
    loop {
        let next = format!("{candidate}（导入{index}）");
        if !existing_names.contains(&next) {
            return Ok(next);
        }
        index += 1;
    }
}

fn validate_import_payload(payload: &PrepProjectExportPayload) -> Result<(), String> {
    if payload.schema_version != EXPORT_SCHEMA_VERSION {
        return Err(format!(
            "不支持的导入版本 {}, 当前仅支持 {}",
            payload.schema_version, EXPORT_SCHEMA_VERSION
        ));
    }

    if payload.prep.name.trim().is_empty() {
        return Err("导入内容缺少准备项目名称".to_string());
    }
    validate_prep_params(&payload.prep.params)?;

    for file in &payload.files {
        validate_import_file_path(&file.path)?;
    }

    Ok(())
}

fn validate_import_file_path(path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("导入文件路径不能为空".to_string());
    }

    let path = Path::new(path);
    if path.is_absolute() {
        return Err("导入文件路径必须为相对路径".to_string());
    }

    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => return Err(format!("导入文件路径不合法: {}", path.display())),
        }
    }

    Ok(())
}

async fn collect_export_files(project_dir: &Path) -> Result<Vec<PrepProjectExportFile>, String> {
    let mut files = Vec::new();

    for entry in WalkDir::new(project_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let relative = path
            .strip_prefix(project_dir)
            .map_err(|error| format!("生成导出文件相对路径失败: {error}"))?;
        if !should_manage_export_file(relative) {
            continue;
        }

        let Ok(content) = fs::read_to_string(path).await else {
            continue;
        };
        files.push(PrepProjectExportFile {
            path: normalize_relative_path(relative),
            content,
        });
    }

    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

async fn restore_imported_files(
    project_dir: &Path,
    files: &[PrepProjectExportFile],
    delete_missing: bool,
) -> Result<(), String> {
    let imported_paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<HashSet<_>>();

    if delete_missing {
        let existing_paths = collect_managed_paths(project_dir)?;
        for relative in existing_paths {
            let relative_str = normalize_relative_path(&relative);
            if imported_paths.contains(relative_str.as_str()) {
                continue;
            }
            let absolute = project_dir.join(&relative);
            if absolute.exists() {
                fs::remove_file(&absolute)
                    .await
                    .map_err(|error| format!("删除旧文件失败 {}: {error}", absolute.display()))?;
            }
        }
    }

    for file in files {
        validate_import_file_path(&file.path)?;
        let absolute = project_dir.join(&file.path);
        if let Some(parent) = absolute.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("创建导入文件目录失败: {error}"))?;
        }
        fs::write(&absolute, file.content.as_bytes())
            .await
            .map_err(|error| format!("写入导入文件失败 {}: {error}", absolute.display()))?;
    }

    if delete_missing {
        cleanup_empty_dirs(project_dir).await?;
    }

    Ok(())
}

fn collect_managed_paths(project_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for entry in WalkDir::new(project_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let relative = path
            .strip_prefix(project_dir)
            .map_err(|error| format!("生成文件相对路径失败: {error}"))?;
        if should_manage_export_file(relative) {
            paths.push(relative.to_path_buf());
        }
    }
    Ok(paths)
}

async fn cleanup_empty_dirs(project_dir: &Path) -> Result<(), String> {
    let mut directories = WalkDir::new(project_dir)
        .min_depth(1)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_dir())
        .map(|entry| entry.path().to_path_buf())
        .collect::<Vec<_>>();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));

    for directory in directories {
        let mut entries = fs::read_dir(&directory)
            .await
            .map_err(|error| format!("读取目录失败 {}: {error}", directory.display()))?;
        if entries
            .next_entry()
            .await
            .map_err(|error| format!("读取目录项失败 {}: {error}", directory.display()))?
            .is_none()
        {
            fs::remove_dir(&directory)
                .await
                .map_err(|error| format!("删除空目录失败 {}: {error}", directory.display()))?;
        }
    }

    Ok(())
}

fn should_manage_export_file(relative: &Path) -> bool {
    if relative.as_os_str().is_empty() {
        return false;
    }

    let file_name = relative
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if file_name == "uv.lock" {
        return false;
    }

    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return false;
        };
        let segment = segment.to_string_lossy();
        if segment == ".venv" || segment == "__pycache__" || segment == ".git" || segment == "data"
        {
            return false;
        }
    }

    true
}

fn normalize_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(segment) => Some(segment.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn build_prep_option_id(param_name: &str, index: usize, value: &str) -> String {
    format!(
        "option_{}_{}_{}",
        sanitize_identifier(param_name),
        index,
        sanitize_identifier(value)
    )
}

fn sanitize_identifier(input: &str) -> String {
    let sanitized = input
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_lowercase();
    if sanitized.is_empty() {
        "value".to_string()
    } else {
        sanitized
    }
}

async fn init_uv_project(project_dir: &Path) -> Result<(), String> {
    let output = Command::new("uv")
        .arg("init")
        .arg("--bare")
        .current_dir(project_dir)
        .output()
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "未检测到 uv 命令，请先安装 uv".to_string()
            } else {
                format!("执行 uv init 失败: {error}")
            }
        })?;

    if !output.status.success() {
        return Err(format!(
            "uv init 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(())
}

async fn write_prep_templates(
    project_dir: &Path,
    params: &[PrepParam],
    write_default_lib: bool,
) -> Result<(), String> {
    fs::write(project_dir.join("models.py"), generate_models_py(params))
        .await
        .map_err(|error| format!("写入 models.py 失败: {error}"))?;
    fs::write(project_dir.join("main.py"), generate_main_py(params))
        .await
        .map_err(|error| format!("写入 main.py 失败: {error}"))?;
    if write_default_lib || !project_dir.join("lib.py").exists() {
        fs::write(project_dir.join("lib.py"), default_lib_py())
            .await
            .map_err(|error| format!("写入 lib.py 失败: {error}"))?;
    }
    Ok(())
}

async fn save_prep_project_meta(state: &AppState, project: &PrepProject) -> Result<(), String> {
    let preps_dir = state.preps_dir();
    fs::create_dir_all(&preps_dir)
        .await
        .map_err(|error| format!("创建准备项目目录失败: {error}"))?;
    let meta_path = prep_meta_path(state, &project.id);
    let content = serde_json::to_vec_pretty(project)
        .map_err(|error| format!("序列化准备项目失败: {error}"))?;
    fs::write(meta_path, content)
        .await
        .map_err(|error| format!("写入准备项目元数据失败: {error}"))
}

fn prep_project_dir(state: &AppState, prep_project_id: &str) -> PathBuf {
    state.preps_dir().join(format!("prep_{prep_project_id}"))
}

fn prep_meta_path(state: &AppState, prep_project_id: &str) -> PathBuf {
    state
        .preps_dir()
        .join(format!("prep_{prep_project_id}.json"))
}

fn generate_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    nanos.to_string()
}

fn generate_models_py(params: &[PrepParam]) -> String {
    let mut fields = String::new();
    for param in params {
        let py_type = match param.param_type {
            PrepParamType::Str | PrepParamType::Select => "str",
            PrepParamType::Int => "int",
            PrepParamType::Bool => "bool",
        };

        if param.optional {
            fields.push_str(&format!("    {}: {} = None\n", param.name, py_type));
        } else {
            fields.push_str(&format!("    {}: {}\n", param.name, py_type));
        }
    }

    format!(
        "from dataclasses import dataclass\n\n@dataclass\nclass Person:\n{}",
        fields
    )
}

fn generate_main_py(params: &[PrepParam]) -> String {
    let mut args_code = String::new();
    let mut init_args = Vec::new();

    for param in params {
        let py_type = match param.param_type {
            PrepParamType::Str | PrepParamType::Select => "str",
            PrepParamType::Int => "int",
            PrepParamType::Bool => "bool",
        };
        args_code.push_str(&format!(
            "    parser.add_argument(\"--{}\", type={}, required={}, help=\"请输入{}\")\n",
            param.name,
            py_type,
            if param.optional { "False" } else { "True" },
            param.name
        ));
        init_args.push(format!("{}=args.{}", param.name, param.name));
    }

    format!(
        "##不要动main.py里的代码\nimport argparse\nimport sys\nimport io\nfrom lib import process_person\nfrom models import Person\n\nif sys.stdout.encoding != 'utf-8':\n    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8')\n\ndef main():\n    parser = argparse.ArgumentParser(description=\"请输入参数\")\n{}\n    args = parser.parse_args()\n    person = Person({})\n    process_person(person)\n\nif __name__ == \"__main__\":\n    main()\n",
        args_code,
        init_args.join(", ")
    )
}

fn default_lib_py() -> &'static str {
    "## 你的逻辑写在这里，不要动 main.py 里的代码\n## data 目录是数据目录，动态生成的资源请放到 data/ 下面\n## 退出码说明：0 表示成功，非 0 表示失败，系统会记录对应退出码\nimport sys\nfrom models import Person\n\ndef process_person(person: Person):\n    print(person)\n    sys.exit(0)\n"
}

fn build_prep_args(
    prep_project: &PrepProject,
    params: &HashMap<String, Value>,
    context: &PlaceholderContext,
) -> Result<Vec<String>, String> {
    validate_prep_target_params(prep_project, params)?;
    let mut args = vec!["--project_path".to_string(), context.project_path.clone()];

    for param in prep_project
        .params
        .iter()
        .filter(|param| param.name != "project_path")
    {
        let Some(value) = resolve_effective_param_value(param, params) else {
            continue;
        };

        let rendered = match (&param.param_type, &value) {
            (PrepParamType::Str, Value::String(text)) => context.replace_text(text),
            (PrepParamType::Str, other) => context.replace_text(&other.to_string()),
            (PrepParamType::Select, Value::String(text)) => context.replace_text(text),
            (PrepParamType::Select, other) => context.replace_text(&other.to_string()),
            (PrepParamType::Int, Value::Number(number)) => number.to_string(),
            (PrepParamType::Int, Value::String(text)) => text.to_string(),
            (PrepParamType::Bool, Value::Bool(boolean)) => boolean.to_string(),
            (PrepParamType::Bool, Value::String(text)) => text.to_string(),
            _ => value.to_string(),
        };

        args.push(format!("--{}", param.name));
        args.push(rendered);
    }

    Ok(args)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;
    use crate::{
        models::{AppSettings, BuildMode, Engine, PackageTask, Project, TaskProjectConfig},
        state::AppState,
    };

    fn sample_context() -> PlaceholderContext {
        let project = Project {
            id: "project_1".to_string(),
            name: "demo".to_string(),
            workspace_dir_key: "workspace_1".to_string(),
            git_url: "https://example.com/demo.git".to_string(),
            engine_name: "cocos".to_string(),
            version: "1.2.3".to_string(),
            minor_version: 7,
            build_mode: BuildMode::Release,
            is_hot_update: false,
            enable_pay: true,
            review_mode: false,
        };
        let engine = Engine {
            name: "cocos".to_string(),
            path: "/engine".to_string(),
        };

        PlaceholderContext::new(
            &project,
            &engine,
            "main".to_string(),
            "/project".to_string(),
            String::new(),
            String::new(),
        )
    }

    fn select_param() -> PrepParam {
        PrepParam {
            name: "channel".to_string(),
            param_type: PrepParamType::Select,
            value_source: PrepParamValueSource::Runtime,
            optional: false,
            options: vec![
                PrepParamOption {
                    id: String::new(),
                    label: "安卓".to_string(),
                    value: "android".to_string(),
                },
                PrepParamOption {
                    id: String::new(),
                    label: "iOS".to_string(),
                    value: "ios".to_string(),
                },
            ],
            fixed_value: None,
        }
    }

    async fn create_test_state(name: &str) -> (AppState, PathBuf) {
        let data_dir = temp_test_dir(name);
        fs::create_dir_all(&data_dir)
            .await
            .expect("create temp data dir");
        let state = AppState::load(data_dir.clone()).await;
        (state, data_dir)
    }

    async fn create_saved_prep_project(state: &AppState, id: &str, path: PathBuf) -> PrepProject {
        let project = PrepProject {
            id: id.to_string(),
            name: "批量准备".to_string(),
            path: path.to_string_lossy().to_string(),
            description: String::new(),
            create_time: "2026-03-23 00:00:00".to_string(),
            params: normalized_prep_params(&[]),
        };
        save_prep_project_meta(state, &project)
            .await
            .expect("save prep meta");
        project
    }

    #[test]
    fn select_param_should_round_trip_with_options() {
        let param = select_param();
        let text = serde_json::to_string(&param).expect("serialize prep param");
        let parsed: PrepParam = serde_json::from_str(&text).expect("deserialize prep param");

        assert_eq!(parsed.param_type, PrepParamType::Select);
        assert_eq!(parsed.options.len(), 2);
        assert_eq!(parsed.options[0].label, "安卓");
        assert_eq!(parsed.options[1].value, "ios");
    }

    #[test]
    fn validate_should_reject_select_without_options() {
        let error = validate_prep_params(&[PrepParam {
            name: "channel".to_string(),
            param_type: PrepParamType::Select,
            value_source: PrepParamValueSource::Runtime,
            optional: false,
            options: Vec::new(),
            fixed_value: None,
        }])
        .expect_err("select without options should fail");

        assert!(error.contains("至少需要配置一个选项"));
    }

    #[test]
    fn validate_should_reject_blank_option_label_or_value() {
        let blank_label = validate_prep_params(&[PrepParam {
            name: "channel".to_string(),
            param_type: PrepParamType::Select,
            value_source: PrepParamValueSource::Runtime,
            optional: false,
            options: vec![PrepParamOption {
                id: String::new(),
                label: " ".to_string(),
                value: "android".to_string(),
            }],
            fixed_value: None,
        }])
        .expect_err("blank label should fail");
        assert!(blank_label.contains("显示文案不能为空"));

        let blank_value = validate_prep_params(&[PrepParam {
            name: "channel".to_string(),
            param_type: PrepParamType::Select,
            value_source: PrepParamValueSource::Runtime,
            optional: false,
            options: vec![PrepParamOption {
                id: String::new(),
                label: "安卓".to_string(),
                value: " ".to_string(),
            }],
            fixed_value: None,
        }])
        .expect_err("blank value should fail");
        assert!(blank_value.contains("选项值不能为空"));
    }

    #[test]
    fn validate_should_reject_duplicate_option_label_or_value() {
        let duplicate_label = validate_prep_params(&[PrepParam {
            name: "channel".to_string(),
            param_type: PrepParamType::Select,
            value_source: PrepParamValueSource::Runtime,
            optional: false,
            options: vec![
                PrepParamOption {
                    id: String::new(),
                    label: "安卓".to_string(),
                    value: "android".to_string(),
                },
                PrepParamOption {
                    id: String::new(),
                    label: "安卓".to_string(),
                    value: "ios".to_string(),
                },
            ],
            fixed_value: None,
        }])
        .expect_err("duplicate label should fail");
        assert!(duplicate_label.contains("显示文案不能重复"));

        let duplicate_value = validate_prep_params(&[PrepParam {
            name: "channel".to_string(),
            param_type: PrepParamType::Select,
            value_source: PrepParamValueSource::Runtime,
            optional: false,
            options: vec![
                PrepParamOption {
                    id: String::new(),
                    label: "安卓".to_string(),
                    value: "android".to_string(),
                },
                PrepParamOption {
                    id: String::new(),
                    label: "iOS".to_string(),
                    value: "android".to_string(),
                },
            ],
            fixed_value: None,
        }])
        .expect_err("duplicate value should fail");
        assert!(duplicate_value.contains("选项值不能重复"));
    }

    #[test]
    fn build_prep_args_should_accept_valid_select_value() {
        let prep_project = PrepProject {
            id: "1".to_string(),
            name: "demo".to_string(),
            path: "/tmp/demo".to_string(),
            description: String::new(),
            create_time: "2026-03-21 00:00:00".to_string(),
            params: normalized_prep_params(&[select_param()]),
        };
        let context = sample_context();
        let args = build_prep_args(
            &prep_project,
            &HashMap::from([(String::from("channel"), json!("android"))]),
            &context,
        )
        .expect("valid select value should pass");

        assert!(args.windows(2).any(|pair| pair == ["--channel", "android"]));
    }

    #[test]
    fn build_prep_args_should_reject_invalid_select_value() {
        let prep_project = PrepProject {
            id: "1".to_string(),
            name: "demo".to_string(),
            path: "/tmp/demo".to_string(),
            description: String::new(),
            create_time: "2026-03-21 00:00:00".to_string(),
            params: normalized_prep_params(&[select_param()]),
        };
        let context = sample_context();
        let error = build_prep_args(
            &prep_project,
            &HashMap::from([(String::from("channel"), json!("windows"))]),
            &context,
        )
        .expect_err("invalid select value should fail");

        assert!(error.contains("选项值不合法"));
    }

    #[test]
    fn validate_should_reject_fixed_param_without_value() {
        let error = validate_prep_params(&[PrepParam {
            name: "channel".to_string(),
            param_type: PrepParamType::Str,
            value_source: PrepParamValueSource::Fixed,
            optional: false,
            options: Vec::new(),
            fixed_value: None,
        }])
        .expect_err("fixed param without value should fail");

        assert!(error.contains("必须填写固定值"));
    }

    #[test]
    fn build_prep_args_should_use_fixed_value_and_ignore_runtime_override() {
        let prep_project = PrepProject {
            id: "1".to_string(),
            name: "demo".to_string(),
            path: "/tmp/demo".to_string(),
            description: String::new(),
            create_time: "2026-03-21 00:00:00".to_string(),
            params: normalized_prep_params(&[
                PrepParam {
                    name: "channel".to_string(),
                    param_type: PrepParamType::Str,
                    value_source: PrepParamValueSource::Fixed,
                    optional: false,
                    options: Vec::new(),
                    fixed_value: Some(json!("${build_mode}")),
                },
                PrepParam {
                    name: "tag".to_string(),
                    param_type: PrepParamType::Bool,
                    value_source: PrepParamValueSource::Fixed,
                    optional: false,
                    options: Vec::new(),
                    fixed_value: Some(json!(true)),
                },
            ]),
        };
        let context = sample_context();
        let args = build_prep_args(
            &prep_project,
            &HashMap::from([
                (String::from("channel"), json!("override")),
                (String::from("tag"), json!(false)),
            ]),
            &context,
        )
        .expect("fixed values should be used");

        assert!(args.windows(2).any(|pair| pair == ["--channel", "release"]));
        assert!(args.windows(2).any(|pair| pair == ["--tag", "true"]));
        assert!(
            !args
                .windows(2)
                .any(|pair| pair == ["--channel", "override"])
        );
    }

    #[test]
    fn normalized_non_select_param_should_clear_options() {
        let params = normalized_prep_params(&[PrepParam {
            name: "message".to_string(),
            param_type: PrepParamType::Str,
            value_source: PrepParamValueSource::Runtime,
            optional: false,
            options: vec![PrepParamOption {
                id: "1".to_string(),
                label: "显示".to_string(),
                value: "值".to_string(),
            }],
            fixed_value: None,
        }]);

        let message_param = params
            .iter()
            .find(|param| param.name == "message")
            .expect("message param should exist");
        assert!(message_param.options.is_empty());
    }

    #[test]
    fn import_path_validation_should_reject_parent_or_absolute_path() {
        let parent_error =
            validate_import_file_path("../lib.py").expect_err("parent path should fail");
        assert!(parent_error.contains("不合法"));

        let absolute_error =
            validate_import_file_path("/tmp/lib.py").expect_err("absolute path should fail");
        assert!(absolute_error.contains("相对路径"));
    }

    #[test]
    fn managed_export_filter_should_skip_runtime_files() {
        assert!(should_manage_export_file(Path::new("lib.py")));
        assert!(should_manage_export_file(Path::new("nested/helper.py")));
        assert!(!should_manage_export_file(Path::new("data/output.json")));
        assert!(!should_manage_export_file(Path::new(
            "nested/data/output.json"
        )));
        assert!(!should_manage_export_file(Path::new("uv.lock")));
        assert!(!should_manage_export_file(Path::new(".venv/bin/python")));
        assert!(!should_manage_export_file(Path::new("__pycache__/lib.pyc")));
    }

    #[test]
    fn default_lib_template_should_describe_data_dir_and_exit_codes() {
        let content = default_lib_py();

        assert!(content.contains("data 目录是数据目录"));
        assert!(content.contains("动态生成的资源请放到 data/ 下面"));
        assert!(content.contains("退出码说明：0 表示成功，非 0 表示失败"));
    }

    #[tokio::test]
    async fn write_templates_should_not_overwrite_existing_lib_on_update() {
        let project_dir = temp_test_dir("prep_templates_keep_lib");
        fs::create_dir_all(&project_dir)
            .await
            .expect("create temp prep dir");
        fs::write(project_dir.join("lib.py"), "print('custom lib')\n")
            .await
            .expect("write custom lib");

        write_prep_templates(&project_dir, &normalized_prep_params(&[]), false)
            .await
            .expect("write templates");

        let content = fs::read_to_string(project_dir.join("lib.py"))
            .await
            .expect("read lib");
        assert_eq!(content, "print('custom lib')\n");

        let _ = fs::remove_dir_all(&project_dir).await;
    }

    #[tokio::test]
    async fn restore_imported_files_should_delete_missing_managed_files_on_update() {
        let project_dir = temp_test_dir("prep_restore_delete");
        fs::create_dir_all(project_dir.join("nested"))
            .await
            .expect("create temp prep dir");
        fs::write(project_dir.join("lib.py"), "old\n")
            .await
            .expect("write old lib");
        fs::write(project_dir.join("nested/extra.py"), "old extra\n")
            .await
            .expect("write old extra");
        fs::write(project_dir.join("uv.lock"), "keep lock\n")
            .await
            .expect("write lock");

        restore_imported_files(
            &project_dir,
            &[PrepProjectExportFile {
                path: "lib.py".to_string(),
                content: "new\n".to_string(),
            }],
            true,
        )
        .await
        .expect("restore imported files");

        let lib = fs::read_to_string(project_dir.join("lib.py"))
            .await
            .expect("read lib");
        assert_eq!(lib, "new\n");
        assert!(!project_dir.join("nested/extra.py").exists());
        assert!(project_dir.join("uv.lock").exists());

        let _ = fs::remove_dir_all(&project_dir).await;
    }

    #[tokio::test]
    async fn build_task_placeholder_context_should_render_task_workspace_paths() {
        let (state, data_dir) = create_test_state("prep_task_context").await;
        state
            .save_settings(AppSettings {
                engines: vec![Engine {
                    name: "cocos".to_string(),
                    path: "C:\\engine\\Creator.exe".to_string(),
                }],
                projects: vec![Project {
                    name: "演示项目".to_string(),
                    git_url: "https://example.com/demo.git".to_string(),
                    engine_name: "cocos".to_string(),
                    version: "6.7.3.1".to_string(),
                    minor_version: 8,
                    build_mode: BuildMode::Pre,
                    is_hot_update: true,
                    enable_pay: false,
                    ..Project::default()
                }],
                package_tasks: vec![PackageTask {
                    id: "task_1".to_string(),
                    name: "安卓任务".to_string(),
                    project: Some(TaskProjectConfig {
                        project_id: "演示项目".to_string(),
                        branch: "release/task".to_string(),
                    }),
                    ..PackageTask::default()
                }],
                ..AppSettings::default()
            })
            .await
            .expect("save settings");
        let project = state.find_project("演示项目").await.expect("find project");
        fs::create_dir_all(state.workspace_main_repo_dir(&project).join(".git"))
            .await
            .expect("create fake git dir");

        let context = build_task_prep_execution_context(&state, "task_1")
            .await
            .expect("build task prep context");
        let rendered = context.placeholder_context.replace_text(
            "${project_path}|${code_package_path}|${code_repo_path}|${remote_package_path}|${project_branch}",
        );
        let project_path = state.workspace_main_repo_dir(&project);
        let task_dir = state
            .workspace_project_dir(&project)
            .join("tasks")
            .join("task_1");

        assert_eq!(
            rendered,
            format!(
                "{}|{}|{}|{}|release/task",
                project_path.to_string_lossy().replace('\\', "/"),
                task_dir
                    .join("code-repo")
                    .to_string_lossy()
                    .replace('\\', "/"),
                task_dir
                    .join("code-repo")
                    .to_string_lossy()
                    .replace('\\', "/"),
                task_dir
                    .join("asset-repo")
                    .to_string_lossy()
                    .replace('\\', "/"),
            )
        );
        assert!(task_dir.join("code-repo").exists());
        assert!(task_dir.join("asset-repo").exists());

        let _ = fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn run_prep_project_for_tasks_should_keep_order_and_collect_task_failures() {
        let (state, data_dir) = create_test_state("prep_task_batch").await;
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
                package_tasks: vec![
                    PackageTask {
                        id: "task_a".to_string(),
                        name: "任务A".to_string(),
                        project: Some(TaskProjectConfig {
                            project_id: "演示项目".to_string(),
                            branch: "main".to_string(),
                        }),
                        ..PackageTask::default()
                    },
                    PackageTask {
                        id: "task_b".to_string(),
                        name: "任务B".to_string(),
                        ..PackageTask::default()
                    },
                    PackageTask {
                        id: "task_c".to_string(),
                        name: "任务C".to_string(),
                        project: Some(TaskProjectConfig {
                            project_id: "演示项目".to_string(),
                            branch: "main".to_string(),
                        }),
                        ..PackageTask::default()
                    },
                ],
                ..AppSettings::default()
            })
            .await
            .expect("save settings");
        let project = state.find_project("演示项目").await.expect("find project");
        fs::create_dir_all(state.workspace_main_repo_dir(&project).join(".git"))
            .await
            .expect("create fake git dir");

        let prep_project = create_saved_prep_project(
            &state,
            "prep_batch_missing_dir",
            data_dir.join("missing-prep-dir"),
        )
        .await;

        let response = run_prep_project_for_tasks(
            &state,
            &prep_project.id,
            PrepProjectRunForTasksRequest {
                task_ids: vec![
                    "task_a".to_string(),
                    "task_b".to_string(),
                    "task_c".to_string(),
                ],
                params: HashMap::new(),
            },
        )
        .await
        .expect("run prep project for tasks");

        assert_eq!(response.total_count, 3);
        assert_eq!(response.success_count, 0);
        assert_eq!(response.failed_count, 3);
        assert_eq!(
            response
                .results
                .iter()
                .map(|item| item.task_name.as_str())
                .collect::<Vec<_>>(),
            vec!["任务A", "任务B", "任务C"]
        );
        assert!(
            response.results[0]
                .error_message
                .as_deref()
                .expect("task a should have error")
                .contains("准备项目目录不存在")
        );
        assert!(
            response.results[1]
                .error_message
                .as_deref()
                .expect("task b should have error")
                .contains("未绑定项目")
        );
        assert!(
            response.results[2]
                .error_message
                .as_deref()
                .expect("task c should have error")
                .contains("准备项目目录不存在")
        );

        let _ = fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn run_prep_project_for_tasks_should_reject_empty_task_ids() {
        let (state, data_dir) = create_test_state("prep_task_empty").await;
        let prep_project = create_saved_prep_project(
            &state,
            "prep_empty_request",
            data_dir.join("prep-empty-request"),
        )
        .await;

        let error = run_prep_project_for_tasks(
            &state,
            &prep_project.id,
            PrepProjectRunForTasksRequest {
                task_ids: Vec::new(),
                params: HashMap::new(),
            },
        )
        .await
        .expect_err("empty task ids should fail");

        assert!(error.message().contains("请至少选择一个任务"));

        let _ = fs::remove_dir_all(data_dir).await;
    }

    fn temp_test_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}_{nanos}"))
    }
}
