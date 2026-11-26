use crate::{AppContext, ConfirmDialog, api};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

pub const LOCAL_ONLY: &str = "请在服务启动的机器上访问设置。";

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessContext {
    pub can_manage_machine: bool,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MachineSettings {
    lan_port: u16,
    active_port: u16,
    git_username: String,
    git_credentials_configured: bool,
    workspace_root: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MachineUpdate {
    lan_port: u16,
    git_username: String,
    git_password: Option<String>,
    clear_git_password: bool,
    workspace_root: String,
}
#[derive(Deserialize)]
struct PickResult {
    path: Option<String>,
}

pub async fn pick_path(kind: &str) -> Result<Option<String>, String> {
    api::post::<PickResult, _>("/api/local/pick-path", &serde_json::json!({"kind": kind}))
        .await
        .map(|result| result.path)
}

#[component]
pub fn ServiceSettings(can_manage: bool, on_saved: EventHandler<()>) -> Element {
    let context = use_context::<AppContext>();
    let mut current = use_signal(MachineSettings::default);
    let mut port = use_signal(String::new);
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut clear_password = use_signal(|| false);
    let mut workspace = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut picking = use_signal(|| false);
    let mut loaded = use_signal(|| false);
    let mut pending = use_signal(|| None::<MachineUpdate>);
    use_effect(use_reactive!(|(can_manage,)| {
        if can_manage {
            spawn(async move {
                match api::get::<MachineSettings>("/api/local/settings").await {
                    Ok(value) => {
                        port.set(value.lan_port.to_string());
                        username.set(value.git_username.clone());
                        workspace.set(value.workspace_root.clone());
                        current.set(value);
                        loaded.set(true);
                    }
                    Err(error) => context.error(error),
                }
            });
        }
    }));
    let save = move |update: MachineUpdate| {
        spawn(async move {
            busy.set(true);
            match api::put::<MachineSettings, _>("/api/local/settings", &update).await {
                Ok(value) => {
                    current.set(value);
                    password.set(String::new());
                    clear_password.set(false);
                    context.success("设置已保存；端口变更将在下次启动生效");
                    on_saved.call(());
                }
                Err(error) => context.error(error),
            }
            busy.set(false);
        });
    };
    rsx! {
        section { class: "settings-section", id: "sec-service",
            div { class: "section-heading", strong { "服务设置" } }
            if !can_manage {
                p { class: "section-copy", "{LOCAL_ONLY}" }
            } else if !loaded() {
                p { class: "section-copy", "正在读取本机设置…" }
            } else {
                div { class: "field", label { class: "field__label", "局域网端口" }
                    input { class: "input input--mono", r#type: "number", min: "1", max: "65535", value: "{port}", oninput: move |event| port.set(event.value()) }
                    p { class: "hint", "当前端口：{current().active_port}。保存后的端口下次启动生效。" }
                    if current().lan_port != current().active_port { p { class: "hint", "待生效端口：{current().lan_port}；下次访问 http://127.0.0.1:{current().lan_port}" } }
                }
                div { class: "field", label { class: "field__label", "构建工作目录" }
                    div { class: "toolbar",
                        input { class: "input input--mono", placeholder: "留空使用默认目录；建议使用短路径", value: "{workspace}", oninput: move |event| workspace.set(event.value()) }
                        button { class: "btn", disabled: picking(), onclick: move |_| { picking.set(true); spawn(async move { match pick_path("workspace").await { Ok(Some(path)) => workspace.set(path), Ok(None) => {}, Err(error) => context.error(error) } picking.set(false); }); }, if picking() { "选择中…" } else { "选择目录…" } }
                    }
                    p { class: "hint", "切换目录不会移动已有文件；已有项目可能需要重新初始化。" }
                }
                div { class: "section-heading section-heading--sub", id: "sec-git", strong { "Git 凭据" }
                    span { class: if current().git_credentials_configured { "tag tag--ok" } else { "tag tag--warn" }, if current().git_credentials_configured { "已配置" } else { "未配置" } }
                }
                div { class: "field", label { class: "field__label", "Git 用户名" } input { class: "input", autocomplete: "username", value: "{username}", oninput: move |event| username.set(event.value()) } }
                div { class: "field", label { class: "field__label", "Git Token / 密码" } input { class: "input", r#type: "password", autocomplete: "new-password", placeholder: "留空保留已有凭据", value: "{password}", oninput: move |event| password.set(event.value()) } }
                label { class: "hint", input { r#type: "checkbox", checked: clear_password(), onchange: move |event| clear_password.set(event.checked()) } "清除已保存的 Git Token / 密码" }
                div { class: "toolbar",
                    button { class: "btn btn--primary", disabled: busy() || picking(), onclick: move |_| {
                        let Ok(lan_port) = port().parse::<u16>() else { context.error("端口必须在 1 到 65535 之间"); return; };
                        if lan_port == 0 { context.error("端口必须在 1 到 65535 之间"); return; }
                        let update = MachineUpdate { lan_port, git_username: username(), git_password: (!password().is_empty()).then(&*password), clear_git_password: clear_password(), workspace_root: workspace().trim().to_string() };
                        if update.workspace_root != current().workspace_root { pending.set(Some(update)); } else { save(update); }
                    }, if busy() { "保存中…" } else { "保存服务设置" } }
                }
            }
        }
        if let Some(update) = pending() {
            ConfirmDialog { title: "切换构建工作目录".to_string(), message: "现有文件不会搬移。切换后请检查项目工作区状态，必要时重新初始化。".to_string(), confirm_label: "确认切换".to_string(), danger: false,
                on_cancel: move |_| pending.set(None), on_confirm: move |_| { pending.set(None); save(update.clone()); } }
        }
    }
}
