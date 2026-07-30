use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::Local;
use serde_json::{Value, json};
use tokio::{
    fs,
    sync::{Mutex, RwLock, watch},
    time::sleep,
};
use tracing::{info, warn};

use crate::{
    error::AppError,
    models::{
        AppSettings, BuildMode, BuildStatusResponse, BuildTaskStatusDto, Engine, GitConfig,
        LegacyImportPreview, PackageTask, PackageTaskRuntime, PackageTaskStatus, ParamDefinition,
        ParamKind, Project, RuntimeState, TaskGroup,
    },
};
use cocos_build_lan_contract::ToolStatus;
use cocos_build_lan_core::{RestartBlocker, RestartGuard, RestartProtection, RestartRegistry};

const RUNTIME_FLUSH_INTERVAL: Duration = Duration::from_secs(5);
static GENERATED_PROJECT_FIELD_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeFlushMode {
    Deferred,
    Immediate,
}

#[derive(Clone)]
pub struct AppState {
    data_dir: PathBuf,
    settings_path: PathBuf,
    runtime_path: PathBuf,
    settings: Arc<RwLock<AppSettings>>,
    git_config: Arc<RwLock<GitConfig>>,
    runtime_state: Arc<RwLock<RuntimeState>>,
    runtime_revision: Arc<AtomicU64>,
    persisted_runtime_revision: Arc<AtomicU64>,
    build_in_progress: Arc<Mutex<bool>>,
    restart_registry: RestartRegistry,
    cancel_sender: Arc<Mutex<Option<watch::Sender<bool>>>>,
    active_task_id: Arc<RwLock<Option<String>>>,
}

impl AppState {
    #[allow(dead_code)]
    pub async fn load(data_dir: PathBuf) -> Self {
        Self::load_with_restart_registry(data_dir, RestartRegistry::default()).await
    }

