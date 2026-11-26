use crate::{
    AppContext, api,
    models::{BuildScheme, SchemeApplied},
    package_ui::{PackageUi, SelectedTasks},
};
use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub struct ActiveSchemes(pub Signal<BTreeMap<String, String>>);

#[derive(Clone, Default, PartialEq)]
pub struct SelectionSave {
    pub scheme: String,
    pub tasks: Vec<String>,
    pub desired: Vec<String>,
    pub saved: Vec<String>,
    pub running: bool,
    pub error: String,
}
impl SelectionSave {
    // An acknowledgement only advances the persisted snapshot, never local selection.
    fn complete(&mut self, submitted: Vec<String>, result: Result<Vec<String>, String>) -> bool {
        match result {
            Ok(_) => self.saved = submitted,
            Err(error) => self.error = error,
        }
        self.running = self.error.is_empty() && self.saved != self.desired;
        !self.running
    }
}

#[derive(Clone, Copy)]
pub struct SchemeSaves(pub Signal<BTreeMap<String, SelectionSave>>);
impl SchemeSaves {
    pub fn pending(self) -> bool {
        self.0
            .read()
            .values()
            .any(|s| s.running || s.desired != s.saved || !s.error.is_empty())
    }
    pub fn ready(self, group: &str) -> bool {
        self.0.read().contains_key(group)
    }
    pub async fn flush(mut self, group: &str, retry: bool) -> Result<(), String> {
        if retry && let Some(s) = self.0.write().get_mut(group) {
            s.error.clear();
        }
        loop {
            let Some(s) = self.0.peek().get(group).cloned() else {
                return Ok(());
            };
            if !s.error.is_empty() && !s.running {
                return Err(s.error);
            }
            if !s.running && s.desired == s.saved {
                return Ok(());
            }
            TimeoutFuture::new(30).await;
        }
    }
}

// Installed at App scope so navigating away cannot cancel a pending write.
pub fn use_selection_saves(selected: Signal<Vec<String>>) -> SchemeSaves {
    let mut saves = SchemeSaves(use_signal(BTreeMap::<String, SelectionSave>::new));
    use_context_provider(|| saves);
    use_effect(move || {
        let selection = selected();
        let previous = saves.0();
        let mut next = previous.clone();
        for s in next.values_mut() {
            s.desired = s
                .tasks
                .iter()
                .filter(|id| selection.contains(id))
                .cloned()
                .collect();
        }
        if next != previous {
            saves.0.set(next);
        }
    });
    use_effect(move || {
        let entries = saves.0();
        for (group, entry) in entries {
            if entry.running || !entry.error.is_empty() || entry.desired == entry.saved {
                continue;
            }
            saves.0.write().get_mut(&group).unwrap().running = true;
            let scheme_id = entry.scheme.clone();
            spawn(async move {
                loop {
                    let Some(before) = saves.0.peek().get(&group).map(|s| s.desired.clone()) else {
                        break;
                    };
                    TimeoutFuture::new(250).await;
                    let Some(entry) = saves.0.peek().get(&group).cloned() else {
                        break;
                    };
                    if entry.scheme != scheme_id {
                        break;
                    }
                    if entry.desired != before {
                        continue;
                    }
                    let result = api::put::<Vec<String>, _>(
                        &format!("/api/task-groups/{group}/schemes/{}/tasks", entry.scheme),
                        &serde_json::json!({"taskIds": entry.desired}),
                    )
                    .await;
                    let mut map = saves.0.write();
                    let Some(current) = map.get_mut(&group) else {
                        break;
                    };
                    if current.scheme != scheme_id {
                        break;
                    }
                    if current.complete(entry.desired, result) {
                        break;
                    }
                }
            });
        }
    });
    saves
}

#[derive(Clone, Copy)]
struct SchemeUi {
    ui: PackageUi,
    selected: Signal<Vec<String>>,
    active: ActiveSchemes,
    saves: SchemeSaves,
}
impl SchemeUi {
    fn accept(mut self, result: SchemeApplied, tasks: Vec<String>) {
        let group = result.group;
        let ids: Vec<_> = tasks
            .iter()
            .filter(|id| result.task_ids.contains(id))
            .cloned()
            .collect();
        self.selected.write().retain(|id| !tasks.contains(id));
        self.selected.write().extend(ids.clone());
        self.saves.0.write().insert(
            group.id.clone(),
            SelectionSave {
                scheme: result.scheme_id.clone(),
                tasks,
                saved: ids.clone(),
                desired: ids,
                ..Default::default()
            },
        );
        self.active
            .0
            .write()
            .insert(group.id.clone(), result.scheme_id);
        self.ui.baseline.set(group.clone());
        self.ui.draft.set(group);
        self.ui.revision += 1;
    }
    async fn apply(self, id: &str, tasks: Vec<String>) -> Result<(), String> {
        let group = self.ui.draft.peek().id.clone();
        let result =
            api::post(&format!("/api/task-groups/{group}/schemes/{id}/apply"), &()).await?;
        self.accept(result, tasks);
        Ok(())
    }
}

