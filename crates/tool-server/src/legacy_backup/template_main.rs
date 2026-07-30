//! 用户项目的服务端入口。
//!
//! 可在本文件添加业务路由；`ToolSettings` 和 `ToolStatus` 定义在本地
//! `tool-contract`，控制端会直接读取这些端点。

use std::{fs, net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
    response::Html,
    routing::get,
};
use dioxus::prelude::VirtualDom;
use semver::Version;
use tokio::sync::RwLock;

use cocos_build_lan_contract::{ToolSettings, ToolStatus};
use cocos_build_lan_core::{ToolLaunchSpec, ToolPaths, ToolRuntime, control_port_candidates};

#[derive(Clone)]
struct ServerState {
    settings: Arc<RwLock<ToolSettings>>,
    paths: ToolPaths,
}

#[tokio::main]
async fn main() {
    let server_binary = "cocos-build-lan-server";
    let control_binary = "cocos-build-lan-control";
    let spec = ToolLaunchSpec::discover(server_binary, control_binary)
        .expect("需要从包含 tool.json 的生成项目启动服务端");
    let settings = load_settings(&spec.paths).expect("读取本地配置");
    let state = ServerState {
        settings: Arc::new(RwLock::new(settings)),
        paths: spec.paths.clone(),
    };
    let runtime = ToolRuntime::new(
        spec.identity.clone(),
        Version::parse(env!("CARGO_PKG_VERSION")).expect("有效的包版本"),
    );
    let app = runtime.clone().router().merge(business_router(state));
    let control_port = control_port_candidates(spec.identity.tool_id)[0];
    let address: SocketAddr = format!("127.0.0.1:{control_port}")
        .parse()
        .expect("有效的回环监听地址");
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("绑定当前工具的回环控制端口");
    println!("服务端已监听 http://{address}");
    let shutdown_runtime = runtime.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown_runtime.wait_for_shutdown().await })
        .await
        .expect("运行服务端");
}

fn business_router(state: ServerState) -> Router {
    Router::new()
        .route("/", get(tool_page))
        .route("/api/control-status", get(control_status))
        .route(
            "/api/control-config",
            get(control_config).put(save_control_config),
        )
        .with_state(state)
}

fn load_settings(paths: &ToolPaths) -> Result<ToolSettings, Box<dyn std::error::Error>> {
    let path = paths.config_file();
    if !path.exists() {
        let settings = ToolSettings::default();
        write_settings(&path, &settings)?;
        return Ok(settings);
    }
    serde_json::from_slice(&fs::read(&path)?).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "本地设置不符合当前开发期结构：{error}。请删除 {} 后重新启动以生成默认设置。",
                path.display()
            ),
        )
        .into()
    })
}

fn write_settings(path: &std::path::Path, settings: &ToolSettings) -> Result<(), std::io::Error> {
    let temporary = path.with_extension("tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(settings).expect("ToolSettings 可序列化"),
    )?;
    fs::rename(temporary, path)
}

async fn control_status(State(state): State<ServerState>) -> Json<ToolStatus> {
    let _settings = state.settings.read().await;
    Json(ToolStatus {
        summary: "服务正在运行".to_owned(),
        completed_jobs: 0,
    })
}

async fn control_config(State(state): State<ServerState>) -> Json<ToolSettings> {
    Json(state.settings.read().await.clone())
}

async fn save_control_config(
    State(state): State<ServerState>,
    Json(settings): Json<ToolSettings>,
) -> Result<Json<ToolSettings>, (axum::http::StatusCode, String)> {
    write_settings(&state.paths.config_file(), &settings).map_err(|error| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("保存配置失败：{error}"),
        )
    })?;
    *state.settings.write().await = settings.clone();
    Ok(Json(settings))
}

async fn tool_page() -> Html<String> {
    let mut dom = VirtualDom::new(cocos_build_lan_app::App);
    dom.rebuild_in_place();
    Html(format!(
        "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>cocos-build-lan</title></head><body>{}</body></html>",
        dioxus::ssr::render(&dom)
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    async fn control_config_api_persists_typed_settings() {
        let paths = ToolPaths::for_tool(Uuid::new_v4()).expect("test paths");
        let state = ServerState {
            settings: Arc::new(RwLock::new(ToolSettings::default())),
            paths: paths.clone(),
        };
        let app = business_router(state);
        let mut expected = ToolSettings::default();
        expected.update.lan_dev_enabled = true;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/control-config")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&expected).expect("settings json"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("save response");
        assert!(response.status().is_success());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/control-config")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("load response");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(
            serde_json::from_slice::<ToolSettings>(&body).expect("typed settings"),
            expected
        );
        assert_eq!(load_settings(&paths).expect("persisted settings"), expected);
        fs::remove_dir_all(paths.root()).expect("remove test state");
    }
}