    pub async fn load_with_restart_registry(
        data_dir: PathBuf,
        restart_registry: RestartRegistry,
    ) -> Self {
        let settings_path = data_dir.join("settings.json");
        let runtime_path = data_dir.join("runtime_state.json");
        let loaded_settings = load_settings(&settings_path).await.unwrap_or_else(|error| {
            warn!(error = %error, path = %settings_path.display(), "加载设置文件失败，将使用默认配置");
            AppSettings::default()
        });
        let (normalized_settings, normalized_settings_changed) =
            normalize_settings(loaded_settings);
        if normalized_settings_changed {
            let static_settings = strip_runtime_from_settings(normalized_settings.clone());
            if let Err(error) = persist_settings(&settings_path, &static_settings).await {
                warn!(
                    error = %error,
                    path = %settings_path.display(),
                    "启动时回写规范化设置失败"
                );
            }
        }
        let runtime_state = if runtime_path.exists() {
            load_runtime_state(&runtime_path)
                .await
                .unwrap_or_else(|error| {
                    warn!(
                        error = %error,
                        path = %runtime_path.display(),
                        "加载运行态失败，将从设置文件恢复默认运行态"
                    );
                    extract_runtime_state(&normalized_settings)
                })
        } else {
            extract_runtime_state(&normalized_settings)
        };
        let (runtime_state, normalized_stale_builds) =
            normalize_runtime_state_on_startup(runtime_state);
        if normalized_stale_builds
            && let Err(error) = persist_runtime_state(&runtime_path, &runtime_state).await
        {
            warn!(
                error = %error,
                path = %runtime_path.display(),
                "启动时回写运行态失败，已在内存中清理残留打包状态"
            );
        }
        let imported_git_config = normalized_settings.git_config.clone();
        let settings = strip_runtime_from_settings(normalized_settings);

        let state = Self {
            data_dir,
            settings_path,
            runtime_path,
            settings: Arc::new(RwLock::new(settings)),
            git_config: Arc::new(RwLock::new(imported_git_config)),
            runtime_state: Arc::new(RwLock::new(runtime_state)),
            runtime_revision: Arc::new(AtomicU64::new(0)),
            persisted_runtime_revision: Arc::new(AtomicU64::new(0)),
            build_in_progress: Arc::new(Mutex::new(false)),
            restart_registry,
            cancel_sender: Arc::new(Mutex::new(None)),
            active_task_id: Arc::new(RwLock::new(None)),
        };
        state.spawn_runtime_flush_task();
        info!(
            settings_path = %state.settings_path.display(),
            runtime_path = %state.runtime_path.display(),
            normalized_settings_changed,
            normalized_stale_builds,
            "应用状态加载完成"
        );
        state
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn preps_dir(&self) -> PathBuf {
        self.data_dir.join("preps")
    }

    pub fn workspaces_dir(&self) -> PathBuf {
        self.data_dir.join("workspaces")
    }

    pub fn workspace_main_repo_dir(&self, project: &Project) -> PathBuf {
        self.workspace_project_dir(project).join("main-repo")
    }

    pub fn is_project_initialized(&self, project: &Project) -> bool {
        self.workspace_main_repo_dir(project).join(".git").exists()
    }

    pub async fn get_settings(&self) -> AppSettings {
        let mut settings = self.settings.read().await.clone();
        settings.git_config = self.git_config.read().await.clone();
        let runtime_state = self.runtime_state.read().await.clone();
        apply_runtime_state(settings, &runtime_state)
    }

    pub async fn get_build_status(&self) -> BuildStatusResponse {
        let task_ids = self
            .settings
            .read()
            .await
            .package_tasks
            .iter()
            .map(|task| task.id.clone())
            .collect::<HashSet<_>>();
        let runtime_state = self.runtime_state.read().await.clone();

        BuildStatusResponse {
            package_tasks: runtime_state
                .package_tasks
                .into_iter()
                .filter(|runtime| task_ids.contains(&runtime.task_id))
                .map(|runtime| BuildTaskStatusDto {
                    task_id: runtime.task_id,
                    progress: runtime.progress,
                    step_label: runtime.step_label,
                    status: runtime.status,
                    last_error: runtime.last_error,
                    started_at: runtime.started_at,
                    finished_at: runtime.finished_at,
                })
                .collect(),
        }
    }

    pub async fn save_settings(&self, settings: AppSettings) -> Result<(), AppError> {
        let (normalized_settings, _) = normalize_settings(settings);
        let static_settings = strip_runtime_from_settings(normalized_settings);

        persist_settings(&self.settings_path, &static_settings).await?;
        let filtered_runtime = {
            let task_ids = static_settings
                .package_tasks
                .iter()
                .map(|task| task.id.as_str())
                .collect::<HashSet<_>>();
            let mut runtime_state = self.runtime_state.write().await;
            runtime_state
                .package_tasks
                .retain(|runtime| task_ids.contains(runtime.task_id.as_str()));
            runtime_state.clone()
        };
        self.persist_runtime_snapshot(&filtered_runtime).await?;

        let mut current = self.settings.write().await;
        *current = static_settings;
        info!("设置与运行态保存完成");
        Ok(())
    }

    pub async fn set_git_config(&self, git_config: GitConfig) -> Result<(), AppError> {
        *self.git_config.write().await = git_config;
        Ok(())
    }

    pub async fn control_status(&self) -> ToolStatus {
        let settings = self.get_settings().await;
        let runtime = self.runtime_state.read().await;
        let running = runtime
            .package_tasks
            .iter()
            .filter(|task| {
                task.status == PackageTaskStatus::Running
                    || task.status == PackageTaskStatus::Canceling
            })
            .count();
        let completed_jobs = runtime
            .package_tasks
            .iter()
            .filter(|task| task.status == PackageTaskStatus::Success)
            .count() as u64;
        ToolStatus {
            summary: if running > 0 {
                format!("正在执行 {running} 个构建任务")
            } else if settings.git_config.username.trim().is_empty() {
                "服务运行中，尚未在控制端配置 Git 凭据".to_owned()
            } else {
                "服务运行中，等待构建任务".to_owned()
            },
            completed_jobs,
        }
    }

    pub async fn find_task_group(&self, group_id: &str) -> Result<TaskGroup, AppError> {
        self.settings
            .read()
            .await
            .task_groups
            .iter()
            .find(|group| group.id == group_id)
            .cloned()
            .ok_or_else(|| AppError::not_found(format!("未找到任务组 {group_id}")))
    }

    /// 将任务组覆盖到项目的旧构建字段，保证旧 Cocos 执行代码仍可使用同一套
    /// Project 结构，同时新参数可以通过 PlaceholderContext 访问。
    pub async fn resolve_task_context(
        &self,
        task: &PackageTask,
    ) -> Result<(Project, TaskGroup), AppError> {
        if !task.task_group_id.trim().is_empty() {
            let group = self.find_task_group(&task.task_group_id).await?;
            let mut project = self.find_project(&group.project_id).await?;
            apply_group_params(&mut project, &group.params);
            return Ok((project, group));
        }

        let legacy = task
            .project
            .clone()
            .ok_or_else(|| AppError::validation(format!("任务 {} 未绑定项目", task.name)))?;
        let project = self.find_project(&legacy.project_id).await?;
        Ok((
            project,
            TaskGroup {
                id: String::new(),
                project_id: legacy.project_id,
                name: task.group.clone(),
                branch: legacy.branch,
                params: BTreeMap::new(),
                order: 0,
            },
        ))
    }

    pub fn restart_guard(&self, task_name: &str) -> RestartGuard {
        self.restart_registry.protect(
            RestartProtection::Blocked,
            RestartBlocker {
                code: "build_queue_running".to_owned(),
                summary: format!("构建队列正在执行：{task_name}"),
                started_at_unix_secs: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                cancellable: true,
            },
        )
    }

    pub async fn find_project(&self, project_ref: &str) -> Result<Project, AppError> {
        self.get_settings()
            .await
            .projects
            .into_iter()
            .find(|project| project.id == project_ref || project.name == project_ref)
            .ok_or_else(|| AppError::not_found(format!("未找到项目 {project_ref}")))
    }

    pub async fn find_engine(&self, engine_name: &str) -> Result<Engine, AppError> {
        self.get_settings()
            .await
            .engines
            .into_iter()
            .find(|engine| engine.name == engine_name)
            .ok_or_else(|| AppError::not_found(format!("未找到引擎 {engine_name}")))
    }

    pub async fn find_task(&self, task_id: &str) -> Result<PackageTask, AppError> {
        self.get_settings()
            .await
            .package_tasks
            .into_iter()
            .find(|task| task.id == task_id)
            .ok_or_else(|| AppError::not_found(format!("未找到任务 {task_id}")))
    }

    pub async fn update_task_runtime<F>(
        &self,
        task_id: &str,
        flush_mode: RuntimeFlushMode,
        updater: F,
    ) -> Result<PackageTaskRuntime, AppError>
    where
        F: FnOnce(&mut PackageTaskRuntime),
    {
        self.ensure_task_exists(task_id).await?;

        let updated = {
            let mut runtime_state = self.runtime_state.write().await;
            let runtime = ensure_runtime_entry(&mut runtime_state.package_tasks, task_id);
            updater(runtime);
            runtime.clone()
        };

        self.mark_runtime_updated();
        if flush_mode == RuntimeFlushMode::Immediate {
            self.flush_runtime_state().await?;
        }
        Ok(updated)
    }

    pub async fn get_task_runtime(&self, task_id: &str) -> Result<PackageTaskRuntime, AppError> {
        self.ensure_task_exists(task_id).await?;
        let runtime_state = self.runtime_state.read().await;
        Ok(runtime_state
            .package_tasks
            .iter()
            .find(|runtime| runtime.task_id == task_id)
            .cloned()
            .unwrap_or_else(|| PackageTaskRuntime {
                task_id: task_id.to_string(),
                ..PackageTaskRuntime::default()
            }))
    }

    pub async fn prepare_tasks_for_build(&self, task_ids: &[String]) -> Result<(), AppError> {
        {
            let settings = self.settings.read().await;
            let mut runtime_state = self.runtime_state.write().await;
            for task_id in task_ids {
                settings
                    .package_tasks
                    .iter()
                    .find(|task| task.id == *task_id)
                    .ok_or_else(|| AppError::not_found(format!("未找到任务 {task_id}")))?;
                let runtime = ensure_runtime_entry(&mut runtime_state.package_tasks, task_id);
                runtime.progress = 0;
                runtime.status = PackageTaskStatus::Pending;
                runtime.last_error = None;
                runtime.last_log_path = None;
                runtime.started_at = None;
                runtime.finished_at = None;
            }
        }

        self.mark_runtime_updated();
        self.flush_runtime_state().await?;
        Ok(())
    }

    pub async fn try_start_build(&self) -> Result<(), AppError> {
        let mut flag = self.build_in_progress.lock().await;
        if *flag {
            return Err(AppError::conflict("已有打包任务在进行中"));
        }
        *flag = true;
        let (sender, _) = watch::channel(false);
        *self.cancel_sender.lock().await = Some(sender);
        Ok(())
    }

    pub async fn cancel_active_build(&self, task_id: &str) -> Result<(), AppError> {
        let active = self.active_task_id.read().await.clone();
        if active.as_deref() != Some(task_id) {
            return Err(AppError::conflict("该任务不是当前正在执行的构建任务"));
        }
        {
            let sender = self.cancel_sender.lock().await;
            let Some(sender) = sender.as_ref() else {
                return Err(AppError::conflict("当前没有可终止的打包任务"));
            };
            sender.send_replace(true);
        }
        self.update_task_runtime(task_id, RuntimeFlushMode::Immediate, |runtime| {
            runtime.status = PackageTaskStatus::Canceling;
            runtime.step_label = "正在终止子进程…".to_owned();
        })
        .await?;
        Ok(())
    }

    pub async fn cancellation_receiver(&self) -> Option<watch::Receiver<bool>> {
        self.cancel_sender
            .lock()
            .await
            .as_ref()
            .map(watch::Sender::subscribe)
    }

    pub async fn set_active_task(&self, task_id: Option<String>) {
        *self.active_task_id.write().await = task_id;
    }

    pub async fn finish_build(&self) {
        let mut flag = self.build_in_progress.lock().await;
        *flag = false;
        *self.cancel_sender.lock().await = None;
        *self.active_task_id.write().await = None;
    }

    pub async fn flush_runtime_state(&self) -> Result<(), AppError> {
        let revision = self.runtime_revision.load(Ordering::SeqCst);
        if revision == self.persisted_runtime_revision.load(Ordering::SeqCst) {
            return Ok(());
        }

        let runtime_state = self.runtime_state.read().await.clone();
        self.persist_runtime_snapshot(&runtime_state).await?;
        info!(revision, "运行态已刷盘");
        Ok(())
    }

    pub async fn create_log_path(&self, task_name: &str) -> Result<PathBuf, AppError> {
        let logs_dir = self.logs_dir();
        fs::create_dir_all(&logs_dir).await?;

        let timestamp = Local::now().format("%Y%m%d_%H%M%S");
        let safe_name = slugify(task_name);
        Ok(logs_dir.join(format!("build_log_{timestamp}_{safe_name}.log")))
    }

    pub fn workspace_project_dir(&self, project: &Project) -> PathBuf {
        self.workspaces_dir().join(&project.workspace_dir_key)
    }

    pub async fn preview_legacy_import(
        &self,
        data_dir: PathBuf,
    ) -> Result<LegacyImportPreview, AppError> {
        let settings_path = data_dir.join("settings.json");
        let settings = load_settings(&settings_path).await?;
        let (_, groups) = preview_task_groups(&settings);
        let prep_project_count = count_legacy_preps(&data_dir.join("preps")).await?;
        Ok(LegacyImportPreview {
            settings_found: settings_path.exists(),
            prep_project_count,
            project_count: settings.projects.len(),
            task_count: settings.package_tasks.len(),
            task_group_count: groups,
            imported: false,
        })
    }

    pub async fn import_legacy(
        &self,
        data_dir: PathBuf,
    ) -> Result<(LegacyImportPreview, GitConfig), AppError> {
        let current = self.settings.read().await;
        if !current.projects.is_empty()
            || !current.package_tasks.is_empty()
            || !current.task_groups.is_empty()
        {
            return Err(AppError::conflict("目标工具已有业务数据，拒绝覆盖导入"));
        }
        drop(current);
        let settings_path = data_dir.join("settings.json");
        let legacy = load_settings(&settings_path).await?;
        if !settings_path.exists() {
            return Err(AppError::not_found(format!(
                "未找到旧设置文件：{}",
                settings_path.display()
            )));
        }
        let imported_git_config = legacy.git_config.clone();
        let (normalized, _) = normalize_settings(legacy);
        let preview = LegacyImportPreview {
            settings_found: true,
            prep_project_count: copy_legacy_preps(&data_dir.join("preps"), &self.preps_dir())
                .await?,
            project_count: normalized.projects.len(),
            task_count: normalized.package_tasks.len(),
            task_group_count: normalized.task_groups.len(),
            imported: true,
        };
        self.save_settings(normalized).await?;
        Ok((preview, imported_git_config))
    }

    fn spawn_runtime_flush_task(&self) {
        let state = self.clone();
        tokio::spawn(async move {
            loop {
                sleep(RUNTIME_FLUSH_INTERVAL).await;
                let runtime_revision = state.runtime_revision.load(Ordering::SeqCst);
                let persisted_revision = state.persisted_runtime_revision.load(Ordering::SeqCst);
                if runtime_revision == persisted_revision {
                    continue;
                }
                if let Err(error) = state.flush_runtime_state().await {
                    warn!(error = %error, "后台运行态刷盘失败");
                }
            }
        });
    }

    async fn ensure_task_exists(&self, task_id: &str) -> Result<(), AppError> {
        let settings = self.settings.read().await;
        settings
            .package_tasks
            .iter()
            .find(|task| task.id == task_id)
            .ok_or_else(|| AppError::not_found(format!("未找到任务 {task_id}")))?;
        Ok(())
    }

    fn mark_runtime_updated(&self) {
        self.runtime_revision.fetch_add(1, Ordering::SeqCst);
    }

    async fn persist_runtime_snapshot(&self, runtime_state: &RuntimeState) -> Result<(), AppError> {
        persist_runtime_state(&self.runtime_path, runtime_state).await?;
        let revision = self.runtime_revision.load(Ordering::SeqCst);
        self.persisted_runtime_revision
            .store(revision, Ordering::SeqCst);
        Ok(())
    }
}

pub async fn load_settings(path: &Path) -> Result<AppSettings, AppError> {
    if !path.exists() {
        return Ok(AppSettings::default());
    }

    let raw = fs::read(path).await?;
    Ok(serde_json::from_slice(&raw)?)
}

pub async fn persist_settings(path: &Path, settings: &AppSettings) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let content = serde_json::to_vec_pretty(settings)?;
    fs::write(path, content).await?;
    Ok(())
}

