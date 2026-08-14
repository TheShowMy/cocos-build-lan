#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! Cocos Build 的局域网服务端。
//!
//! 业务执行逻辑来自旧工具，但生命周期、数据根目录和更新协议由
//! lan-toolkit 管理。浏览器端只访问公开 API；Git 凭据通过控制端的
//! loopback API 管理，绝不包含在 `/api/settings` 响应中。

mod error;
mod git;
mod models;
mod routes;
mod services;
mod state;

use std::{
    fs,
    net::SocketAddr,
    path::{Component, PathBuf},
    sync::Arc,
};

use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, Request},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use cocos_build_lan_contract::{ToolSettings, ToolStatus};
use cocos_build_lan_core::{
    ToolLaunchSpec, ToolRuntime, control_port_candidates, first_available_business_port,
};
use semver::Version;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt::time::ChronoLocal};

use crate::state::AppState;

#[derive(Clone)]
pub(crate) struct ControlState {
    pub settings: Arc<RwLock<ToolSettings>>,
    pub settings_path: PathBuf,
    pub app: AppState,
    pub control_port: u16,
    pub lan_port: u16,
}

#[tokio::main]
async fn main() {
    init_tracing();
    let server_binary = "cocos-build-lan-server";
    let control_binary = "cocos-build-lan-control";
    let spec = ToolLaunchSpec::discover(server_binary, control_binary)
        .expect("需要从包含 tool.json 的生成项目目录启动服务端");
    let control_port = resolve_control_port(spec.identity.tool_id).expect("选择本机控制端口");
    let control_settings = load_control_settings(&spec.paths, spec.identity.tool_id, control_port)
        .expect("读取控制端配置");
    let lan_port = control_settings.network.lan_port;
    let runtime = ToolRuntime::new(
        spec.identity.clone(),
        Version::parse(env!("CARGO_PKG_VERSION")).expect("有效版本"),
    );
    let app_state = AppState::load_with_restart_registry(
        spec.paths.root().join("data").join("cocos-build"),
        runtime.restart_registry(),
    )
    .await;
    app_state
        .set_git_config(control_settings.business.git_config().into())
        .await
        .expect("同步控制端 Git 凭据");
    app_state
        .set_workspace_root(
            (!control_settings.business.workspace_root.trim().is_empty())
                .then(|| PathBuf::from(control_settings.business.workspace_root.trim())),
        )
        .await;

    let control = ControlState {
        settings: Arc::new(RwLock::new(control_settings)),
        settings_path: spec.paths.config_file(),
        app: app_state.clone(),
        control_port,
        lan_port,
    };
    let web_root = resolve_web_root();
    let public_router = public_router(app_state);
    let public_router = if web_root.exists() {
        public_router
            .fallback(serve_web)
            .layer(Extension(WebRoot(web_root.clone())))
    } else {
        public_router.route(
            "/",
            get(|| async { "Cocos Build LAN 服务正在运行；请先构建 Web SPA。" }),
        )
    };
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
        ])
        .allow_headers(Any);
    let public_router = public_router.layer(cors);
    let control_router = control_router(runtime.clone(), control);
    let control_address = SocketAddr::from(([127, 0, 0, 1], control_port));
    let lan_address = SocketAddr::from(([0, 0, 0, 0], lan_port));
    let control_listener = tokio::net::TcpListener::bind(control_address)
        .await
        .expect("绑定本机控制端口");
    let lan_listener = tokio::net::TcpListener::bind(lan_address)
        .await
        .expect("绑定局域网业务端口");
    info!(control_address = %control_address, lan_address = %lan_address, web_root = %web_root.display(), "Cocos Build LAN 服务已启动");
    let control_shutdown = runtime.clone();
    let lan_shutdown = runtime;
    tokio::try_join!(
        axum::serve(
            control_listener,
            control_router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move { control_shutdown.wait_for_shutdown().await }),
        axum::serve(
            lan_listener,
            public_router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move { lan_shutdown.wait_for_shutdown().await }),
    )
    .expect("运行服务端监听器");
}

