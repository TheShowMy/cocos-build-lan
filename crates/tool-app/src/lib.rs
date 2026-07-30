use dioxus::{document, prelude::*};
use serde_json::{Value, json};

const APP_CSS: &str = include_str!("../assets/app.css");

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Package,
    Prep,
    Logs,
    Settings,
}

impl Page {
    fn from_path(path: &str) -> Self {
        match path.trim_matches('/') {
            "prep" => Self::Prep,
            "logs" => Self::Logs,
            "settings" => Self::Settings,
            _ => Self::Package,
        }
    }

    fn href(self) -> &'static str {
        match self {
            Self::Package => "/package",
            Self::Prep => "/prep",
            Self::Logs => "/logs",
            Self::Settings => "/settings",
        }
    }
}

/// 同源 Dioxus Web SPA。页面只使用 `/api/*`，不会读取控制端私有配置。
#[component]
pub fn App() -> Element {
    let mut page = use_signal(|| Page::Package);
    let mut dark = use_signal(|| false);
    let settings = use_signal(|| Value::Null);
    let mut status = use_signal(|| Value::Null);
    let mut preps = use_signal(Vec::<Value>::new);
    let mut logs = use_signal(Vec::<Value>::new);
    let selected_group = use_signal(String::new);
    let selected_tasks = use_signal(Vec::<String>::new);
    let notice = use_signal(String::new);

    use_effect(move || {
        spawn(async move {
            if let Ok(path) = document::eval("return window.location.pathname;")
                .join::<String>()
                .await
            {
                page.set(Page::from_path(&path));
            }
            if let Ok(saved) =
                document::eval("return localStorage.getItem('cocos-build-lan-theme');")
                    .join::<Option<String>>()
                    .await
            {
                dark.set(saved.as_deref() == Some("dark"));
            }
            refresh_settings(settings, selected_group).await;
            if let Ok(value) = request_json("GET", "/api/build/status", None).await {
                status.set(value);
            }
            if let Ok(value) = request_json("GET", "/api/prep-projects", None).await {
                preps.set(as_array(&value, ""));
            }
            if let Ok(value) = request_json("GET", "/api/logs", None).await {
                logs.set(as_array(&value, ""));
            }
        });
    });

    let theme = if dark() { "dark" } else { "light" };
    let settings_value = settings();
    let groups = as_array(&settings_value, "taskGroups");
    let selected = selected_group();
    let group = groups
        .iter()
        .find(|group| text(group, "id") == selected)
        .cloned()
        .or_else(|| groups.first().cloned());

    rsx! {
        style { {APP_CSS} }
        div { "data-theme": "{theme}",
            Header { page: page(), dark: dark(), on_toggle_theme: move |_| {
                let next = !dark();
                dark.set(next);
                let marker = if next { "dark" } else { "light" };
                spawn(async move {
                    let script = format!("localStorage.setItem('cocos-build-lan-theme', {});", js_string(marker));
                    let _ = document::eval(&script).await;
                });
            } }
            if !notice().is_empty() {
                div { class: "toast toast--visible", "{notice}" }
            }
            match page() {
                Page::Package => rsx! {
                    PackagePage {
                        settings: settings_value,
                        settings_signal: settings,
                        status: status(),
                        group: group,
                        selected_group: selected_group,
                        selected_tasks: selected_tasks,
                        notice: notice,
                    }
                },
                Page::Prep => rsx! { PrepPage { preps: preps(), notice: notice } },
                Page::Logs => rsx! { LogsPage { logs: logs(), notice: notice } },
                Page::Settings => rsx! { SettingsPage { settings: settings_value, settings_signal: settings, notice: notice } },
            }
        }
    }
}