pub async fn load_runtime_state(path: &Path) -> Result<RuntimeState, AppError> {
    if !path.exists() {
        return Ok(RuntimeState::default());
    }

    let raw = fs::read(path).await?;
    Ok(serde_json::from_slice(&raw)?)
}

pub async fn persist_runtime_state(
    path: &Path,
    runtime_state: &RuntimeState,
) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let content = serde_json::to_vec_pretty(runtime_state)?;
    fs::write(path, content).await?;
    Ok(())
}

fn extract_runtime_state(settings: &AppSettings) -> RuntimeState {
    RuntimeState {
        package_tasks: settings
            .package_tasks
            .iter()
            .map(task_runtime_from_task)
            .collect(),
    }
}

pub fn strip_runtime_from_settings(mut settings: AppSettings) -> AppSettings {
    // 凭据只由桌面控制端的 config/tool-settings.json 保存。旧 settings.json
    // 在读取时仍支持该字段，以便一次性迁移，但绝不再写回业务数据目录。
    settings.git_config = GitConfig::default();
    settings.package_tasks = settings
        .package_tasks
        .into_iter()
        .map(strip_runtime_from_task)
        .collect();
    settings
}

fn strip_runtime_from_task(mut task: PackageTask) -> PackageTask {
    task.progress = 0;
    task.status = PackageTaskStatus::Pending;
    task.last_log_path = None;
    task.last_error = None;
    task.started_at = None;
    task.finished_at = None;
    task
}

