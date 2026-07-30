//! 分发当前平台控制端与服务端完整版本包的 LAN Dev 更新源。

use std::{
    env,
    ffi::OsString,
    fs,
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
    BundleResource, ToolIdentity, UpdateManifest, UpdatePayloadFormat, UpdateSource,
    create_update_bundle, current_target, lan_discovery_ports,
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
        eprintln!("用法：tool-dev-update <tool.json> <server-binary> <control-binary> --web <web-dir> --scripts <scripts-dir> [--listen lan --advertise <ip> --broadcast] [--version <version>]");
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
    let resources = [
        BundleResource {
            source: arguments.web.clone(),
            destination: PathBuf::from("web"),
        },
        BundleResource {
            source: arguments.scripts.clone(),
            destination: PathBuf::from("scripts"),
        },
    ];
    create_update_bundle(&bundle, &arguments.server, &arguments.control, &resources)
        .expect("创建完整版本包");
    let bytes = fs::read(&bundle).expect("读取完整版本包");
    let listener = bind_update_listener(arguments.listen_lan)
        .await
        .expect("绑定更新服务端口");
    let port = listener.local_addr().expect("读取更新服务端口").port();
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
        let discovery_ports = lan_discovery_ports(identity.tool_id);
        tokio::spawn(async move {
            broadcast_hint(manifest_for_broadcast, discovery_ports).await;
        });
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
    axum::serve(listener, app).await.expect("运行更新服务");
}

async fn bind_update_listener(listen_lan: bool) -> std::io::Result<TcpListener> {
    let host = if listen_lan {
        Ipv4Addr::UNSPECIFIED
    } else {
        Ipv4Addr::LOCALHOST
    };
    TcpListener::bind(SocketAddr::new(IpAddr::V4(host), 0)).await
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

async fn broadcast_hint(manifest: UpdateManifest, discovery_ports: Vec<u16>) {
    let socket = UdpSocket::bind("0.0.0.0:0").await.expect("绑定 UDP 广播");
    socket.set_broadcast(true).expect("开启 UDP 广播");
    let body = serde_json::to_vec(&manifest).expect("序列化更新清单");
    loop {
        for port in &discovery_ports {
            let _ = socket
                .send_to(&body, format!("255.255.255.255:{port}"))
                .await;
        }
        sleep(Duration::from_secs(5)).await;
    }
}

#[derive(Debug)]
struct Arguments {
    tool_file: PathBuf,
    server: PathBuf,
    control: PathBuf,
    web: PathBuf,
    scripts: PathBuf,
    listen_lan: bool,
    advertise: Option<Ipv4Addr>,
    broadcast: bool,
    version: Version,
}

impl Arguments {
    fn parse() -> Result<Self, String> {
        Self::parse_from(env::args_os().skip(1))
    }

    fn parse_from(values: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut values = values.into_iter();
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
        let mut web = None;
        let mut scripts = None;
        let mut listen_lan = false;
        let mut advertise = None;
        let mut broadcast = false;
        let mut version = Version::parse("0.1.0-dev.1").expect("有效默认版本");
        while let Some(flag) = values.next() {
            match flag.to_string_lossy().as_ref() {
                "--listen" => {
                    if values.next().as_deref() != Some(std::ffi::OsStr::new("lan")) {
                        return Err("--listen 只接受 lan".to_owned());
                    }
                    listen_lan = true;
                }
                "--advertise" => {
                    let value = values
                        .next()
                        .ok_or_else(|| "缺少 --advertise 值".to_owned())?;
                    advertise = Some(
                        value
                            .to_string_lossy()
                            .parse()
                            .map_err(|_| "--advertise 必须是 IPv4 地址".to_owned())?,
                    );
                }
                "--broadcast" => broadcast = true,
                "--version" => {
                    let value = values
                        .next()
                        .ok_or_else(|| "缺少 --version 值".to_owned())?;
                    version = value
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "--version 必须是语义化版本".to_owned())?;
                }
                "--web" => {
                    web = Some(
                        values
                            .next()
                            .map(PathBuf::from)
                            .ok_or_else(|| "缺少 --web 路径".to_owned())?,
                    );
                }
                "--scripts" => {
                    scripts = Some(
                        values
                            .next()
                            .map(PathBuf::from)
                            .ok_or_else(|| "缺少 --scripts 路径".to_owned())?,
                    );
                }
                other => return Err(format!("未知选项：{other}")),
            }
        }
        if broadcast && !listen_lan {
            return Err("--broadcast 需要同时指定 --listen lan".to_owned());
        }
        if listen_lan && advertise.is_none() {
            return Err("--listen lan 需要 --advertise <LAN-IP>".to_owned());
        }
        Ok(Self {
            tool_file,
            server,
            control,
            web: web.ok_or_else(|| "缺少 --web 路径".to_owned())?,
            scripts: scripts.ok_or_else(|| "缺少 --scripts 路径".to_owned())?,
            listen_lan,
            advertise,
            broadcast,
            version,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Arguments, String> {
        Arguments::parse_from(arguments.iter().map(|value| OsString::from(*value)))
    }

    #[test]
    fn complete_resources_are_required() {
        let missing_web = parse(&["tool.json", "server", "control", "--scripts", "scripts"])
            .expect_err("web resource should be required");
        assert!(missing_web.contains("--web"));

        let missing_scripts = parse(&["tool.json", "server", "control", "--web", "web"])
            .expect_err("scripts resource should be required");
        assert!(missing_scripts.contains("--scripts"));
    }

    #[test]
    fn complete_resources_and_lan_options_are_parsed() {
        let arguments = parse(&[
            "tool.json",
            "server",
            "control",
            "--web",
            "web",
            "--scripts",
            "scripts",
            "--listen",
            "lan",
            "--advertise",
            "192.168.1.24",
            "--broadcast",
            "--version",
            "0.1.1-dev.1",
        ])
        .expect("valid arguments");

        assert_eq!(arguments.web, PathBuf::from("web"));
        assert_eq!(arguments.scripts, PathBuf::from("scripts"));
        assert!(arguments.listen_lan);
        assert!(arguments.broadcast);
        assert_eq!(arguments.advertise, Some(Ipv4Addr::new(192, 168, 1, 24)));
        assert_eq!(arguments.version, Version::parse("0.1.1-dev.1").unwrap());
    }

    #[tokio::test]
    async fn simultaneous_update_sources_receive_different_http_ports() {
        let first = bind_update_listener(false).await.expect("first listener");
        let second = bind_update_listener(false).await.expect("second listener");
        assert_ne!(
            first.local_addr().unwrap().port(),
            second.local_addr().unwrap().port()
        );
    }
}
