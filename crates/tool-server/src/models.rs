use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Deserializer, Serialize, de::Error as DeError};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default)]
    pub engines: Vec<Engine>,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub git_config: GitConfig,
    #[serde(default)]
    pub feishu_bots: Vec<FeishuBotConfig>,
    #[serde(default)]
    pub package_tasks: Vec<PackageTask>,
    #[serde(default)]
    pub task_groups: Vec<TaskGroup>,
    #[serde(default)]
    pub param_definitions: Vec<ParamDefinition>,
}

/// Web 端可读写的设置。Git 凭据故意不在该类型中，避免被 LAN 页面或
/// 浏览器日志意外暴露。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PublicSettings {
    #[serde(default)]
    pub engines: Vec<Engine>,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub feishu_bots: Vec<FeishuBotConfig>,
    #[serde(default)]
    pub package_tasks: Vec<PackageTask>,
    #[serde(default)]
    pub task_groups: Vec<TaskGroup>,
    #[serde(default)]
    pub param_definitions: Vec<ParamDefinition>,
    #[serde(default)]
    pub git_credentials_configured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PublicSettingsUpdate {
    #[serde(default)]
    pub engines: Vec<Engine>,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub feishu_bots: Vec<FeishuBotConfig>,
    #[serde(default)]
    pub param_definitions: Vec<ParamDefinition>,
}