fn apply_runtime_state(mut settings: AppSettings, runtime_state: &RuntimeState) -> AppSettings {
    let runtime_map = runtime_state
        .package_tasks
        .iter()
        .map(|runtime| (runtime.task_id.as_str(), runtime))
        .collect::<HashMap<_, _>>();

    for task in &mut settings.package_tasks {
        if let Some(runtime) = runtime_map.get(task.id.as_str()) {
            task.progress = runtime.progress;
            task.status = runtime.status.clone();
            task.last_log_path = runtime.last_log_path.clone();
            task.last_error = runtime.last_error.clone();
            task.started_at = runtime.started_at.clone();
            task.finished_at = runtime.finished_at.clone();
        }
    }

    settings
}

fn normalize_runtime_state_on_startup(mut runtime_state: RuntimeState) -> (RuntimeState, bool) {
    let mut changed = false;
    let finished_at = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    for runtime in &mut runtime_state.package_tasks {
        if runtime.status != PackageTaskStatus::Running
            && runtime.status != PackageTaskStatus::Canceling
        {
            continue;
        }

        runtime.status = PackageTaskStatus::Failed;
        runtime.last_error = Some("后端已重启，上一次打包被中断".to_string());
        runtime.finished_at = Some(finished_at.clone());
        changed = true;
    }

    (runtime_state, changed)
}

fn normalize_settings(mut settings: AppSettings) -> (AppSettings, bool) {
    let mut changed = false;
    let mut used_project_ids = HashSet::new();
    let mut used_workspace_keys = HashSet::new();

    for project in &mut settings.projects {
        let normalized_project_id =
            normalize_project_field(&project.id, &mut used_project_ids, generate_project_id);
        if project.id != normalized_project_id {
            project.id = normalized_project_id;
            changed = true;
        }

        let normalized_workspace_key = normalize_project_field(
            &project.workspace_dir_key,
            &mut used_workspace_keys,
            generate_workspace_dir_key,
        );
        if project.workspace_dir_key != normalized_workspace_key {
            project.workspace_dir_key = normalized_workspace_key;
            changed = true;
        }
    }

    let project_ids = settings
        .projects
        .iter()
        .map(|project| project.id.clone())
        .collect::<HashSet<_>>();
    let project_name_to_id = settings
        .projects
        .iter()
        .map(|project| (project.name.clone(), project.id.clone()))
        .collect::<HashMap<_, _>>();

    for task in &mut settings.package_tasks {
        let Some(task_project) = task.project.as_mut() else {
            continue;
        };

        let project_ref = task_project.project_id.trim().to_string();
        if project_ref.is_empty() {
            task.project = None;
            changed = true;
            continue;
        }

        if project_ids.contains(&project_ref) {
            if task_project.project_id != project_ref {
                task_project.project_id = project_ref;
                changed = true;
            }
            continue;
        }

        if let Some(project_id) = project_name_to_id.get(&project_ref) {
            if &task_project.project_id != project_id {
                task_project.project_id = project_id.clone();
                changed = true;
            }
            continue;
        }

        task.project = None;
        changed = true;
    }

    if settings.param_definitions.is_empty() && !settings.package_tasks.is_empty() {
        settings.param_definitions = legacy_param_definitions();
        changed = true;
    }

    let project_by_id = settings
        .projects
        .iter()
        .map(|project| (project.id.clone(), project.clone()))
        .collect::<HashMap<_, _>>();
    let mut group_by_legacy_binding = settings
        .task_groups
        .iter()
        .map(|group| {
            (
                (
                    group.project_id.clone(),
                    group.name.clone(),
                    group.branch.clone(),
                ),
                group.id.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut used_group_ids = settings
        .task_groups
        .iter()
        .map(|group| group.id.clone())
        .collect::<HashSet<_>>();
    let mut next_order = settings
        .task_groups
        .iter()
        .map(|group| group.order)
        .max()
        .unwrap_or(0)
        .saturating_add(1);

    for (task_index, task) in settings.package_tasks.iter_mut().enumerate() {
        if !task.task_group_id.trim().is_empty() {
            continue;
        }
        let Some(task_project) = task.project.as_ref() else {
            continue;
        };
        let project_id = task_project.project_id.clone();
        let name = if task.group.trim().is_empty() {
            "未分组".to_owned()
        } else {
            task.group.trim().to_owned()
        };
        let branch = task_project.branch.trim().to_owned();
        let key = (project_id.clone(), name.clone(), branch.clone());
        let group_id = if let Some(existing) = group_by_legacy_binding.get(&key) {
            existing.clone()
        } else {
            let id = loop {
                let candidate = format!("group_{}", generate_short_internal_id());
                if used_group_ids.insert(candidate.clone()) {
                    break candidate;
                }
            };
            let params = project_by_id
                .get(&project_id)
                .map(legacy_project_params)
                .unwrap_or_default();
            settings.task_groups.push(TaskGroup {
                id: id.clone(),
                project_id,
                name,
                branch,
                params,
                order: next_order,
            });
            next_order = next_order.saturating_add(1);
            group_by_legacy_binding.insert(key, id.clone());
            id
        };
        task.task_group_id = group_id;
        task.order = task_index as u32;
        changed = true;
    }

    for group in &mut settings.task_groups {
        if group.params.is_empty()
            && let Some(project) = project_by_id.get(&group.project_id)
        {
            group.params = legacy_project_params(project);
            changed = true;
        }
    }

    (settings, changed)
}

fn legacy_param_definitions() -> Vec<ParamDefinition> {
    vec![
        ParamDefinition {
            key: "version".to_owned(),
            label: "版本号".to_owned(),
            kind: ParamKind::Text,
            default_value: json!("1.0.0"),
            required: true,
            order: 0,
            description: "主版本号".to_owned(),
            ..ParamDefinition::default()
        },
        ParamDefinition {
            key: "minor_version".to_owned(),
            label: "小版本".to_owned(),
            kind: ParamKind::Number,
            default_value: json!(0),
            required: true,
            order: 1,
            description: String::new(),
            ..ParamDefinition::default()
        },
        ParamDefinition {
            key: "build_mode".to_owned(),
            label: "构建模式".to_owned(),
            kind: ParamKind::Select,
            options: vec!["test".to_owned(), "pre".to_owned(), "release".to_owned()],
            default_value: json!("release"),
            required: true,
            order: 2,
            description: String::new(),
        },
        ParamDefinition {
            key: "is_hot_update".to_owned(),
            label: "热更新".to_owned(),
            kind: ParamKind::Switch,
            default_value: json!(false),
            required: false,
            order: 3,
            description: String::new(),
            ..ParamDefinition::default()
        },
        ParamDefinition {
            key: "enable_pay".to_owned(),
            label: "支付".to_owned(),
            kind: ParamKind::Switch,
            default_value: json!(false),
            required: false,
            order: 4,
            description: String::new(),
            ..ParamDefinition::default()
        },
        ParamDefinition {
            key: "review_mode".to_owned(),
            label: "审核模式".to_owned(),
            kind: ParamKind::Switch,
            default_value: json!(false),
            required: false,
            order: 5,
            description: String::new(),
            ..ParamDefinition::default()
        },
    ]
}

fn legacy_project_params(project: &Project) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("version".to_owned(), json!(project.version)),
        ("minor_version".to_owned(), json!(project.minor_version)),
        ("build_mode".to_owned(), json!(project.build_mode.as_str())),
        ("is_hot_update".to_owned(), json!(project.is_hot_update)),
        ("enable_pay".to_owned(), json!(project.enable_pay)),
        ("review_mode".to_owned(), json!(project.review_mode)),
    ])
}

