use crate::{error::AppError, models::Engine, state::AppState};
use axum::{
    Json, Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, header},
    routing::{get, post},
};
use cocos_build_lan_contract::ToolSettings;
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{Mutex, RwLock};

pub const LOCAL_ONLY: &str = "请在服务启动的机器上访问设置。";

#[derive(Clone)]
pub struct Machine {
    pub app: AppState,
    pub settings: Arc<RwLock<ToolSettings>>,
    pub path: PathBuf,
    pub port: u16,
    pub ips: Vec<IpAddr>,
    pub urls: Vec<String>,
    pub mutations: Arc<Mutex<()>>,
    pub dialog: Arc<Mutex<()>>,
}

impl Machine {
    fn local(&self, peer: SocketAddr) -> bool {
        peer.ip().is_loopback() || self.ips.contains(&peer.ip())
    }

    fn valid_host(&self, headers: &HeaderMap) -> bool {
        let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
            return false;
        };
        let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
            return false;
        };
        authority.port_u16() == Some(self.port)
            && (authority.host() == "localhost"
                || authority
                    .host()
                    .trim_matches(['[', ']'])
                    .parse::<IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback() || self.ips.contains(&ip)))
    }

    fn require_local(
        &self,
        peer: SocketAddr,
        headers: &HeaderMap,
        write: bool,
    ) -> Result<(), AppError> {
        if !self.local(peer) || !self.valid_host(headers) {
            return Err(AppError::forbidden(LOCAL_ONLY));
        }
        let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
        if let Some(origin) = origin {
            let host = headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            let same = origin == format!("http://{host}");
            let dev = cfg!(debug_assertions)
                && peer.ip().is_loopback()
                && std::env::var("COCOS_BUILD_LAN_DEV_ORIGIN")
                    .is_ok_and(|allowed| allowed == origin);
            if !same && !dev {
                return Err(AppError::forbidden("本机设置仅允许同源页面访问"));
            }
        } else if write {
            return Err(AppError::forbidden("本机设置请求缺少 Origin"));
        }
        if headers
            .get("sec-fetch-site")
            .is_some_and(|value| value == "cross-site")
        {
            return Err(AppError::forbidden("本机设置不允许跨站访问"));
        }
        Ok(())
    }
}

