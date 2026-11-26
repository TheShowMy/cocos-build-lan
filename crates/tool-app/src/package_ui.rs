use std::collections::BTreeMap;

use dioxus::{document, prelude::*};
use dioxus_router::use_navigator;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    AppContext, Route, api,
    models::{
        PlaceholderInfo, Project, ProjectBranchesResponse, TaskGroup, TaskGroupParamsRequest,
    },
};

fn storage(session: bool) -> Option<web_sys::Storage> {
    #[cfg(target_arch = "wasm32")]
    {
        let window = web_sys::window()?;
        if session {
            window.session_storage().ok().flatten()
        } else {
            window.local_storage().ok().flatten()
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = session;
        None
    }
}

fn storage_key(name: &str) -> String {
    let tool: serde_json::Value =
        serde_json::from_str(include_str!("../../../tool.json")).unwrap_or_default();
    format!(
        "{}:{name}",
        tool["tool_id"].as_str().unwrap_or("cocos-build-lan")
    )
}

pub fn read_storage<T: DeserializeOwned + Default>(name: &str, session: bool) -> T {
    storage(session)
        .and_then(|s| s.get_item(&storage_key(name)).ok().flatten())
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

pub fn write_storage<T: Serialize>(name: &str, session: bool, value: &T) {
    if let Some(storage) = storage(session)
        && let Ok(json) = serde_json::to_string(value)
    {
        let _ = storage.set_item(&storage_key(name), &json);
    }
}

#[derive(Clone, Copy)]
pub struct SelectedTasks(pub Signal<Vec<String>>);

#[derive(Clone, Copy)]
pub struct SelectionOwners(pub Signal<BTreeMap<String, String>>);

#[derive(Clone, PartialEq)]
pub enum Destination {
    Group(String),
    Page(Route),
}

#[derive(Clone, Copy)]
pub struct PackageUi {
    pub active: Signal<BTreeMap<String, String>>,
    pub managing: Signal<bool>,
    pub selected_group: Signal<String>,
    pub draft: Signal<TaskGroup>,
    pub baseline: Signal<TaskGroup>,
    pub saving: Signal<bool>,
    pub pending: Signal<Option<Destination>>,
    pub revision: Signal<u64>,
}

impl PackageUi {
    pub fn dirty(&self) -> bool {
        let draft = self.draft.read();
        let baseline = self.baseline.read();
        draft.id == baseline.id
            && (draft.branch != baseline.branch || draft.params != baseline.params)
    }
    pub fn select_group(mut self, id: String) {
        if *self.managing.peek() || *self.saving.peek() {
            return;
        }
        if *self.selected_group.peek() == id {
            return;
        }
        if self.dirty() {
            self.pending.set(Some(Destination::Group(id)));
        } else {
            self.selected_group.set(id);
        }
    }
    pub async fn save(mut self) -> Result<(), String> {
        if *self.saving.peek() {
            return Err("正在保存组参数".into());
        }
        self.saving.set(true);
        let group = self.draft.peek().clone();
        let result = api::put::<TaskGroup, _>(
            &format!("/api/task-groups/{}/params", group.id),
            &TaskGroupParamsRequest {
                scheme_id: self.active.peek().get(&group.id).cloned(),
                branch: group.branch,
                params: group.params,
            },
        )
        .await;
        self.saving.set(false);
        match result {
            Ok(group) => {
                self.baseline.set(group.clone());
                self.draft.set(group);
                self.revision += 1;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

#[component]
pub fn UnsavedDialog() -> Element {
    let mut ui = use_context::<PackageUi>();
    let context = use_context::<AppContext>();
    let navigator = use_navigator();
    let destination = (ui.pending)();
    let mut proceed = move |destination: Destination| {
        ui.pending.set(None);
        match destination {
            Destination::Group(id) => ui.selected_group.set(id),
            Destination::Page(route) => {
                navigator.push(route);
            }
        }
    };
    rsx! { if let Some(destination) = destination {
        div { class: "overlay is-open" }
        section { class: "modal modal--sm is-open", role: "dialog", "aria-label": "组参数未保存",
            div { class: "drawer__head", "组参数未保存" }
            div { class: "drawer__body", p { "保存后再继续，或放弃此次修改。" } }
            div { class: "drawer__foot",
                button { class: "btn", disabled: (ui.saving)(), onclick: move |_| ui.pending.set(None), "留在当前页" }
                button { class: "btn", disabled: (ui.saving)(), onclick: { let destination = destination.clone(); move |_| { ui.draft.set((ui.baseline)()); proceed(destination.clone()); } }, "放弃修改" }
                button { class: "btn btn--primary", disabled: (ui.saving)(), onclick: move |_| { let destination = destination.clone(); spawn(async move { match ui.save().await { Ok(()) => proceed(destination), Err(error) => context.error(error) } }); }, "保存并继续" }
            }
        }
    } }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct BranchEntry {
    pub branches: Vec<String>,
    pub fetched_at: String,
    #[serde(skip)]
    pub loading: bool,
    #[serde(skip)]
    pub error: String,
}

#[derive(Clone, Copy)]
pub struct BranchCache(pub Signal<BTreeMap<String, BranchEntry>>);

pub fn branch_key(project: &Project) -> String {
    format!("{}\n{}", project.id, project.git_url)
}

pub fn prune_branches(mut cache: BranchCache, projects: &[Project]) {
    cache
        .0
        .write()
        .retain(|key, _| projects.iter().any(|project| branch_key(project) == *key));
}

fn fetch_branches(mut cache: BranchCache, project: Project, force: bool) {
    let key = branch_key(&project);
    if cache.0.peek().get(&key).is_some_and(|entry| {
        entry.loading || (!force && (!entry.fetched_at.is_empty() || !entry.error.is_empty()))
    }) {
        return;
    }
    cache.0.write().entry(key.clone()).or_default().loading = true;
    // Cache requests outlive the group/editor that first requested them.
    dioxus::dioxus_core::spawn_forever(async move {
        let result = api::get::<ProjectBranchesResponse>(&format!(
            "/api/project-branches?projectId={}",
            api::encode_query(&project.id)
        ))
        .await;
        let timestamp = document::eval("return new Date().toLocaleString();")
            .join::<String>()
            .await
            .unwrap_or_else(|_| "已获取".into());
        // A removed/replaced project must not resurrect an obsolete entry.
        if let Some(entry) = cache.0.write().get_mut(&key) {
            entry.loading = false;
            match result {
                Ok(value) => {
                    entry.branches = value.branches;
                    entry.fetched_at = timestamp;
                    entry.error.clear();
                }
                Err(error) => entry.error = error,
            }
        }
    });
}

#[component]
pub fn BranchPicker(
    project: Option<Project>,
    value: String,
    on_change: EventHandler<String>,
    disabled: bool,
) -> Element {
    let cache = use_context::<BranchCache>();
    let mut manual = use_signal(|| false);
    use_effect(use_reactive((&project,), move |(project,)| {
        if let Some(project) = project {
            fetch_branches(cache, project, false);
        }
    }));
    let entry = project
        .as_ref()
        .and_then(|project| cache.0.read().get(&branch_key(project)).cloned())
        .unwrap_or_default();
    let missing = !value.is_empty() && !entry.branches.contains(&value);
    rsx! { div { class: "field field--span-all", label { class: "field__label", "Git 分支" }
        div { class: "input-action",
            if manual() || entry.branches.is_empty() {
                input { class: "input input--mono", value: "{value}", disabled, oninput: move |event| on_change.call(event.value()) }
            } else {
                select { class: "select select--mono", value: "{value}", disabled, onchange: move |event| on_change.call(event.value()),
                    if value.is_empty() { option { value: "", "请选择分支" } }
                    if missing { option { value: "{value}", "{value}（当前值，列表中未找到）" } }
                    for branch in &entry.branches { option { value: "{branch}", selected: branch == &value, "{branch}" } }
                }
            }
            button { class: "btn btn--sm", disabled, onclick: move |_| { let next = !manual(); manual.set(next); }, if manual() { "下拉选择" } else { "手动输入" } }
            button { class: "btn btn--sm", title: "上次刷新：{entry.fetched_at}", disabled: entry.loading || disabled || project.is_none(), onclick: move |_| if let Some(project) = project.clone() { fetch_branches(cache, project, true); }, if entry.loading { "获取中…" } else { "刷新分支" } }
        }
        if !entry.error.is_empty() { p { class: "field-error", "获取失败：{entry.error}；已保留缓存，可手动重试。" } }
    } }
}

#[component]
pub fn PlaceholderHelp() -> Element {
    let mut open = use_signal(|| false);
    let mut catalog = use_signal(Vec::<PlaceholderInfo>::new);
    let context = use_context::<AppContext>();
    use_effect(move || {
        if open() {
            spawn(async move {
                match api::get::<Vec<PlaceholderInfo>>("/api/parameters/placeholders").await {
                    Ok(rows) => catalog.set(rows),
                    Err(error) => context.error(error),
                }
            });
        }
    });
    let rows = catalog()
        .into_iter()
        .map(|row| (row.reference, row.description));
    rsx! { div { class: "placeholder-help",
        button { r#type: "button", class: "btn btn--sm", onclick: move |_| { let next = !open(); open.set(next); }, "占位符说明" }
        if open() { div { class: "placeholder-help__body",
            p { class: "hint", "适用于构建 JSON、准备参数的字符串和选项实际值、条件来源。整数/布尔输入不接受占位符；条件“等于”按字面值比较。未知引用保留原样，不递归替换。" }
            p { class: "hint", "独立运行准备项目没有任务组参数，version 及所有参数前缀也不可用（保留引用原样）；请从打包任务执行以使用这些参数。路径统一使用 /。" }
            for (token, description) in rows { div { class: "placeholder-row",
                code { "{token}" } span { "{description}" }
                button { class: "btn btn--sm", onclick: move |_| { let token = token.clone(); spawn(async move { let script = format!("const text = {}; if (navigator.clipboard && window.isSecureContext) {{ await navigator.clipboard.writeText(text); }} else {{ const node = document.createElement('textarea'); node.value = text; document.body.appendChild(node); node.select(); const ok = document.execCommand('copy'); node.remove(); if (!ok) throw new Error('copy failed'); }} return true;", serde_json::to_string(&token).unwrap()); match document::eval(&script).join::<bool>().await { Ok(true) => context.success("已复制占位符"), _ => context.error("复制失败，请手动复制") } }); }, "复制" }
            } }
        } }
    } }
}

#[component]
pub fn BuiltinPlaceholders() -> Element {
    let mut catalog = use_signal(Vec::<PlaceholderInfo>::new);
    let context = use_context::<AppContext>();
    use_future(move || async move {
        match api::get::<Vec<PlaceholderInfo>>("/api/parameters/placeholders").await {
            Ok(rows) => catalog.set(rows),
            Err(error) => context.error(error),
        }
    });
    rsx! { for row in catalog().into_iter().filter(|row| row.builtin) {
        div { class: "placeholder-row", code { "{row.reference}" } span { "{row.description}" } }
    } }
}

#[component]
pub fn FieldHelp(id: String, text: String) -> Element {
    let button_id = format!("{id}-button");
    rsx! {
        button { id: "{button_id}", class: "field-help-button", r#type: "button", "aria-label": "参数说明", "aria-controls": "{id}", onclick: { let id = id.clone(); move |_| {
            let id = serde_json::to_string(&id).unwrap();
            spawn(async move { let _ = document::eval(&format!(r#"
                const p = document.getElementById({id});
                const b = document.getElementById({id} + '-button');
                if (p.matches(':popover-open')) {{ p.hidePopover(); return; }}
                p.showPopover();
                const r = b.getBoundingClientRect();
                const w = p.offsetWidth, h = p.offsetHeight;
                p.style.left = Math.max(8, Math.min(r.left, innerWidth-w-8))+'px';
                p.style.top = Math.max(8, Math.min(r.bottom+6, innerHeight-h-8))+'px';
                p.querySelector('button').focus();
            "#)).await; });
        } }, "?" }
        div { id: "{id}", class: "field-help", "popover": "auto", role: "dialog", "aria-label": "参数说明",
            p { "{text}" }
            button { class: "btn btn--sm", "popovertarget": "{id}", "popovertargetaction": "hide", "关闭" }
        }
    }
}