fn apply_group_params(project: &mut Project, params: &BTreeMap<String, Value>) {
    if let Some(value) = params.get("version").and_then(Value::as_str) {
        project.version = value.to_owned();
    }
    if let Some(value) = params.get("minor_version").and_then(Value::as_u64) {
        project.minor_version = value as u32;
    }
    if let Some(value) = params.get("build_mode").and_then(Value::as_str) {
        project.build_mode = match value {
            "test" => BuildMode::Test,
            "pre" => BuildMode::Pre,
            _ => BuildMode::Release,
        };
    }
    if let Some(value) = params.get("is_hot_update").and_then(Value::as_bool) {
        project.is_hot_update = value;
    }
    if let Some(value) = params.get("enable_pay").and_then(Value::as_bool) {
        project.enable_pay = value;
    }
    if let Some(value) = params.get("review_mode").and_then(Value::as_bool) {
        project.review_mode = value;
    }
}

fn preview_task_groups(settings: &AppSettings) -> (AppSettings, usize) {
    let (normalized, _) = normalize_settings(settings.clone());
    let count = normalized.task_groups.len();
    (normalized, count)
}

async fn count_legacy_preps(path: &Path) -> Result<usize, AppError> {
    if !path.exists() {
        return Ok(0);
    }
    let mut entries = fs::read_dir(path).await?;
    let mut count = 0;
    while entries.next_entry().await?.is_some() {
        count += 1;
    }
    Ok(count)
}

async fn copy_legacy_preps(source: &Path, destination: &Path) -> Result<usize, AppError> {
    let count = count_legacy_preps(source).await?;
    if count == 0 {
        return Ok(0);
    }
    let source = source.to_path_buf();
    let destination = destination.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<(), std::io::Error> {
        for entry in walkdir::WalkDir::new(&source) {
            let entry = entry.map_err(std::io::Error::other)?;
            let relative = entry
                .path()
                .strip_prefix(&source)
                .map_err(std::io::Error::other)?;
            let target = destination.join(relative);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(target)?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| AppError::internal(format!("复制旧准备项目失败: {error}")))??;
    Ok(count)
}

fn normalize_project_field(
    raw: &str,
    used: &mut HashSet<String>,
    fallback: fn() -> String,
) -> String {
    let trimmed = raw.trim();
    if !trimmed.is_empty() && used.insert(trimmed.to_string()) {
        return trimmed.to_string();
    }

    loop {
        let candidate = fallback();
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
}

fn generate_project_id() -> String {
    generate_short_internal_id()
}

fn generate_workspace_dir_key() -> String {
    generate_short_internal_id()
}

fn generate_short_internal_id() -> String {
    let counter = GENERATED_PROJECT_FIELD_COUNTER.fetch_add(1, Ordering::SeqCst);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let raw = format!("{nanos:x}{counter:x}");
    if raw.len() <= 12 {
        raw
    } else {
        raw[raw.len() - 12..].to_string()
    }
}

#[cfg(test)]
fn filter_runtime_state_for_settings(
    settings: &AppSettings,
    runtime_state: RuntimeState,
) -> RuntimeState {
    let task_ids = settings
        .package_tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<HashSet<_>>();

    RuntimeState {
        package_tasks: runtime_state
            .package_tasks
            .into_iter()
            .filter(|runtime| task_ids.contains(runtime.task_id.as_str()))
            .collect(),
    }
}

fn task_runtime_from_task(task: &PackageTask) -> PackageTaskRuntime {
    PackageTaskRuntime {
        task_id: task.id.clone(),
        progress: task.progress,
        step_label: String::new(),
        status: task.status.clone(),
        last_log_path: task.last_log_path.clone(),
        last_error: task.last_error.clone(),
        started_at: task.started_at.clone(),
        finished_at: task.finished_at.clone(),
    }
}

fn ensure_runtime_entry<'a>(
    entries: &'a mut Vec<PackageTaskRuntime>,
    task_id: &str,
) -> &'a mut PackageTaskRuntime {
    if let Some(index) = entries.iter().position(|item| item.task_id == task_id) {
        &mut entries[index]
    } else {
        entries.push(PackageTaskRuntime {
            task_id: task_id.to_string(),
            ..PackageTaskRuntime::default()
        });
        entries
            .last_mut()
            .expect("runtime entry should exist immediately after push")
    }
}