fn public_router(app: AppState) -> Router {
    Router::new()
        .merge(routes::settings::router())
        .merge(routes::projects::router())
        .merge(routes::logs::router())
        .merge(routes::build::router())
        .merge(routes::package_tasks::router())
        .merge(routes::preps::router())
        .merge(routes::task_groups::router())
        .with_state(app)
}

fn control_router(runtime: ToolRuntime, control: ControlState) -> Router {
    runtime
        .router()
        .route("/api/control-status", get(control_status))
        .route(
            "/api/control-config",
            get(control_config).put(save_control_config),
        )
        .route(
            "/api/control/import/preview",
            axum::routing::post(import_preview),
        )
        .route("/api/control/import", axum::routing::post(import_legacy))
        .layer(Extension(control))
}

async fn control_status(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(control): Extension<ControlState>,
) -> Result<Json<ToolStatus>, crate::error::AppError> {
    require_loopback(peer)?;
    let status = control.app.control_status().await;
    Ok(Json(status))
}

async fn control_config(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(control): Extension<ControlState>,
) -> Result<Json<ToolSettings>, crate::error::AppError> {
    require_loopback(peer)?;
    Ok(Json(control.settings.read().await.clone()))
}

async fn save_control_config(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(control): Extension<ControlState>,
    Json(settings): Json<ToolSettings>,
) -> Result<Json<ToolSettings>, crate::error::AppError> {
    require_loopback(peer)?;
    validate_lan_port(&control, &settings)?;
    write_control_settings(&control.settings_path, &settings)?;
    control
        .app
        .set_git_config(settings.business.git_config().into())
        .await?;
    control
        .app
        .set_workspace_root(
            (!settings.business.workspace_root.trim().is_empty())
                .then(|| PathBuf::from(settings.business.workspace_root.trim())),
        )
        .await;
    *control.settings.write().await = settings.clone();
    Ok(Json(settings))
}

async fn import_preview(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(control): Extension<ControlState>,
    Json(request): Json<crate::models::LegacyImportRequest>,
) -> Result<Json<crate::models::LegacyImportPreview>, crate::error::AppError> {
    require_loopback(peer)?;
    Ok(Json(
        control
            .app
            .preview_legacy_import(PathBuf::from(request.data_dir))
            .await?,
    ))
}

async fn import_legacy(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(control): Extension<ControlState>,
    Json(request): Json<crate::models::LegacyImportRequest>,
) -> Result<Json<crate::models::LegacyImportPreview>, crate::error::AppError> {
    require_loopback(peer)?;
    let (result, imported_git_config) = control
        .app
        .import_legacy(PathBuf::from(request.data_dir))
        .await?;
    let mut private = control.settings.read().await.clone();
    private
        .business
        .set_git_config(cocos_build_lan_contract::GitConfig {
            username: imported_git_config.username.clone(),
            password: imported_git_config.password.clone(),
        });
    write_control_settings(&control.settings_path, &private)?;
    control.app.set_git_config(imported_git_config).await?;
    *control.settings.write().await = private;
    Ok(Json(result))
}

fn require_loopback(peer: SocketAddr) -> Result<(), crate::error::AppError> {
    if peer.ip().is_loopback() {
        Ok(())
    } else {
        Err(crate::error::AppError::forbidden(
            "该控制端 API 仅允许本机访问",
        ))
    }
}

fn load_control_settings(
    paths: &cocos_build_lan_core::ToolPaths,
    tool_id: uuid::Uuid,
    control_port: u16,
) -> Result<ToolSettings, std::io::Error> {
    let path = paths.config_file();
    let mut settings = if path.exists() {
        serde_json::from_slice(&fs::read(&path)?).map_err(std::io::Error::other)?
    } else {
        ToolSettings::default()
    };
    if settings.network.lan_port == 0 {
        settings.network.lan_port = first_available_business_port(tool_id, control_port)
            .ok_or_else(|| std::io::Error::other("局域网业务端口候选均被占用"))?;
        write_control_settings(&path, &settings)?;
    }
    Ok(settings)
}