#[component]
fn Header(page: Page, dark: bool, on_toggle_theme: EventHandler<MouseEvent>) -> Element {
    let link = |label: &'static str, target: Page| {
        let active = if page == target { "is-active" } else { "" };
        rsx! { a { class: "{active}", href: "{target.href()}", "{label}" } }
    };
    rsx! {
        header { class: "topbar",
            div { class: "topbar__brand", span { class: "topbar__logo", "CB" }, "Cocos Build Console" }
            nav { class: "topbar__nav",
                {link("打包页", Page::Package)}
                {link("打包准备", Page::Prep)}
                {link("构建日志", Page::Logs)}
                {link("设置", Page::Settings)}
            }
            div { class: "topbar__right",
                span { class: "saved-hint is-visible", "同源 LAN 服务" }
                button { class: "btn btn--ghost btn--icon", title: "切换亮/暗主题", onclick: move |event| on_toggle_theme.call(event), if dark { "☀" } else { "☾" } }
            }
        }
    }
}

#[component]
fn PackagePage(
    settings: Value,
    settings_signal: Signal<Value>,
    status: Value,
    group: Option<Value>,
    selected_group: Signal<String>,
    selected_tasks: Signal<Vec<String>>,
    notice: Signal<String>,
) -> Element {
    let mut show_group_drawer = use_signal(|| false);
    let groups = as_array(&settings, "taskGroups");
    let projects = as_array(&settings, "projects");
    let tasks = as_array(&settings, "packageTasks");
    let selected_id = selected_group();
    let statuses = as_array(&status, "packageTasks");
    let selected = selected_tasks();
    let current_tasks = group.as_ref().map_or_else(Vec::new, |group| {
        tasks
            .iter()
            .filter(|task| text(task, "taskGroupId") == text(group, "id"))
            .cloned()
            .collect()
    });
    let current_group = group.unwrap_or(Value::Null);
    let group_name = text(&current_group, "name");
    let project_name = projects
        .iter()
        .find(|project| text(project, "id") == text(&current_group, "projectId"))
        .map(|project| text(project, "name"))
        .unwrap_or_else(|| "未选择项目".to_owned());
    let definitions = as_array(&settings, "paramDefinitions");
    let dynamic_placeholder = "${param.<key>}";
    let all_task_ids = current_tasks
        .iter()
        .map(|task| text(task, "id"))
        .collect::<Vec<_>>();

    rsx! {
        div { class: "page",
            aside { class: "page__side",
                div { class: "toolbar", strong { style: "font-size:12.5px", "任务组" }, span { class: "spacer" }, button { class: "btn btn--sm", disabled: projects.is_empty(), onclick: move |_| show_group_drawer.set(true), "+ 新建组" } }
                div { class: "tree",
                    for project in projects.iter() {
                        div { class: "tree__project", span { class: "dot", style: "color:var(--ok)" }, {text(project, "name")} }
                        for item in groups.iter().filter(|item| text(item, "projectId") == text(project, "id")) {
                            {
                                let item = item.clone();
                                let group_id = text(&item, "id");
                                let count = tasks.iter().filter(|task| text(task, "taskGroupId") == group_id).count();
                                let active = if group_id == selected_id { "is-active" } else { "" };
                                rsx! { button { class: "tree__group {active}", onclick: move |_| selected_group.set(group_id.clone()), {text(&item, "name")}, span { class: "count", "{count}" } } }
                            }
                        }
                    }
                }
                hr { class: "divider" }
                p { class: "hint", "同一项目可建多个任务组；参数按组独立保存。" }
            }
            main { class: "page__main",
                section { class: "card",
                    div { class: "card__head", "任务组详情 · {group_name}", span { class: "tag tag--mono", "{project_name}" }, span { class: "spacer" }, span { class: "hint", "300ms 保存提示" } }
                    div { class: "card__body",
                        div { class: "form-grid",
                            div { class: "field", label { class: "field__label", "Git 分支（固定字段 · 远程拉取）" }, input { class: "input input--mono", value: text(&current_group, "branch"), readonly: true } }
                            for definition in definitions {
                                DynamicParam {
                                    definition: definition.clone(),
                                    value: current_group.get("params").and_then(|params| params.get(text(&definition, "key"))).cloned().unwrap_or(Value::Null),
                                    group_id: text(&current_group, "id"),
                                    branch: text(&current_group, "branch"),
                                    params: current_group.get("params").cloned().unwrap_or_else(|| json!({})),
                                    notice: notice,
                                }
                            }
                        }
                        p { class: "hint", style: "margin:10px 0 0", "任务组参数由设置页定义；构建会将 ", span { class: "mono", "{dynamic_placeholder}" }, " 渲染为当前组值。" }
                    }
                }
                div { class: "batch-bar",
                    strong { if selected.is_empty() { "未选中任务 · 勾选下方任务后可批量操作" } else { "已选中 {selected.len()} 个任务" } }
                    button { class: "btn btn--sm btn--primary", disabled: selected.is_empty(), onclick: move |_| {
                        let task_ids = selected_tasks();
                        spawn(async move {
                            match request_json("POST", "/api/build/start", Some(json!({"taskIds": task_ids}))).await {
                                Ok(_) => notice.set("已启动所选任务".to_owned()),
                                Err(error) => notice.set(error),
                            }
                        });
                    }, "启动" }
                    button { class: "btn btn--sm", disabled: selected.len() != 1, onclick: move |_| {
                        let task_id = selected_tasks().first().cloned().unwrap_or_default();
                        spawn(async move {
                            match request_json("POST", "/api/build/stop", Some(json!({"taskId": task_id}))).await {
                                Ok(_) => notice.set("已请求终止构建队列".to_owned()),
                                Err(error) => notice.set(error),
                            }
                        });
                    }, "停止" }
                }
                section { class: "card",
                    div { class: "card__head", "组内任务", span { class: "spacer" }, button { class: "btn btn--sm btn--primary", disabled: true, "+ 新增任务" } }
                    table { class: "table",
                        thead { tr { th { input { r#type: "checkbox", checked: !all_task_ids.is_empty() && selected.len() == all_task_ids.len(), onchange: move |event| {
                            if event.checked() { selected_tasks.set(all_task_ids.clone()); } else { selected_tasks.set(Vec::new()); }
                        } } }, th { "任务名" }, th { "状态" }, th { style: "min-width:150px", "进度" }, th { "代码包" }, th { "混淆" }, th { style: "text-align:right", "操作" } } }
                        tbody {
                            for task in current_tasks {
                                {
                                    let task_id = text(&task, "id");
                                    let runtime = statuses.iter().find(|item| text(item, "taskId") == task_id).cloned().unwrap_or(Value::Null);
                                    let checked = selected.contains(&task_id);
                                    let status_label = if runtime.is_null() { "未开始".to_owned() } else { status_text(&runtime) };
                                    let progress = runtime.get("progress").and_then(Value::as_u64).unwrap_or(0);
                                    let progress_style = format!("width:{progress}%");
                                    let checkbox_task_id = task_id.clone();
                                    let start_task_id = task_id.clone();
                                    let duplicate_task_id = task_id.clone();
                                    let mut start_notice = notice;
                                    let mut duplicate_notice = notice;
                                    rsx! { tr {
                                        td { input { r#type: "checkbox", checked: checked, onchange: move |event| {
                                            let mut next = selected_tasks();
                                            if event.checked() { if !next.contains(&checkbox_task_id) { next.push(checkbox_task_id.clone()); } } else { next.retain(|id| id != &checkbox_task_id); }
                                            selected_tasks.set(next);
                                        } } }
                                        td { strong { {text(&task, "name")} } br {} span { class: "hint mono", "{task_id}" } }
                                        td { span { class: "tag {status_class(&runtime)}", span { class: "dot" }, "{status_label}" } }
                                        td { div { class: "progress", div { class: "progress__bar", style: "{progress_style}" }, span { class: "progress__text", {text(&runtime, "stepLabel")} } } }
                                        td { class: "mono", {text(&task, "codeRepoUrl")} }
                                        td { span { class: "tag", if task.get("enableObfuscation").and_then(Value::as_bool).unwrap_or(false) { "开启" } else { "关闭" } } }
                                        td { div { class: "table__actions",
                                            button { class: "btn btn--sm btn--primary", onclick: move |_| { let id = start_task_id.clone(); spawn(async move { let _ = request_json("POST", "/api/build/start", Some(json!({"taskIds":[id]}))).await; start_notice.set("构建已加入队列".to_owned()); }); }, "启动" }
                                            button { class: "btn btn--sm", onclick: move |_| { let id = duplicate_task_id.clone(); spawn(async move { match request_json("POST", &format!("/api/package-tasks/{id}/duplicate"), None).await { Ok(_) => duplicate_notice.set("任务副本已创建，请在编辑抽屉继续调整".to_owned()), Err(error) => duplicate_notice.set(error) } }); }, "复制" }
                                        } }
                                    } }
                                }
                            }
                        }
                    }
                }
            }
            if show_group_drawer() {
                GroupDrawer {
                    projects: projects.clone(),
                    settings_signal: settings_signal,
                    selected_group: selected_group,
                    notice: notice,
                    on_close: move |_| show_group_drawer.set(false),
                }
            }
        }
    }
}