pub fn router(machine: Machine) -> Router {
    Router::new()
        .route("/api/access-context", get(access))
        .route("/api/local/settings", get(settings).put(save_settings))
        .route("/api/local/engines", get(engines).put(save_engines))
        .route("/api/local/pick-path", post(pick_path))
        .with_state(machine)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Access {
    can_manage_machine: bool,
    version: &'static str,
    urls: Vec<String>,
    active_port: u16,
    pending_port: Option<u16>,
}

async fn access(
    State(machine): State<Machine>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Json<Access> {
    let configured = machine.settings.read().await.network.lan_port;
    Json(Access {
        can_manage_machine: machine.require_local(peer, &headers, false).is_ok(),
        version: env!("CARGO_PKG_VERSION"),
        urls: machine.urls.clone(),
        active_port: machine.port,
        pending_port: (configured != machine.port).then_some(configured),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsView {
    lan_port: u16,
    active_port: u16,
    git_username: String,
    git_credentials_configured: bool,
    workspace_root: String,
}
fn view(settings: &ToolSettings, port: u16) -> SettingsView {
    SettingsView {
        lan_port: settings.network.lan_port,
        active_port: port,
        git_username: settings.business.git_username.clone(),
        git_credentials_configured: !settings.business.git_password.is_empty(),
        workspace_root: settings.business.workspace_root.clone(),
    }
}
async fn settings(
    State(machine): State<Machine>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<SettingsView>, AppError> {
    machine.require_local(peer, &headers, false)?;
    Ok(Json(view(&*machine.settings.read().await, machine.port)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsUpdate {
    lan_port: u16,
    git_username: String,
    git_password: Option<String>,
    #[serde(default)]
    clear_git_password: bool,
    workspace_root: String,
}

async fn save_settings(
    State(machine): State<Machine>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(update): Json<SettingsUpdate>,
) -> Result<Json<SettingsView>, AppError> {
    machine.require_local(peer, &headers, true)?;
    let _mutation = machine.mutations.lock().await;
    let mut next = machine.settings.read().await.clone();
    let workspace = update.workspace_root.trim();
    let _exclusive = if workspace != next.business.workspace_root {
        Some(
            machine
                .app
                .operations
                .clone()
                .try_write_owned()
                .map_err(|_| AppError::conflict("有活动任务，暂时不能修改工作目录"))?,
        )
    } else {
        None
    };
    if machine
        .app
        .shutting_down
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err(AppError::conflict("服务正在退出"));
    }
    validate_port(update.lan_port, machine.port)?;
    if !workspace.is_empty() {
        validate_workspace(Path::new(workspace))?;
    }
    next.network.lan_port = update.lan_port;
    next.business.workspace_root = workspace.to_string();
    next.business.git_username = update.git_username.trim().to_string();
    if update.clear_git_password {
        next.business.git_password.clear();
    } else if let Some(password) = update.git_password.filter(|value| !value.is_empty()) {
        next.business.git_password = password;
    }
    write_settings(&machine.path, &next)?;
    machine
        .app
        .set_git_config(next.business.git_config().into())
        .await?;
    machine
        .app
        .set_workspace_root((!workspace.is_empty()).then(|| PathBuf::from(workspace)))
        .await;
    let result = view(&next, machine.port);
    *machine.settings.write().await = next;
    tracing::info!("本机设置已保存；端口变更将在下次启动生效");
    Ok(Json(result))
}

pub fn validate_port(port: u16, active: u16) -> Result<(), AppError> {
    if port == 0 {
        return Err(AppError::validation("端口必须在 1 到 65535 之间"));
    }
    if port != active && std::net::TcpListener::bind(("0.0.0.0", port)).is_err() {
        return Err(AppError::conflict(format!("端口 {port} 已被占用")));
    }
    Ok(())
}
fn validate_workspace(path: &Path) -> Result<(), AppError> {
    if !path.is_absolute() || !path.is_dir() {
        return Err(AppError::validation("工作目录必须是已存在的绝对目录"));
    }
    let probe = path.join(format!(".cocos-write-check-{}", uuid::Uuid::new_v4()));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|_| AppError::validation("工作目录不可写"))?;
    drop(file);
    std::fs::remove_file(probe)?;
    Ok(())
}
pub fn write_settings(path: &Path, settings: &ToolSettings) -> Result<(), AppError> {
    let temp = path.with_extension("tmp");
    std::fs::write(
        &temp,
        serde_json::to_vec_pretty(settings).map_err(|e| AppError::internal(e.to_string()))?,
    )?;
    std::fs::rename(temp, path)?;
    Ok(())
}
async fn engines(
    State(machine): State<Machine>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<Vec<Engine>>, AppError> {
    machine.require_local(peer, &headers, false)?;
    Ok(Json(machine.app.get_settings().await.engines))
}
async fn save_engines(
    State(machine): State<Machine>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(mut engines): Json<Vec<Engine>>,
) -> Result<Json<Vec<Engine>>, AppError> {
    machine.require_local(peer, &headers, true)?;
    let _mutation = machine.mutations.lock().await;
    let _exclusive = machine
        .app
        .operations
        .clone()
        .try_write_owned()
        .map_err(|_| AppError::conflict("有活动任务，暂时不能修改引擎"))?;
    if machine
        .app
        .shutting_down
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err(AppError::conflict("服务正在退出"));
    }
    let mut names = std::collections::HashSet::new();
    for engine in &mut engines {
        engine.name = engine.name.trim().to_string();
        engine.path = engine.path.trim().to_string();
        if engine.name.is_empty() || !names.insert(engine.name.to_lowercase()) {
            return Err(AppError::validation("引擎名称不能为空或重复"));
        }
        if !Path::new(&engine.path).is_absolute() {
            return Err(AppError::validation("引擎路径必须是绝对路径"));
        }
        crate::services::build::cocos::resolve_creator_executable(&engine.path)?;
    }
    let mut settings = machine.app.get_settings().await;
    for project in &settings.projects {
        if !engines
            .iter()
            .any(|engine| engine.name == project.engine_name)
        {
            return Err(AppError::conflict(format!(
                "项目 {} 仍引用引擎 {}",
                project.name, project.engine_name
            )));
        }
    }
    settings.engines = engines.clone();
    machine.app.save_settings(settings).await?;
    tracing::info!("本机引擎配置已保存");
    Ok(Json(engines))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum PathKind {
    Engine,
    Workspace,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PickRequest {
    kind: PathKind,
}
#[derive(Serialize)]
struct PickResult {
    path: Option<String>,
}
async fn pick_path(
    State(machine): State<Machine>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<PickRequest>,
) -> Result<Json<PickResult>, AppError> {
    machine.require_local(peer, &headers, true)?;
    if machine
        .app
        .shutting_down
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err(AppError::conflict("服务正在退出"));
    }
    let guard = machine
        .dialog
        .clone()
        .try_lock_owned()
        .map_err(|_| AppError::conflict("已有路径选择窗口，请先完成选择"))?;
    let path = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        match request.kind {
            PathKind::Engine => rfd::FileDialog::new()
                .set_title("选择 CocosCreator.exe")
                .add_filter("Cocos Creator", &["exe"])
                .pick_file(),
            PathKind::Workspace => rfd::FileDialog::new()
                .set_title("选择构建工作目录")
                .pick_folder(),
        }
    })
    .await
    .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Json(PickResult {
        path: path.map(|p| p.to_string_lossy().into_owned()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    async fn fixture() -> (tempfile::TempDir, Machine) {
        let dir = tempfile::tempdir().unwrap();
        let app = AppState::load(dir.path().join("data")).await;
        let mut settings = ToolSettings::default();
        settings.network.lan_port = 44588;
        settings.business.git_username = "builder".into();
        settings.business.git_password = "private-test-token".into();
        let machine = Machine {
            app,
            settings: Arc::new(RwLock::new(settings)),
            path: dir.path().join("settings.json"),
            port: 44588,
            ips: vec!["192.168.2.7".parse().unwrap()],
            urls: vec!["http://127.0.0.1:44588".into()],
            mutations: Arc::default(),
            dialog: Arc::default(),
        };
        (dir, machine)
    }
    fn request(
        path: &str,
        method: &str,
        peer: &str,
        host: &str,
        origin: Option<&str>,
        body: Value,
    ) -> Request<Body> {
        let mut request = Request::builder()
            .uri(path)
            .method(method)
            .header("host", host)
            .header("content-type", "application/json");
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let mut request = request.body(Body::from(body.to_string())).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        request
    }
    #[tokio::test]
    async fn local_capability_requires_real_peer_and_valid_host() {
        let (_dir, machine) = fixture().await;
        for (peer, host, allowed) in [
            ("127.0.0.1:3210", "localhost:44588", true),
            ("192.168.2.7:3210", "192.168.2.7:44588", true),
            ("192.168.2.9:3210", "192.168.2.7:44588", false),
            ("127.0.0.1:3210", "attacker.example:44588", false),
        ] {
            let response = router(machine.clone())
                .oneshot(request(
                    "/api/access-context",
                    "GET",
                    peer,
                    host,
                    None,
                    Value::Null,
                ))
                .await
                .unwrap();
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(body["canManageMachine"], allowed);
        }
    }
    #[tokio::test]
    async fn remote_and_cross_origin_requests_cannot_write_or_open_dialogs() {
        let (_dir, machine) = fixture().await;
        for (path, body) in [
            (
                "/api/local/settings",
                json!({"lanPort":44588,"gitUsername":"x","workspaceRoot":""}),
            ),
            ("/api/local/pick-path", json!({"kind":"engine"})),
        ] {
            for (peer, origin) in [
                ("192.168.2.9:3210", Some("http://192.168.2.7:44588")),
                ("127.0.0.1:3210", Some("http://attacker.example")),
                ("127.0.0.1:3210", None),
            ] {
                let method = if path.ends_with("settings") {
                    "PUT"
                } else {
                    "POST"
                };
                let mut req = request(
                    path,
                    method,
                    peer,
                    "192.168.2.7:44588",
                    origin,
                    body.clone(),
                );
                req.headers_mut()
                    .insert("x-forwarded-for", "127.0.0.1".parse().unwrap());
                let response = router(machine.clone()).oneshot(req).await.unwrap();
                assert_eq!(response.status(), StatusCode::FORBIDDEN);
            }
        }
    }
    #[tokio::test]
    async fn credentials_are_write_only_and_empty_password_preserves_them() {
        let (_dir, machine) = fixture().await;
        for (password, clear, expected) in [
            (None, false, "private-test-token"),
            (Some("replacement"), false, "replacement"),
            (None, true, ""),
        ] {
            let response = router(machine.clone()).oneshot(request("/api/local/settings", "PUT", "127.0.0.1:3210", "127.0.0.1:44588", Some("http://127.0.0.1:44588"), json!({"lanPort":44588,"gitUsername":"builder","gitPassword":password,"clearGitPassword":clear,"workspaceRoot":""}))).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(body.get("gitPassword").is_none());
            assert_eq!(
                machine.settings.read().await.business.git_password,
                expected
            );
        }
    }
    #[tokio::test]
    async fn busy_workspace_and_engine_changes_are_rejected() {
        let (dir, machine) = fixture().await;
        let _active = machine.app.operation().await.unwrap();
        for (path, body) in [
            (
                "/api/local/settings",
                json!({"lanPort":44588,"gitUsername":"builder","workspaceRoot":dir.path()}),
            ),
            ("/api/local/engines", json!([])),
        ] {
            let response = router(machine.clone())
                .oneshot(request(
                    path,
                    "PUT",
                    "127.0.0.1:3210",
                    "127.0.0.1:44588",
                    Some("http://127.0.0.1:44588"),
                    body,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT);
        }
    }
    #[test]
    fn rejects_invalid_paths_and_ports() {
        assert!(validate_port(0, 44588).is_err());
        let port = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        assert!(validate_port(port.local_addr().unwrap().port(), 44588).is_err());
        assert!(validate_workspace(Path::new("relative")).is_err());
        let dir = tempfile::tempdir().unwrap();
        validate_workspace(dir.path()).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
