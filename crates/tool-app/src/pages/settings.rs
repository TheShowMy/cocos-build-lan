use dioxus::prelude::*;
use dioxus_free_icons::{
    Icon,
    icons::ld_icons::{
        LdCheck, LdChevronDown, LdChevronUp, LdCircleAlert, LdFolderCog, LdKeyRound, LdPencil,
        LdPlus, LdRefreshCw, LdSettings2, LdTrash2, LdX,
    },
};
use serde_json::{Value, json};

use crate::{
    AppContext, ConfirmDialog, api,
    models::{
        Engine, FeishuBot, ParamDefinition, ParamKind, Project, ProjectWorkspaceStatus,
        PublicSettings, PublicSettingsUpdate,
    },
};

#[component]
pub fn Settings() -> Element {
    let context = use_context::<AppContext>();
    let mut settings = use_signal(PublicSettings::default);
    let mut statuses = use_signal(Vec::<ProjectWorkspaceStatus>::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(String::new);
    let mut refresh = use_signal(|| 0u64);
    let mut engine_editor = use_signal(|| None::<(Option<String>, Engine)>);
    let mut project_editor = use_signal(|| None::<Project>);
    let mut bot_editor = use_signal(|| None::<FeishuBot>);
    let mut param_editor = use_signal(|| None::<(Option<String>, ParamDefinition)>);
    let mut delete_engine = use_signal(|| None::<String>);
    let mut delete_project = use_signal(|| None::<Project>);
    let mut delete_bot = use_signal(|| None::<FeishuBot>);
    let mut delete_param = use_signal(|| None::<ParamDefinition>);
    let mut initialize_project = use_signal(|| None::<Project>);
    let mut cleanup_project = use_signal(|| None::<Project>);
    let mut action_busy = use_signal(String::new);

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            match api::get::<PublicSettings>("/api/settings").await {
                Ok(value) => {
                    settings.set(value);
                    error.set(String::new());
                }
                Err(load_error) => error.set(load_error),
            }
            if let Ok(value) =
                api::get::<Vec<ProjectWorkspaceStatus>>("/api/projects/init-statuses").await
            {
                statuses.set(value);
            }
            loading.set(false);
        });
    });

    if loading() && settings().engines.is_empty() && settings().projects.is_empty() {
        return rsx! { main { class: "page__main", div { class: "empty-state", "正在加载设置…" } } };
    }
    if !error().is_empty() {
        return rsx! { main { class: "page__main", div { class: "error-state", p { "{error}" } button { class: "btn", onclick: move |_| refresh += 1, "重试" } } } };
    }

    rsx! {
        div { class: "page settings-page",
            aside { class: "page__side settings-nav",
                nav { class: "anchor-nav", "aria-label": "设置分区",
                    a { href: "#sec-engine", class: "is-active", "引擎与项目" }
                    a { href: "#sec-git", "Git 凭据" }
                    a { href: "#sec-feishu", "飞书机器人" }
                    a { href: "#sec-params", "任务参数" }
                }
            }
            main { class: "page__main settings-col",
                div { class: "toolbar settings-toolbar", div { h1 { "设置" } p { class: "toolbar__subtitle", "部署机路径、项目仓库和构建参数" } } span { class: "spacer" } button { class: "btn btn--icon", title: "刷新设置", onclick: move |_| refresh += 1, Icon { width: 16, height: 16, icon: LdRefreshCw } } }
                section { class: "settings-section", id: "sec-engine",
                    div { class: "section-heading",
                        div { Icon { width: 17, height: 17, icon: LdFolderCog } strong { "Cocos 引擎" } }
                        span { class: "spacer" }
                        button { class: "btn btn--sm", onclick: move |_| engine_editor.set(Some((None, Engine::default()))), Icon { width: 14, height: 14, icon: LdPlus } "添加引擎" }
                    }
                    if settings().engines.is_empty() {
                        div { class: "empty-inline", "还没有配置引擎路径" }
                    } else {
                        div { class: "table-panel table-panel--flush",
                            table { class: "table",
                                thead { tr { th { "名称" } th { "部署机路径" } th { class: "table__actions-head", "操作" } } }
                                tbody { for engine in settings().engines {
                                    tr { key: "{engine.name}",
                                        td { strong { "{engine.name}" } }
                                        td { class: "mono path-cell", "{engine.path}" }
                                        td { div { class: "table__actions",
                                            button { class: "btn btn--sm", onclick: { let engine = engine.clone(); move |_| engine_editor.set(Some((Some(engine.name.clone()), engine.clone()))) }, Icon { width: 14, height: 14, icon: LdPencil } "编辑" }
                                            button { class: "btn btn--sm btn--danger btn--icon", title: "删除引擎", onclick: move |_| delete_engine.set(Some(engine.name.clone())), Icon { width: 14, height: 14, icon: LdTrash2 } }
                                        } }
                                    }
                                } }
                            }
                        }
                    }
                    div { class: "section-heading section-heading--sub",
                        div { Icon { width: 17, height: 17, icon: LdSettings2 } strong { "项目仓库" } }
                        span { class: "spacer" }
                        button { class: "btn btn--sm", disabled: settings().engines.is_empty(), onclick: move |_| project_editor.set(Some(Project { engine_name: settings().engines.first().map(|engine| engine.name.clone()).unwrap_or_default(), ..Project::default() })), Icon { width: 14, height: 14, icon: LdPlus } "添加项目" }
                    }
                    if settings().projects.is_empty() {
                        div { class: "empty-inline", "还没有配置项目" }
                    } else {
                        div { class: "table-panel table-panel--flush",
                            table { class: "table",
                                thead { tr { th { "项目" } th { "引擎 / 仓库" } th { "工作区" } th { class: "table__actions-head", "操作" } } }
                                tbody { for project in settings().projects {
                                    ProjectRow { project, statuses: statuses(), busy: action_busy(), on_initialize: move |project| initialize_project.set(Some(project)), on_cleanup: move |project| cleanup_project.set(Some(project)), on_edit: move |project| project_editor.set(Some(project)), on_delete: move |project| delete_project.set(Some(project)) }
                                } }
                            }
                        }
                    }
                }

                section { class: "settings-section", id: "sec-git",
                    div { class: "section-heading",
                        div { Icon { width: 17, height: 17, icon: LdKeyRound } strong { "Git 凭据" } }
                        span { class: "spacer" }
                        span { class: if settings().git_credentials_configured { "tag tag--ok" } else { "tag tag--warn" },
                            if settings().git_credentials_configured { Icon { width: 13, height: 13, icon: LdCheck } "已配置" }
                            else { Icon { width: 13, height: 13, icon: LdCircleAlert } "未配置" }
                        }
                    }
                    p { class: "section-copy", "Git 用户名和 Token 只在部署机器的控制 App 中管理，局域网页面无法读取或修改其内容。" }
                }

                section { class: "settings-section", id: "sec-feishu",
                    div { class: "section-heading", strong { "飞书机器人" } span { class: "spacer" } button { class: "btn btn--sm", onclick: move |_| bot_editor.set(Some(FeishuBot::default())), Icon { width: 14, height: 14, icon: LdPlus } "添加机器人" } }
                    p { class: "section-copy", "所有机器人接收固定的构建开始、失败和完成通知。" }
                    if settings().feishu_bots.is_empty() {
                        div { class: "empty-inline", "未配置飞书机器人" }
                    } else {
                        div { class: "table-panel table-panel--flush",
                            table { class: "table",
                                thead { tr { th { "名称" } th { "Webhook" } th { class: "table__actions-head", "操作" } } }
                                tbody { for bot in settings().feishu_bots {
                                    tr { key: "{bot.id}",
                                        td { strong { "{bot.name}" } }
                                        td { class: "mono", {redact_webhook(&bot.api_key)} }
                                        td { div { class: "table__actions",
                                            button { class: "btn btn--sm btn--icon", title: "编辑机器人", onclick: { let bot = bot.clone(); move |_| bot_editor.set(Some(bot.clone())) }, Icon { width: 14, height: 14, icon: LdPencil } }
                                            button { class: "btn btn--sm btn--danger btn--icon", title: "删除机器人", onclick: move |_| delete_bot.set(Some(bot.clone())), Icon { width: 14, height: 14, icon: LdTrash2 } }
                                        } }
                                    }
                                } }
                            }
                        }
                    }
                }

                section { class: "settings-section", id: "sec-params",
                    div { class: "section-heading", strong { "任务参数配置" } span { class: "spacer" } button { class: "btn btn--sm", onclick: move |_| param_editor.set(Some((None, ParamDefinition::default()))), Icon { width: 14, height: 14, icon: LdPlus } "添加参数" } }
                    p { class: "section-copy", "参数按顺序显示在任务组详情中；修改类型时，不兼容的旧值会恢复为新默认值。" }
                    if settings().param_definitions.is_empty() {
                        div { class: "empty-inline", "没有任务参数定义" }
                    } else {
                        div { class: "table-panel table-panel--flush",
                            table { class: "table",
                                thead { tr { th { "显示名 / key" } th { "类型" } th { "默认值" } th { "使用" } th { class: "table__actions-head", "操作" } } }
                                tbody { for (index, definition) in settings().param_definitions.iter().cloned().enumerate() {
                                    ParameterRow { index, definition, settings, on_edit: move |value: ParamDefinition| param_editor.set(Some((Some(value.key.clone()), value))), on_delete: move |value: ParamDefinition| delete_param.set(Some(value)) }
                                } }
                            }
                        }
                    }
                }
            }
        }

        if let Some((original, value)) = engine_editor() { EngineEditor { value, on_cancel: move |_| engine_editor.set(None), on_save: move |engine: Engine| { let mut next = settings(); if let Some(original) = &original { if let Some(slot) = next.engines.iter_mut().find(|item| item.name == *original) { *slot = engine; } } else { next.engines.push(engine); } engine_editor.set(None); save_settings(next, settings, context); } } }
        if let Some(value) = project_editor() { ProjectEditor { value, engines: settings().engines, on_cancel: move |_| project_editor.set(None), on_save: move |project: Project| { let mut next = settings(); if project.id.is_empty() { next.projects.push(project); } else if let Some(slot) = next.projects.iter_mut().find(|item| item.id == project.id) { *slot = project; } project_editor.set(None); save_settings(next, settings, context); } } }
        if let Some(value) = bot_editor() { BotEditor { value, on_cancel: move |_| bot_editor.set(None), on_save: move |bot: FeishuBot| { let mut next = settings(); if bot.id.is_empty() { next.feishu_bots.push(bot); } else if let Some(slot) = next.feishu_bots.iter_mut().find(|item| item.id == bot.id) { *slot = bot; } bot_editor.set(None); save_settings(next, settings, context); } } }
        if let Some((original, value)) = param_editor() { ParamEditor { value, on_cancel: move |_| param_editor.set(None), on_save: move |definition: ParamDefinition| { let mut next = settings(); if let Some(original) = &original { if let Some(slot) = next.param_definitions.iter_mut().find(|item| item.key == *original) { *slot = definition; } } else { next.param_definitions.push(definition); } param_editor.set(None); save_settings(next, settings, context); } } }

        if let Some(name) = delete_engine() { ConfirmDialog { title: "删除引擎".to_owned(), message: format!("确认删除引擎「{name}」？仍被项目使用时服务端会拒绝。"), confirm_label: "删除".to_owned(), danger: true, on_cancel: move |_| delete_engine.set(None), on_confirm: move |_| { let mut next = settings(); next.engines.retain(|engine| engine.name != name); delete_engine.set(None); save_settings(next, settings, context); } } }
        if let Some(project) = delete_project() { ConfirmDialog { title: "删除项目".to_owned(), message: format!("确认删除项目「{}」？仍有任务组时服务端会拒绝，工作区文件不会随配置删除。", project.name), confirm_label: "删除".to_owned(), danger: true, on_cancel: move |_| delete_project.set(None), on_confirm: move |_| { let mut next = settings(); next.projects.retain(|item| item.id != project.id); delete_project.set(None); save_settings(next, settings, context); } } }
        if let Some(bot) = delete_bot() { ConfirmDialog { title: "删除飞书机器人".to_owned(), message: format!("确认删除「{}」？后续构建将不再向它发送通知。", bot.name), confirm_label: "删除".to_owned(), danger: true, on_cancel: move |_| delete_bot.set(None), on_confirm: move |_| { let mut next = settings(); next.feishu_bots.retain(|item| item.id != bot.id); delete_bot.set(None); save_settings(next, settings, context); } } }
        if let Some(definition) = delete_param() { ConfirmDialog { title: "删除任务参数".to_owned(), message: format!("参数「{}」当前被 {} 个任务组使用。确认删除并清理这些已保存值？", definition.label, parameter_usage(&settings(), &definition.key)), confirm_label: "删除参数".to_owned(), danger: true, on_cancel: move |_| delete_param.set(None), on_confirm: move |_| { let mut next = settings(); next.param_definitions.retain(|item| item.key != definition.key); delete_param.set(None); save_settings(next, settings, context); } } }
        if let Some(project) = initialize_project() { ConfirmDialog { title: "初始化项目工作区".to_owned(), message: format!("确认在部署机初始化「{}」？已有工作区会重新同步并重置到远端状态。", project.name), confirm_label: "开始初始化".to_owned(), danger: statuses().iter().any(|status| status.project_id == project.id && status.initialized), on_cancel: move |_| initialize_project.set(None), on_confirm: move |_| { let id = project.id.clone(); initialize_project.set(None); action_busy.set(id.clone()); spawn(async move { match api::post::<ProjectWorkspaceStatus, _>("/api/projects/initialize", &json!({"projectId": id})).await { Ok(_) => { context.success("项目工作区初始化完成"); refresh += 1; }, Err(error) => context.error(error) } action_busy.set(String::new()); }); } } }
        if let Some(project) = cleanup_project() { ConfirmDialog { title: "清理项目工作区".to_owned(), message: format!("确认丢弃「{}」工作区内已暂存、未暂存和未跟踪的全部改动？", project.name), confirm_label: "清理全部改动".to_owned(), danger: true, on_cancel: move |_| cleanup_project.set(None), on_confirm: move |_| { let id = project.id.clone(); cleanup_project.set(None); action_busy.set(id.clone()); spawn(async move { match api::post::<Value, _>("/api/projects/cleanup-worktree", &json!({"projectId": id})).await { Ok(_) => context.success("项目工作区已清理"), Err(error) => context.error(error) } action_busy.set(String::new()); }); } } }
    }
}