#[component]
fn GroupDrawer(
    projects: Vec<Value>,
    settings_signal: Signal<Value>,
    selected_group: Signal<String>,
    notice: Signal<String>,
    on_close: EventHandler<MouseEvent>,
) -> Element {
    let mut name = use_signal(String::new);
    let mut branch = use_signal(|| "main".to_owned());
    let mut project_id = use_signal(String::new);
    let current_project = if project_id().is_empty() {
        projects
            .first()
            .map(|project| text(project, "id"))
            .unwrap_or_default()
    } else {
        project_id()
    };
    rsx! {
        div { class: "overlay is-open", onclick: move |event| on_close.call(event) }
        section { class: "drawer is-open", role: "dialog", "aria-label": "新建任务组",
            div { class: "drawer__head", "新建任务组", span { class: "spacer" }, button { class: "btn btn--sm", onclick: move |event| on_close.call(event), "关闭" } }
            div { class: "drawer__body",
                div { class: "field", label { class: "field__label", "项目" },
                    select { class: "select", value: "{current_project}", onchange: move |event| project_id.set(event.value()),
                        for project in projects.iter() { option { value: "{text(project, \"id\")}", "{text(project, \"name\")}" } }
                    }
                }
                div { class: "field", label { class: "field__label", "任务组名称" }, input { class: "input", value: "{name}", placeholder: "例如：Android 正式包", oninput: move |event| name.set(event.value()) } }
                div { class: "field", label { class: "field__label", "Git 分支" }, input { class: "input input--mono", value: "{branch}", oninput: move |event| branch.set(event.value()) } }
                p { class: "hint", "创建后可在此组独立维护动态参数；任务会继承组分支和参数。" }
            }
            div { class: "drawer__foot",
                button { class: "btn", onclick: move |event| on_close.call(event), "取消" }
                button { class: "btn btn--primary", disabled: name().trim().is_empty() || current_project.is_empty(), onclick: move |event| {
                    let payload = json!({"projectId": current_project, "name": name(), "branch": branch(), "params": {}});
                    spawn(async move {
                        match request_json("POST", "/api/task-groups", Some(payload)).await {
                            Ok(group) => {
                                selected_group.set(text(&group, "id"));
                                refresh_settings(settings_signal, selected_group).await;
                                notice.set("任务组已创建".to_owned());
                            }
                            Err(error) => notice.set(error),
                        }
                    });
                    on_close.call(event);
                }, "创建" }
            }
        }
    }
}