impl From<&AppSettings> for PublicSettings {
    fn from(value: &AppSettings) -> Self {
        Self {
            engines: value.engines.clone(),
            projects: value.projects.clone(),
            feishu_bots: value.feishu_bots.clone(),
            package_tasks: value.package_tasks.clone(),
            task_groups: value.task_groups.clone(),
            param_definitions: value.param_definitions.clone(),
            git_credentials_configured: !value.git_config.username.trim().is_empty()
                || !value.git_config.password.trim().is_empty(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Engine {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BuildMode {
    Test,
    Pre,
    #[default]
    Release,
}

impl BuildMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Pre => "pre",
            Self::Release => "release",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub workspace_dir_key: String,
    pub git_url: String,
    pub engine_name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub minor_version: u32,
    #[serde(default)]
    pub build_mode: BuildMode,
    #[serde(default)]
    pub is_hot_update: bool,
    #[serde(default)]
    pub enable_pay: bool,
    #[serde(default)]
    pub review_mode: bool,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            workspace_dir_key: String::new(),
            git_url: String::new(),
            engine_name: String::new(),
            version: default_version(),
            minor_version: 0,
            build_mode: BuildMode::Release,
            is_hot_update: false,
            enable_pay: false,
            review_mode: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitConfig {
    pub username: String,
    pub password: String,
}

impl From<cocos_build_lan_contract::GitConfig> for GitConfig {
    fn from(value: cocos_build_lan_contract::GitConfig) -> Self {
        Self {
            username: value.username,
            password: value.password,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FeishuBotConfig {
    pub id: String,
    pub name: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskGroup {
    pub id: String,
    pub project_id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub branch: String,
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
    #[serde(default)]
    pub order: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    #[default]
    Text,
    Number,
    Switch,
    Select,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ParamDefinition {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: ParamKind,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub default_value: Value,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub order: u32,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PrepParamType {
    #[default]
    Str,
    Int,
    Bool,
    Select,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PrepParamValueSource {
    #[default]
    Runtime,
    Fixed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrepParamOption {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrepParam {
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: PrepParamType,
    #[serde(default)]
    pub value_source: PrepParamValueSource,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub options: Vec<PrepParamOption>,
    #[serde(default)]
    pub fixed_value: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PrepProject {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub description: String,
    pub create_time: String,
    #[serde(default)]
    pub params: Vec<PrepParam>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskPrepTarget {
    #[serde(default)]
    pub prep_project_id: String,
    #[serde(default)]
    pub params: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskPrepActionKind {
    #[default]
    Single,
    Conditional,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind")]
pub enum TaskPrepAction {
    #[serde(rename = "single")]
    Single {
        #[serde(rename = "prepProjectId", default)]
        prep_project_id: String,
        #[serde(default)]
        params: HashMap<String, Value>,
    },
    #[serde(rename = "conditional")]
    Conditional {
        #[serde(rename = "conditionSource", default)]
        condition_source: String,
        #[serde(rename = "conditionEquals", default)]
        condition_equals: String,
        #[serde(rename = "onMatchTargets", default)]
        on_match_targets: Vec<TaskPrepTarget>,
        #[serde(rename = "onMismatchTargets", default)]
        on_mismatch_targets: Vec<TaskPrepTarget>,
    },
}

impl Default for TaskPrepAction {
    fn default() -> Self {
        Self::Single {
            prep_project_id: String::new(),
            params: HashMap::new(),
        }
    }
}

impl<'de> Deserialize<'de> for TaskPrepAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        if let Ok(tagged) = serde_json::from_value::<TaggedTaskPrepActionWire>(value.clone()) {
            return Ok(tagged.into());
        }

        let legacy =
            serde_json::from_value::<LegacyTaskPrepActionWire>(value).map_err(D::Error::custom)?;
        legacy.try_into().map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskProjectConfig {
    #[serde(default, alias = "projectName")]
    pub project_id: String,
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PackageTaskStatus {
    #[default]
    Pending,
    Running,
    Canceling,
    Canceled,
    Success,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ObfuscationMode {
    #[default]
    Classic,
    Ast,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PackageTask {
    pub id: String,
    pub name: String,
    pub group: String,
    #[serde(default)]
    pub task_group_id: String,
    #[serde(default)]
    pub order: u32,
    #[serde(default)]
    pub progress: u8,
    #[serde(default)]
    pub status: PackageTaskStatus,
    #[serde(default)]
    pub checked: bool,
    #[serde(default)]
    pub project: Option<TaskProjectConfig>,
    #[serde(default)]
    pub code_repo_url: String,
    #[serde(default)]
    pub asset_repo_url: String,
    #[serde(default)]
    pub build_args_json: String,
    #[serde(default)]
    pub enable_obfuscation: bool,
    #[serde(default)]
    pub obfuscation_mode: ObfuscationMode,
    #[serde(default)]
    pub obfuscation_seed: Option<u64>,
    #[serde(default)]
    pub enable_dead_code_injection: bool,
    #[serde(default = "default_dead_code_injection_count")]
    pub dead_code_injection_count: u32,
    #[serde(default)]
    pub pre_build_actions: Vec<TaskPrepAction>,
    #[serde(default)]
    pub post_build_actions: Vec<TaskPrepAction>,
    #[serde(default)]
    pub last_log_path: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackageTaskRuntime {
    pub task_id: String,
    #[serde(default)]
    pub progress: u8,
    #[serde(default)]
    pub step_label: String,
    #[serde(default)]
    pub status: PackageTaskStatus,
    #[serde(default)]
    pub last_log_path: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeState {
    #[serde(default)]
    pub package_tasks: Vec<PackageTaskRuntime>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BuildTaskStatusDto {
    pub task_id: String,
    pub progress: u8,
    #[serde(default)]
    pub step_label: String,
    pub status: PackageTaskStatus,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BuildStatusResponse {
    #[serde(default)]
    pub package_tasks: Vec<BuildTaskStatusDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStartRequest {
    pub task_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStopRequest {
    pub task_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStopResponse {
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskGroupRequest {
    pub project_id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub branch: String,
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
    #[serde(default)]
    pub copy_from_group_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskGroupParamsRequest {
    pub branch: String,
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageTaskRequest {
    pub name: String,
    pub task_group_id: String,
    #[serde(default)]
    pub code_repo_url: String,
    #[serde(default)]
    pub asset_repo_url: String,
    #[serde(default = "default_build_args_json")]
    pub build_args_json: String,
    #[serde(default)]
    pub enable_obfuscation: bool,
    #[serde(default)]
    pub obfuscation_mode: ObfuscationMode,
    #[serde(default)]
    pub obfuscation_seed: Option<u64>,
    #[serde(default)]
    pub enable_dead_code_injection: bool,
    #[serde(default = "default_dead_code_injection_count")]
    pub dead_code_injection_count: u32,
    #[serde(default)]
    pub pre_build_actions: Vec<TaskPrepAction>,
    #[serde(default)]
    pub post_build_actions: Vec<TaskPrepAction>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageTaskReorderRequest {
    pub task_group_id: String,
    pub task_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyImportRequest {
    pub data_dir: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LegacyImportPreview {
    pub settings_found: bool,
    pub prep_project_count: usize,
    pub project_count: usize,
    pub task_count: usize,
    pub task_group_count: usize,
    pub imported: bool,
}

fn default_dead_code_injection_count() -> u32 {
    200
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStartResponse {
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectBranchesResponse {
    pub project_id: String,
    pub project_name: String,
    pub branches: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWorkspaceInitializeRequest {
    #[serde(alias = "projectName")]
    pub project_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWorktreeCleanupRequest {
    #[serde(alias = "projectName")]
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWorkspaceStatus {
    pub project_id: String,
    pub project_name: String,
    pub workspace_dir_key: String,
    pub initialized: bool,
    pub project_path: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWorktreeCleanupResponse {
    pub project_id: String,
    pub project_name: String,
    pub project_path: String,
    pub had_staged_changes: bool,
    pub had_unstaged_changes: bool,
    pub had_untracked_files: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPrivateReposCleanupResponse {
    pub task_id: String,
    pub task_name: String,
    pub code_repo_path: String,
    pub asset_repo_path: String,
    pub code_repo_git_repo: bool,
    pub asset_repo_git_repo: bool,
    pub code_repo_cleaned: bool,
    pub asset_repo_cleaned: bool,
    pub code_repo_had_staged_changes: bool,
    pub code_repo_had_unstaged_changes: bool,
    pub code_repo_had_untracked_files: bool,
    pub asset_repo_had_staged_changes: bool,
    pub asset_repo_had_unstaged_changes: bool,
    pub asset_repo_had_untracked_files: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePrepProjectRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub params: Vec<PrepParam>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePrepProjectRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub params: Vec<PrepParam>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepProjectRunRequest {
    #[serde(alias = "projectName")]
    pub project_id: String,
    #[serde(default)]
    pub params: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepProjectRunForTasksRequest {
    pub task_ids: Vec<String>,
    #[serde(default)]
    pub params: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepProjectRunResponse {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub command: String,
    pub project_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepProjectTaskRunItem {
    pub task_id: String,
    pub task_name: String,
    pub project_name: String,
    pub success: bool,
    pub exit_code: i32,
    pub command: String,
    pub project_path: String,
    pub stdout: String,
    pub stderr: String,
    #[serde(default)]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepProjectRunForTasksResponse {
    pub total_count: usize,
    pub success_count: usize,
    pub failed_count: usize,
    #[serde(default)]
    pub results: Vec<PrepProjectTaskRunItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrepProjectExportFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrepProjectExportMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub params: Vec<PrepParam>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrepProjectExportPayload {
    pub schema_version: u32,
    pub prep: PrepProjectExportMeta,
    #[serde(default)]
    pub files: Vec<PrepProjectExportFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PrepProjectImportMode {
    Create,
    Update,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepProjectImportRequest {
    pub raw_text: String,
    pub mode: PrepProjectImportMode,
    #[serde(default)]
    pub target_prep_project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFileInfo {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPageResponse {
    pub items: Vec<LogFileInfo>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
}

#[derive(Debug, Clone)]
pub struct RepoSyncResult {
    pub path: std::path::PathBuf,
    pub branch: String,
    pub commit: String,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

fn default_build_args_json() -> String {
    "{}".to_owned()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind")]
enum TaggedTaskPrepActionWire {
    #[serde(rename = "single")]
    Single {
        #[serde(rename = "prepProjectId", default)]
        prep_project_id: String,
        #[serde(default)]
        params: HashMap<String, Value>,
    },
    #[serde(rename = "conditional")]
    Conditional {
        #[serde(rename = "conditionSource", default)]
        condition_source: String,
        #[serde(rename = "conditionEquals", default)]
        condition_equals: String,
        #[serde(rename = "onMatchTargets", default)]
        on_match_targets: Vec<TaskPrepTarget>,
        #[serde(rename = "onMismatchTargets", default)]
        on_mismatch_targets: Vec<TaskPrepTarget>,
    },
}

impl From<TaggedTaskPrepActionWire> for TaskPrepAction {
    fn from(value: TaggedTaskPrepActionWire) -> Self {
        match value {
            TaggedTaskPrepActionWire::Single {
                prep_project_id,
                params,
            } => Self::Single {
                prep_project_id,
                params,
            },
            TaggedTaskPrepActionWire::Conditional {
                condition_source,
                condition_equals,
                on_match_targets,
                on_mismatch_targets,
            } => Self::Conditional {
                condition_source,
                condition_equals,
                on_match_targets,
                on_mismatch_targets,
            },
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct LegacyTaskPrepActionWire {
    #[serde(default)]
    kind: Option<TaskPrepActionKind>,
    #[serde(default)]
    prep_project_id: String,
    #[serde(default)]
    params: HashMap<String, Value>,
    #[serde(default)]
    condition_source: String,
    #[serde(default)]
    condition_equals: String,
    #[serde(default)]
    on_match_targets: Vec<TaskPrepTarget>,
    #[serde(default)]
    on_mismatch_targets: Vec<TaskPrepTarget>,
}

impl TryFrom<LegacyTaskPrepActionWire> for TaskPrepAction {
    type Error = String;

    fn try_from(value: LegacyTaskPrepActionWire) -> Result<Self, Self::Error> {
        Ok(match value.kind.unwrap_or(TaskPrepActionKind::Single) {
            TaskPrepActionKind::Single => TaskPrepAction::Single {
                prep_project_id: value.prep_project_id,
                params: value.params,
            },
            TaskPrepActionKind::Conditional => TaskPrepAction::Conditional {
                condition_source: value.condition_source,
                condition_equals: value.condition_equals,
                on_match_targets: value.on_match_targets,
                on_mismatch_targets: value.on_mismatch_targets,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn task_prep_action_should_deserialize_legacy_single_shape() {
        let action: TaskPrepAction = serde_json::from_value(json!({
            "prepProjectId": "prep_1",
            "params": {
                "channel": "release"
            }
        }))
        .expect("legacy single action should deserialize");

        assert_eq!(
            action,
            TaskPrepAction::Single {
                prep_project_id: "prep_1".to_string(),
                params: HashMap::from([("channel".to_string(), json!("release"))]),
            }
        );
    }

    #[test]
    fn task_prep_action_should_deserialize_legacy_conditional_shape() {
        let action: TaskPrepAction = serde_json::from_value(json!({
            "kind": "conditional",
            "conditionSource": "${build_mode}",
            "conditionEquals": "release",
            "onMatchTargets": [{ "prepProjectId": "prep_release", "params": {} }],
            "onMismatchTargets": [{ "prepProjectId": "prep_other", "params": {} }]
        }))
        .expect("legacy conditional action should deserialize");

        assert!(matches!(
            action,
            TaskPrepAction::Conditional {
                condition_source,
                condition_equals,
                ..
            } if condition_source == "${build_mode}" && condition_equals == "release"
        ));
    }

    #[test]
    fn task_prep_action_should_serialize_to_tagged_shape() {
        let action = TaskPrepAction::Single {
            prep_project_id: "prep_1".to_string(),
            params: HashMap::from([("channel".to_string(), json!("release"))]),
        };

        let value = serde_json::to_value(&action).expect("serialize action");
        assert_eq!(
            value,
            json!({
                "kind": "single",
                "prepProjectId": "prep_1",
                "params": {
                    "channel": "release"
                }
            })
        );
    }

    #[test]
    fn prep_param_should_default_value_source_to_runtime_for_legacy_shape() {
        let param: PrepParam = serde_json::from_value(json!({
            "name": "channel",
            "type": "str",
            "optional": false
        }))
        .expect("legacy prep param should deserialize");

        assert_eq!(param.value_source, PrepParamValueSource::Runtime);
        assert_eq!(param.fixed_value, None);
    }
}
