//! 本地工具的运行时、生命周期、更新和进程监督能力。
//!
//! 此 crate 随生成项目一起交付，不是发布到 crates.io 的框架包。业务配置和
//! 状态请放在相邻的 `tool-contract` crate；这里保持无界面、可复用的本地能力。

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use directories::ProjectDirs;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    net::UdpSocket,
    process::{Child, Command},
    sync::{Mutex, watch},
    time::sleep,
};
use uuid::Uuid;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

pub const CONTROL_PROTOCOL_VERSION: u16 = 1;
pub const CONTROL_PORT: u16 = 8765;
pub const LAN_DEV_PORT: u16 = 49153;

/// 除两个二进制外一同切换的只读发布资源。资源被放入 `bin/`，因此服务端可
/// 通过自身可执行文件相邻目录定位 `web/`、`scripts/` 等文件。
#[derive(Clone, Debug)]
pub struct BundleResource {
    pub source: PathBuf,
    pub destination: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolIdentity {
    pub tool_id: Uuid,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RestartBlocker {
    pub code: String,
    pub summary: String,
    pub started_at_unix_secs: u64,
    pub cancellable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RestartReadiness {
    Ready,
    Deferred {
        reasons: Vec<RestartBlocker>,
        retry_after_secs: Option<u64>,
    },
    ConfirmationRequired {
        reasons: Vec<RestartBlocker>,
    },
    Blocked {
        reasons: Vec<RestartBlocker>,
    },
}

impl RestartReadiness {
    #[must_use]
    pub const fn permits_automatic_apply(&self) -> bool {
        matches!(self, Self::Ready)
    }

    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Deferred { .. } => "Deferred",
            Self::ConfirmationRequired { .. } => "ConfirmationRequired",
            Self::Blocked { .. } => "Blocked",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RestartReadinessResponse {
    pub tool_id: Uuid,
    pub readiness: RestartReadiness,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolIdRequest {
    pub tool_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RestartPreparation {
    pub tool_id: Uuid,
    pub nonce: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShutdownRequest {
    pub tool_id: Uuid,
    pub nonce: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub tool_id: Uuid,
    pub protocol: u16,
    pub version: Version,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateSource {
    Release,
    LanDev,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdatePayloadFormat {
    ToolBundleV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateManifest {
    pub tool_id: Uuid,
    pub version: Version,
    pub source: UpdateSource,
    pub format: UpdatePayloadFormat,
    pub target: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub notes: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RestartProtection {
    Deferred,
    ConfirmationRequired,
    Blocked,
}

#[derive(Clone, Default)]
pub struct RestartRegistry {
    blockers: Arc<StdMutex<BTreeMap<Uuid, RegisteredBlocker>>>,
}

#[derive(Clone, Debug)]
struct RegisteredBlocker {
    protection: RestartProtection,
    blocker: RestartBlocker,
}

impl RestartRegistry {
    #[must_use]
    pub fn protect(&self, protection: RestartProtection, blocker: RestartBlocker) -> RestartGuard {
        let id = Uuid::new_v4();
        self.blockers
            .lock()
            .expect("restart registry lock poisoned")
            .insert(
                id,
                RegisteredBlocker {
                    protection,
                    blocker,
                },
            );
        RestartGuard {
            id,
            blockers: Arc::clone(&self.blockers),
        }
    }

    #[must_use]
    pub fn readiness(&self) -> RestartReadiness {
        let blockers = self
            .blockers
            .lock()
            .expect("restart registry lock poisoned");
        let reasons = |protection| {
            blockers
                .values()
                .filter(|entry| entry.protection == protection)
                .map(|entry| entry.blocker.clone())
                .collect::<Vec<_>>()
        };
        let blocked = reasons(RestartProtection::Blocked);
        if !blocked.is_empty() {
            return RestartReadiness::Blocked { reasons: blocked };
        }
        let confirmation = reasons(RestartProtection::ConfirmationRequired);
        if !confirmation.is_empty() {
            return RestartReadiness::ConfirmationRequired {
                reasons: confirmation,
            };
        }
        let deferred = reasons(RestartProtection::Deferred);
        if deferred.is_empty() {
            RestartReadiness::Ready
        } else {
            RestartReadiness::Deferred {
                reasons: deferred,
                retry_after_secs: Some(30),
            }
        }
    }
}

pub struct RestartGuard {
    id: Uuid,
    blockers: Arc<StdMutex<BTreeMap<Uuid, RegisteredBlocker>>>,
}

impl Drop for RestartGuard {
    fn drop(&mut self) {
        self.blockers
            .lock()
            .expect("restart registry lock poisoned")
            .remove(&self.id);
    }
}

#[derive(Clone)]
pub struct ToolRuntime {
    identity: ToolIdentity,
    version: Version,
    restart_registry: RestartRegistry,
    prepared_nonce: Arc<Mutex<Option<Uuid>>>,
    shutdown: watch::Sender<bool>,
}

impl ToolRuntime {
    #[must_use]
    pub fn new(identity: ToolIdentity, version: Version) -> Self {
        let (shutdown, _) = watch::channel(false);
        Self {
            identity,
            version,
            restart_registry: RestartRegistry::default(),
            prepared_nonce: Arc::new(Mutex::new(None)),
            shutdown,
        }
    }

    pub fn router(self) -> Router {
        Router::new()
            .route("/healthz", get(healthz))
            .route("/_lan/control/restart-readiness", get(restart_readiness))
            .route("/_lan/control/prepare-restart", post(prepare_restart))
            .route("/_lan/control/shutdown", post(shutdown))
            .with_state(self)
    }

    #[must_use]
    pub fn identity(&self) -> &ToolIdentity {
        &self.identity
    }

    #[must_use]
    pub fn restart_registry(&self) -> RestartRegistry {
        self.restart_registry.clone()
    }

    pub async fn wait_for_shutdown(&self) {
        let mut receiver = self.shutdown.subscribe();
        if !*receiver.borrow() {
            let _ = receiver.changed().await;
        }
    }
}

async fn healthz(State(runtime): State<ToolRuntime>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_owned(),
        tool_id: runtime.identity.tool_id,
        protocol: CONTROL_PROTOCOL_VERSION,
        version: runtime.version.clone(),
    })
}

async fn restart_readiness(State(runtime): State<ToolRuntime>) -> Json<RestartReadinessResponse> {
    Json(RestartReadinessResponse {
        tool_id: runtime.identity.tool_id,
        readiness: runtime.restart_registry.readiness(),
    })
}

async fn prepare_restart(
    State(runtime): State<ToolRuntime>,
    Json(request): Json<ToolIdRequest>,
) -> Result<Json<RestartPreparation>, ControlError> {
    ensure_own_tool(&runtime, request.tool_id)?;
    ensure_ready(&runtime)?;
    let nonce = Uuid::new_v4();
    *runtime.prepared_nonce.lock().await = Some(nonce);
    Ok(Json(RestartPreparation {
        tool_id: runtime.identity.tool_id,
        nonce,
    }))
}

async fn shutdown(
    State(runtime): State<ToolRuntime>,
    Json(request): Json<ShutdownRequest>,
) -> Result<StatusCode, ControlError> {
    ensure_own_tool(&runtime, request.tool_id)?;
    ensure_ready(&runtime)?;
    if runtime.prepared_nonce.lock().await.take() != Some(request.nonce) {
        return Err(ControlError::new(
            StatusCode::CONFLICT,
            "restart nonce 无效",
        ));
    }
    runtime.shutdown.send_replace(true);
    Ok(StatusCode::ACCEPTED)
}

fn ensure_own_tool(runtime: &ToolRuntime, tool_id: Uuid) -> Result<(), ControlError> {
    (runtime.identity.tool_id == tool_id)
        .then_some(())
        .ok_or_else(|| ControlError::new(StatusCode::CONFLICT, "tool_id 与当前工具不匹配"))
}

fn ensure_ready(runtime: &ToolRuntime) -> Result<(), ControlError> {
    let readiness = runtime.restart_registry.readiness();
    readiness
        .permits_automatic_apply()
        .then_some(())
        .ok_or_else(|| {
            ControlError::new(
                StatusCode::CONFLICT,
                format!("当前不能安全重启：{}", readiness.label()),
            )
        })
}

struct ControlError {
    status: StatusCode,
    message: String,
}

impl ControlError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for ControlError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

#[derive(Clone, Debug)]
pub struct ToolPaths {
    root: PathBuf,
}

impl ToolPaths {
    pub fn for_tool(tool_id: Uuid) -> Result<Self, ToolError> {
        let dirs = ProjectDirs::from("dev", "lan-toolkit", "lan-toolkit")
            .ok_or(ToolError::NoDataDirectory)?;
        let root = dirs.data_local_dir().join(tool_id.to_string());
        for name in ["releases", "staging", "config", "data", "logs"] {
            fs::create_dir_all(root.join(name))?;
        }
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config").join("tool-settings.json")
    }

    #[must_use]
    pub fn server_log(&self) -> PathBuf {
        self.root.join("logs").join("server.log")
    }

    #[must_use]
    pub fn active_file(&self) -> PathBuf {
        self.root.join("active.json")
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActiveRelease {
    pub current: Option<Version>,
    pub previous: Option<Version>,
}

#[derive(Clone, Debug)]
pub struct UpdateStore {
    paths: ToolPaths,
    tool_id: Uuid,
    target: String,
}

impl UpdateStore {
    #[must_use]
    pub fn new(paths: ToolPaths, tool_id: Uuid) -> Self {
        Self {
            paths,
            tool_id,
            target: current_target(),
        }
    }

    pub fn initialize(&self) -> Result<(), UpdateError> {
        if !self.paths.active_file().exists() {
            self.write_active(&ActiveRelease::default())?;
        }
        Ok(())
    }

    pub fn validate_manifest(&self, manifest: &UpdateManifest) -> Result<(), UpdateError> {
        if manifest.tool_id != self.tool_id {
            return Err(UpdateError::ToolIdMismatch);
        }
        if manifest.target != self.target {
            return Err(UpdateError::TargetMismatch {
                expected: self.target.clone(),
                actual: manifest.target.clone(),
            });
        }
        if manifest.format != UpdatePayloadFormat::ToolBundleV1 {
            return Err(UpdateError::UnsupportedPayloadFormat);
        }
        Ok(())
    }

    pub fn stage_bytes(
        &self,
        manifest: &UpdateManifest,
        payload: &[u8],
    ) -> Result<PathBuf, UpdateError> {
        self.validate_manifest(manifest)?;
        if payload.len() as u64 != manifest.size {
            return Err(UpdateError::SizeMismatch {
                expected: manifest.size,
                actual: payload.len() as u64,
            });
        }
        if sha256_bytes(payload) != manifest.sha256.to_ascii_lowercase() {
            return Err(UpdateError::ChecksumMismatch);
        }
        let staging = self
            .paths
            .root
            .join("staging")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&staging)?;
        fs::write(staging.join("payload.zip"), payload)?;
        fs::write(
            staging.join("manifest.json"),
            serde_json::to_vec_pretty(manifest)?,
        )?;
        Ok(staging)
    }

    pub fn apply_staged(
        &self,
        manifest: &UpdateManifest,
        staging: &Path,
        server_binary_name: &OsString,
        control_binary_name: &OsString,
    ) -> Result<ActiveRelease, UpdateError> {
        self.validate_manifest(manifest)?;
        let old = self.active_release()?;
        let release = self
            .paths
            .root
            .join("releases")
            .join(manifest.version.to_string());
        if release.exists() {
            return Err(UpdateError::ReleaseAlreadyInstalled(
                manifest.version.clone(),
            ));
        }
        let temporary = self.paths.root.join("releases").join(format!(
            ".{}-{}.tmp",
            manifest.version,
            Uuid::new_v4()
        ));
        let bin = temporary.join("bin");
        let prepared = (|| -> Result<(), UpdateError> {
            fs::create_dir_all(&bin)?;
            extract_bundle(
                &staging.join("payload.zip"),
                &bin,
                server_binary_name,
                control_binary_name,
            )?;
            fs::write(
                temporary.join("manifest.json"),
                serde_json::to_vec_pretty(manifest)?,
            )?;
            Ok(())
        })();
        if let Err(error) = prepared {
            let _ = fs::remove_dir_all(&temporary);
            return Err(error);
        }
        if let Err(error) = fs::rename(&temporary, &release) {
            let _ = fs::remove_dir_all(&temporary);
            return Err(UpdateError::Io(error));
        }
        let active = ActiveRelease {
            current: Some(manifest.version.clone()),
            previous: old.current,
        };
        if let Err(error) = self.write_active(&active) {
            let _ = fs::remove_dir_all(&release);
            return Err(error);
        }
        let _ = fs::remove_dir_all(staging);
        Ok(active)
    }

    pub fn active_release(&self) -> Result<ActiveRelease, UpdateError> {
        Ok(serde_json::from_slice(&fs::read(
            self.paths.active_file(),
        )?)?)
    }

    pub fn restore_active(&self, active: &ActiveRelease) -> Result<(), UpdateError> {
        self.write_active(active)
    }

    pub fn rollback(&self) -> Result<ActiveRelease, UpdateError> {
        let active = self.active_release()?;
        let previous = active.previous.ok_or(UpdateError::NoPreviousRelease)?;
        let rolled_back = ActiveRelease {
            current: Some(previous),
            previous: active.current,
        };
        self.write_active(&rolled_back)?;
        Ok(rolled_back)
    }

    #[must_use]
    pub fn release_is_installed(&self, version: &Version) -> bool {
        self.paths
            .root
            .join("releases")
            .join(version.to_string())
            .join("manifest.json")
            .is_file()
    }

    #[must_use]
    pub fn release_binary(&self, version: &Version, binary_name: &OsString) -> PathBuf {
        self.paths
            .root
            .join("releases")
            .join(version.to_string())
            .join("bin")
            .join(binary_name)
    }

    fn write_active(&self, active: &ActiveRelease) -> Result<(), UpdateError> {
        let temporary = self
            .paths
            .root
            .join(format!(".active-{}.tmp", Uuid::new_v4()));
        fs::write(&temporary, serde_json::to_vec_pretty(active)?)?;
        fs::rename(temporary, self.paths.active_file())?;
        Ok(())
    }
}

/// 创建当前平台的完整版本包。资源与二进制在同一原子切换单元中解压。
pub fn create_update_bundle(
    output: &Path,
    server: &Path,
    control: &Path,
    resources: &[BundleResource],
) -> Result<(), UpdateError> {
    let file = fs::File::create(output)?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (path, name) in [(server, "server"), (control, "control")] {
        let file_name = path
            .file_name()
            .ok_or_else(|| UpdateError::InvalidBundle(format!("{name} 缺少文件名")))?
            .to_string_lossy();
        archive.start_file(format!("bin/{file_name}"), options)?;
        io::copy(&mut fs::File::open(path)?, &mut archive)?;
    }
    for resource in resources {
        if !resource.source.is_dir() {
            return Err(UpdateError::InvalidBundle(format!(
                "发布资源不是目录：{}",
                resource.source.display()
            )));
        }
        let destination = Path::new("bin").join(&resource.destination);
        if destination.is_absolute()
            || destination
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(UpdateError::InvalidBundle(format!(
                "发布资源目标非法：{}",
                resource.destination.display()
            )));
        }
        add_directory_to_bundle(&mut archive, options, &resource.source, &destination)?;
    }
    archive.finish()?;
    Ok(())
}

fn add_directory_to_bundle<W: io::Write + io::Seek>(
    archive: &mut ZipWriter<W>,
    options: SimpleFileOptions,
    source: &Path,
    destination: &Path,
) -> Result<(), UpdateError> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let path = entry.path();
        if path.is_dir() {
            add_directory_to_bundle(archive, options, &path, &target)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        archive.start_file(target.to_string_lossy(), options)?;
        io::copy(&mut fs::File::open(path)?, archive)?;
    }
    Ok(())
}

fn extract_bundle(
    bundle: &Path,
    destination: &Path,
    server_binary_name: &OsString,
    control_binary_name: &OsString,
) -> Result<(), UpdateError> {
    let file = fs::File::open(bundle)?;
    let mut archive = ZipArchive::new(file)?;
    for name in [server_binary_name, control_binary_name] {
        let entry_name = format!("bin/{}", name.to_string_lossy());
        let mut entry = archive
            .by_name(&entry_name)
            .map_err(|_| UpdateError::InvalidBundle(format!("缺少 {entry_name}")))?;
        let destination_file = destination.join(name);
        let mut output = fs::File::create(&destination_file)?;
        io::copy(&mut entry, &mut output)?;
        make_executable(&destination_file)?;
    }
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(entry_name) = entry.enclosed_name() else {
            return Err(UpdateError::InvalidBundle(
                "资源路径包含非法父目录".to_owned(),
            ));
        };
        if !entry_name.starts_with("bin/web") && !entry_name.starts_with("bin/scripts") {
            continue;
        }
        let relative = entry_name
            .strip_prefix("bin")
            .map_err(|_| UpdateError::InvalidBundle("资源不在 bin/ 目录下".to_owned()))?;
        let output = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(output)?;
        } else {
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = fs::File::create(output)?;
            io::copy(&mut entry, &mut file)?;
        }
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("更新清单的 tool_id 与当前工具不匹配")]
    ToolIdMismatch,
    #[error("更新目标 {actual:?} 与当前平台 {expected:?} 不匹配")]
    TargetMismatch { expected: String, actual: String },
    #[error("下载文件大小为 {actual}，但清单要求 {expected}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("下载文件的 SHA-256 与清单不匹配")]
    ChecksumMismatch,
    #[error("只接受 lan-toolkit-bundle-v1 完整版本包")]
    UnsupportedPayloadFormat,
    #[error("版本包无效：{0}")]
    InvalidBundle(String),
    #[error("版本 {0} 已安装")]
    ReleaseAlreadyInstalled(Version),
    #[error("没有上一个版本可以回滚")]
    NoPreviousRelease,
    #[error("I/O 错误：{0}")]
    Io(#[from] io::Error),
    #[error("JSON 错误：{0}")]
    Serialization(#[from] serde_json::Error),
    #[error("ZIP 错误：{0}")]
    Zip(#[from] zip::result::ZipError),
}

#[derive(Clone, Debug)]
pub struct ToolLaunchSpec {
    pub identity: ToolIdentity,
    pub project_dir: PathBuf,
    pub fallback_server: PathBuf,
    pub fallback_control: PathBuf,
    pub launcher_path: PathBuf,
    pub server_binary_name: OsString,
    pub control_binary_name: OsString,
    pub paths: ToolPaths,
}

#[derive(Deserialize)]
struct ToolFile {
    tool_id: Uuid,
    display_name: String,
}

impl ToolLaunchSpec {
    pub fn discover(
        server_binary_name: impl Into<OsString>,
        control_binary_name: impl Into<OsString>,
    ) -> Result<Self, ToolError> {
        let server_binary_name = platform_binary_name(server_binary_name.into());
        let control_binary_name = platform_binary_name(control_binary_name.into());
        let project_dir = find_project_dir()?;
        let tool: ToolFile = serde_json::from_slice(&fs::read(project_dir.join("tool.json"))?)?;
        let paths = ToolPaths::for_tool(tool.tool_id)?;
        let executable_dir = std::env::current_exe()?.parent().map(Path::to_path_buf);
        let fallback_server = executable_dir.as_deref().map_or_else(
            || project_dir.join(&server_binary_name),
            |dir| dir.join(&server_binary_name),
        );
        let fallback_control = executable_dir.as_deref().map_or_else(
            || project_dir.join(&control_binary_name),
            |dir| dir.join(&control_binary_name),
        );
        let launcher_name = control_binary_name
            .to_string_lossy()
            .replace("-control", "-launcher");
        Ok(Self {
            identity: ToolIdentity {
                tool_id: tool.tool_id,
                display_name: tool.display_name,
            },
            project_dir: project_dir.clone(),
            fallback_server,
            fallback_control,
            launcher_path: executable_dir.map_or_else(
                || project_dir.join(platform_binary_name(launcher_name.clone().into())),
                |dir| dir.join(platform_binary_name(launcher_name.clone().into())),
            ),
            server_binary_name,
            control_binary_name,
            paths,
        })
    }

    pub fn update_store(&self) -> Result<UpdateStore, ToolError> {
        let store = UpdateStore::new(self.paths.clone(), self.identity.tool_id);
        store.initialize()?;
        Ok(store)
    }

    pub fn active_server_path(&self) -> Result<PathBuf, ToolError> {
        let store = self.update_store()?;
        let active = store.active_release()?;
        if let Some(version) = active.current {
            let candidate = store.release_binary(&version, &self.server_binary_name);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        Ok(self.fallback_server.clone())
    }

    pub fn active_control_path(&self) -> Result<PathBuf, ToolError> {
        let store = self.update_store()?;
        let active = store.active_release()?;
        if let Some(version) = active.current {
            let candidate = store.release_binary(&version, &self.control_binary_name);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        Ok(self.fallback_control.clone())
    }
}

fn find_project_dir() -> Result<PathBuf, ToolError> {
    let mut candidates = Vec::new();
    candidates.push(std::env::current_dir()?);
    if let Some(parent) = std::env::current_exe()?.parent() {
        candidates.push(parent.to_path_buf());
    }
    for candidate in candidates {
        for ancestor in candidate.ancestors() {
            if ancestor.join("tool.json").is_file() {
                return Ok(ancestor.to_path_buf());
            }
        }
    }
    Err(ToolError::ToolFileNotFound)
}

#[derive(Clone)]
pub struct ToolSupervisor {
    spec: ToolLaunchSpec,
    client: reqwest::Client,
    child: Arc<Mutex<Option<Child>>>,
}

#[derive(Clone, Debug)]
pub struct PendingUpdate {
    pub manifest: UpdateManifest,
    staging: PathBuf,
}

impl PendingUpdate {
    #[must_use]
    pub fn staging_path(&self) -> &Path {
        &self.staging
    }

    #[must_use]
    pub fn from_staging(manifest: UpdateManifest, staging: PathBuf) -> Self {
        Self { manifest, staging }
    }
}

#[derive(Clone, Debug)]
pub struct ServiceStatus {
    pub health: HealthResponse,
    pub server_path: PathBuf,
}

impl ToolSupervisor {
    #[must_use]
    pub fn new(spec: ToolLaunchSpec) -> Self {
        Self {
            spec,
            client: reqwest::Client::new(),
            child: Arc::new(Mutex::new(None)),
        }
    }

    #[must_use]
    pub fn spec(&self) -> &ToolLaunchSpec {
        &self.spec
    }

    pub async fn probe(&self) -> Result<ServiceStatus, ToolError> {
        let response = self
            .client
            .get(format!("http://127.0.0.1:{CONTROL_PORT}/healthz"))
            .send()
            .await?
            .error_for_status()?;
        let health: HealthResponse = response.json().await?;
        if health.tool_id != self.spec.identity.tool_id
            || health.protocol != CONTROL_PROTOCOL_VERSION
        {
            return Err(ToolError::DifferentTool);
        }
        Ok(ServiceStatus {
            health,
            server_path: self.spec.active_server_path()?,
        })
    }

    pub async fn start(&self) -> Result<ServiceStatus, ToolError> {
        match self.probe().await {
            Ok(status) => return Ok(status),
            Err(ToolError::DifferentTool) => return Err(ToolError::DifferentTool),
            Err(_) => {}
        }
        let path = self.spec.active_server_path()?;
        if !path.is_file() {
            return Err(ToolError::ServerBinaryMissing(path));
        }
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.spec.paths.server_log())?;
        let log_error = log.try_clone()?;
        let child = Command::new(&path)
            .current_dir(&self.spec.project_dir)
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_error))
            .spawn()?;
        *self.child.lock().await = Some(child);
        for _ in 0..40 {
            sleep(Duration::from_millis(125)).await;
            if let Ok(status) = self.probe().await {
                return Ok(status);
            }
        }
        Err(ToolError::ServiceDidNotBecomeHealthy)
    }

    pub async fn readiness(&self) -> Result<RestartReadiness, ToolError> {
        let response = self
            .client
            .get(format!(
                "http://127.0.0.1:{CONTROL_PORT}/_lan/control/restart-readiness"
            ))
            .send()
            .await?
            .error_for_status()?;
        let response: RestartReadinessResponse = response.json().await?;
        if response.tool_id != self.spec.identity.tool_id {
            return Err(ToolError::DifferentTool);
        }
        Ok(response.readiness)
    }

    pub async fn stop(&self) -> Result<(), ToolError> {
        self.probe().await?;
        let readiness = self.readiness().await?;
        if !readiness.permits_automatic_apply() {
            return Err(ToolError::RestartNotReady(readiness_explanation(
                &readiness,
            )));
        }
        let preparation: RestartPreparation = self
            .client
            .post(format!(
                "http://127.0.0.1:{CONTROL_PORT}/_lan/control/prepare-restart"
            ))
            .json(&ToolIdRequest {
                tool_id: self.spec.identity.tool_id,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let readiness = self.readiness().await?;
        if !readiness.permits_automatic_apply() {
            return Err(ToolError::RestartNotReady(readiness_explanation(
                &readiness,
            )));
        }
        self.client
            .post(format!(
                "http://127.0.0.1:{CONTROL_PORT}/_lan/control/shutdown"
            ))
            .json(&ShutdownRequest {
                tool_id: self.spec.identity.tool_id,
                nonce: preparation.nonce,
            })
            .send()
            .await?
            .error_for_status()?;
        for _ in 0..40 {
            sleep(Duration::from_millis(125)).await;
            if self.probe().await.is_err() {
                *self.child.lock().await = None;
                return Ok(());
            }
        }
        Err(ToolError::ServiceDidNotStop)
    }

    pub async fn restart(&self) -> Result<ServiceStatus, ToolError> {
        self.stop().await?;
        self.start().await
    }

    pub async fn stage_update_from_manifest_url(
        &self,
        url: &str,
    ) -> Result<PendingUpdate, ToolError> {
        let manifest: UpdateManifest = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        self.stage_update(manifest).await
    }

    pub async fn stage_update(&self, manifest: UpdateManifest) -> Result<PendingUpdate, ToolError> {
        let payload = self
            .client
            .get(&manifest.url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let store = self.spec.update_store()?;
        let staging = store.stage_bytes(&manifest, &payload)?;
        Ok(PendingUpdate { manifest, staging })
    }

    pub async fn receive_lan_update(&self, timeout: Duration) -> Result<PendingUpdate, ToolError> {
        let manifest = receive_lan_manifest(timeout).await?;
        self.stage_update(manifest).await
    }

    pub async fn apply_pending(&self, pending: &PendingUpdate) -> Result<ServiceStatus, ToolError> {
        let was_running = self.probe().await.is_ok();
        if was_running {
            let readiness = self.readiness().await?;
            if !readiness.permits_automatic_apply() {
                return Err(ToolError::RestartNotReady(readiness_explanation(
                    &readiness,
                )));
            }
            self.stop().await?;
        }
        let store = self.spec.update_store()?;
        let before = store.active_release()?;
        store.apply_staged(
            &pending.manifest,
            &pending.staging,
            &self.spec.server_binary_name,
            &self.spec.control_binary_name,
        )?;
        match self.start().await {
            Ok(status) => Ok(status),
            Err(error) => {
                store.restore_active(&before)?;
                if was_running {
                    let _ = self.start().await;
                }
                Err(error)
            }
        }
    }
}

pub async fn receive_lan_manifest(timeout: Duration) -> Result<UpdateManifest, ToolError> {
    let socket = UdpSocket::bind(("0.0.0.0", LAN_DEV_PORT)).await?;
    receive_lan_manifest_from(socket, timeout).await
}

async fn receive_lan_manifest_from(
    socket: UdpSocket,
    timeout: Duration,
) -> Result<UpdateManifest, ToolError> {
    let mut data = [0_u8; 16 * 1024];
    let received = tokio::time::timeout(timeout, socket.recv_from(&mut data))
        .await
        .map_err(|_| ToolError::LanDevTimedOut)??;
    Ok(serde_json::from_slice(&data[..received.0])?)
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("无法找到 tool.json；请从生成项目的目录启动控制端")]
    ToolFileNotFound,
    #[error("本机没有可用的数据目录")]
    NoDataDirectory,
    #[error("当前 8765 端口由其他工具占用")]
    DifferentTool,
    #[error("未找到服务端二进制：{0}")]
    ServerBinaryMissing(PathBuf),
    #[error("服务在等待时间内没有通过健康检查")]
    ServiceDidNotBecomeHealthy,
    #[error("服务没有在等待时间内优雅退出")]
    ServiceDidNotStop,
    #[error("重启状态尚不允许操作：{0}")]
    RestartNotReady(String),
    #[error("等待 LAN Dev 广播超时")]
    LanDevTimedOut,
    #[error("I/O 错误：{0}")]
    Io(#[from] io::Error),
    #[error("网络错误：{0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON 错误：{0}")]
    Serialization(#[from] serde_json::Error),
    #[error("更新错误：{0}")]
    Update(#[from] UpdateError),
}

#[cfg(target_os = "macos")]
#[must_use]
pub fn current_target() -> String {
    format!("{}-apple-darwin", std::env::consts::ARCH)
}

#[cfg(target_os = "linux")]
#[must_use]
pub fn current_target() -> String {
    format!("{}-unknown-linux-gnu", std::env::consts::ARCH)
}

#[cfg(target_os = "windows")]
#[must_use]
pub fn current_target() -> String {
    format!("{}-pc-windows-msvc", std::env::consts::ARCH)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
#[must_use]
pub fn current_target() -> String {
    format!(
        "{}-unknown-{}",
        std::env::consts::ARCH,
        std::env::consts::OS
    )
}

#[must_use]
pub fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn platform_binary_name(mut name: OsString) -> OsString {
    if cfg!(windows) && Path::new(&name).extension().is_none() {
        name.push(".exe");
    }
    name
}

fn sha256_bytes(payload: &[u8]) -> String {
    format!("{:x}", Sha256::digest(payload))
}

fn readiness_explanation(readiness: &RestartReadiness) -> String {
    let reasons = |items: &[RestartBlocker]| {
        items
            .iter()
            .map(|item| format!("{}：{}", item.code, item.summary))
            .collect::<Vec<_>>()
            .join("；")
    };
    match readiness {
        RestartReadiness::Ready => "Ready".to_owned(),
        RestartReadiness::Deferred {
            reasons: items,
            retry_after_secs,
        } => match retry_after_secs {
            Some(seconds) => format!("Deferred，{seconds} 秒后重试：{}", reasons(items)),
            None => format!("Deferred：{}", reasons(items)),
        },
        RestartReadiness::ConfirmationRequired { reasons: items } => {
            format!("ConfirmationRequired，需要确认：{}", reasons(items))
        }
        RestartReadiness::Blocked { reasons: items } => {
            format!("Blocked：{}", reasons(items))
        }
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), io::Error> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), io::Error> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write};

    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::*;

    fn identity() -> ToolIdentity {
        ToolIdentity {
            tool_id: Uuid::new_v4(),
            display_name: "测试工具".to_owned(),
        }
    }

    #[test]
    fn strongest_restart_state_wins() {
        let registry = RestartRegistry::default();
        let deferred = registry.protect(
            RestartProtection::Deferred,
            RestartBlocker {
                code: "import".to_owned(),
                summary: "导入中".to_owned(),
                started_at_unix_secs: 1,
                cancellable: true,
            },
        );
        let _blocked = registry.protect(
            RestartProtection::Blocked,
            RestartBlocker {
                code: "migration".to_owned(),
                summary: "迁移中".to_owned(),
                started_at_unix_secs: 1,
                cancellable: false,
            },
        );
        assert!(matches!(
            registry.readiness(),
            RestartReadiness::Blocked { .. }
        ));
        drop(deferred);
    }

    #[test]
    fn every_restart_state_is_exposed() {
        let ready = RestartRegistry::default();
        assert!(matches!(ready.readiness(), RestartReadiness::Ready));
        for (protection, expected) in [
            (RestartProtection::Deferred, "Deferred"),
            (
                RestartProtection::ConfirmationRequired,
                "ConfirmationRequired",
            ),
            (RestartProtection::Blocked, "Blocked"),
        ] {
            let registry = RestartRegistry::default();
            let _guard = registry.protect(
                protection,
                RestartBlocker {
                    code: "test".to_owned(),
                    summary: "测试".to_owned(),
                    started_at_unix_secs: 0,
                    cancellable: true,
                },
            );
            assert_eq!(registry.readiness().label(), expected);
        }
    }

    #[tokio::test]
    async fn shutdown_route_notifies_graceful_server() {
        let runtime = ToolRuntime::new(identity(), Version::new(1, 0, 0));
        let waiter = runtime.clone();
        let completed = tokio::spawn(async move {
            waiter.wait_for_shutdown().await;
        });
        let response = runtime
            .clone()
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/_lan/control/prepare-restart")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&ToolIdRequest {
                            tool_id: runtime.identity().tool_id,
                        })
                        .expect("request json"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let preparation: RestartPreparation = serde_json::from_slice(&bytes).expect("preparation");
        let response = runtime
            .clone()
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/_lan/control/shutdown")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&ShutdownRequest {
                            tool_id: runtime.identity().tool_id,
                            nonce: preparation.nonce,
                        })
                        .expect("request json"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        completed.await.expect("shutdown waiter");
    }

    #[test]
    fn update_store_rejects_identity_hash_and_target() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = ToolPaths {
            root: temp.path().join("state"),
        };
        fs::create_dir_all(&paths.root).expect("state");
        let root = paths.root.clone();
        let id = Uuid::new_v4();
        let store = UpdateStore {
            paths,
            tool_id: id,
            target: "test-target".to_owned(),
        };
        store.initialize().expect("init");
        let server = temp.path().join("server");
        let control = temp.path().join("control");
        let web = temp.path().join("web");
        fs::write(&server, b"server").expect("server");
        fs::write(&control, b"control").expect("control");
        fs::create_dir_all(&web).expect("web dir");
        fs::write(web.join("index.html"), b"<html>spa</html>").expect("web entry");
        let bundle = temp.path().join("payload.zip");
        create_update_bundle(
            &bundle,
            &server,
            &control,
            &[BundleResource {
                source: web,
                destination: PathBuf::from("web"),
            }],
        )
        .expect("bundle");
        let payload = fs::read(&bundle).expect("payload");
        let manifest = UpdateManifest {
            tool_id: id,
            version: Version::new(2, 0, 0),
            source: UpdateSource::LanDev,
            format: UpdatePayloadFormat::ToolBundleV1,
            target: "test-target".to_owned(),
            url: "http://localhost/payload".to_owned(),
            size: payload.len() as u64,
            sha256: sha256_bytes(&payload),
            notes: String::new(),
        };
        let staging = store.stage_bytes(&manifest, &payload).expect("stage");
        store
            .apply_staged(
                &manifest,
                &staging,
                &OsString::from("server"),
                &OsString::from("control"),
            )
            .expect("apply");
        assert_eq!(
            store.active_release().expect("active").current,
            Some(Version::new(2, 0, 0))
        );
        assert_eq!(
            fs::read(
                root.join("releases")
                    .join("2.0.0")
                    .join("bin/web/index.html"),
            )
            .expect("released web"),
            b"<html>spa</html>",
        );
        let wrong_id = UpdateManifest {
            tool_id: Uuid::new_v4(),
            ..manifest.clone()
        };
        assert!(matches!(
            store.stage_bytes(&wrong_id, &payload),
            Err(UpdateError::ToolIdMismatch)
        ));
        let wrong_target = UpdateManifest {
            target: "other-target".to_owned(),
            ..manifest.clone()
        };
        assert!(matches!(
            store.stage_bytes(&wrong_target, &payload),
            Err(UpdateError::TargetMismatch { .. })
        ));
        let wrong_hash = UpdateManifest {
            sha256: "00".repeat(32),
            ..manifest
        };
        assert!(matches!(
            store.stage_bytes(&wrong_hash, &payload),
            Err(UpdateError::ChecksumMismatch)
        ));
    }

    #[test]
    fn update_store_switches_to_new_release_and_rolls_back() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = ToolPaths {
            root: temp.path().join("state"),
        };
        fs::create_dir_all(&paths.root).expect("state");
        let id = Uuid::new_v4();
        let store = UpdateStore {
            paths,
            tool_id: id,
            target: "test-target".to_owned(),
        };
        store.initialize().expect("init");
        let server = temp.path().join("server");
        let control = temp.path().join("control");
        fs::write(&server, b"server").expect("server");
        fs::write(&control, b"control").expect("control");

        for version in [Version::new(1, 0, 0), Version::new(1, 1, 0)] {
            let bundle = temp.path().join(format!("{version}.zip"));
            create_update_bundle(&bundle, &server, &control, &[]).expect("bundle");
            let payload = fs::read(&bundle).expect("payload");
            let manifest = UpdateManifest {
                tool_id: id,
                version,
                source: UpdateSource::LanDev,
                format: UpdatePayloadFormat::ToolBundleV1,
                target: "test-target".to_owned(),
                url: "http://localhost/payload".to_owned(),
                size: payload.len() as u64,
                sha256: sha256_bytes(&payload),
                notes: String::new(),
            };
            let staging = store.stage_bytes(&manifest, &payload).expect("stage");
            store
                .apply_staged(
                    &manifest,
                    &staging,
                    &OsString::from("server"),
                    &OsString::from("control"),
                )
                .expect("apply");
        }

        assert_eq!(
            store.rollback().expect("rollback"),
            ActiveRelease {
                current: Some(Version::new(1, 0, 0)),
                previous: Some(Version::new(1, 1, 0)),
            }
        );
    }

    #[test]
    fn invalid_bundle_leaves_no_release_and_a_retry_can_succeed() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = ToolPaths {
            root: temp.path().join("state"),
        };
        fs::create_dir_all(paths.root.join("releases")).expect("releases");
        fs::create_dir_all(paths.root.join("staging")).expect("staging");
        let id = Uuid::new_v4();
        let store = UpdateStore {
            paths,
            tool_id: id,
            target: "test-target".to_owned(),
        };
        store.initialize().expect("init");
        let version = Version::new(3, 0, 0);
        let broken = temp.path().join("broken.zip");
        let file = fs::File::create(&broken).expect("broken file");
        let mut archive = ZipWriter::new(file);
        archive
            .start_file("bin/server", SimpleFileOptions::default())
            .expect("server entry");
        archive.write_all(b"server").expect("server bytes");
        archive.finish().expect("finish broken bundle");
        let broken_payload = fs::read(&broken).expect("broken payload");
        let broken_manifest = UpdateManifest {
            tool_id: id,
            version: version.clone(),
            source: UpdateSource::LanDev,
            format: UpdatePayloadFormat::ToolBundleV1,
            target: "test-target".to_owned(),
            url: "http://localhost/payload".to_owned(),
            size: broken_payload.len() as u64,
            sha256: sha256_bytes(&broken_payload),
            notes: String::new(),
        };
        let staging = store
            .stage_bytes(&broken_manifest, &broken_payload)
            .expect("stage broken bundle");
        assert!(matches!(
            store.apply_staged(
                &broken_manifest,
                &staging,
                &OsString::from("server"),
                &OsString::from("control"),
            ),
            Err(UpdateError::InvalidBundle(_))
        ));
        assert!(!store.release_is_installed(&version));
        assert!(staging.is_dir());

        let server = temp.path().join("server");
        let control = temp.path().join("control");
        fs::write(&server, b"server").expect("server");
        fs::write(&control, b"control").expect("control");
        let valid = temp.path().join("valid.zip");
        create_update_bundle(&valid, &server, &control, &[]).expect("valid bundle");
        let valid_payload = fs::read(&valid).expect("valid payload");
        let valid_manifest = UpdateManifest {
            size: valid_payload.len() as u64,
            sha256: sha256_bytes(&valid_payload),
            ..broken_manifest
        };
        let staging = store
            .stage_bytes(&valid_manifest, &valid_payload)
            .expect("stage valid bundle");
        store
            .apply_staged(
                &valid_manifest,
                &staging,
                &OsString::from("server"),
                &OsString::from("control"),
            )
            .expect("retry apply");
        assert!(store.release_is_installed(&version));
        assert!(!staging.exists());
    }

    #[tokio::test]
    async fn lan_manifest_is_received_before_tool_identity_is_accepted() {
        let receiver = UdpSocket::bind("127.0.0.1:0").await.expect("receiver");
        let address = receiver.local_addr().expect("receiver address");
        let foreign = UpdateManifest {
            tool_id: Uuid::new_v4(),
            version: Version::new(1, 0, 1),
            source: UpdateSource::LanDev,
            format: UpdatePayloadFormat::ToolBundleV1,
            target: "test-target".to_owned(),
            url: "http://localhost/payload".to_owned(),
            size: 1,
            sha256: "00".repeat(32),
            notes: String::new(),
        };
        UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("sender")
            .send_to(
                &serde_json::to_vec(&foreign).expect("manifest json"),
                address,
            )
            .await
            .expect("send manifest");
        let received = receive_lan_manifest_from(receiver, Duration::from_secs(1))
            .await
            .expect("receive manifest");
        let store = UpdateStore {
            paths: ToolPaths {
                root: tempfile::TempDir::new().expect("state").keep(),
            },
            tool_id: Uuid::new_v4(),
            target: "test-target".to_owned(),
        };
        assert!(matches!(
            store.validate_manifest(&received),
            Err(UpdateError::ToolIdMismatch)
        ));
    }
}
