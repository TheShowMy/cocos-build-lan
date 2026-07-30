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
use cocos_build_lan_core::{CONTROL_PORT, ToolLaunchSpec, ToolRuntime};
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
}

#[tokio::main]
async fn main() {
    init_tracing();
    let server_binary = "cocos-build-lan-server";
    let control_binary = "cocos-build-lan-control";
    let spec = ToolLaunchSpec::discover(server_binary, control_binary)
        .expect("需要从包含 tool.json 的生成项目目录启动服务端");
    let control_settings = load_control_settings(&spec.paths).expect("读取控制端配置");
    let runtime = ToolRuntime::new(
        spec.identity,
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

    let control = ControlState {
        settings: Arc::new(RwLock::new(control_settings)),
        settings_path: spec.paths.config_file(),
        app: app_state.clone(),
    };
    let web_root = resolve_web_root();
    let router = runtime
        .clone()
        .router()
        .merge(business_router(app_state, control));
    let router = if web_root.exists() {
        router
            .fallback(serve_web)
            .layer(Extension(WebRoot(web_root.clone())))
    } else {
        router.route(
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
    let router = router.layer(cors);
    let host = std::env::var("COCOS_BUILD_LAN_HOST").unwrap_or_else(|_| "0.0.0.0".to_owned());
    let address: SocketAddr = format!("{host}:{CONTROL_PORT}")
        .parse()
        .expect("有效监听地址");
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("绑定服务端口");
    info!(address = %address, web_root = %web_root.display(), "Cocos Build LAN 服务已启动");
    let shutdown_runtime = runtime.clone();
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move { shutdown_runtime.wait_for_shutdown().await })
    .await
    .expect("运行服务端");
}

fn business_router(app: AppState, control: ControlState) -> Router {
    Router::new()
        .merge(routes::settings::router())
        .merge(routes::projects::router())
        .merge(routes::logs::router())
        .merge(routes::build::router())
        .merge(routes::package_tasks::router())
        .merge(routes::preps::router())
        .merge(routes::task_groups::router())
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
        .with_state(app)
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
    write_control_settings(&control.settings_path, &settings)?;
    control
        .app
        .set_git_config(settings.business.git_config().into())
        .await?;
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
) -> Result<ToolSettings, std::io::Error> {
    let path = paths.config_file();
    if !path.exists() {
        let settings = ToolSettings::default();
        write_control_settings(&path, &settings)?;
        return Ok(settings);
    }
    serde_json::from_slice(&fs::read(path)?).map_err(std::io::Error::other)
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
