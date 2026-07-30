//! 用户可直接修改的 Dioxus Desktop 控制端。
//!
//! 页面视觉以 `docs/design/control-app.html` 为参考，但不使用模拟标题栏或演示数据。

use std::{fs, process::Command, time::Duration};

use cocos_build_lan_contract::{ToolSettings, ToolStatus};
use cocos_build_lan_core::{
    CONTROL_PORT, CONTROL_PROTOCOL_VERSION, RestartReadiness, ServiceStatus, ToolError,
    ToolLaunchSpec, ToolSupervisor, UpdateManifest, UpdateSource, receive_lan_manifest,
};

use dioxus::{document, prelude::*};
use dioxus_desktop::{Config, WindowBuilder};

const CONTROL_APP_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let server_binary = "cocos-build-lan-server";
    let control_binary = "cocos-build-lan-control";
    let spec = ToolLaunchSpec::discover(server_binary, control_binary)
        .expect("需要从包含 tool.json 的生成项目目录启动控制端");
    let title = format!("{} 控制端", spec.identity.display_name);
    let supervisor = ToolSupervisor::new(spec);
    LaunchBuilder::desktop()
        .with_context(supervisor)
        .with_cfg(Config::new().with_window(WindowBuilder::new().with_title(title)))
        .launch(App);
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Page {
    Overview,
    Updates,
    Rollback,
    Settings,
    Logs,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum NoticeTone {
    Success,
    Warning,
    Error,
}

#[derive(Clone, Eq, PartialEq)]
struct Notice {
    tone: NoticeTone,
    message: String,
}

#[derive(Clone)]
struct ReleaseRow {
    version: String,
    source: String,
    target: String,
    size: String,
    state: String,
    tone: &'static str,
}

struct ControlSnapshot {
    service: ServiceStatus,
    settings: ToolSettings,
    business: ToolStatus,
    readiness: RestartReadiness,
    releases: Vec<ReleaseRow>,
}

#[component]
fn App() -> Element {
    let supervisor = use_context::<ToolSupervisor>();
    let tool_id = supervisor.spec().identity.tool_id.to_string();
    let theme_key = format!("lan-toolkit-theme-{tool_id}");

    let mut page = use_signal(|| Page::Overview);
    let mut dark = use_signal(|| true);
    let mut service_running = use_signal(|| false);
    let mut connection = use_signal(|| "正在连接本机服务…".to_owned());
    let mut version = use_signal(|| "—".to_owned());
    let mut business_status = use_signal(|| "尚未读取业务状态".to_owned());
    let mut server_path = use_signal(|| "—".to_owned());
    let mut readiness = use_signal(|| None::<RestartReadiness>);
    let mut settings = use_signal(ToolSettings::default);
    let mut persisted_settings = use_signal(ToolSettings::default);
    let mut settings_dirty = use_signal(|| false);
    let mut pending = use_signal(|| None::<cocos_build_lan_core::PendingUpdate>);
    let mut releases = use_signal(Vec::<ReleaseRow>::new);
    let mut last_check = use_signal(|| "尚未检查更新".to_owned());
    let mut log_text = use_signal(|| "尚未读取日志。".to_owned());
    let mut log_level = use_signal(|| "全部级别".to_owned());
    let mut log_filter = use_signal(String::new);
    let notice = use_signal(|| None::<Notice>);
    let mut legacy_data_dir = use_signal(String::new);
    let mut legacy_preview = use_signal(|| None::<serde_json::Value>);

    {
        let theme_key = theme_key.clone();
        use_effect(move || {
            let theme_key = theme_key.clone();
            spawn(async move {
                let key = serde_json::to_string(&theme_key).expect("主题键可序列化");
                let script = format!("return localStorage.getItem({key});");
                if let Ok(Some(saved)) = document::eval(&script).join::<Option<String>>().await {
                    dark.set(saved != "light");
                }
            });
        });
    }

    {
        let supervisor = supervisor.clone();
        use_effect(move || {
            let supervisor = supervisor.clone();
            spawn(async move {
                match load_snapshot(&supervisor).await {
                    Ok(snapshot) => {
                        service_running.set(true);
                        connection.set("运行中（健康检查已确认）".to_owned());
                        version.set(snapshot.service.health.version.to_string());
                        server_path.set(snapshot.service.server_path.display().to_string());
                        business_status.set(format!(
                            "{}（已完成任务：{}）",
                            snapshot.business.summary, snapshot.business.completed_jobs
                        ));
                        readiness.set(Some(snapshot.readiness));
                        persisted_settings.set(snapshot.settings.clone());
                        settings.set(snapshot.settings);
                        settings_dirty.set(false);
                        releases.set(snapshot.releases);
                    }
                    Err(error) => {
                        service_running.set(false);
                        connection.set(format!("服务不可用：{error}"));
                        post_notice(
                            notice,
                            NoticeTone::Error,
                            format!("读取控制状态失败：{error}"),
                        );
                    }
                }
            });
        });
    }

    {
        let supervisor = supervisor.clone();
        use_effect(move || {
            let supervisor = supervisor.clone();
            spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
                    let Ok(settings) = load_settings().await else {
                        continue;
                    };
                    if settings.update.release_manifest_url.trim().is_empty() {
                        continue;
                    }
                    if let Ok(staged) = supervisor
                        .stage_update_from_manifest_url(&settings.update.release_manifest_url)
                        .await
                    {
                        last_check.set(format!("已校验并暂存 {}", staged.manifest.version));
                        pending.set(Some(staged.clone()));
                        if settings.update.auto_apply_updates
                            && !settings_dirty()
                            && handoff_update(&supervisor, &staged).is_ok()
                        {
                            std::process::exit(0);
                        }
                    }
                }
            });
        });
    }

    {
        let supervisor = supervisor.clone();
        use_effect(move || {
            let supervisor = supervisor.clone();
            spawn(async move {
                loop {
                    if !persisted_settings().update.lan_dev_enabled {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                    match receive_lan_manifest(Duration::from_secs(2)).await {
                        Ok(manifest) => {
                            if !persisted_settings().update.lan_dev_enabled {
                                continue;
                            }
                            if manifest.source != UpdateSource::LanDev {
                                last_check.set("已忽略非 LAN Dev 广播。".to_owned());
                                continue;
                            }
                            let store = match supervisor.spec().update_store() {
                                Ok(store) => store,
                                Err(error) => {
                                    last_check.set(format!("LAN Dev 初始化失败：{error}"));
                                    continue;
                                }
                            };
                            if let Err(error) = store.validate_manifest(&manifest) {
                                last_check.set(format!("LAN Dev 广播已拒绝：{error}"));
                                continue;
                            }
                            if pending_matches(&pending(), &manifest)
                                || store.release_is_installed(&manifest.version)
                            {
                                last_check.set(format!(
                                    "LAN Dev v{} 已暂存或安装，已忽略重复广播。",
                                    manifest.version
                                ));
                                continue;
                            }
                            match supervisor.stage_update(manifest).await {
                                Ok(staged) => {
                                    last_check.set(format!(
                                        "LAN Dev 已校验并暂存 {}",
                                        staged.manifest.version
                                    ));
                                    pending.set(Some(staged.clone()));
                                    post_notice(
                                        notice,
                                        NoticeTone::Success,
                                        "收到并暂存 LAN Dev 完整版本包。".to_owned(),
                                    );
                                    let saved = persisted_settings();
                                    if saved.update.auto_apply_updates
                                        && !settings_dirty()
                                        && handoff_update(&supervisor, &staged).is_ok()
                                    {
                                        std::process::exit(0);
                                    }
                                }
                                Err(error) => {
                                    last_check.set(format!("LAN Dev 下载或校验失败：{error}"))
                                }
                            }
                        }
                        Err(ToolError::LanDevTimedOut) => {}
                        Err(error) => last_check.set(format!("LAN Dev 监听失败：{error}")),
                    }
                }
            });
        });
    }

    let theme_class = if dark() {
        "control dark"
    } else {
        "control light"
    };
    let pending_value = pending();
    let readiness_value = readiness();
    let release_rows = releases();
    let current_notice = notice();
    let rendered_log = filter_logs(&log_text(), &log_level(), &log_filter());
    let import_supervisor = supervisor.clone();
    let overview_selected = nav_class(page() == Page::Overview);
    let updates_selected = nav_class(page() == Page::Updates);
    let rollback_selected = nav_class(page() == Page::Rollback);
    let settings_selected = nav_class(page() == Page::Settings);
    let logs_selected = nav_class(page() == Page::Logs);
    let readiness_label = readiness_value
        .as_ref()
        .map_or("读取中", RestartReadiness::label);
    let readiness_tone = readiness_value.as_ref().map_or("info", readiness_tone);
    let readiness_explanation = readiness_value.as_ref().map_or_else(
        || "等待服务返回重启就绪状态。".to_owned(),
        readiness_explanation,
    );
    let update_pill_tone = if pending_value.is_some() {
        "warn"
    } else {
        "info"
    };
    let update_pill_label = if pending_value.is_some() {
        "等待安全切换"
    } else {
        "空闲"
    };
    let service_pill_label = if service_running() {
        "运行中"
    } else if connection() == "正在连接本机服务…" {
        "连接中"
    } else if connection() == "已停止" {
        "已停止"
    } else {
        "不可用"
    };
    let service_pill_tone = if service_running() {
        "ok"
    } else if service_pill_label == "连接中" || service_pill_label == "已停止" {
        "info"
    } else {
        "err"
    };

    rsx! {
        style { {CONTROL_CSS} }
        main { class: "{theme_class}",
            aside { class: "sidebar",
                div { class: "sb-tool",
                    div { class: "name", "{supervisor.spec().identity.display_name}" }
                    div { class: "tid", "tool_id {tool_id}" }
                }
                nav { class: "nav-list",
                    button { class: "{overview_selected}", onclick: move |_| page.set(Page::Overview),
                        Icon { kind: IconKind::Activity }
                        "概览"
                    }
                    button { class: "{updates_selected}", onclick: move |_| page.set(Page::Updates),
                        Icon { kind: IconKind::Download }
                        "更新"
                        if let Some(update) = pending_value.as_ref() {
                            span { class: "badge", "v{update.manifest.version}" }
                        }
                    }
                    button { class: "{rollback_selected}", onclick: move |_| page.set(Page::Rollback),
                        Icon { kind: IconKind::History }
                        "回滚"
                    }
                    button { class: "{settings_selected}", onclick: move |_| page.set(Page::Settings),
                        Icon { kind: IconKind::Settings }
                        "设置"
                    }
                    button { class: "{logs_selected}", onclick: move |_| page.set(Page::Logs),
                        Icon { kind: IconKind::Logs }
                        "日志"
                    }
                }
                div { class: "sb-footer",
                    div { class: "row", span { "控制端版本" } code { "{CONTROL_APP_VERSION}" } }
                    div { class: "row", span { "协议" } code { "lan-toolkit/{CONTROL_PROTOCOL_VERSION}" } }
                    div { class: "path-row", span { "数据目录" } code { "{supervisor.spec().paths.root().display()}" } }
                }
            }
            section { class: "content",
                header { class: "content-toolbar",
                    div { class: "page-head",
                        h1 { "{page_title(page())}" }
                        p { "{page_subtitle(page())}" }
                    }
                    div { class: "toolbar-actions",
                        button { class: "icon-button", title: "重新读取本机状态", aria_label: "重新读取本机状态", onclick: {
                            let supervisor = supervisor.clone();
                            move |_| {
                                let supervisor = supervisor.clone();
                                spawn(async move {
                                    match load_snapshot(&supervisor).await {
                                        Ok(snapshot) => {
                                            service_running.set(true);
                                            connection.set("运行中（健康检查已确认）".to_owned());
                                            version.set(snapshot.service.health.version.to_string());
                                            server_path.set(snapshot.service.server_path.display().to_string());
                                            business_status.set(format!("{}（已完成任务：{}）", snapshot.business.summary, snapshot.business.completed_jobs));
                                            readiness.set(Some(snapshot.readiness));
                                            settings.set(snapshot.settings);
                                            settings_dirty.set(false);
                                            releases.set(snapshot.releases);
                                            post_notice(notice, NoticeTone::Success, "已重新读取本机状态。".to_owned());
                                        }
                                        Err(error) => {
                                            service_running.set(false);
                                            post_notice(notice, NoticeTone::Error, format!("读取失败：{error}"));
                                        }
                                    }
                                });
                            }
                        }, Icon { kind: IconKind::Refresh } }
                        button { class: "icon-button", title: "切换浅色 / 深色主题", aria_label: "切换浅色 / 深色主题", onclick: {
                            let theme_key = theme_key.clone();
                            move |_| {
                                let next = !dark();
                                dark.set(next);
                                let key = serde_json::to_string(&theme_key).expect("主题键可序列化");
                                let value = if next { "dark" } else { "light" };
                                let script = format!("localStorage.setItem({key}, {value:?});");
                                spawn(async move {
                                    let _ = document::eval(&script).join::<()>().await;
                                });
                            }
                        },
                            if dark() { Icon { kind: IconKind::Sun } } else { Icon { kind: IconKind::Moon } }
                        }
                    }
                }

                if page() == Page::Overview {
                    if let Some(update) = pending_value.as_ref() {
                        div { class: "banner",
                            Icon { kind: IconKind::Warning }
                            div {
                                div { class: "b-title", "已收到更新 v{update.manifest.version}，等待安全切换" }
                                div { class: "b-text", "载荷已通过 tool_id、平台、大小与 SHA-256 校验。{readiness_explanation}" }
                            }
                        }
                    }
                    section { class: "grid-2",
                        article { class: "card",
                            div { class: "card-head",
                                h2 { "工具服务" }
                                span { class: "pill {service_pill_tone}", "{service_pill_label}" }
                                span { class: "spacer" }
                                button { class: "btn", onclick: {
                                    let supervisor = supervisor.clone();
                                    move |_| {
                                        let supervisor = supervisor.clone();
                                        let running = service_running();
                                        spawn(async move {
                                            let result = if running {
                                                supervisor.restart().await
                                            } else {
                                                supervisor.start().await
                                            };
                                            match result {
                                                Ok(service) => {
                                                    service_running.set(true);
                                                    connection.set(if running { "运行中（已重启）" } else { "运行中（已启动）" }.to_owned());
                                                    version.set(service.health.version.to_string());
                                                    server_path.set(service.server_path.display().to_string());
                                                    post_notice(notice, NoticeTone::Success, if running { "已完成安全重启。" } else { "已启动服务。" }.to_owned());
                                                }
                                                Err(error) => post_notice(notice, NoticeTone::Warning, format!("操作未执行：{error}")),
                                            }
                                        });
                                    }
                                }, if service_running() { "重启" } else { "启动" } }
                                button { class: "btn danger", onclick: {
                                    let supervisor = supervisor.clone();
                                    move |_| {
                                        let supervisor = supervisor.clone();
                                        spawn(async move {
                                            match supervisor.stop().await {
                                                Ok(()) => {
                                                    service_running.set(false);
                                                    connection.set("已停止".to_owned());
                                                    version.set("—".to_owned());
                                                    readiness.set(None);
                                                    post_notice(notice, NoticeTone::Success, "服务已优雅退出。".to_owned());
                                                }
                                                Err(error) => post_notice(notice, NoticeTone::Warning, format!("停止未执行：{error}")),
                                            }
                                        });
                                    }
                                }, "停止" }
                            }
                            div { class: "card-body kv",
                                div { class: "k", "当前状态" } div { class: "v", "{connection}" }
                                div { class: "k", "当前版本" } div { class: "v", "{version}" }
                                div { class: "k", "回环 URL" } div { class: "v link", "http://127.0.0.1:{CONTROL_PORT}" }
                                div { class: "k", "服务二进制" } div { class: "v", "{server_path}" }
                                div { class: "k", "业务状态" } div { class: "v", "{business_status}" }
                            }
                        }
                        article { class: "card",
                            div { class: "card-head",
                                h2 { "重启就绪（RestartReadiness）" }
                                span { class: "pill {readiness_tone}", "{readiness_label}" }
                            }
                            div { class: "card-body kv",
                                div { class: "k", "当前状态" } div { class: "v", "{readiness_label}" }
                                div { class: "k", "说明" } div { class: "v", "{readiness_explanation}" }
                                div { class: "k", "自动应用" } div { class: "v", if readiness_value.as_ref().is_some_and(RestartReadiness::permits_automatic_apply) { "允许" } else { "等待就绪" } }
                                div { class: "k", "安全流程" } div { class: "v", "readiness → prepare → readiness → shutdown" }
                            }
                        }
                    }
                    if let Some(update) = pending_value.as_ref() {
                        article { class: "card",
                            div { class: "card-head", h2 { "待应用更新" } span { class: "pill dev", "{update_source_label(&update.manifest.source)}" } }
                            div { class: "card-body kv",
                                div { class: "k", "待应用版本" } div { class: "v", "v{update.manifest.version}" }
                                div { class: "k", "目标平台" } div { class: "v", "{update.manifest.target}" }
                                div { class: "k", "SHA-256" } div { class: "v", "{short_hash(&update.manifest.sha256)} · 已校验" }
                                div { class: "k", "发布说明" } div { class: "v", "{update.manifest.notes}" }
                            }
                        }
                    } else {
                        EmptyCard { title: "待应用更新", message: "当前没有通过校验且等待应用的更新。" }
                    }
                }

                if page() == Page::Updates {
                    article { class: "card",
                        div { class: "card-head",
                            h2 { "更新事务" }
                            span { class: "pill {update_pill_tone}", "{update_pill_label}" }
                            span { class: "spacer" }
                            button { class: "btn", onclick: {
                                let supervisor = supervisor.clone();
                                move |_| {
                                    let supervisor = supervisor.clone();
                                    let url = settings().update.release_manifest_url;
                                    spawn(async move {
                                        if url.trim().is_empty() {
                                            post_notice(notice, NoticeTone::Warning, "请先在设置中填写 Release 清单 URL。".to_owned());
                                            return;
                                        }
                                        match supervisor.stage_update_from_manifest_url(&url).await {
                                            Ok(staged) => {
                                                last_check.set(format!("已校验并暂存 {}", staged.manifest.version));
                                                pending.set(Some(staged));
                                                post_notice(notice, NoticeTone::Success, "更新已下载并完成校验。".to_owned());
                                            }
                                            Err(error) => post_notice(notice, NoticeTone::Error, format!("检查更新失败：{error}")),
                                        }
                                    });
                                }
                            }, "检查更新" }
                            button { class: "btn primary", disabled: pending_value.is_none(), onclick: {
                                let supervisor = supervisor.clone();
                                move |_| {
                                    let supervisor = supervisor.clone();
                                    let staged = pending();
                                    spawn(async move {
                                        let Some(staged) = staged else { return };
                                        if settings_dirty() {
                                            post_notice(notice, NoticeTone::Warning, "请先保存或还原设置，再应用完整版本更新。".to_owned());
                                            return;
                                        }
                                        match handoff_update(&supervisor, &staged) {
                                            Ok(()) => std::process::exit(0),
                                            Err(error) => post_notice(notice, NoticeTone::Warning, format!("更新保留为待应用：{error}")),
                                        }
                                    });
                                }
                            }, "应用已下载更新" }
                        }
                        div { class: "card-body steps",
                            UpdateStep { state: if pending_value.is_some() { "done" } else { "idle" }, index: "1", title: "发现与 tool_id 过滤", detail: if pending_value.is_some() { "当前待应用清单已匹配此工具。" } else { "尚未取得待应用清单。" } }
                            UpdateStep { state: if pending_value.is_some() { "done" } else { "idle" }, index: "2", title: "下载与 SHA-256 校验", detail: if pending_value.is_some() { "载荷已暂存，平台、大小与哈希均已验证。" } else { "下载后才会写入 staging/。" } }
                            UpdateStep { state: if pending_value.is_some() { "wait" } else { "idle" }, index: "3", title: "等待 RestartReadiness = Ready", detail: "{readiness_explanation}" }
                            UpdateStep { state: "idle", index: "4", title: "原子切换与健康检查", detail: "仅在切换动作执行后更新 active.json 并验证新服务。" }
                        }
                    }
                    article { class: "card",
                        div { class: "card-head", h2 { "更新偏好" } }
                        div { class: "card-body kv",
                            div { class: "k", "Release 清单" } div { class: "v", if settings().update.release_manifest_url.is_empty() { "尚未配置" } else { "{settings().update.release_manifest_url}" } }
                            div { class: "k", "自动应用" } div { class: "v", if settings().update.auto_apply_updates { "仅在 Ready 且设置已保存时应用" } else { "已关闭" } }
                            div { class: "k", "LAN Dev" } div { class: "v", if persisted_settings().update.lan_dev_enabled { "已启用并后台监听（仅可信局域网）" } else if settings().update.lan_dev_enabled { "等待保存后启用" } else { "未启用" } }
                            div { class: "k", "最近检查" } div { class: "v", "{last_check}" }
                        }
                    }
                }

                if page() == Page::Rollback {
                    article { class: "card",
                        div { class: "card-head", h2 { "已安装版本" } }
                        if release_rows.is_empty() {
                            div { class: "empty", "本地 releases/ 目录尚无已安装版本；首次运行使用同目录的服务二进制。" }
                        } else {
                            div { class: "table-wrap",
                                table { class: "dense",
                                    thead { tr { th { "版本" } th { "来源" } th { "目标平台" } th { "大小" } th { "状态" } } }
                                    tbody {
                                        for release in release_rows {
                                            tr {
                                                td { class: "mono", "v{release.version}" }
                                                td { span { class: "pill {source_tone(&release.source)}", "{release.source}" } }
                                                td { class: "mono", "{release.target}" }
                                                td { class: "mono", "{release.size}" }
                                                td { span { class: "pill {release.tone}", "{release.state}" } }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    article { class: "card",
                        div { class: "card-head", h2 { "回滚保护" } }
                        div { class: "card-body kv",
                            div { class: "k", "切换指针" } div { class: "v", "active.json（临时文件 + rename）" }
                            div { class: "k", "自动回滚" } div { class: "v", "新版健康、协议或启动检查失败时恢复旧指针并重启旧版本。" }
                            div { class: "k", "配置与数据" } div { class: "v", "位于 releases/ 目录外，更新与回滚不会覆盖。" }
                            div { class: "k", "手动回滚" } div { class: "v", "v0.1 不提供控制端手动回滚操作。" }
                        }
                    }
                }

                if page() == Page::Settings {
                    article { class: "card settings-card",
                        div { class: "card-head", h2 { "本地设置（可在 tool-contract 自由扩展）" } span { class: "pill info", if settings_dirty() { "未保存" } else { "已保存" } } }
                        div { class: "card-body",
                            div { class: "form-intro", "服务端将设置持久化到本机数据目录。tool_id 来自 tool.json，不能在此覆盖。扩展 BusinessSettings 后，在这里添加相应字段。" }
                            div { class: "settings-grid",
                                label { class: "field",
                                    span { "业务问候语" }
                                    input { value: "{settings().business.greeting}", placeholder: "例如：你好，来自局域网工具", oninput: move |event| {
                                        let mut next = settings();
                                        next.business.greeting = event.value();
                                        settings.set(next);
                                        settings_dirty.set(true);
                                    } }
                                    small { "模板业务示例；你可以替换 BusinessSettings 并新增字段。" }
                                }
                                label { class: "field",
                                    span { "Git 账号（仅本机）" }
                                    input { value: "{settings().business.git_username}", autocomplete: "username", oninput: move |event| {
                                        let mut next = settings();
                                        next.business.git_username = event.value();
                                        settings.set(next);
                                        settings_dirty.set(true);
                                    } }
                                    small { "仅写入控制端私有配置；LAN 网页只会看到“已配置”。" }
                                }
                                label { class: "field",
                                    span { "Git 密码 / Token（仅本机）" }
                                    input { r#type: "password", value: "{settings().business.git_password}", autocomplete: "current-password", oninput: move |event| {
                                        let mut next = settings();
                                        next.business.git_password = event.value();
                                        settings.set(next);
                                        settings_dirty.set(true);
                                    } }
                                    small { "不进入 /api/settings，也不写入业务 data/settings.json。" }
                                }
                                label { class: "field wide",
                                    span { "Release 清单 URL" }
                                    input { r#type: "url", value: "{settings().update.release_manifest_url}", placeholder: "https://github.com/.../manifest.json", oninput: move |event| {
                                        let mut next = settings();
                                        next.update.release_manifest_url = event.value();
                                        settings.set(next);
                                        settings_dirty.set(true);
                                    } }
                                    small { "留空即不检查 GitHub Releases 或静态更新源。" }
                                }
                                label { class: "switch-field",
                                    input { r#type: "checkbox", checked: settings().update.auto_apply_updates, onchange: move |event| {
                                        let mut next = settings();
                                        next.update.auto_apply_updates = event.checked();
                                        settings.set(next);
                                        settings_dirty.set(true);
                                    } }
                                    span { "服务就绪时自动应用完整版本更新" }
                                }
                                label { class: "switch-field",
                                    input { r#type: "checkbox", checked: settings().update.lan_dev_enabled, onchange: move |event| {
                                        let mut next = settings();
                                        next.update.lan_dev_enabled = event.checked();
                                        settings.set(next);
                                        settings_dirty.set(true);
                                    } }
                                    span { "接收可信局域网的 LAN Dev 完整版本包" }
                                }
                            }
                            div { class: "form-actions",
                                button { class: "btn primary", onclick: {
                                    move |_| {
                                        let value = settings();
                                        spawn(async move {
                                            match save_settings(&value).await {
                                                Ok(saved) => {
                                                    persisted_settings.set(saved.clone());
                                                    settings.set(saved);
                                                    settings_dirty.set(false);
                                                    post_notice(notice, NoticeTone::Success, "设置已保存到运行中的服务端。".to_owned());
                                                }
                                                Err(error) => post_notice(notice, NoticeTone::Error, format!("保存设置失败：{error}")),
                                            }
                                        });
                                    }
                                }, "保存设置" }
                                button { class: "btn", onclick: move |_| {
                                    spawn(async move {
                                        match load_settings().await {
                                            Ok(saved) => {
                                                persisted_settings.set(saved.clone());
                                                settings.set(saved);
                                                settings_dirty.set(false);
                                                post_notice(notice, NoticeTone::Success, "已还原为服务端已保存的设置。".to_owned());
                                            }
                                            Err(error) => post_notice(notice, NoticeTone::Error, format!("读取设置失败：{error}")),
                                        }
                                    });
                                }, "还原" }
                            }
                            article { class: "card", style: "margin-top:18px",
                                div { class: "card-head", h2 { "一次性导入旧 Cocos Build 数据" } }
                                div { class: "card-body",
                                    p { class: "form-intro", "选择旧工具 data/ 目录。导入前只预览；确认后迁移 settings、Git 凭据和 preps，绝不修改源目录。目标已有业务数据时会拒绝。" }
                                    div { class: "settings-grid",
                                        label { class: "field wide",
                                            span { "旧工具 data/ 路径" }
                                            input { value: "{legacy_data_dir}", placeholder: "/Users/.../cocos_build/data", oninput: move |event| legacy_data_dir.set(event.value()) }
                                        }
                                    }
                                    div { class: "form-actions",
                                        button { class: "btn", disabled: legacy_data_dir().trim().is_empty(), onclick: move |_| {
                                            let path = legacy_data_dir();
                                            spawn(async move {
                                                match preview_legacy_import(&path).await {
                                                    Ok(preview) => {
                                                        legacy_preview.set(Some(preview));
                                                        post_notice(notice, NoticeTone::Success, "旧数据预览完成；请核对后确认导入。".to_owned());
                                                    }
                                                    Err(error) => post_notice(notice, NoticeTone::Error, format!("预览失败：{error}")),
                                                }
                                            });
                                        }, "预览导入" }
                                        if let Some(preview) = legacy_preview() {
                                            LegacyPreview { preview: preview.clone() }
                                            button { class: "btn danger", onclick: move |_| {
                                                let path = legacy_data_dir();
                                                let supervisor = import_supervisor.clone();
                                                spawn(async move {
                                                    match import_legacy_data(&path).await {
                                                        Ok(_) => match load_snapshot(&supervisor).await {
                                                            Ok(snapshot) => {
                                                                settings.set(snapshot.settings.clone());
                                                                persisted_settings.set(snapshot.settings);
                                                                legacy_preview.set(None);
                                                                post_notice(notice, NoticeTone::Success, "旧数据已导入；Git 凭据仅保存于本机控制端。".to_owned());
                                                            }
                                                            Err(error) => post_notice(notice, NoticeTone::Warning, format!("导入完成，但刷新状态失败：{error}")),
                                                        },
                                                        Err(error) => post_notice(notice, NoticeTone::Error, format!("导入失败：{error}")),
                                                    }
                                                });
                                            }, "确认导入" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                if page() == Page::Logs {
                    div { class: "log-toolbar",
                        select { value: "{log_level}", onchange: move |event| log_level.set(event.value()),
                            option { value: "全部级别", "全部级别" }
                            option { value: "INFO", "INFO" }
                            option { value: "WARN", "WARN" }
                            option { value: "ERROR", "ERROR" }
                            option { value: "DEBUG", "DEBUG" }
                        }
                        input { value: "{log_filter}", placeholder: "按关键字过滤，如 update / guard / healthz", oninput: move |event| log_filter.set(event.value()) }
                        button { class: "btn", onclick: {
                            let path = supervisor.spec().paths.server_log();
                            move |_| match fs::read_to_string(&path) {
                                Ok(contents) => {
                                    log_text.set(contents);
                                    post_notice(notice, NoticeTone::Success, "已刷新服务端日志。".to_owned());
                                }
                                Err(error) => post_notice(notice, NoticeTone::Error, format!("读取日志失败：{error}")),
                            }
                        }, "刷新日志" }
                    }
                    pre { class: "log-view", "{rendered_log}" }
                }
            }
            if let Some(current) = current_notice {
                div { class: "toast {notice_tone(current.tone)}", "{current.message}" }
            }
        }
    }
}

#[component]
fn LegacyPreview(preview: serde_json::Value) -> Element {
    let number = |key: &str| {
        preview
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let project_count = number("projectCount");
    let task_count = number("taskCount");
    let task_group_count = number("taskGroupCount");
    let prep_project_count = number("prepProjectCount");
    rsx! {
        span { class: "hint mono", "项目 {project_count} · 任务 {task_count} · 任务组 {task_group_count} · 准备项目 {prep_project_count}" }
    }
}

#[component]
fn EmptyCard(title: &'static str, message: &'static str) -> Element {
    rsx! {
        article { class: "card",
            div { class: "card-head", h2 { "{title}" } }
            div { class: "empty", "{message}" }
        }
    }
}

#[component]
fn UpdateStep(state: String, index: String, title: String, detail: String) -> Element {
    rsx! {
        div { class: "step {state}",
            span { class: "step-icon", "{index}" }
            div { div { class: "step-title", "{title}" } div { class: "step-detail", "{detail}" } }
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum IconKind {
    Activity,
    Download,
    History,
    Settings,
    Logs,
    Refresh,
    Moon,
    Sun,
    Warning,
}

#[component]
fn Icon(kind: IconKind) -> Element {
    match kind {
        IconKind::Activity => {
            rsx! { svg { class: "icon", view_box: "0 0 24 24", path { d: "M3 12h4l3-8 4 16 3-8h4" } } }
        }
        IconKind::Download => {
            rsx! { svg { class: "icon", view_box: "0 0 24 24", path { d: "M12 3v12m0 0-4-4m4 4 4-4M4 21h16" } } }
        }
        IconKind::History => {
            rsx! { svg { class: "icon", view_box: "0 0 24 24", path { d: "M3 12a9 9 0 1 0 9-9M3 12V7m0 5h5" } } }
        }
        IconKind::Settings => {
            rsx! { svg { class: "icon", view_box: "0 0 24 24", circle { cx: "12", cy: "12", r: "3" } path { d: "M19 12a7 7 0 0 0-.1-1.2l2-1.6-2-3.4-2.4 1a7 7 0 0 0-2-1.2L14 3h-4l-.5 2.6a7 7 0 0 0-2 1.2l-2.4-1-2 3.4 2 1.6A7 7 0 0 0 5 12a7 7 0 0 0 .1 1.2l-2 1.6 2 3.4 2.4-1a7 7 0 0 0 2 1.2L10 21h4l.5-2.6a7 7 0 0 0 2-1.2l2.4 1 2-3.4-2-1.6A7 7 0 0 0 19 12z" } } }
        }
        IconKind::Logs => {
            rsx! { svg { class: "icon", view_box: "0 0 24 24", path { d: "M4 6h16M4 12h16M4 18h10" } } }
        }
        IconKind::Refresh => {
            rsx! { svg { class: "icon", view_box: "0 0 24 24", path { d: "M20 11a8 8 0 1 0 2 5.5M20 4v7h-7" } } }
        }
        IconKind::Moon => {
            rsx! { svg { class: "icon", view_box: "0 0 24 24", path { d: "M21 12.8A9 9 0 1 1 11.2 3 7 7 0 0 0 21 12.8z" } } }
        }
        IconKind::Sun => {
            rsx! { svg { class: "icon", view_box: "0 0 24 24", circle { cx: "12", cy: "12", r: "4" } path { d: "M12 2v2m0 16v2M4.9 4.9l1.4 1.4m11.4 11.4 1.4 1.4M2 12h2m16 0h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" } } }
        }
        IconKind::Warning => {
            rsx! { svg { class: "icon", view_box: "0 0 24 24", path { d: "M12 9v4m0 4h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" } } }
        }
    }
}

async fn load_snapshot(supervisor: &ToolSupervisor) -> Result<ControlSnapshot, String> {
    let service = supervisor
        .start()
        .await
        .map_err(|error| error.to_string())?;
    let (settings, business) = load_service_data()
        .await
        .map_err(|error| error.to_string())?;
    let readiness = supervisor
        .readiness()
        .await
        .map_err(|error| error.to_string())?;
    let releases = load_release_rows(supervisor)?;
    Ok(ControlSnapshot {
        service,
        settings,
        business,
        readiness,
        releases,
    })
}

async fn load_service_data() -> Result<(ToolSettings, ToolStatus), reqwest::Error> {
    let client = reqwest::Client::new();
    let settings = client
        .get(format!(
            "http://127.0.0.1:{CONTROL_PORT}/api/control-config"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let business = client
        .get(format!(
            "http://127.0.0.1:{CONTROL_PORT}/api/control-status"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok((settings, business))
}

async fn load_settings() -> Result<ToolSettings, reqwest::Error> {
    reqwest::Client::new()
        .get(format!(
            "http://127.0.0.1:{CONTROL_PORT}/api/control-config"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
}

async fn save_settings(settings: &ToolSettings) -> Result<ToolSettings, reqwest::Error> {
    reqwest::Client::new()
        .put(format!(
            "http://127.0.0.1:{CONTROL_PORT}/api/control-config"
        ))
        .json(settings)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
}

async fn preview_legacy_import(data_dir: &str) -> Result<serde_json::Value, reqwest::Error> {
    reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{CONTROL_PORT}/api/control/import/preview"
        ))
        .json(&serde_json::json!({ "dataDir": data_dir }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
}

async fn import_legacy_data(data_dir: &str) -> Result<serde_json::Value, reqwest::Error> {
    reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{CONTROL_PORT}/api/control/import"
        ))
        .json(&serde_json::json!({ "dataDir": data_dir }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
}

fn load_release_rows(supervisor: &ToolSupervisor) -> Result<Vec<ReleaseRow>, String> {
    let store = supervisor
        .spec()
        .update_store()
        .map_err(|error| error.to_string())?;
    let active = store.active_release().map_err(|error| error.to_string())?;
    let releases_path = supervisor.spec().paths.root().join("releases");
    let mut rows = Vec::new();
    if !releases_path.exists() {
        return Ok(rows);
    }
    for entry in fs::read_dir(releases_path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let manifest_path = entry.path().join("manifest.json");
        let Ok(bytes) = fs::read(manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<UpdateManifest>(&bytes) else {
            continue;
        };
        let (state, tone) = if active.current.as_ref() == Some(&manifest.version) {
            ("当前运行".to_owned(), "ok")
        } else if active.previous.as_ref() == Some(&manifest.version) {
            ("可自动恢复".to_owned(), "info")
        } else {
            ("已安装".to_owned(), "info")
        };
        rows.push(ReleaseRow {
            version: manifest.version.to_string(),
            source: update_source_label(&manifest.source).to_owned(),
            target: manifest.target,
            size: human_size(manifest.size),
            state,
            tone,
        });
    }
    rows.sort_by(|left, right| right.version.cmp(&left.version));
    Ok(rows)
}

fn post_notice(mut signal: Signal<Option<Notice>>, tone: NoticeTone, message: String) {
    let value = Notice { tone, message };
    signal.set(Some(value.clone()));
    spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        if signal() == Some(value) {
            signal.set(None);
        }
    });
}

fn nav_class(selected: bool) -> &'static str {
    if selected {
        "nav-item active"
    } else {
        "nav-item"
    }
}

fn readiness_tone(readiness: &RestartReadiness) -> &'static str {
    match readiness {
        RestartReadiness::Ready => "ok",
        RestartReadiness::Deferred { .. } => "warn",
        RestartReadiness::ConfirmationRequired { .. } | RestartReadiness::Blocked { .. } => "err",
    }
}

fn readiness_explanation(readiness: &RestartReadiness) -> String {
    let blocker_text = |items: &[cocos_build_lan_core::RestartBlocker]| {
        items
            .iter()
            .map(|item| item.summary.as_str())
            .collect::<Vec<_>>()
            .join("；")
    };
    match readiness {
        RestartReadiness::Ready => "服务允许立即安全重启。".to_owned(),
        RestartReadiness::Deferred {
            reasons,
            retry_after_secs,
        } => match retry_after_secs {
            Some(seconds) => format!("{}；约 {seconds} 秒后自动重试。", blocker_text(reasons)),
            None => blocker_text(reasons),
        },
        RestartReadiness::ConfirmationRequired { reasons } => {
            format!("需要人工确认：{}", blocker_text(reasons))
        }
        RestartReadiness::Blocked { reasons } => format!("重启已阻止：{}", blocker_text(reasons)),
    }
}

fn update_source_label(source: &UpdateSource) -> &'static str {
    match source {
        UpdateSource::Release => "Release",
        UpdateSource::LanDev => "LAN Dev",
    }
}

fn pending_matches(
    pending: &Option<cocos_build_lan_core::PendingUpdate>,
    manifest: &UpdateManifest,
) -> bool {
    pending
        .as_ref()
        .is_some_and(|update| update.manifest.version == manifest.version)
}

fn source_tone(source: &str) -> &'static str {
    if source == "LAN Dev" { "dev" } else { "info" }
}

fn short_hash(hash: &str) -> String {
    if hash.len() > 12 {
        format!("{}…{}", &hash[..6], &hash[hash.len() - 4..])
    } else {
        hash.to_owned()
    }
}

fn human_size(size: u64) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", size as f64 / 1024.0)
    }
}

fn handoff_update(
    supervisor: &ToolSupervisor,
    pending: &cocos_build_lan_core::PendingUpdate,
) -> Result<(), String> {
    let launcher = &supervisor.spec().launcher_path;
    if !launcher.is_file() {
        return Err(format!(
            "当前为开发启动，未找到稳定启动器 {}；请构建并通过 *-launcher 启动后再更新控制端。",
            launcher.display()
        ));
    }
    Command::new(launcher)
        .current_dir(&supervisor.spec().project_dir)
        .args([
            "--apply-staged",
            &pending.staging_path().display().to_string(),
            "--wait-pid",
            &std::process::id().to_string(),
        ])
        .spawn()
        .map_err(|error| format!("启动更新交接器失败：{error}"))?;
    Ok(())
}

fn filter_logs(contents: &str, level: &str, filter: &str) -> String {
    let keyword = filter.trim().to_lowercase();
    let result = contents
        .lines()
        .filter(|line| {
            (level == "全部级别" || line.contains(level))
                && (keyword.is_empty() || line.to_lowercase().contains(&keyword))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if result.is_empty() && !contents.is_empty() {
        "没有符合当前筛选条件的日志。".to_owned()
    } else {
        result
    }
}

const fn page_title(page: Page) -> &'static str {
    match page {
        Page::Overview => "运行状态",
        Page::Updates => "更新",
        Page::Rollback => "回滚",
        Page::Settings => "配置",
        Page::Logs => "日志摘要",
    }
}

const fn page_subtitle(page: Page) -> &'static str {
    match page {
        Page::Overview => "工具服务进程与本机控制面概览",
        Page::Updates => "清单、下载、校验与切换事务",
        Page::Rollback => "active.json 记录当前与上一个版本；切换失败自动恢复",
        Page::Settings => "仅编辑工具声明的配置字段，更新不会覆盖",
        Page::Logs => "服务端日志保留在本机数据目录，可按级别和关键字筛选",
    }
}

fn notice_tone(tone: NoticeTone) -> &'static str {
    match tone {
        NoticeTone::Success => "success",
        NoticeTone::Warning => "warning",
        NoticeTone::Error => "error",
    }
}

const CONTROL_CSS: &str = r#"
:root{font-family:-apple-system,BlinkMacSystemFont,"Inter","PingFang SC","Segoe UI",system-ui,sans-serif;color-scheme:dark}*{box-sizing:border-box}html,body{width:100%;margin:0}body{font:14px/1.55 var(--font-body);-webkit-font-smoothing:antialiased}.control{width:100%;height:100vh;min-height:0;display:flex;--font-body:-apple-system,BlinkMacSystemFont,"Inter","PingFang SC","Segoe UI",system-ui,sans-serif;--font-mono:"JetBrains Mono","IBM Plex Mono",ui-monospace,Menlo,monospace;--bg:oklch(17.5% .01 250);--surface:oklch(21.5% .012 250);--fg:oklch(91% .008 250);--muted:oklch(67% .02 250);--border:oklch(30% .012 250);--hover:oklch(25% .014 250);--accent:oklch(70% .13 145);--accent-on:oklch(18% .03 145);--accent-hover:oklch(76% .13 145);--nav-active-bg:color-mix(in oklch,var(--accent) 16%,transparent);--nav-active-fg:oklch(84% .12 150);--ok-bg:color-mix(in oklch,oklch(72% .14 145) 15%,transparent);--ok-fg:oklch(82% .13 150);--ok-bd:color-mix(in oklch,oklch(72% .14 145) 32%,transparent);--warn-bg:color-mix(in oklch,oklch(80% .13 85) 14%,transparent);--warn-fg:oklch(86% .12 85);--warn-bd:color-mix(in oklch,oklch(80% .13 85) 30%,transparent);--err-bg:color-mix(in oklch,oklch(72% .17 25) 13%,transparent);--err-fg:oklch(78% .15 25);--err-bd:color-mix(in oklch,oklch(72% .17 25) 30%,transparent);--info-bg:color-mix(in oklch,oklch(76% .11 240) 15%,transparent);--info-fg:oklch(80% .1 240);--info-bd:color-mix(in oklch,oklch(76% .11 240) 30%,transparent);--dev-bg:color-mix(in oklch,oklch(75% .13 300) 15%,transparent);--dev-fg:oklch(81% .12 300);--dev-bd:color-mix(in oklch,oklch(75% .13 300) 30%,transparent);--banner-bg:color-mix(in oklch,oklch(80% .13 85) 11%,transparent);--banner-bd:color-mix(in oklch,oklch(80% .13 85) 26%,transparent);--input-bg:oklch(18% .012 250);background:var(--bg);color:var(--fg)}.control.light{color-scheme:light;--bg:oklch(98% .005 250);--surface:oklch(100% 0 0);--fg:oklch(22% .02 240);--muted:oklch(50% .018 240);--border:oklch(90% .008 240);--hover:oklch(95% .008 250);--accent:oklch(58% .16 145);--accent-on:white;--accent-hover:oklch(53% .16 145);--nav-active-bg:oklch(93% .03 145);--nav-active-fg:oklch(38% .1 145);--ok-bg:oklch(94% .05 145);--ok-fg:oklch(40% .11 145);--ok-bd:oklch(88% .06 145);--warn-bg:oklch(95% .06 80);--warn-fg:oklch(45% .12 60);--warn-bd:oklch(89% .07 80);--err-bg:oklch(95% .04 25);--err-fg:oklch(45% .15 25);--err-bd:oklch(89% .05 25);--info-bg:oklch(94% .035 245);--info-fg:oklch(42% .11 245);--info-bd:oklch(87% .05 245);--dev-bg:oklch(94% .045 300);--dev-fg:oklch(43% .11 300);--dev-bd:oklch(87% .06 300);--banner-bg:oklch(96% .06 80);--banner-bd:oklch(88% .08 80);--input-bg:oklch(100% 0 0)}.sidebar{width:216px;flex:none;min-height:0;border-right:1px solid var(--border);background:var(--surface);padding:14px 10px;display:flex;flex-direction:column;gap:2px}.sb-tool{padding:8px 10px 12px;border-bottom:1px solid var(--border);margin-bottom:10px}.sb-tool .name{font-size:14px;font-weight:600;letter-spacing:-.01em}.tid,.sb-footer code,.mono,.kv .v{font-family:var(--font-mono);font-variant-numeric:tabular-nums}.tid{font-size:10.5px;color:var(--muted);margin-top:3px;overflow-wrap:anywhere}.nav-list{display:grid;gap:2px}.nav-item{display:flex;align-items:center;gap:9px;width:100%;padding:7px 10px;border:0;border-radius:6px;background:transparent;color:var(--fg);font:550 13px/1.4 var(--font-body);letter-spacing:.01em;cursor:pointer;text-align:left}.nav-item:hover{background:var(--hover)}.nav-item.active{background:var(--nav-active-bg);color:var(--nav-active-fg)}.icon{width:15px;height:15px;flex:none;fill:none;stroke:currentColor;stroke-width:1.7;stroke-linecap:round;stroke-linejoin:round}.badge{margin-left:auto;font:600 10.5px/1 var(--font-mono);padding:3px 6px;border-radius:999px;background:var(--warn-bg);color:var(--warn-fg);max-width:78px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.sb-footer{margin-top:auto;padding:10px;border-top:1px solid var(--border);font-size:11.5px;color:var(--muted)}.row{display:flex;justify-content:space-between;gap:8px;padding:2px 0}.row code{font-size:10.5px}.path-row{display:grid;gap:2px;padding-top:4px}.path-row code{font-size:10px;overflow-wrap:anywhere}.content{flex:1;min-width:0;min-height:0;overflow-y:auto;padding:22px 26px 40px}.content-toolbar{display:flex;align-items:start;justify-content:space-between;gap:16px;margin-bottom:18px}.page-head h1{font-size:20px;font-weight:600;letter-spacing:-.015em;margin:0}.page-head p{color:var(--muted);font-size:13px;margin:3px 0 0}.toolbar-actions{display:flex;gap:8px}.icon-button{width:28px;height:28px;display:grid;place-items:center;border:1px solid var(--border);border-radius:6px;background:transparent;color:var(--muted);cursor:pointer}.icon-button:hover{background:var(--hover);color:var(--fg)}.icon-button .icon{width:14px;height:14px}.card{background:var(--surface);border:1px solid var(--border);border-radius:8px;margin-bottom:16px;overflow:hidden}.card-head{display:flex;align-items:center;gap:10px;padding:12px 16px;border-bottom:1px solid var(--border)}.card-head h2{font-size:13.5px;font-weight:600;letter-spacing:.01em;margin:0}.spacer{flex:1}.card-body{padding:14px 16px}.grid-2{display:grid;grid-template-columns:1fr 1fr;gap:16px}.pill{display:inline-flex;align-items:center;gap:6px;font:600 11px/1 var(--font-body);letter-spacing:.03em;padding:4px 9px;border:1px solid transparent;border-radius:999px;white-space:nowrap}.pill::before{content:"";width:6px;height:6px;border-radius:50%;background:currentColor}.pill.ok{background:var(--ok-bg);color:var(--ok-fg);border-color:var(--ok-bd)}.pill.warn{background:var(--warn-bg);color:var(--warn-fg);border-color:var(--warn-bd)}.pill.err{background:var(--err-bg);color:var(--err-fg);border-color:var(--err-bd)}.pill.info{background:var(--info-bg);color:var(--info-fg);border-color:var(--info-bd)}.pill.dev{background:var(--dev-bg);color:var(--dev-fg);border-color:var(--dev-bd)}.kv{display:grid;grid-template-columns:150px minmax(0,1fr);row-gap:0}.kv>div{padding:8px 0;border-bottom:1px solid var(--border);font-size:13px}.kv>div:nth-last-child(-n+2){border-bottom:0}.kv .k{color:var(--muted)}.kv .v{font-size:12.5px;overflow-wrap:anywhere}.kv .link{color:var(--info-fg)}.btn{display:inline-flex;align-items:center;justify-content:center;gap:7px;padding:6px 13px;border:1px solid var(--border);border-radius:6px;background:var(--surface);color:var(--fg);font:550 12.5px/1.5 var(--font-body);letter-spacing:.02em;cursor:pointer;box-shadow:0 1px 2px rgb(20 24 40/.08)}.btn:hover{background:var(--hover)}.btn.primary{background:var(--accent);border-color:var(--accent);color:var(--accent-on)}.btn.primary:hover{background:var(--accent-hover)}.btn.danger{color:var(--err-fg)}.btn.danger:hover{background:var(--err-bg)}.btn:disabled{opacity:.45;cursor:not-allowed}.banner{display:flex;gap:12px;align-items:flex-start;padding:13px 16px;border:1px solid var(--banner-bd);border-radius:8px;background:var(--banner-bg);margin-bottom:16px}.banner>.icon{width:17px;height:17px;color:var(--warn-fg);margin-top:1px}.b-title{font-size:13px;font-weight:600}.b-text{font-size:12.5px;color:var(--muted);margin-top:2px}.steps{display:flex;flex-direction:column}.step{display:flex;gap:12px;padding:10px 0;border-bottom:1px solid var(--border)}.step:last-child{border-bottom:0}.step-icon{width:22px;height:22px;display:grid;place-items:center;flex:none;border-radius:50%;font:600 11px/1 var(--font-mono)}.step.done .step-icon{background:var(--ok-bg);color:var(--ok-fg)}.step.wait .step-icon{background:var(--warn-bg);color:var(--warn-fg)}.step.idle .step-icon{background:var(--hover);color:var(--muted)}.step-title{font-size:13px;font-weight:550}.step-detail{font-size:12px;color:var(--muted);margin-top:1px}.empty{padding:22px 16px;color:var(--muted);font-size:13px}.table-wrap{overflow-x:auto}.dense{width:100%;border-collapse:collapse;font-size:12.5px}.dense th{text-align:left;padding:8px 12px;border-bottom:1px solid var(--border);background:var(--hover);color:var(--muted);font:600 11px/1.4 var(--font-body);letter-spacing:.06em;text-transform:uppercase}.dense td{padding:9px 12px;border-bottom:1px solid var(--border);vertical-align:middle}.dense tr:last-child td{border-bottom:0}.form-intro{font-size:12.5px;color:var(--muted);margin-bottom:12px}.config-editor{display:block;width:100%;min-height:270px;resize:vertical;border:1px solid var(--border);border-radius:6px;padding:12px;background:var(--input-bg);color:var(--fg);font:12.5px/1.55 var(--font-mono)}.config-editor:focus,.log-toolbar input:focus,.log-toolbar select:focus{outline:2px solid color-mix(in oklch,var(--accent) 45%,transparent);border-color:var(--accent)}.form-actions{display:flex;gap:10px;align-items:center;padding-top:14px}.log-toolbar{display:flex;gap:8px;align-items:center;margin-bottom:12px}.log-toolbar select,.log-toolbar input{padding:5px 9px;border:1px solid var(--border);border-radius:6px;background:var(--input-bg);color:var(--fg);font:12.5px/1.5 var(--font-body)}.log-toolbar input{flex:1;min-width:120px;font-family:var(--font-mono);font-size:12px}.log-view{max-height:420px;min-height:170px;margin:0;overflow:auto;border-radius:8px;padding:14px 16px;background:oklch(20% .015 250);color:oklch(88% .01 250);font:12px/1.75 var(--font-mono);font-variant-numeric:tabular-nums;white-space:pre-wrap}.toast{position:fixed;right:20px;bottom:20px;z-index:10;padding:9px 16px;border-radius:8px;box-shadow:0 8px 24px rgb(0 0 0/.25);font-size:12.5px;font-weight:550}.toast.info{background:var(--info-bg);border:1px solid var(--info-bd);color:var(--info-fg)}.toast.success{background:var(--ok-bg);border:1px solid var(--ok-bd);color:var(--ok-fg)}.toast.warning{background:var(--warn-bg);border:1px solid var(--warn-bd);color:var(--warn-fg)}.toast.error{background:var(--err-bg);border:1px solid var(--err-bd);color:var(--err-fg)}@media(max-width:780px){.sidebar{width:64px;padding:12px 8px}.sb-tool,.sb-footer,.nav-item:not(.active)::after,.nav-item{font-size:0}.nav-item{justify-content:center;padding:9px}.nav-item .icon{width:18px;height:18px}.badge{display:none}.content{padding:18px}.grid-2{grid-template-columns:1fr}.kv{grid-template-columns:112px minmax(0,1fr)}}
.settings-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:14px}.field{display:grid;gap:6px;font-size:12.5px;font-weight:550}.field.wide{grid-column:1/-1}.field input{width:100%;padding:8px 10px;border:1px solid var(--border);border-radius:6px;background:var(--input-bg);color:var(--fg);font:13px/1.4 var(--font-body)}.field input:focus{outline:2px solid color-mix(in oklch,var(--accent) 45%,transparent);border-color:var(--accent)}.field small{font-size:11.5px;font-weight:400;color:var(--muted)}.switch-field{display:flex;align-items:center;gap:9px;padding:10px 11px;border:1px solid var(--border);border-radius:6px;background:var(--input-bg);font-size:12.5px;cursor:pointer}.switch-field input{inline-size:15px;block-size:15px;accent-color:var(--accent)}@media(max-width:780px){.settings-grid{grid-template-columns:1fr}.field.wide{grid-column:auto}}
"#;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use semver::Version;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn duplicate_lan_manifest_is_not_staged_twice() {
        let manifest = UpdateManifest {
            tool_id: Uuid::new_v4(),
            version: Version::new(1, 0, 1),
            source: UpdateSource::LanDev,
            format: cocos_build_lan_core::UpdatePayloadFormat::ToolBundleV1,
            target: "test-target".to_owned(),
            url: "http://localhost/payload".to_owned(),
            size: 1,
            sha256: "00".repeat(32),
            notes: String::new(),
        };
        let pending = Some(cocos_build_lan_core::PendingUpdate::from_staging(
            manifest.clone(),
            PathBuf::from("staging"),
        ));
        assert!(pending_matches(&pending, &manifest));
    }
}