#[component]
fn ProjectRow(
    project: Project,
    statuses: Vec<ProjectWorkspaceStatus>,
    busy: String,
    on_initialize: EventHandler<Project>,
    on_cleanup: EventHandler<Project>,
    on_edit: EventHandler<Project>,
    on_delete: EventHandler<Project>,
) -> Element {
    let status = statuses
        .into_iter()
        .find(|status| status.project_id == project.id);
    let initialized = status.as_ref().is_some_and(|status| status.initialized);
    rsx! {
        tr { key: "{project.id}",
            td { strong { "{project.name}" } br {} span { class: "hint mono", "{project.workspace_dir_key}" } }
            td { span { class: "tag", "{project.engine_name}" } br {} span { class: "hint mono", "{project.git_url}" } }
            td {
                span { class: if initialized { "tag tag--ok" } else { "tag tag--warn" }, if initialized { "已初始化" } else { "未初始化" } }
                if let Some(status) = status { p { class: "hint mono path-cell", title: "{status.project_path}", "{status.project_path}" } }
            }
            td { div { class: "table__actions",
                button { class: "btn btn--sm", disabled: busy == project.id, onclick: { let project = project.clone(); move |_| on_initialize.call(project.clone()) }, if initialized { "重新初始化" } else { "初始化" } }
                button { class: "btn btn--sm", disabled: !initialized || busy == project.id, onclick: { let project = project.clone(); move |_| on_cleanup.call(project.clone()) }, "清理改动" }
                button { class: "btn btn--sm btn--icon", title: "编辑项目", onclick: { let project = project.clone(); move |_| on_edit.call(project.clone()) }, Icon { width: 14, height: 14, icon: LdPencil } }
                button { class: "btn btn--sm btn--danger btn--icon", title: "删除项目", onclick: move |_| on_delete.call(project.clone()), Icon { width: 14, height: 14, icon: LdTrash2 } }
            } }
        }
    }
}