pub fn slugify(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut last_was_underscore = false;

    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            result.push(ch.to_ascii_lowercase());
            last_was_underscore = false;
        } else if !last_was_underscore {
            result.push('_');
            last_was_underscore = true;
        }
    }

    let trimmed = result.trim_matches('_');
    if trimmed.is_empty() {
        "workspace".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::models::{ObfuscationMode, Project, TaskPrepAction, TaskProjectConfig};

    fn sample_task() -> PackageTask {
        PackageTask {
            id: "task_1".to_string(),
            name: "demo".to_string(),
            group: "group".to_string(),
            task_group_id: String::new(),
            order: 0,
            progress: 80,
            status: PackageTaskStatus::Running,
            checked: true,
            project: None,
            code_repo_url: String::new(),
            asset_repo_url: String::new(),
            build_args_json: "{}".to_string(),
            enable_obfuscation: false,
            obfuscation_mode: ObfuscationMode::Classic,
            obfuscation_seed: None,
            enable_dead_code_injection: false,
            dead_code_injection_count: 200,
            pre_build_actions: Vec::<TaskPrepAction>::new(),
            post_build_actions: Vec::<TaskPrepAction>::new(),
            last_log_path: Some("/tmp/log".to_string()),
            last_error: Some("boom".to_string()),
            started_at: Some("start".to_string()),
            finished_at: Some("end".to_string()),
        }
    }

    fn temp_data_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        std::env::temp_dir().join(format!("cocos_build_state_test_{name}_{unique}"))
    }

    #[test]
    fn strip_runtime_should_keep_static_fields() {
        let task = sample_task();
        let stripped = strip_runtime_from_task(task.clone());

        assert_eq!(stripped.id, task.id);
        assert_eq!(stripped.name, task.name);
        assert_eq!(stripped.checked, task.checked);
        assert_eq!(stripped.status, PackageTaskStatus::Pending);
        assert_eq!(stripped.progress, 0);
        assert_eq!(stripped.last_log_path, None);
        assert_eq!(stripped.last_error, None);
    }

    #[test]
    fn apply_runtime_should_override_task_runtime_fields() {
        let settings = AppSettings {
            package_tasks: vec![strip_runtime_from_task(sample_task())],
            ..AppSettings::default()
        };
        let runtime_state = RuntimeState {
            package_tasks: vec![PackageTaskRuntime {
                task_id: "task_1".to_string(),
                progress: 66,
                step_label: String::new(),
                status: PackageTaskStatus::Failed,
                last_log_path: Some("/tmp/runtime.log".to_string()),
                last_error: Some("runtime".to_string()),
                started_at: Some("s".to_string()),
                finished_at: Some("f".to_string()),
            }],
        };

        let merged = apply_runtime_state(settings, &runtime_state);
        let task = &merged.package_tasks[0];
        assert_eq!(task.progress, 66);
        assert_eq!(task.status, PackageTaskStatus::Failed);
        assert_eq!(task.last_log_path.as_deref(), Some("/tmp/runtime.log"));
        assert_eq!(task.last_error.as_deref(), Some("runtime"));
    }

    #[test]
    fn filter_runtime_should_drop_deleted_tasks() {
        let settings = AppSettings {
            package_tasks: vec![PackageTask {
                id: "task_2".to_string(),
                ..PackageTask::default()
            }],
            ..AppSettings::default()
        };
        let runtime_state = RuntimeState {
            package_tasks: vec![
                PackageTaskRuntime {
                    task_id: "task_1".to_string(),
                    ..PackageTaskRuntime::default()
                },
                PackageTaskRuntime {
                    task_id: "task_2".to_string(),
                    ..PackageTaskRuntime::default()
                },
            ],
        };

        let filtered = filter_runtime_state_for_settings(&settings, runtime_state);
        assert_eq!(filtered.package_tasks.len(), 1);
        assert_eq!(filtered.package_tasks[0].task_id, "task_2");
    }

    #[test]
    fn normalize_runtime_state_on_startup_should_mark_running_tasks_as_failed() {
        let runtime_state = RuntimeState {
            package_tasks: vec![
                PackageTaskRuntime {
                    task_id: "task_1".to_string(),
                    progress: 60,
                    status: PackageTaskStatus::Running,
                    started_at: Some("2026-03-24 10:00:00".to_string()),
                    ..PackageTaskRuntime::default()
                },
                PackageTaskRuntime {
                    task_id: "task_2".to_string(),
                    progress: 10,
                    status: PackageTaskStatus::Pending,
                    ..PackageTaskRuntime::default()
                },
            ],
        };

        let (normalized, changed) = normalize_runtime_state_on_startup(runtime_state);

        assert!(changed);
        assert_eq!(
            normalized.package_tasks[0].status,
            PackageTaskStatus::Failed
        );
        assert_eq!(
            normalized.package_tasks[0].last_error.as_deref(),
            Some("后端已重启，上一次打包被中断")
        );
        assert!(normalized.package_tasks[0].finished_at.is_some());
        assert_eq!(normalized.package_tasks[0].progress, 60);
        assert_eq!(
            normalized.package_tasks[1].status,
            PackageTaskStatus::Pending
        );
    }

    #[test]
    fn normalize_settings_should_generate_unique_project_identity_fields_and_migrate_legacy_task_binding()
     {
        let settings = AppSettings {
            projects: vec![
                Project {
                    name: "演示项目一".to_string(),
                    git_url: "https://example.com/a.git".to_string(),
                    engine_name: "cocos".to_string(),
                    ..Project::default()
                },
                Project {
                    name: "演示项目二".to_string(),
                    git_url: "https://example.com/b.git".to_string(),
                    engine_name: "cocos".to_string(),
                    ..Project::default()
                },
            ],
            package_tasks: vec![PackageTask {
                id: "task_1".to_string(),
                project: Some(TaskProjectConfig {
                    project_id: "演示项目一".to_string(),
                    branch: "main".to_string(),
                }),
                ..PackageTask::default()
            }],
            ..AppSettings::default()
        };

        let (normalized, changed) = normalize_settings(settings);

        assert!(changed);
        assert_eq!(normalized.projects.len(), 2);
        assert!(!normalized.projects[0].id.is_empty());
        assert!(!normalized.projects[0].workspace_dir_key.is_empty());
        assert_ne!(normalized.projects[0].id, normalized.projects[1].id);
        assert_ne!(
            normalized.projects[0].workspace_dir_key,
            normalized.projects[1].workspace_dir_key
        );
        assert_eq!(
            normalized.package_tasks[0]
                .project
                .as_ref()
                .expect("task project should exist")
                .project_id,
            normalized.projects[0].id
        );
    }

    #[test]
    fn normalize_settings_should_preserve_existing_workspace_key_for_renamed_project() {
        let settings = AppSettings {
            projects: vec![Project {
                id: "project_1".to_string(),
                name: "新的展示名".to_string(),
                workspace_dir_key: "fixed_workspace".to_string(),
                git_url: "https://example.com/demo.git".to_string(),
                engine_name: "cocos".to_string(),
                ..Project::default()
            }],
            package_tasks: vec![PackageTask {
                id: "task_1".to_string(),
                project: Some(TaskProjectConfig {
                    project_id: "project_1".to_string(),
                    branch: "main".to_string(),
                }),
                ..PackageTask::default()
            }],
            ..AppSettings::default()
        };

        let (normalized, changed) = normalize_settings(settings);

        assert!(changed, "旧任务绑定应迁移为任务组与动态参数");
        assert_eq!(normalized.projects[0].workspace_dir_key, "fixed_workspace");
        assert_eq!(normalized.projects[0].name, "新的展示名");
        assert_eq!(normalized.task_groups.len(), 1);
        assert_eq!(
            normalized.package_tasks[0].task_group_id,
            normalized.task_groups[0].id
        );
    }

    #[test]
    fn legacy_group_migration_splits_same_group_name_by_branch() {
        let settings = AppSettings {
            projects: vec![Project {
                id: "project_1".to_owned(),
                workspace_dir_key: "workspace_1".to_owned(),
                name: "项目".to_owned(),
                ..Project::default()
            }],
            package_tasks: vec![
                PackageTask {
                    id: "task_main".to_owned(),
                    group: "发布包".to_owned(),
                    project: Some(TaskProjectConfig {
                        project_id: "project_1".to_owned(),
                        branch: "main".to_owned(),
                    }),
                    ..PackageTask::default()
                },
                PackageTask {
                    id: "task_hotfix".to_owned(),
                    group: "发布包".to_owned(),
                    project: Some(TaskProjectConfig {
                        project_id: "project_1".to_owned(),
                        branch: "hotfix/1.0.1".to_owned(),
                    }),
                    ..PackageTask::default()
                },
            ],
            ..AppSettings::default()
        };

        let (normalized, changed) = normalize_settings(settings);

        assert!(changed);
        assert_eq!(normalized.task_groups.len(), 2);
        assert_ne!(
            normalized.package_tasks[0].task_group_id,
            normalized.package_tasks[1].task_group_id
        );
        assert!(
            normalized
                .task_groups
                .iter()
                .any(|group| group.branch == "main")
        );
        assert!(
            normalized
                .task_groups
                .iter()
                .any(|group| group.branch == "hotfix/1.0.1")
        );
    }

    #[tokio::test]
    async fn load_should_recover_stale_running_runtime_after_restart() {
        let data_dir = temp_data_dir("restart_recovery");
        let state = AppState::load(data_dir.clone()).await;
        state
            .save_settings(AppSettings {
                package_tasks: vec![PackageTask {
                    id: "task_1".to_string(),
                    ..PackageTask::default()
                }],
                ..AppSettings::default()
            })
            .await
            .expect("save settings");
        state
            .update_task_runtime("task_1", RuntimeFlushMode::Immediate, |runtime| {
                runtime.progress = 45;
                runtime.status = PackageTaskStatus::Running;
                runtime.started_at = Some("2026-03-24 11:00:00".to_string());
            })
            .await
            .expect("update runtime");

        let restarted = AppState::load(data_dir.clone()).await;
        let runtime = restarted
            .get_task_runtime("task_1")
            .await
            .expect("get task runtime");

        assert_eq!(runtime.status, PackageTaskStatus::Failed);
        assert_eq!(runtime.progress, 45);
        assert_eq!(
            runtime.last_error.as_deref(),
            Some("后端已重启，上一次打包被中断")
        );
        assert!(runtime.finished_at.is_some());

        let _ = fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn stop_request_marks_only_active_task_canceling_and_signals_queue() {
        let data_dir = temp_data_dir("stop_queue");
        let state = AppState::load(data_dir.clone()).await;
        state
            .save_settings(AppSettings {
                package_tasks: vec![PackageTask {
                    id: "task_1".to_owned(),
                    ..PackageTask::default()
                }],
                ..AppSettings::default()
            })
            .await
            .expect("settings");
        state.try_start_build().await.expect("start build");
        state.set_active_task(Some("task_1".to_owned())).await;
        let receiver = state.cancellation_receiver().await.expect("receiver");

        state
            .cancel_active_build("task_1")
            .await
            .expect("cancel active task");

        assert!(*receiver.borrow());
        assert_eq!(
            state
                .get_task_runtime("task_1")
                .await
                .expect("runtime")
                .status,
            PackageTaskStatus::Canceling
        );
        state.finish_build().await;
        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn flush_runtime_state_should_persist_latest_runtime() {
        let data_dir = temp_data_dir("flush");
        let state = AppState::load(data_dir.clone()).await;
        state
            .save_settings(AppSettings {
                package_tasks: vec![PackageTask {
                    id: "task_1".to_string(),
                    ..PackageTask::default()
                }],
                ..AppSettings::default()
            })
            .await
            .expect("save settings");

        state
            .update_task_runtime("task_1", RuntimeFlushMode::Deferred, |runtime| {
                runtime.progress = 42;
                runtime.status = PackageTaskStatus::Running;
            })
            .await
            .expect("update runtime");
        state.flush_runtime_state().await.expect("flush runtime");

        let runtime = load_runtime_state(&data_dir.join("runtime_state.json"))
            .await
            .expect("load runtime");
        assert_eq!(runtime.package_tasks[0].progress, 42);

        let _ = fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn save_settings_should_drop_removed_runtime_entries() {
        let data_dir = temp_data_dir("filter");
        let state = AppState::load(data_dir.clone()).await;
        state
            .save_settings(AppSettings {
                package_tasks: vec![
                    PackageTask {
                        id: "task_1".to_string(),
                        ..PackageTask::default()
                    },
                    PackageTask {
                        id: "task_2".to_string(),
                        ..PackageTask::default()
                    },
                ],
                ..AppSettings::default()
            })
            .await
            .expect("save initial settings");
        state
            .update_task_runtime("task_1", RuntimeFlushMode::Immediate, |runtime| {
                runtime.progress = 10;
            })
            .await
            .expect("update task_1");
        state
            .update_task_runtime("task_2", RuntimeFlushMode::Immediate, |runtime| {
                runtime.progress = 20;
            })
            .await
            .expect("update task_2");

        state
            .save_settings(AppSettings {
                package_tasks: vec![PackageTask {
                    id: "task_2".to_string(),
                    ..PackageTask::default()
                }],
                ..AppSettings::default()
            })
            .await
            .expect("save filtered settings");

        let runtime = load_runtime_state(&data_dir.join("runtime_state.json"))
            .await
            .expect("load filtered runtime");
        assert_eq!(runtime.package_tasks.len(), 1);
        assert_eq!(runtime.package_tasks[0].task_id, "task_2");

        let _ = fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn save_settings_should_preserve_runtime_for_existing_tasks() {
        let data_dir = temp_data_dir("preserve_runtime");
        let state = AppState::load(data_dir.clone()).await;
        state
            .save_settings(AppSettings {
                package_tasks: vec![PackageTask {
                    id: "task_1".to_string(),
                    name: "demo".to_string(),
                    ..PackageTask::default()
                }],
                ..AppSettings::default()
            })
            .await
            .expect("save initial settings");

        state
            .update_task_runtime("task_1", RuntimeFlushMode::Immediate, |runtime| {
                runtime.progress = 55;
                runtime.status = PackageTaskStatus::Running;
                runtime.last_error = Some("still running".to_string());
            })
            .await
            .expect("update runtime");

        state
            .save_settings(AppSettings {
                package_tasks: vec![PackageTask {
                    id: "task_1".to_string(),
                    name: "demo-updated".to_string(),
                    ..PackageTask::default()
                }],
                ..AppSettings::default()
            })
            .await
            .expect("save updated settings");

        let merged = state.get_settings().await;
        let task = merged
            .package_tasks
            .into_iter()
            .find(|task| task.id == "task_1")
            .expect("task should exist");
        assert_eq!(task.name, "demo-updated");
        assert_eq!(task.progress, 55);
        assert_eq!(task.status, PackageTaskStatus::Running);
        assert_eq!(task.last_error.as_deref(), Some("still running"));

        let runtime = state
            .get_task_runtime("task_1")
            .await
            .expect("get runtime after save");
        assert_eq!(runtime.progress, 55);
        assert_eq!(runtime.status, PackageTaskStatus::Running);

        let _ = fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn update_task_runtime_should_preserve_existing_fields_when_only_progress_changes() {
        let data_dir = temp_data_dir("preserve_fields_on_progress");
        let state = AppState::load(data_dir.clone()).await;
        state
            .save_settings(AppSettings {
                package_tasks: vec![PackageTask {
                    id: "task_1".to_string(),
                    ..PackageTask::default()
                }],
                ..AppSettings::default()
            })
            .await
            .expect("save settings");

        state
            .update_task_runtime("task_1", RuntimeFlushMode::Immediate, |runtime| {
                runtime.progress = 5;
                runtime.status = PackageTaskStatus::Running;
                runtime.started_at = Some("2026-03-23 15:00:00".to_string());
                runtime.last_log_path = Some("/tmp/demo.log".to_string());
            })
            .await
            .expect("set initial runtime");

        state
            .update_task_runtime("task_1", RuntimeFlushMode::Immediate, |runtime| {
                runtime.progress = 20;
            })
            .await
            .expect("update progress only");

        let runtime = state
            .get_task_runtime("task_1")
            .await
            .expect("get updated runtime");
        assert_eq!(runtime.progress, 20);
        assert_eq!(runtime.status, PackageTaskStatus::Running);
        assert_eq!(runtime.started_at.as_deref(), Some("2026-03-23 15:00:00"));
        assert_eq!(runtime.last_log_path.as_deref(), Some("/tmp/demo.log"));

        let _ = fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn update_task_runtime_should_preserve_existing_fields_when_only_error_changes() {
        let data_dir = temp_data_dir("preserve_fields_on_error");
        let state = AppState::load(data_dir.clone()).await;
        state
            .save_settings(AppSettings {
                package_tasks: vec![PackageTask {
                    id: "task_1".to_string(),
                    ..PackageTask::default()
                }],
                ..AppSettings::default()
            })
            .await
            .expect("save settings");

        state
            .update_task_runtime("task_1", RuntimeFlushMode::Immediate, |runtime| {
                runtime.progress = 66;
                runtime.status = PackageTaskStatus::Running;
                runtime.started_at = Some("2026-03-23 15:05:00".to_string());
            })
            .await
            .expect("set initial runtime");

        state
            .update_task_runtime("task_1", RuntimeFlushMode::Immediate, |runtime| {
                runtime.last_error = Some("boom".to_string());
                runtime.finished_at = Some("2026-03-23 15:06:00".to_string());
            })
            .await
            .expect("update error only");

        let runtime = state
            .get_task_runtime("task_1")
            .await
            .expect("get updated runtime");
        assert_eq!(runtime.progress, 66);
        assert_eq!(runtime.status, PackageTaskStatus::Running);
        assert_eq!(runtime.started_at.as_deref(), Some("2026-03-23 15:05:00"));
        assert_eq!(runtime.last_error.as_deref(), Some("boom"));
        assert_eq!(runtime.finished_at.as_deref(), Some("2026-03-23 15:06:00"));

        let _ = fs::remove_dir_all(data_dir).await;
    }
}