fn resolve_control_port(tool_id: uuid::Uuid) -> Result<u16, std::io::Error> {
    if let Ok(value) = std::env::var("COCOS_BUILD_LAN_CONTROL_PORT") {
        let port = value
            .parse::<u16>()
            .map_err(|_| std::io::Error::other("控制端口环境变量无效"))?;
        if control_port_candidates(tool_id).contains(&port) {
            return Ok(port);
        }
        return Err(std::io::Error::other("控制端口不属于当前工具的候选范围"));
    }
    control_port_candidates(tool_id)
        .into_iter()
        .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
        .ok_or_else(|| std::io::Error::other("本机控制端口候选均被占用"))
}

fn validate_lan_port(
    control: &ControlState,
    settings: &ToolSettings,
) -> Result<(), crate::error::AppError> {
    validate_lan_port_value(
        control.control_port,
        control.lan_port,
        settings.network.lan_port,
    )
}

fn validate_lan_port_value(
    control_port: u16,
    current_lan_port: u16,
    port: u16,
) -> Result<(), crate::error::AppError> {
    if port == 0 {
        return Err(crate::error::AppError::validation(
            "局域网业务端口必须在 1 到 65535 之间",
        ));
    }
    if port == control_port {
        return Err(crate::error::AppError::validation(
            "局域网业务端口不能使用当前工具的本机控制端口",
        ));
    }
    if port != current_lan_port && std::net::TcpListener::bind(("0.0.0.0", port)).is_err() {
        return Err(crate::error::AppError::conflict(format!(
            "局域网业务端口 {port} 已被占用"
        )));
    }
    Ok(())
}

fn write_control_settings(
    path: &std::path::Path,
    settings: &ToolSettings,
) -> Result<(), std::io::Error> {
    let temporary = path.with_extension("tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(settings).expect("settings 可序列化"),
    )?;
    fs::rename(temporary, path)
}

fn resolve_web_root() -> PathBuf {
    if let Ok(path) = std::env::var("COCOS_BUILD_LAN_WEB_ROOT") {
        return PathBuf::from(path);
    }
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("web")))
        .unwrap_or_else(|| PathBuf::from("web"))
}

#[derive(Clone)]
struct WebRoot(PathBuf);

async fn serve_web(Extension(web_root): Extension<WebRoot>, request: Request) -> Response {
    let requested = request.uri().path().trim_start_matches('/');
    if requested == "api" || requested.starts_with("api/") || requested.starts_with("_lan/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let candidate = PathBuf::from(requested);
    let is_safe_file = !requested.is_empty()
        && candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    let file = is_safe_file
        .then(|| web_root.0.join(candidate))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| web_root.0.join("index.html"));

    match tokio::fs::read(&file).await {
        Ok(contents) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, content_type(&file))],
            contents,
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("无法读取 Web 资源: {error}"),
        )
            .into_response(),
    }
}

fn content_type(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S".to_owned()))
        .compact()
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[test]
    fn lan_port_validation_rejects_zero_control_and_occupied_ports() {
        assert!(validate_lan_port_value(41_001, 43_001, 0).is_err());
        assert!(validate_lan_port_value(41_001, 43_001, 41_001).is_err());

        let occupied = std::net::TcpListener::bind("0.0.0.0:0").expect("occupied port");
        let port = occupied.local_addr().unwrap().port();
        assert!(validate_lan_port_value(41_001, 43_001, port).is_err());
        assert!(validate_lan_port_value(41_001, port, port).is_ok());
    }

    #[tokio::test]
    async fn public_spa_fallback_never_masks_unknown_api_routes() {
        let web = tempfile::TempDir::new().expect("web root");
        fs::write(web.path().join("index.html"), "web").expect("index");
        let app = Router::new()
            .fallback(serve_web)
            .layer(Extension(WebRoot(web.path().to_path_buf())));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/control-config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/projects")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