#[component]
fn ParameterRow(
    index: usize,
    definition: ParamDefinition,
    settings: Signal<PublicSettings>,
    on_edit: EventHandler<ParamDefinition>,
    on_delete: EventHandler<ParamDefinition>,
) -> Element {
    let context = use_context::<AppContext>();
    let count = parameter_usage(&settings(), &definition.key);
    let total = settings().param_definitions.len();
    rsx! {
        tr { key: "{definition.key}",
            td { strong { "{definition.label}" } br {} span { class: "hint mono", "{definition.key}" } }
            td { span { class: "tag", {definition.kind.label()} } if definition.required { span { class: "tag tag--warn", "必填" } } }
            td { class: "mono", {display_value(&definition.default_value)} }
            td { "{count} 个组" }
            td { div { class: "table__actions",
                button { class: "btn btn--sm btn--icon", title: "上移", disabled: index == 0, onclick: { let key = definition.key.clone(); move |_| move_parameter(settings, &key, -1, context) }, Icon { width: 14, height: 14, icon: LdChevronUp } }
                button { class: "btn btn--sm btn--icon", title: "下移", disabled: index + 1 == total, onclick: { let key = definition.key.clone(); move |_| move_parameter(settings, &key, 1, context) }, Icon { width: 14, height: 14, icon: LdChevronDown } }
                button { class: "btn btn--sm btn--icon", title: "编辑参数", onclick: { let definition = definition.clone(); move |_| on_edit.call(definition.clone()) }, Icon { width: 14, height: 14, icon: LdPencil } }
                button { class: "btn btn--sm btn--danger btn--icon", title: "删除参数", onclick: move |_| on_delete.call(definition.clone()), Icon { width: 14, height: 14, icon: LdTrash2 } }
            } }
        }
    }
}

