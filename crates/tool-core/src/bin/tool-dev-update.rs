//! 分发当前平台控制端与服务端完整版本包的 LAN Dev 更新源。

use std::{
    env, fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::Response,
    routing::get,
};
use cocos_build_lan_core::{
    LAN_DEV_PORT, ToolIdentity, UpdateManifest, UpdatePayloadFormat, UpdateSource,
    create_update_bundle, current_target,
};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{
    net::{TcpListener, UdpSocket},
    time::sleep,
};

#[derive(Deserialize)]
struct ToolFile {
    tool_id: uuid::Uuid,
    display_name: String,
}

#[derive(Clone)]
struct DevServer {
    manifest: UpdateManifest,
    payload: PathBuf,
}

#[tokio::main]
async fn main() {
    let arguments = Arguments::parse().unwrap_or_else(|error| {
        eprintln!("{error}");
        eprintln!("用法：tool-dev-update <tool.json> <server-binary> <control-binary> [--web <web-dir>] [--listen lan --advertise <ip> --broadcast] [--version <version>]");
        std::process::exit(2);
    });
    let tool: ToolFile =
        serde_json::from_slice(&fs::read(&arguments.tool_file).expect("读取 tool.json"))
            .expect("解析 tool.json");
    let identity = ToolIdentity {
        tool_id: tool.tool_id,
        display_name: tool.display_name,
    };
    let bundle =
        std::env::temp_dir().join(format!("{}-{}.zip", identity.tool_id, arguments.version));
    let resources = arguments
        .web
        .iter()
        .map(|source| cocos_build_lan_core::BundleResource {
            source: source.clone(),
            destination: PathBuf::from("web"),
        })
        .collect::<Vec<_>>();
    create_update_bundle(&bundle, &arguments.server, &arguments.control, &resources)
        .expect("创建完整版本包");
    let bytes = fs::read(&bundle).expect("读取完整版本包");
    let port = 49_152;
    let bind_address = if arguments.listen_lan {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)
    } else {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    };
    let advertised_host = arguments.advertise.unwrap_or(Ipv4Addr::LOCALHOST);
    let update_manifest = UpdateManifest {
        tool_id: identity.tool_id,
        version: arguments.version,
        source: UpdateSource::LanDev,
        format: UpdatePayloadFormat::ToolBundleV1,
        target: current_target(),
        url: format!("http://{advertised_host}:{port}/payload"),
        size: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        notes: format!("{} 的开发完整版本包", identity.display_name),
    };
    if arguments.broadcast {
        let manifest_for_broadcast = update_manifest.clone();
        tokio::spawn(async move { broadcast_hint(manifest_for_broadcast).await });
    }
    println!("开发更新清单：http://{advertised_host}:{port}/manifest.json");
    println!("tool_id: {}", update_manifest.tool_id);
    let app = Router::new()
        .route("/manifest.json", get(manifest))
        .route("/payload", get(payload))
        .with_state(Arc::new(DevServer {
            manifest: update_manifest,
            payload: bundle,
        }));
    let listener = TcpListener::bind(bind_address)
        .await
        .expect("绑定更新服务端口");
    axum::serve(listener, app).await.expect("运行更新服务");
}

async fn manifest(State(server): State<Arc<DevServer>>) -> Json<UpdateManifest> {
    Json(server.manifest.clone())
}

async fn payload(State(server): State<Arc<DevServer>>) -> Result<Response, StatusCode> {
    let bytes = fs::read(&server.payload).map_err(|_| StatusCode::NOT_FOUND)?;
    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(bytes.into())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn broadcast_hint(manifest: UpdateManifest) {
    let socket = UdpSocket::bind("0.0.0.0:0").await.expect("绑定 UDP 广播");
    socket.set_broadcast(true).expect("开启 UDP 广播");
    let body = serde_json::to_vec(&manifest).expect("序列化更新清单");
    loop {
        let _ = socket
            .send_to(&body, format!("255.255.255.255:{LAN_DEV_PORT}"))
            .await;
        sleep(Duration::from_secs(5)).await;
    }
}

struct Arguments {
    tool_file: PathBuf,
    server: PathBuf,
    control: PathBuf,
    web: Option<PathBuf>,
    listen_lan: bool,
    advertise: Option<Ipv4Addr>,
    broadcast: bool,
    version: Version,
}

impl Arguments {
    fn parse() -> Result<Self, String> {
        let mut values = env::args_os().skip(1);
        let tool_file = values
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| "缺少 tool.json 路径".to_owned())?;
        let server = values
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| "缺少服务端二进制路径".to_owned())?;
        let control = values
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| "缺少控制端二进制路径".to_owned())?;
        let mut arguments = Self {
            tool_file,
            server,
            control,
            web: None,
            listen_lan: false,
            advertise: None,
            broadcast: false,
            version: Version::parse("0.1.0-dev.1").expect("有效默认版本"),
        };
        while let Some(flag) = values.next() {
            match flag.to_string_lossy().as_ref() {
                "--listen" => {
                    if values.next().as_deref() != Some(std::ffi::OsStr::new("lan")) {
                        return Err("--listen 只接受 lan".to_owned());
                    }
                    arguments.listen_lan = true;
                }
                "--advertise" => {
                    let value = values
                        .next()
                        .ok_or_else(|| "缺少 --advertise 值".to_owned())?;
                    arguments.advertise = Some(
                        value
                            .to_string_lossy()
                            .parse()
                            .map_err(|_| "--advertise 必须是 IPv4 地址".to_owned())?,
                    );
                }
                "--broadcast" => arguments.broadcast = true,
                "--version" => {
                    let value = values
                        .next()
                        .ok_or_else(|| "缺少 --version 值".to_owned())?;
                    arguments.version = value
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "--version 必须是语义化版本".to_owned())?;
                }
                "--web" => {
                    arguments.web = Some(
                        values
                            .next()
                            .map(PathBuf::from)
                            .ok_or_else(|| "缺少 --web 路径".to_owned())?,
                    );
                }
                other => return Err(format!("未知选项：{other}")),
            }
        }
        if arguments.broadcast && !arguments.listen_lan {
            return Err("--broadcast 需要同时指定 --listen lan".to_owned());
        }
        if arguments.listen_lan && arguments.advertise.is_none() {
            return Err("--listen lan 需要 --advertise <LAN-IP>".to_owned());
        }
        Ok(arguments)
    }
}