#[derive(Clone, PartialEq, serde::Serialize)]
struct Entry {
    id: Option<String>,
    name: String,
}

#[component]
pub fn Schemes(task_ids: Vec<String>, queue_busy: bool) -> Element {
    let mut ui = use_context::<PackageUi>();
    let mut controller = SchemeUi {
        ui,
        selected: use_context::<SelectedTasks>().0,
        active: use_context::<ActiveSchemes>(),
        saves: use_context::<SchemeSaves>(),
    };
    let context = use_context::<AppContext>();
    let mut pending = use_signal(|| None::<String>);
    let mut entries = use_signal(Vec::<Entry>::new);
    let mut template = use_signal(BuildScheme::default);
    let mut initialized = use_signal(|| false);
    let mut error = use_signal(String::new);
    let mut ids = use_signal(|| task_ids.clone());
    use_effect(use_reactive((&task_ids,), move |(tasks,)| {
        ids.set(tasks.clone());
        let group = ui.draft.peek().id.clone();
        if let Some(s) = controller.saves.0.write().get_mut(&group) {
            s.tasks = tasks;
        }
    }));
    use_effect(use_reactive((&queue_busy,), move |(busy,)| {
        if busy || initialized() || *ui.saving.peek() {
            return;
        }
        let group = ui.draft.peek().clone();
        let remembered = controller.active.0.peek().get(&group.id).cloned();
        let Some(scheme) = group
            .schemes
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|s| Some(&s.id) == remembered.as_ref())
            .or_else(|| group.schemes.as_ref()?.first())
            .cloned()
        else {
            return;
        };
        initialized.set(true);
        ui.saving.set(true);
        spawn(async move {
            let result = controller.saves.flush(&group.id, false).await;
            match result {
                Ok(()) => {
                    if let Err(e) = controller.apply(&scheme.id, ids()).await {
                        error.set(e);
                    }
                }
                Err(e) => error.set(e),
            }
            ui.saving.set(false);
        });
    }));
    let group = (ui.draft)();
    let active = controller.active.0()
        .get(&group.id)
        .cloned()
        .unwrap_or_default();
    let schemes = group.schemes.clone().unwrap_or_default();
    let saving = controller.saves.0().get(&group.id).cloned();
    let disabled =
        queue_busy || (ui.saving)() || (ui.managing)() || !controller.saves.ready(&group.id);
    let mut begin_editor = move || {
        let group = ui.draft.peek().clone();
        template.set(BuildScheme {
            id: String::new(),
            name: String::new(),
            branch: group.branch,
            params: group.params,
            task_ids: controller
                .selected
                .peek()
                .iter()
                .filter(|id| ids.peek().contains(id))
                .cloned()
                .collect(),
        });
        entries.set(
            ui.draft
                .peek()
                .schemes
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|s| Entry {
                    id: Some(s.id.clone()),
                    name: s.name.clone(),
                })
                .collect(),
        );
        ui.managing.set(true);
    };
    let mut request = move |target: String| {
        error.set(String::new());
        ui.saving.set(true);
        spawn(async move {
            let group = ui.draft.peek().id.clone();
            let result = controller.saves.flush(&group, false).await;
            ui.saving.set(false);
            if ui.dirty() || result.is_err() {
                pending.set(Some(target));
            } else if target.is_empty() {
                begin_editor();
            } else {
                ui.saving.set(true);
                if let Err(e) = controller.apply(&target, ids()).await {
                    error.set(e);
                }
                ui.saving.set(false);
            }
        });
    };
    rsx! {
        div { class: "scheme-switcher", role: "group", "aria-label": "构建方案",
            for scheme in schemes {
                button { class: if scheme.id == active { "preset-chip is-applied" } else { "preset-chip" }, disabled, "aria-pressed": scheme.id == active,
                    onclick: move |_| request(scheme.id.clone()), "{scheme.name}" }
            }
            if (ui.managing)() {
                button { class: "btn btn--primary btn--sm", disabled: queue_busy || (ui.saving)(), onclick: move |_| {
                    ui.saving.set(true);
                    spawn(async move {
                        let group = ui.draft.peek().id.clone();
                        let current = controller.active.0.peek().get(&group).cloned().unwrap_or_default();
                        match api::put::<SchemeApplied, _>(&format!("/api/task-groups/{group}/schemes/manage"), &serde_json::json!({"currentId": current, "schemes": entries(), "template": template()})).await {
                            Ok(result) => { controller.accept(result, ids()); ui.managing.set(false); error.set(String::new()); },
                            Err(e) => error.set(e),
                        }
                        ui.saving.set(false);
                    });
                }, "保存" }
                button { class: "btn btn--sm", disabled: (ui.saving)(), onclick: move |_| { ui.managing.set(false); error.set(String::new()); }, "取消" }
            } else { button { class: "btn btn--sm", disabled, onclick: move |_| request(String::new()), "编辑方案" } }
            if (ui.saving)() || saving.as_ref().is_some_and(|s| s.running || s.desired != s.saved && s.error.is_empty()) { span { class: "hint", role: "status", "保存中…" } }
            if !initialized() { span { class: "hint", "等待队列空闲后加载方案…" } }
            if let Some(s) = saving { if !s.error.is_empty() { span { class: "field-error", title: "{s.error}", "勾选未保存" } button { class: "btn btn--sm", onclick: move |_| { let group = ui.draft.peek().id.clone(); spawn(async move { let _ = controller.saves.flush(&group, true).await; }); }, "重试" } } }
            if !error().is_empty() { span { class: "field-error", role: "alert", "{error}" } if !controller.saves.ready(&group.id) { button { class: "btn btn--sm", onclick: move |_| { initialized.set(false); error.set(String::new()); }, "重新加载" } } }
        }
        if (ui.managing)() {
            div { class: "scheme-editor", role: "group", "aria-label": "编辑构建方案",
                for (index, entry) in entries().into_iter().enumerate() {
                    div { class: "input-action", input { class: "input", "aria-label": "方案名称", value: "{entry.name}", disabled: (ui.saving)(), oninput: move |event| entries.write()[index].name = event.value() }
                        button { class: "btn btn--sm btn--danger", disabled: (ui.saving)() || entries().len() <= 1, onclick: move |_| { entries.write().remove(index); }, "删除" }
                    }
                }
                button { class: "btn btn--sm", disabled: (ui.saving)() || entries().len() >= 5, onclick: move |_| entries.write().push(Entry { id: None, name: String::new() }), "新增方案" }
            }
        }
        if let Some(target) = pending() {
            div { class: "overlay is-open" }
            section { class: "modal modal--sm is-open", role: "dialog", "aria-label": "方案有未保存修改",
                div { class: "drawer__head", "方案有未保存修改" }
                div { class: "drawer__body", p { "参数草稿或勾选尚未保存。保存后继续，或放弃这些改动。" } }
                div { class: "drawer__foot",
                    button { class: "btn", disabled: (ui.saving)(), onclick: move |_| pending.set(None), "取消" }
                    button { class: "btn", disabled: (ui.saving)() || queue_busy, onclick: { let target = target.clone(); move |_| { let target = target.clone(); spawn(async move {
                        ui.saving.set(true);
                        let group = ui.draft.peek().id.clone();
                        let current = controller.active.0.peek().get(&group).cloned().unwrap_or_default();
                        match controller.apply(if target.is_empty() { &current } else { &target }, ids()).await {
                            Ok(()) => { pending.set(None); if target.is_empty() { begin_editor(); } }, Err(e) => context.error(e),
                        }
                        ui.saving.set(false);
                    }); } }, "放弃并继续" }
                    button { class: "btn btn--primary", disabled: (ui.saving)() || queue_busy, onclick: move |_| { let target = target.clone(); ui.saving.set(true); spawn(async move {
                        let group = ui.draft.peek().id.clone();
                        let flushed = controller.saves.flush(&group, true).await;
                        ui.saving.set(false);
                        if let Err(e) = flushed { context.error(e); return; }
                        if ui.dirty() && let Err(e) = ui.save().await { context.error(e); return; }
                        if target.is_empty() { begin_editor(); pending.set(None); }
                        else { ui.saving.set(true); match controller.apply(&target, ids()).await { Ok(()) => pending.set(None), Err(e) => context.error(e) } ui.saving.set(false); }
                    }); }, "保存并继续" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_acknowledgement_keeps_latest_selection_pending() {
        let mut state = SelectionSave {
            desired: vec!["new".into()],
            running: true,
            ..Default::default()
        };
        assert!(!state.complete(vec!["old".into()], Ok(vec!["old".into()])));
        assert_eq!(state.desired, vec!["new"]);
        assert_eq!(state.saved, vec!["old"]);
        assert!(state.complete(vec!["new".into()], Ok(vec!["new".into()])));
        assert_eq!(state.saved, state.desired);
    }
    #[test]
    fn failed_selection_write_preserves_both_snapshots() {
        let mut state = SelectionSave {
            desired: vec!["new".into()],
            saved: vec!["old".into()],
            running: true,
            ..Default::default()
        };
        assert!(state.complete(vec!["new".into()], Err("disk full".into())));
        assert_eq!(state.desired, vec!["new"]);
        assert_eq!(state.saved, vec!["old"]);
        assert!(!state.error.is_empty());
    }
}