#[component]
fn EngineEditor(
    value: Engine,
    on_cancel: EventHandler<MouseEvent>,
    on_save: EventHandler<Engine>,
) -> Element {
    let mut draft = use_signal(|| value);
    rsx! { ModalFrame { title: "引擎配置".to_owned(), on_cancel,
        div { class: "field", label { class: "field__label", "引擎名称" } input { class: "input", value: "{draft().name}", oninput: move |event| draft.write().name = event.value() } }
        div { class: "field", label { class: "field__label", "部署机路径" } input { class: "input input--mono", placeholder: "例如 C:\\ProgramData\\cocos\\Creator\\3.8.5", value: "{draft().path}", oninput: move |event| draft.write().path = event.value() } p { class: "hint", "填写部署机器上的实际 Cocos Creator 路径。" } }
        div { class: "drawer__foot", button { class: "btn", onclick: move |event| on_cancel.call(event), "取消" } button { class: "btn btn--primary", disabled: draft().name.trim().is_empty() || draft().path.trim().is_empty(), onclick: move |_| on_save.call(draft()), "保存" } }
    } }
}

#[component]
fn ProjectEditor(
    value: Project,
    engines: Vec<Engine>,
    on_cancel: EventHandler<MouseEvent>,
    on_save: EventHandler<Project>,
) -> Element {
    let mut draft = use_signal(|| value);
    rsx! { ModalFrame { title: if draft().id.is_empty() { "添加项目".to_owned() } else { "编辑项目".to_owned() }, on_cancel,
        div { class: "field", label { class: "field__label", "项目名称" } input { class: "input", value: "{draft().name}", oninput: move |event| draft.write().name = event.value() } }
        div { class: "field", label { class: "field__label", "关联引擎" } select { class: "select", value: "{draft().engine_name}", onchange: move |event| draft.write().engine_name = event.value(), for engine in engines { option { value: "{engine.name}", "{engine.name}" } } } }
        div { class: "field", label { class: "field__label", "Git 地址" } input { class: "input input--mono", value: "{draft().git_url}", oninput: move |event| draft.write().git_url = event.value() } }
        p { class: "hint", "修改 Git 地址后请重新初始化工作区；凭据由本机控制 App 提供。" }
        div { class: "drawer__foot", button { class: "btn", onclick: move |event| on_cancel.call(event), "取消" } button { class: "btn btn--primary", disabled: draft().name.trim().is_empty() || draft().engine_name.is_empty() || draft().git_url.trim().is_empty(), onclick: move |_| on_save.call(draft()), "保存" } }
    } }
}

