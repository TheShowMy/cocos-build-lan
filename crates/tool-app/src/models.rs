use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PublicSettings {
    #[serde(default)]
    pub engines: Vec<Engine>,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub feishu_bots: Vec<FeishuBot>,
    #[serde(default)]
    pub package_tasks: Vec<PackageTask>,
    #[serde(default)]
    pub task_groups: Vec<TaskGroup>,
    #[serde(default)]
    pub param_definitions: Vec<ParamDefinition>,
    #[serde(default)]
    pub git_credentials_configured: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicSettingsUpdate {
    pub engines: Vec<Engine>,
    pub projects: Vec<Project>,
    pub feishu_bots: Vec<FeishuBot>,
    pub param_definitions: Vec<ParamDefinition>,
}

impl From<&PublicSettings> for PublicSettingsUpdate {
    fn from(value: &PublicSettings) -> Self {
        Self {
            engines: value.engines.clone(),
            projects: value.projects.clone(),
            feishu_bots: value.feishu_bots.clone(),
            param_definitions: value.param_definitions.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Engine {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BuildMode {
    Test,
    Pre,
    #[default]
    Release,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FeishuBot {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub api_key: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
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

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskGroupRequest {
    pub project_id: String,
    pub name: String,
    pub description: String,
    pub branch: String,
    pub params: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_from_group_id: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskGroupParamsRequest {
    pub branch: String,
    pub params: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    #[default]
    Text,
    Number,
    Switch,
    Select,
}

impl ParamKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Text => "文本",
            Self::Number => "数字",
            Self::Switch => "开关",
            Self::Select => "选择",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    #[default]
    Pending,
    Running,
    Canceling,
    Canceled,
    Success,
    Failed,
}

impl TaskStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pending => "未开始",
            Self::Running => "进行中",
            Self::Canceling => "停止中",
            Self::Canceled => "已取消",
            Self::Success => "成功",
            Self::Failed => "失败",
        }
    }

    pub fn class(&self) -> &'static str {
        match self {
            Self::Running | Self::Canceling => "tag--run",
            Self::Success => "tag--ok",
            Self::Failed => "tag--err",
            Self::Canceled => "tag--warn",
            Self::Pending => "",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ObfuscationMode {
    #[default]
    Classic,
    Ast,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskProjectConfig {
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub branch: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PackageTask {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub task_group_id: String,
    #[serde(default)]
    pub order: u32,
    #[serde(default)]
    pub progress: u8,
    #[serde(default)]
    pub status: TaskStatus,
    #[serde(default)]
    pub checked: bool,
    #[serde(default)]
    pub project: Option<TaskProjectConfig>,
    #[serde(default)]
    pub code_repo_url: String,
    #[serde(default)]
    pub asset_repo_url: String,
    #[serde(default = "default_json")]
    pub build_args_json: String,
    #[serde(default)]
    pub enable_obfuscation: bool,
    #[serde(default)]
    pub obfuscation_mode: ObfuscationMode,
    #[serde(default)]
    pub obfuscation_seed: Option<u64>,
    #[serde(default)]
    pub enable_dead_code_injection: bool,
    #[serde(default = "default_dead_code_count")]
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

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageTaskRequest {
    pub name: String,
    pub task_group_id: String,
    pub code_repo_url: String,
    pub asset_repo_url: String,
    pub build_args_json: String,
    pub enable_obfuscation: bool,
    pub obfuscation_mode: ObfuscationMode,
    pub obfuscation_seed: Option<u64>,
    pub enable_dead_code_injection: bool,
    pub dead_code_injection_count: u32,
    pub pre_build_actions: Vec<TaskPrepAction>,
    pub post_build_actions: Vec<TaskPrepAction>,
}

impl PackageTaskRequest {
    pub fn from_task(task: &PackageTask) -> Self {
        Self {
            name: task.name.clone(),
            task_group_id: task.task_group_id.clone(),
            code_repo_url: task.code_repo_url.clone(),
            asset_repo_url: task.asset_repo_url.clone(),
            build_args_json: task.build_args_json.clone(),
            enable_obfuscation: task.enable_obfuscation,
            obfuscation_mode: task.obfuscation_mode.clone(),
            obfuscation_seed: task.obfuscation_seed,
            enable_dead_code_injection: task.enable_dead_code_injection,
            dead_code_injection_count: task.dead_code_injection_count,
            pre_build_actions: task.pre_build_actions.clone(),
            post_build_actions: task.post_build_actions.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskPrepTarget {
    #[serde(default)]
    pub prep_project_id: String,
    #[serde(default)]
    pub params: HashMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BuildStatusResponse {
    #[serde(default)]
    pub package_tasks: Vec<BuildTaskStatus>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BuildTaskStatus {
    pub task_id: String,
    #[serde(default)]
    pub progress: u8,
    #[serde(default)]
    pub step_label: String,
    #[serde(default)]
    pub status: TaskStatus,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PrepParamType {
    #[default]
    Str,
    Int,
    Bool,
    Select,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PrepValueSource {
    #[default]
    Runtime,
    Fixed,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrepParamOption {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrepParam {
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: PrepParamType,
    #[serde(default)]
    pub value_source: PrepValueSource,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub options: Vec<PrepParamOption>,
    #[serde(default)]
    pub fixed_value: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrepProject {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub create_time: String,
    #[serde(default)]
    pub params: Vec<PrepParam>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepSaveRequest {
    pub name: String,
    pub description: String,
    pub params: Vec<PrepParam>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepRunRequest {
    pub project_id: String,
    pub params: HashMap<String, Value>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepRunForTasksRequest {
    pub task_ids: Vec<String>,
    pub params: HashMap<String, Value>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepImportRequest {
    pub raw_text: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_prep_project_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepRunResponse {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub command: String,
    pub project_path: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepTaskRunResponse {
    pub total_count: usize,
    pub success_count: usize,
    pub failed_count: usize,
    #[serde(default)]
    pub results: Vec<PrepTaskRunItem>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepTaskRunItem {
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrepExportPayload {
    pub schema_version: u32,
    pub prep: PrepExportMeta,
    #[serde(default)]
    pub files: Vec<PrepExportFile>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrepExportMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub params: Vec<PrepParam>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrepExportFile {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LogFile {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub created_at: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LogPageResponse {
    #[serde(default)]
    pub items: Vec<LogFile>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectBranchesResponse {
    pub project_id: String,
    pub project_name: String,
    #[serde(default)]
    pub branches: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
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

pub fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn default_version() -> String {
    "1.0.0".to_owned()
}

fn default_json() -> String {
    "{}".to_owned()
}

fn default_dead_code_count() -> u32 {
    200
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_update_excludes_tasks_and_git_state() {
        let settings = PublicSettings {
            git_credentials_configured: true,
            package_tasks: vec![PackageTask {
                id: "task_1".to_owned(),
                ..PackageTask::default()
            }],
            ..PublicSettings::default()
        };
        let value = serde_json::to_value(PublicSettingsUpdate::from(&settings)).unwrap();
        assert!(value.get("packageTasks").is_none());
        assert!(value.get("gitCredentialsConfigured").is_none());
    }
}
