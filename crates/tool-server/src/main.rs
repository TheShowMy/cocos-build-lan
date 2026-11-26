//! npm entrypoint: one foreground process owns the web service, tasks and TUI.
mod error;
mod git;
mod machine;
mod models;
mod process;
mod routes;
mod services;
mod state;
mod tui;

use crate::state::AppState;
use axum::{
    Extension, Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use cocos_build_lan_contract::ToolSettings;
use cocos_build_lan_core::{ToolIdentity, ToolPaths, business_port_candidates};
use fs2::FileExt;
use std::{
    fs,
    io::{IsTerminal, Write},
    net::{IpAddr, SocketAddr},
    path::{Component, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt::time::ChronoLocal};

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args
        .first()
        .is_some_and(|arg| arg == "--obfuscation-worker")
    {
        if let Err(error) = services::obfuscation::worker(&args[1..]) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::args().any(|arg| arg == "--version") {
        println!("cocos-build-lan {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("创建运行时");
    let result = runtime.block_on(run());
    runtime.shutdown_timeout(Duration::from_secs(1));
    if let Err(error) = result {
        eprintln!("启动/运行失败: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let identity: ToolIdentity = serde_json::from_str(include_str!("../../../tool.json"))?;
    let paths = ToolPaths::for_tool(identity.tool_id)?;
    let mut lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(paths.root().join("instance.lock"))?;
    if lock.try_lock_exclusive().is_err() {
        let address = fs::read_to_string(paths.root().join("instance.json"))
            .unwrap_or_else(|_| "实例正在启动，请稍后查看原终端".into());
        println!("已有 cocos-build-lan 实例正在运行：{address}");
        return Ok(());
    }
    lock.set_len(0)?;
    writeln!(lock, "{}", std::process::id())?;
    let mut settings: ToolSettings = if paths.config_file().exists() {
        serde_json::from_slice(&fs::read(paths.config_file())?)?
    } else {
        ToolSettings::default()
    };
    if settings.network.lan_port == 0 {
        settings.network.lan_port = business_port_candidates(identity.tool_id)
            .into_iter()
            .find(|port| std::net::TcpListener::bind(("0.0.0.0", *port)).is_ok())
            .ok_or("没有可用的业务端口")?;
    }
    let port = settings.network.lan_port;
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| {
            format!(
                "无法监听端口 {port}：{e}。请确认旧实例已退出或修改 {}",
                paths.config_file().display()
            )
        })?;
    backup_before_npm(&paths)?;
    // Persist the current schema only after preserving the original configuration.
    machine::write_settings(&paths.config_file(), &settings)?;
    let interactive = std::io::stdout().is_terminal()
        && std::io::stdin().is_terminal()
        && !std::env::args().any(|arg| arg == "--no-tui");
    let sink = tui::LogSink::new(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths.server_log())?,
        interactive,
    );
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S".into()))
        .with_ansi(false)
        .with_writer(sink.clone())
        .compact()
        .init();
    let ips: Vec<IpAddr> = local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .map(|(_, ip)| ip)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut urls = vec![format!("http://127.0.0.1:{port}")];
    urls.extend(
        ips.iter()
            .filter(|ip| matches!(ip, IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() && !ip.is_link_local()))
            .map(|ip| format!("http://{ip}:{port}")),
    );
    fs::write(
        paths.root().join("instance.json"),
        serde_json::to_vec(
            &serde_json::json!({"pid": std::process::id(), "version": env!("CARGO_PKG_VERSION"), "urls": urls}),
        )?,
    )?;
    let app = AppState::load(paths.root().join("data/cocos-build")).await;
    app.set_git_config(settings.business.git_config().into())
        .await?;
    app.set_workspace_root(
        (!settings.business.workspace_root.is_empty())
            .then(|| PathBuf::from(&settings.business.workspace_root)),
    )
    .await;
    let machine = machine::Machine {
        app: app.clone(),
        settings: Arc::new(RwLock::new(settings)),
        path: paths.config_file(),
        port,
        ips,
        urls: urls.clone(),
        mutations: Arc::new(Mutex::new(())),
        dialog: Arc::new(Mutex::new(())),
    };
    let web_root = resolve_web_root();
    let public = public_router(app.clone()).merge(machine::router(machine)).route("/healthz", get(move || async move { axum::Json(serde_json::json!({"toolId": identity.tool_id, "version": env!("CARGO_PKG_VERSION"), "status": "ok"})) })).fallback(serve_web).layer(Extension(WebRoot(web_root.clone())));
    for url in &urls {
        info!(%url, "访问地址");
    }
    info!(version = env!("CARGO_PKG_VERSION"), "服务已启动");
    diagnose(&web_root).await;
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let mut server = tokio::spawn(async move {
        axum::serve(
            listener,
            public.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.changed().await;
        })
        .await
    });
    let ui_stop = Arc::new(AtomicBool::new(false));
    let mut ui = if interactive {
        let stop = ui_stop.clone();
        Some(tokio::task::spawn_blocking(move || {
            tui::run(urls, sink.logs, stop)
        }))
    } else {
        None
    };
    tokio::select! {
        result = &mut server => { ui_stop.store(true, Ordering::SeqCst); warn!(?result, "Web 服务已停止"); }
        result = async { match &mut ui { Some(handle) => handle.await.map_err(|e| e.to_string())?.map_err(|e| e.to_string()), None => std::future::pending().await } } => { if let Err(error) = result { warn!(%error, "TUI 已停止"); } }
        _ = termination_signal(interactive) => { ui_stop.store(true, Ordering::SeqCst); }
    }
    info!("正在停止全部任务并退出");
    app.stop_all().await;
    process::stop_all();
    let _ = shutdown_tx.send(true);
    let drained = tokio::time::timeout(
        Duration::from_secs(10),
        app.operations.clone().write_owned(),
    )
    .await;
    if drained.is_err() {
        warn!("任务收尾超时，终止剩余请求");
    }
    server.abort();
    process::stop_all();
    app.finish_shutdown().await?;
    ui_stop.store(true, Ordering::SeqCst);
    if let Some(handle) = ui
        && !handle.is_finished()
    {
        let _ = handle.await;
    }
    info!("全部任务已停止，服务已退出");
    let _ = fs::remove_file(paths.root().join("instance.json"));
    Ok(())
}

fn backup_before_npm(paths: &ToolPaths) -> std::io::Result<()> {
    let backup = paths.root().join("backups/before-npm-0.2");
    if backup.join("complete").exists() {
        return Ok(());
    }
    fs::create_dir_all(&backup)?;
    for (source, name) in [
        (paths.config_file(), "tool-settings.json"),
        (
            paths.root().join("data/cocos-build/settings.json"),
            "settings.json",
        ),
        (
            paths.root().join("data/cocos-build/runtime_state.json"),
            "runtime_state.json",
        ),
    ] {
        if source.is_file() && !backup.join(name).exists() {
            fs::copy(source, backup.join(name))?;
        }
    }
    fs::write(backup.join("complete"), b"0.2.0")?;
    Ok(())
}

async fn termination_signal(interactive: bool) {
    #[cfg(windows)]
    {
        let mut close = tokio::signal::windows::ctrl_close().expect("注册关闭通知");
        let mut shutdown = tokio::signal::windows::ctrl_shutdown().expect("注册关机通知");
        let mut ctrl_break = tokio::signal::windows::ctrl_break().expect("注册退出通知");
        tokio::select! { _ = close.recv() => {}, _ = shutdown.recv() => {}, _ = ctrl_break.recv() => {}, _ = async { if interactive { std::future::pending::<()>().await } else { let _ = tokio::signal::ctrl_c().await; } } => {} }
    }
    #[cfg(not(windows))]
    {
        let _ = interactive;
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn diagnose(web_root: &std::path::Path) {
    for program in ["git", "uv", "node"] {
        let executable = if program == "node" {
            std::env::var_os("COCOS_BUILD_LAN_NODE_EXE").unwrap_or_else(|| program.into())
        } else {
            program.into()
        };
        match tokio::time::timeout(
            Duration::from_secs(5),
            process::Command::new(executable).arg("--version").output(),
        )
        .await
        {
            Ok(Ok(result)) if result.status.success() => {
                info!(tool = program, version = %String::from_utf8_lossy(&result.stdout).trim(), "依赖检查通过")
            }
            _ => warn!(
                tool = program,
                "依赖缺失或不可运行，请安装并配置 PATH 后重试；相关任务暂不可用"
            ),
        }
    }
    if !web_root.join("index.html").is_file() {
        warn!("Web 资源缺失，请重新构建或安装完整 npm 包");
    }
    let scripts = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("scripts/ast_obfuscate.cjs")));
    if !scripts.is_some_and(|p| p.is_file()) {
        warn!("混淆脚本缺失，请重新构建或安装完整 npm 包");
    }
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
        .layer(middleware::from_fn_with_state(
            app.clone(),
            operation_request,
        ))
        .with_state(app)
}
async fn operation_request(
    State(app): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, error::AppError> {
    let _operation = app.operation().await?;
    Ok(next.run(request).await)
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    #[tokio::test]
    async fn schemes_work_through_operation_middleware_but_reject_a_busy_queue() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::load(dir.path().to_path_buf()).await;
        state
            .save_settings(models::AppSettings {
                task_groups: vec![models::TaskGroup {
                    id: "g".into(),
                    name: "G".into(),
                    branch: "main".into(),
                    ..Default::default()
                }],
                ..Default::default()
            })
            .await
            .unwrap();
        let scheme_id = state.find_task_group("g").await.unwrap().schemes.unwrap()[0]
            .id
            .clone();
        let app = public_router(state.clone());
        let uri = format!("/api/task-groups/g/schemes/{scheme_id}/apply");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .method("POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        state.try_start_build().await.unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .method("POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
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