#[component]
fn BotEditor(
    value: FeishuBot,
    on_cancel: EventHandler<MouseEvent>,
    on_save: EventHandler<FeishuBot>,
) -> Element {
    let mut draft = use_signal(|| value);
    rsx! { ModalFrame { title: "飞书机器人".to_owned(), on_cancel,
        div { class: "field", label { class: "field__label", "机器人名称" } input { class: "input", value: "{draft().name}", oninput: move |event| draft.write().name = event.value() } }
        div { class: "field", label { class: "field__label", "Webhook 地址" } input { class: "input input--mono", placeholder: "https://open.feishu.cn/open-apis/bot/v2/hook/...", value: "{draft().api_key}", oninput: move |event| draft.write().api_key = event.value() } }
        div { class: "drawer__foot", button { class: "btn", onclick: move |event| on_cancel.call(event), "取消" } button { class: "btn btn--primary", disabled: draft().name.trim().is_empty() || draft().api_key.trim().is_empty(), onclick: move |_| on_save.call(draft()), "保存" } }
    } }
}

#[component]
fn ParamEditor(
    value: ParamDefinition,
    on_cancel: EventHandler<MouseEvent>,
    on_save: EventHandler<ParamDefinition>,
) -> Element {
    let mut draft = use_signal(|| value);
    let options = draft().options.join("\n");
    rsx! { ModalFrame { title: "任务参数定义".to_owned(), on_cancel,
        div { class: "form-grid form-grid--two", div { class: "field", label { class: "field__label", "显示名" } input { class: "input", value: "{draft().label}", oninput: move |event| draft.write().label = event.value() } } div { class: "field", label { class: "field__label", "key" } input { class: "input input--mono", value: "{draft().key}", oninput: move |event| draft.write().key = event.value() } } }
        div { class: "form-grid form-grid--two", div { class: "field", label { class: "field__label", "类型" } select { class: "select", value: "{param_kind_value(&draft().kind)}", onchange: move |event| { let kind = parse_param_kind(&event.value()); draft.write().kind = kind.clone(); draft.write().default_value = default_for_kind(&kind); }, option { value: "text", "文本" } option { value: "number", "数字" } option { value: "switch", "开关" } option { value: "select", "选择" } } } label { class: "checkbox param-required", input { r#type: "checkbox", checked: draft().required, onchange: move |event| draft.write().required = event.checked() } "必填参数" } }
        if draft().kind == ParamKind::Select { div { class: "field", label { class: "field__label", "选项（每行一个）" } textarea { class: "textarea textarea--mono", value: "{options}", oninput: move |event| draft.write().options = event.value().lines().map(str::trim).filter(|value| !value.is_empty()).map(ToOwned::to_owned).collect() } } }
        div { class: "field", label { class: "field__label", "默认值" } if draft().kind == ParamKind::Switch { label { class: "switch", input { r#type: "checkbox", checked: draft().default_value.as_bool().unwrap_or(false), onchange: move |event| draft.write().default_value = Value::Bool(event.checked()) } i {} } } else if draft().kind == ParamKind::Select { select { class: "select", value: "{display_value(&draft().default_value)}", onchange: move |event| draft.write().default_value = Value::String(event.value()), option { value: "", "请选择" } for option in draft().options { option { value: "{option}", "{option}" } } } } else { input { class: "input input--mono", value: "{display_value(&draft().default_value)}", oninput: move |event| draft.write().default_value = if draft().kind == ParamKind::Number { event.value().parse::<f64>().map(Value::from).unwrap_or(Value::String(event.value())) } else { Value::String(event.value()) } } } }
        div { class: "field", label { class: "field__label", "描述" } input { class: "input", value: "{draft().description}", oninput: move |event| draft.write().description = event.value() } }
        div { class: "drawer__foot", button { class: "btn", onclick: move |event| on_cancel.call(event), "取消" } button { class: "btn btn--primary", disabled: draft().label.trim().is_empty() || draft().key.trim().is_empty(), onclick: move |_| on_save.call(draft()), "保存" } }
    } }
}

#[component]
fn ModalFrame(title: String, on_cancel: EventHandler<MouseEvent>, children: Element) -> Element {
    rsx! { div { class: "overlay is-open", onclick: move |event| on_cancel.call(event) } section { class: "modal modal--sm is-open", role: "dialog", "aria-modal": "true", "aria-label": title.clone(), div { class: "drawer__head", "{title}" span { class: "spacer" } button { class: "btn btn--ghost btn--icon", onclick: move |event| on_cancel.call(event), Icon { width: 17, height: 17, icon: LdX } } } div { class: "drawer__body", {children} } } }
}

fn save_settings(next: PublicSettings, mut signal: Signal<PublicSettings>, context: AppContext) {
    spawn(async move {
        match api::put::<PublicSettings, _>("/api/settings", &PublicSettingsUpdate::from(&next))
            .await
        {
            Ok(saved) => {
                signal.set(saved);
                context.success("设置已保存");
            }
            Err(error) => context.error(error),
        }
    });
}

fn move_parameter(settings: Signal<PublicSettings>, key: &str, offset: isize, context: AppContext) {
    let mut next = settings();
    let Some(index) = next
        .param_definitions
        .iter()
        .position(|item| item.key == key)
    else {
        return;
    };
    let target = index as isize + offset;
    if target < 0 || target >= next.param_definitions.len() as isize {
        return;
    }
    next.param_definitions.swap(index, target as usize);
    save_settings(next, settings, context);
}

fn parameter_usage(settings: &PublicSettings, key: &str) -> usize {
    settings
        .task_groups
        .iter()
        .filter(|group| group.params.contains_key(key))
        .count()
}

fn display_value(value: &Value) -> String {
    if value.is_null() {
        String::new()
    } else {
        value
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| value.to_string())
    }
}

fn redact_webhook(value: &str) -> String {
    value
        .rsplit('/')
        .next()
        .map(|token| {
            if token.len() > 10 {
                format!("…{}", &token[token.len() - 8..])
            } else {
                "已配置".to_owned()
            }
        })
        .unwrap_or_else(|| "已配置".to_owned())
}

fn parse_param_kind(value: &str) -> ParamKind {
    match value {
        "number" => ParamKind::Number,
        "switch" => ParamKind::Switch,
        "select" => ParamKind::Select,
        _ => ParamKind::Text,
    }
}

fn param_kind_value(value: &ParamKind) -> &'static str {
    match value {
        ParamKind::Text => "text",
        ParamKind::Number => "number",
        ParamKind::Switch => "switch",
        ParamKind::Select => "select",
    }
}

fn default_for_kind(value: &ParamKind) -> Value {
    match value {
        ParamKind::Text | ParamKind::Select => Value::String(String::new()),
        ParamKind::Number => json!(0),
        ParamKind::Switch => Value::Bool(false),
    }
}