#[component]
fn DynamicParam(
    definition: Value,
    value: Value,
    group_id: String,
    branch: String,
    params: Value,
    notice: Signal<String>,
) -> Element {
    let label = text(&definition, "label");
    let key = text(&definition, "key");
    let kind = text(&definition, "type");
    let rendered = value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string());
    let parameter_key = key.clone();
    let save_value = move |value: Value| {
        let mut params = params.clone();
        let key = parameter_key.clone();
        let group_id = group_id.clone();
        let branch = branch.clone();
        let mut notice = notice;
        spawn(async move {
            if let Some(map) = params.as_object_mut() {
                map.insert(key, value);
            }
            match request_json(
                "PUT",
                &format!("/api/task-groups/{group_id}/params"),
                Some(json!({"branch": branch, "params": params})),
            )
            .await
            {
                Ok(_) => notice.set("任务组参数已保存".to_owned()),
                Err(error) => notice.set(error),
            }
        });
    };
    rsx! {
        div { class: "field",
            label { class: "field__label", "{label} ", span { class: "tag tag--mono", "{key}" } }
            if kind == "switch" {
                label { class: "switch", input { r#type: "checkbox", checked: value.as_bool().unwrap_or(false), onchange: move |event| save_value(Value::Bool(event.checked())) }, i {} }
            } else if kind == "select" {
                select { class: "select", value: "{rendered}", onchange: move |event| save_value(Value::String(event.value())),
                    for option in as_array(&definition, "options") { option { value: "{option}", "{option}" } }
                }
            } else {
                input { class: "input input--mono", value: "{rendered}", oninput: move |event| {
                    let raw = event.value();
                    let value = if kind == "number" { raw.parse::<u64>().map_or_else(|_| Value::String(raw.clone()), |number| json!(number)) } else { Value::String(raw) };
                    save_value(value);
                } }
            }
        }
    }
}

#[component]
fn PrepPage(preps: Vec<Value>, notice: Signal<String>) -> Element {
    rsx! {
        main { class: "page__main", style: "max-width:1100px",
            div { class: "toolbar", h1 { style: "font-size:15px", "打包准备项目" }, span { class: "hint", "可执行的 uv 脚本，固定保留 uv run --python 3.14 main.py。" }, span { class: "spacer" }, button { class: "btn btn--primary", disabled: true, "+ 新建准备项目" } }
            div { class: "prep-grid",
                for prep in preps {
                    PrepCard { prep, notice: notice }
                }
            }
        }
    }
}

#[component]
fn PrepCard(prep: Value, notice: Signal<String>) -> Element {
    let prep_id = text(&prep, "id");
    let params = as_array(&prep, "params");
    let parameter_count = params.len();
    rsx! {
        article { class: "card prep-card",
            div { class: "card__body",
                strong { {text(&prep, "name")} }
                span { class: "tag tag--mono", style: "margin-left:6px", "{parameter_count} 参数" }
                p { class: "prep-card__desc", {text(&prep, "description")} }
                div { class: "chips",
                    for parameter in params {
                        span { class: "chip", {text(&parameter, "name")}, ":", {text(&parameter, "type")} }
                    }
                }
                div { class: "prep-card__foot",
                    span { class: "hint mono", {text(&prep, "createTime")} }
                    span { class: "spacer" }
                    button { class: "btn btn--sm", onclick: move |_| {
                        let id = prep_id.clone();
                        spawn(async move {
                            match request_json("GET", &format!("/api/prep-projects/{id}/export"), None).await {
                                Ok(_) => notice.set("准备项目已导出到浏览器数据流".to_owned()),
                                Err(error) => notice.set(error),
                            }
                        });
                    }, "导出" }
                }
            }
        }
    }
}

#[component]
fn LogsPage(logs: Vec<Value>, notice: Signal<String>) -> Element {
    let selected = use_signal(String::new);
    let content = use_signal(String::new);
    let mut query = use_signal(String::new);
    let needle = query().to_lowercase();
    let filtered = logs
        .into_iter()
        .filter(|log| {
            needle.is_empty()
                || text(log, "name").to_lowercase().contains(&needle)
                || text(log, "path").to_lowercase().contains(&needle)
        })
        .collect::<Vec<_>>();
    rsx! {
        main { class: "page__main", style: "max-width:1100px",
            div { class: "toolbar", h1 { style: "font-size:15px", "构建日志" }, span { class: "hint", "按文件查看构建输出；搜索和滚动保留在本地。" }, input { class: "input", style: "max-width:240px", placeholder: "搜索日志文件", value: "{query}", oninput: move |event| query.set(event.value()) }, span { class: "spacer" }, button { class: "btn btn--danger", onclick: move |_| { spawn(async move { match request_empty("DELETE", "/api/logs").await { Ok(()) => notice.set("日志已清空".to_owned()), Err(error) => notice.set(error) } }); }, "清空日志" } }
            div { class: "log-layout",
                aside { class: "log-list",
                    for log in filtered {
                        LogRow { log, selected: selected, content: content, notice: notice }
                    }
                }
                section { class: "logview", pre { "{content}" } }
            }
        }
    }
}

#[component]
fn LogRow(
    log: Value,
    selected: Signal<String>,
    content: Signal<String>,
    notice: Signal<String>,
) -> Element {
    let path = text(&log, "path");
    let active = if path == selected() { "is-active" } else { "" };
    rsx! {
        div { class: "log-item {active}", onclick: move |_| {
            selected.set(path.clone());
            let target = path.clone();
            spawn(async move {
                match request_text("GET", &format!("/api/log-content?path={}", url_encode(&target))).await {
                    Ok(text) => content.set(text),
                    Err(error) => notice.set(error),
                }
            });
        },
            strong { {text(&log, "name")} }
            span { class: "hint mono", {text(&log, "createdAt")}, " · ", {text(&log, "size")} }
        }
    }
}

#[component]
fn SettingsPage(
    settings: Value,
    settings_signal: Signal<Value>,
    notice: Signal<String>,
) -> Element {
    let mut draft =
        use_signal(|| serde_json::to_string_pretty(&settings).unwrap_or_else(|_| "{}".to_owned()));
    use_effect(move || {
        draft.set(
            serde_json::to_string_pretty(&settings_signal()).unwrap_or_else(|_| "{}".to_owned()),
        );
    });
    let configured = settings
        .get("gitCredentialsConfigured")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let git_badge = if configured { "tag--ok" } else { "tag--warn" };
    rsx! {
        div { class: "page",
            aside { class: "page__side", style: "width:180px",
                nav { class: "anchor-nav",
                    a { href: "#sec-engine", class: "is-active", "引擎与项目" }
                    a { href: "#sec-git", "Git 凭据" }
                    a { href: "#sec-feishu", "飞书机器人" }
                    a { href: "#sec-params", "任务参数配置" }
                }
            }
            main { class: "page__main",
                div { class: "settings-col",
                    section { class: "card", id: "sec-engine",
                        div { class: "card__head", "引擎与项目", span { class: "spacer" }, span { class: "hint", "由 LAN 页面保存到 /api/settings" } }
                        div { class: "card__body",
                            div { class: "settings-summary",
                                strong { "Cocos 引擎" }
                                for engine in as_array(&settings, "engines") {
                                    p { class: "mono", {text(&engine, "name")}, " · ", {text(&engine, "path")} }
                                }
                                strong { "项目（Git 仓库）" }
                                for project in as_array(&settings, "projects") {
                                    p { class: "mono", {text(&project, "name")}, " · ", {text(&project, "gitUrl")} }
                                }
                            }
                        }
                    }
                    section { class: "card", id: "sec-git",
                        div { class: "card__head",
                            "Git 凭据"
                            span { class: "spacer" }
                            span { class: "tag {git_badge}", if configured { "已在本机控制端配置" } else { "未配置" } }
                        }
                        div { class: "card__body", p { class: "hint", "账号和 Token 只可在部署机器的控制 App 修改；LAN 网页永不读取或写入其值。" } }
                    }
                    section { class: "card", id: "sec-feishu",
                        div { class: "card__head", "飞书机器人", span { class: "spacer" }, span { class: "hint", "公开设置" } }
                        div { class: "card__body",
                            table { class: "table",
                                tbody {
                                    for bot in as_array(&settings, "feishuBots") {
                                        tr { td { {text(&bot, "name")} }, td { class: "mono", {redact(&text(&bot, "apiKey"))} } }
                                    }
                                }
                            }
                        }
                    }
                    section { class: "card", id: "sec-params",
                        div { class: "card__head", "任务参数配置", span { class: "tag", "动态表单" }, span { class: "spacer" }, span { class: "hint", "保存后任务组立即使用" } }
                        div { class: "card__body",
                            table { class: "table",
                                thead { tr { th { "显示名" } th { "key" } th { "类型" } th { "默认值" } th { "必填" } } }
                                tbody {
                                    for definition in as_array(&settings, "paramDefinitions") {
                                        tr {
                                            td { {text(&definition, "label")} }
                                            td { class: "mono", {text(&definition, "key")} }
                                            td { span { class: "tag", {text(&definition, "type")} } }
                                            td { class: "mono", {definition.get("defaultValue").cloned().unwrap_or(Value::Null).to_string()} }
                                            td { if definition.get("required").and_then(Value::as_bool).unwrap_or(false) { "是" } else { "否" } }
                                        }
                                    }
                                }
                            }
                            p { class: "hint", "高级编辑：这里可直接编辑所有网页可配设置（引擎、项目、飞书、参数定义和任务/任务组）。" }
                            textarea { class: "textarea textarea--mono", value: "{draft}", oninput: move |event| draft.set(event.value()) }
                            div { class: "form-actions",
                                button { class: "btn btn--primary", onclick: move |_| {
                                    let input = draft();
                                    spawn(async move {
                                        match serde_json::from_str::<Value>(&input) {
                                            Ok(value) => match request_json("PUT", "/api/settings", Some(value)).await {
                                                Ok(saved) => { settings_signal.set(saved); notice.set("设置已保存".to_owned()); }
                                                Err(error) => notice.set(error),
                                            },
                                            Err(error) => notice.set(format!("设置 JSON 无效：{error}")),
                                        }
                                    });
                                }, "保存设置" }
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn refresh_settings(mut settings: Signal<Value>, mut selected_group: Signal<String>) {
    if let Ok(value) = request_json("GET", "/api/settings", None).await {
        if selected_group().is_empty()
            && let Some(group) = as_array(&value, "taskGroups").first()
        {
            selected_group.set(text(group, "id"));
        }
        settings.set(value);
    }
}

async fn request_json(method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let options = json!({
        "method": method,
        "headers": {"content-type": "application/json"},
        "body": body.map(|value| value.to_string()),
    });
    let script = format!(
        "return fetch({}, {}).then(async response => {{ const raw = await response.text(); if (!response.ok) throw new Error(raw || response.statusText); return raw ? raw : '{{}}'; }});",
        js_string(path),
        options,
    );
    let raw = document::eval(&script)
        .join::<String>()
        .await
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&raw).map_err(|error| error.to_string())
}

async fn request_text(method: &str, path: &str) -> Result<String, String> {
    let script = format!(
        "return fetch({}, {{method:{}}}).then(async response => {{ const raw = await response.text(); if (!response.ok) throw new Error(raw || response.statusText); return raw; }});",
        js_string(path),
        js_string(method),
    );
    document::eval(&script)
        .join::<String>()
        .await
        .map_err(|error| error.to_string())
}

async fn request_empty(method: &str, path: &str) -> Result<(), String> {
    request_text(method, path).await.map(|_| ())
}

fn as_array(value: &Value, key: &str) -> Vec<Value> {
    let target = if key.is_empty() {
        value
    } else {
        value.get(key).unwrap_or(&Value::Null)
    };
    target.as_array().cloned().unwrap_or_default()
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn status_text(value: &Value) -> String {
    match text(value, "status").as_str() {
        "running" | "canceling" => "进行中".to_owned(),
        "success" => "成功".to_owned(),
        "failed" => "失败".to_owned(),
        "canceled" => "已取消".to_owned(),
        _ => "未开始".to_owned(),
    }
}

fn status_class(value: &Value) -> &'static str {
    match text(value, "status").as_str() {
        "running" | "canceling" => "tag--run",
        "success" => "tag--ok",
        "failed" => "tag--err",
        "canceled" => "tag--warn",
        _ => "",
    }
}

fn redact(value: &str) -> String {
    if value.len() <= 8 {
        "已配置".to_owned()
    } else {
        format!("{}…", &value[..8])
    }
}

fn js_string(value: &str) -> String {
    serde_json::to_string(value).expect("JavaScript 字符串可序列化")
}

fn url_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}
