use std::collections::{BTreeMap, HashMap};

use dioxus::{document, prelude::*};
use dioxus_free_icons::{
    Icon,
    icons::ld_icons::{
        LdChevronDown, LdChevronUp, LdCircleStop, LdCopy, LdHammer, LdPencil, LdPlay, LdPlus,
        LdRefreshCw, LdSave, LdSettings, LdSparkles, LdTrash2, LdX,
    },
};
use gloo_timers::future::TimeoutFuture;
use serde_json::{Value, json};

use crate::{
    AppContext, ConfirmDialog, api,
    models::{
        BuildStatusResponse, GroupParamPreset, ObfuscationMode, PackageTask, PackageTaskRequest,
        ParamDefinition, ParamKind, PrepParam, PrepParamType, PrepProject, PrepRunForTasksRequest,
        PrepTaskRunResponse, Project, PublicSettings, TaskGroup, TaskGroupRequest, TaskPrepAction,
        TaskPrepTarget, TaskStatus, parse_number_value, value_text,
    },
    package_ui::{
        BranchCache, BranchPicker, PackageUi, PlaceholderHelp, SelectedTasks, SelectionOwners,
        prune_branches, write_storage,
    },
};

#[component]
pub fn Package() -> Element {
    let context = use_context::<AppContext>();
    let mut settings = use_signal(PublicSettings::default);
    let mut preps = use_signal(Vec::<PrepProject>::new);
    let mut build_status = use_signal(BuildStatusResponse::default);
    let mut loading = use_signal(|| true);
    let mut load_error = use_signal(String::new);
    let mut poll_error = use_signal(String::new);
    let mut refresh = use_signal(|| 0u64);
    let mut ui = use_context::<PackageUi>();
    let mut saves = use_context::<crate::schemes::SchemeSaves>();
    let mut selected_group = ui.selected_group;
    let mut selected_tasks = use_context::<SelectedTasks>().0;
    let mut selection_owners = use_context::<SelectionOwners>().0;
    let cache = use_context::<BranchCache>();
    let mut selection_ready = use_signal(|| false);
    let mut status_known = use_signal(|| false);
    let mut stop_target = use_signal(|| None::<String>);
    let mut group_editor = use_signal(|| None::<(bool, TaskGroup)>);
    let mut task_editor = use_signal(|| None::<(bool, PackageTask)>);
    let mut delete_group = use_signal(|| None::<TaskGroup>);
    let mut delete_task = use_signal(|| None::<PackageTask>);
    let mut confirm_batch_delete = use_signal(|| false);
    let mut cleanup_task = use_signal(|| None::<PackageTask>);
    let mut batch_prep_open = use_signal(|| false);
    let mut batch_prep_id = use_signal(String::new);
    let mut batch_prep_values = use_signal(HashMap::<String, Value>::new);
    let mut batch_prep_result = use_signal(|| None::<PrepTaskRunResponse>);
    let action_busy = use_signal(String::new);
    use_effect(move || {
        if selection_ready() {
            let selections = selected_tasks();
            let owners = settings()
                .package_tasks
                .iter()
                .filter(|task| selections.contains(&task.id))
                .map(|task| (task.id.clone(), task.task_group_id.clone()))
                .collect::<BTreeMap<_, _>>();
            write_storage("selected-tasks", true, &owners);
            selection_owners.set(owners);
        }
    });

    use_effect(move || {
        let _ = refresh();
        let _ = (ui.revision)();
        spawn(async move {
            loading.set(true);
            match api::get::<PublicSettings>("/api/settings").await {
                Ok(mut value) => {
                    value.task_groups.sort_by_key(|group| group.order);
                    prune_branches(cache, &value.projects);
                    let owners = selection_owners.peek().clone();
                    selected_tasks.write().retain(|id| {
                        value.package_tasks.iter().any(|task| {
                            &task.id == id
                                && owners
                                    .get(id)
                                    .is_none_or(|group| group == &task.task_group_id)
                        })
                    });
                    {
                        let mut entries = saves.0.write();
                        entries.retain(|id, entry| {
                            value.task_groups.iter().any(|g| {
                                &g.id == id
                                    && g.schemes.iter().flatten().any(|s| s.id == entry.scheme)
                            })
                        });
                        for group in &value.task_groups {
                            if entries.contains_key(&group.id) {
                                continue;
                            }
                            let remembered = ui.active.peek().get(&group.id).cloned();
                            if let Some(scheme) = group
                                .schemes
                                .iter()
                                .flatten()
                                .find(|s| Some(&s.id) == remembered.as_ref())
                            {
                                let mut tasks: Vec<_> = value
                                    .package_tasks
                                    .iter()
                                    .filter(|t| t.task_group_id == group.id)
                                    .map(|t| t.id.clone())
                                    .collect();
                                tasks.sort();
                                let ids: Vec<_> = tasks
                                    .iter()
                                    .filter(|id| scheme.task_ids.contains(id))
                                    .cloned()
                                    .collect();
                                selected_tasks.write().retain(|id| !tasks.contains(id));
                                selected_tasks.write().extend(ids.clone());
                                entries.insert(
                                    group.id.clone(),
                                    crate::schemes::SelectionSave {
                                        scheme: scheme.id.clone(),
                                        tasks,
                                        desired: ids.clone(),
                                        saved: ids,
                                        ..Default::default()
                                    },
                                );
                            }
                        }
                        for (id, entry) in entries.iter_mut() {
                            entry.tasks = value
                                .package_tasks
                                .iter()
                                .filter(|t| &t.task_group_id == id)
                                .map(|t| t.id.clone())
                                .collect();
                            entry.tasks.sort();
                        }
                    }
                    selection_ready.set(true);
                    if !value
                        .task_groups
                        .iter()
                        .any(|group| group.id == selected_group())
                    {
                        selected_group.set(
                            value
                                .task_groups
                                .first()
                                .map(|group| group.id.clone())
                                .unwrap_or_default(),
                        );
                    }
                    settings.set(value);
                    load_error.set(String::new());
                }
                Err(error) => load_error.set(error),
            }
            if let Ok(value) = api::get::<Vec<PrepProject>>("/api/prep-projects").await {
                preps.set(value);
            }
            loading.set(false);
        });
    });

    use_future(move || async move {
        loop {
            match api::get::<BuildStatusResponse>("/api/build/status").await {
                Ok(value) => {
                    build_status.set(value);
                    status_known.set(true);
                    poll_error.set(String::new());
                }
                Err(error) => {
                    poll_error.set(error);
                    status_known.set(false);
                }
            }
            TimeoutFuture::new(2000).await;
        }
    });

    let current_group = settings()
        .task_groups
        .into_iter()
        .find(|group| group.id == selected_group());
    let current_tasks = current_group
        .as_ref()
        .map(|group| {
            let mut tasks = settings()
                .package_tasks
                .into_iter()
                .filter(|task| task.task_group_id == group.id)
                .collect::<Vec<_>>();
            tasks.sort_by_key(|task| task.order);
            tasks
        })
        .unwrap_or_default();
    let current_ids =
        use_memo(move || selected_in_group(&settings(), &selected_tasks(), &selected_group()));
    let selected_count = current_ids().len();
    let all_ids = use_memo(move || ordered_selection(&settings(), &selected_tasks()));
    let selection_locked = (ui.managing)() || (ui.saving)() || !saves.ready(&selected_group());
    let blocked = if selection_locked {
        "方案尚未就绪或正在编辑"
    } else if !status_known() {
        "状态尚未确认"
    } else if !action_busy().is_empty() {
        "提交中…"
    } else if build_status().build_in_progress {
        "全局队列占用"
    } else if ui.dirty() {
        "请先保存或放弃组参数修改"
    } else {
        ""
    };
    let global_dirty = ui.dirty()
        && all_ids().iter().any(|id| {
            settings()
                .package_tasks
                .iter()
                .any(|task| &task.id == id && task.task_group_id == (ui.draft)().id)
        });
    let global_disabled = selection_locked
        || all_ids().is_empty()
        || !status_known()
        || !action_busy().is_empty()
        || build_status().build_in_progress
        || global_dirty;
    let active_task = settings()
        .package_tasks
        .into_iter()
        .find(|task| Some(&task.id) == build_status().active_task_id.as_ref());

    rsx! {
        div { class: "page package-page",
            aside { class: "page__side package-tree",
                div { class: "tree-head", strong { "任务组" } span { class: "spacer" } button { class: "btn btn--sm btn--icon", title: "新建任务组", disabled: settings().projects.is_empty(), onclick: move |_| { let project_id = settings().projects.first().map(|project| project.id.clone()).unwrap_or_default(); group_editor.set(Some((true, TaskGroup { project_id, branch: "main".to_owned(), ..TaskGroup::default() }))); }, Icon { width: 15, height: 15, icon: LdPlus } } }
                nav { class: "package-group-list", "aria-label": "任务组列表", tabindex: "0",
                    if settings().projects.is_empty() { div { class: "empty-inline", "请先在设置页添加项目" } }
                    for project in settings().projects {
                        ProjectGroups { project, groups: settings().task_groups, tasks: settings().package_tasks, statuses: build_status(), selected_group, selections: selected_tasks(), on_new: move |project_id| group_editor.set(Some((true, TaskGroup { project_id, branch: "main".to_owned(), ..TaskGroup::default() }))) }
                    }
                }
                section { class: "global-queue",
                    strong { "全局构建队列" }
                    span { "全局已勾选 {all_ids().len()} 项" }
                    div { class: "global-queue__details", role: "region", "aria-label": "全局队列详情", tabindex: "0",
                        for group in settings().task_groups {
                            { let count = selected_in_group(&settings(), &selected_tasks(), &group.id).len(); rsx! { if count > 0 { span { "{group.name}：已选 {count} 项" } } } }
                        }
                        if global_dirty { p { class: "field-error", "所选任务组有未保存参数" } }
                        if !status_known() { p { class: "hint", "队列状态尚未确认，暂不能启动" } }
                        if status_known() && build_status().build_in_progress {
                            if build_status().cancel_requested { span { "正在停止整个队列…" } }
                            if let Some(task) = &active_task { span { "本轮正在执行：{task.name} · {task.group}" } }
                            span { "本轮等待 {build_status().queued_task_ids.len()} 项" }
                        } else if status_known() {
                            span { "当前无构建队列" }
                        }
                    }
                    button { class: "btn btn--primary", disabled: global_disabled, onclick: move |_| { let ids = all_ids(); let message = format!("已提交全局勾选的 {} 项任务", ids.len()); start_tasks(ids, "global".into(), message, context, build_status, action_busy); }, if action_busy() == "global" { "提交中…" } else { "启动全部已选任务" } }
                    button { class: "btn btn--sm", disabled: (ui.managing)() || (ui.saving)() || selected_tasks().is_empty(), onclick: move |_| selected_tasks.set(Vec::new()), "清空全部勾选" }
                    if build_status().build_in_progress {
                        if let Some(task) = active_task {
                            button { class: "btn btn--sm", onclick: { let id = task.task_group_id.clone(); move |_| ui.select_group(id.clone()) }, "查看运行组" }
                            button { class: "btn btn--sm btn--danger", disabled: build_status().cancel_requested || !action_busy().is_empty(), onclick: move |_| stop_target.set(Some(task.id.clone())), "停止整个队列" }
                        }
                    }
                }
            }
            main { class: "page__main package-main", "aria-label": "打包任务内容", tabindex: "0",
                if loading() && settings().task_groups.is_empty() { div { class: "empty-state", "正在加载打包配置…" } }
                else if !load_error().is_empty() { div { class: "error-state", p { "{load_error}" } button { class: "btn", onclick: move |_| refresh += 1, "重试" } } }
                else if let Some(group) = current_group.clone() {
                    GroupDetail { task_ids: current_tasks.iter().map(|t| t.id.clone()).collect::<Vec<_>>(), queue_busy: !status_known() || build_status().build_in_progress, key: "{group.id}", group: group.clone(), project: settings().projects.into_iter().find(|project| project.id == group.project_id), definitions: settings().param_definitions, on_edit: move |group| group_editor.set(Some((false, group))), on_delete: move |group| delete_group.set(Some(group)), on_saved: move |_| refresh += 1 }
                    div { class: if selected_count == 0 { "batch-bar is-empty" } else { "batch-bar" },
                            strong { "本组已选 {selected_count} 项" }
                            button { class: "btn btn--sm btn--primary", disabled: selected_count == 0 || !blocked.is_empty(), title: blocked, onclick: move |_| { let ids = current_ids(); let message = format!("已提交本组勾选的 {} 项任务", ids.len()); start_tasks(ids, format!("group:{}", selected_group()), message, context, build_status, action_busy); }, Icon { width: 14, height: 14, icon: LdPlay } if action_busy() == format!("group:{}", group.id) { "提交中…" } else { "启动本组已选" } }
                            button { class: "btn btn--sm", disabled: selected_count == 0 || preps().is_empty() || !blocked.is_empty(), onclick: move |_| { batch_prep_result.set(None); if let Some(prep) = preps().first() { batch_prep_id.set(prep.id.clone()); batch_prep_values.set(default_prep_values(prep)); } batch_prep_open.set(true); }, Icon { width: 14, height: 14, icon: LdHammer } "执行准备项目" }
                            button { class: "btn btn--sm btn--danger", disabled: selected_count == 0 || build_status().build_in_progress, onclick: move |_| confirm_batch_delete.set(true), Icon { width: 14, height: 14, icon: LdTrash2 } "删除" }
                            if !poll_error().is_empty() { span { class: "tag tag--err", title: "{poll_error}", "状态连接中断" } }
                            span { class: "spacer" }
                            button { class: "btn btn--sm", disabled: selection_locked || selected_count == 0, onclick: move |_| { let ids = current_ids(); selected_tasks.write().retain(|id| !ids.contains(id)); }, "清空本组勾选" }
                            button { class: "btn btn--sm btn--icon", title: "刷新配置", "aria-label": "刷新配置", disabled: loading(), onclick: move |_| refresh += 1, Icon { width: 16, height: 16, icon: LdRefreshCw } }
                            button { class: "btn btn--sm btn--primary", onclick: move |_| { task_editor.set(Some((true, PackageTask { task_group_id: group.id.clone(), group: group.name.clone(), build_args_json: "{}".to_owned(), dead_code_injection_count: 200, ..PackageTask::default() }))); }, Icon { width: 15, height: 15, icon: LdPlus } "新建任务" }
                    }
                    TaskTable { selection_locked, tasks: current_tasks, statuses: build_status, selected: selected_tasks, busy: blocked.to_owned(), submitting: action_busy, on_stop: move |id| stop_target.set(Some(id)), on_edit: move |task| task_editor.set(Some((false, task))), on_delete: move |task| delete_task.set(Some(task)), on_cleanup: move |task| cleanup_task.set(Some(task)), on_refresh: move |_| refresh += 1 }
                } else { div { class: "empty-state", strong { "还没有任务组" } p { "先在左侧为项目创建任务组，再添加打包任务。" } button { class: "btn btn--sm", disabled: loading(), onclick: move |_| refresh += 1, Icon { width: 16, height: 16, icon: LdRefreshCw } "刷新配置" } } }
            }
        }

        if let Some((is_new, group)) = group_editor() { GroupEditor { is_new, value: group, projects: settings().projects, groups: settings().task_groups, definitions: settings().param_definitions, on_cancel: move |_| group_editor.set(None), on_saved: move |group: TaskGroup| { ui.select_group(group.id); group_editor.set(None); refresh += 1; } } }
        if let Some((is_new, task)) = task_editor() { TaskEditor { is_new, value: task, groups: settings().task_groups, preps: preps(), dark: (context.dark)(), on_cancel: move |_| task_editor.set(None), on_saved: move |_| { task_editor.set(None); refresh += 1; } } }

        if let Some(group) = delete_group() { ConfirmDialog { title: "删除任务组".to_owned(), message: format!("确认删除「{}」及组内 {} 个任务？任务目录也会被清理。", group.name, settings().package_tasks.iter().filter(|task| task.task_group_id == group.id).count()), confirm_label: "删除任务组".to_owned(), danger: true, on_cancel: move |_| delete_group.set(None), on_confirm: move |_| { let id = group.id.clone(); delete_group.set(None); spawn(async move { match api::delete(&format!("/api/task-groups/{id}")).await { Ok(()) => { selected_group.set(String::new()); ui.draft.set(TaskGroup::default()); ui.baseline.set(TaskGroup::default()); context.success("任务组已删除"); refresh += 1; }, Err(error) => context.error(error) } }); } } }
        if let Some(task) = delete_task() { ConfirmDialog { title: "删除任务".to_owned(), message: format!("确认删除「{}」及其本地任务目录？", task.name), confirm_label: "删除任务".to_owned(), danger: true, on_cancel: move |_| delete_task.set(None), on_confirm: move |_| { let id = task.id.clone(); delete_task.set(None); spawn(async move { match api::delete(&format!("/api/package-tasks/{id}")).await { Ok(()) => { selected_tasks.write().retain(|task_id| task_id != &id); context.success("任务已删除"); refresh += 1; }, Err(error) => context.error(error) } }); } } }
        if confirm_batch_delete() { ConfirmDialog { title: "批量删除任务".to_owned(), message: format!("确认删除本组 {selected_count} 个任务及其任务目录？\n{}", settings().package_tasks.iter().filter(|task| current_ids().contains(&task.id)).map(|task| task.name.clone()).collect::<Vec<_>>().join("、")), confirm_label: "全部删除".to_owned(), danger: true, on_cancel: move |_| confirm_batch_delete.set(false), on_confirm: move |_| { let ids = current_ids(); confirm_batch_delete.set(false); spawn(async move { for id in &ids { if let Err(error) = api::delete(&format!("/api/package-tasks/{id}")).await { context.error(error); return; } } selected_tasks.write().retain(|id| !ids.contains(id)); context.success("已删除所选任务"); refresh += 1; }); } } }
        if let Some(task) = cleanup_task() { ConfirmDialog { title: "清理任务私有仓库".to_owned(), message: format!("确认丢弃「{}」代码包和资源包仓库内的全部本地改动？", task.name), confirm_label: "清理全部改动".to_owned(), danger: true, on_cancel: move |_| cleanup_task.set(None), on_confirm: move |_| { let id = task.id.clone(); cleanup_task.set(None); spawn(async move { match api::post::<Value, _>(&format!("/api/package-tasks/{id}/cleanup-private-repos"), &json!({})).await { Ok(_) => context.success("任务私有仓库已清理"), Err(error) => context.error(error) } }); } } }
        if let Some(id) = stop_target() { ConfirmDialog { title: "停止整个构建队列".to_owned(), message: "将停止当前任务，并取消队列中所有后续任务。".to_owned(), confirm_label: "停止整个队列".to_owned(), danger: true, on_cancel: move |_| stop_target.set(None), on_confirm: move |_| { stop_target.set(None); stop_task(id.clone(), context, build_status, action_busy); } } }
        if batch_prep_open() { BatchPrepDialog { preps: preps(), selected_tasks: current_ids(), prep_id: batch_prep_id, values: batch_prep_values, result: batch_prep_result, on_close: move |_| batch_prep_open.set(false) } }
    }
}

#[component]
fn ProjectGroups(
    project: Project,
    groups: Vec<TaskGroup>,
    tasks: Vec<PackageTask>,
    statuses: BuildStatusResponse,
    selected_group: Signal<String>,
    selections: Vec<String>,
    on_new: EventHandler<String>,
) -> Element {
    let ui = use_context::<PackageUi>();
    let mut project_groups = groups
        .into_iter()
        .filter(|group| group.project_id == project.id)
        .collect::<Vec<_>>();
    project_groups.sort_by_key(|group| group.order);
    rsx! {
        section { class: "tree-project",
            div { class: "tree-project__head", span { "{project.name}" } span { class: "spacer" } button { class: "btn btn--ghost btn--icon", title: "为项目新建任务组", onclick: move |_| on_new.call(project.id.clone()), Icon { width: 14, height: 14, icon: LdPlus } } }
            if project_groups.is_empty() { p { class: "tree-empty", "暂无任务组" } }
            for group in project_groups {
                { let group_tasks = tasks.iter().filter(|task| task.task_group_id == group.id).collect::<Vec<_>>(); let running = group_tasks.iter().any(|task| matches!(task_runtime(task, &statuses).0, TaskStatus::Running | TaskStatus::Canceling)); rsx! {
                    button { class: if selected_group() == group.id { "tree__group is-active" } else { "tree__group" }, onclick: move |_| ui.select_group(group.id.clone()),
                        span { class: if running { "status-dot status-dot--run" } else { "status-dot" } }
                        span { class: "tree__group-name", "{group.name}" }
                        span { class: "tag tag--mono", "{group_tasks.iter().filter(|task| selections.contains(&task.id)).count()}/{group_tasks.len()} 已选" }
                    }
                } }
            }
        }
    }
}

#[component]
fn GroupDetail(
    group: TaskGroup,
    task_ids: Vec<String>,
    queue_busy: bool,
    project: Option<Project>,
    definitions: Vec<ParamDefinition>,
    on_edit: EventHandler<TaskGroup>,
    on_delete: EventHandler<TaskGroup>,
    on_saved: EventHandler<MouseEvent>,
) -> Element {
    let context = use_context::<AppContext>();
    let mut ui = use_context::<PackageUi>();
    let mut draft = ui.draft;
    let saving = ui.saving;
    let saves = use_context::<crate::schemes::SchemeSaves>();
    let editing_disabled = saving() || (ui.managing)() || queue_busy || !saves.ready(&group.id);
    use_hook({
        let group = group.clone();
        move || {
            ui.draft.set(group.clone());
            ui.baseline.set(group);
        }
    });
    use_effect(use_reactive((&group,), move |(group,)| {
        let clean = {
            let a = ui.draft.peek();
            let b = ui.baseline.peek();
            a.branch == b.branch && a.params == b.params
        };
        if clean && !*ui.saving.peek() {
            ui.draft.set(group.clone());
            ui.baseline.set(group);
        }
    }));
    let mut config_open = use_signal(|| false);
    let project_name = project
        .as_ref()
        .map(|project| project.name.as_str())
        .unwrap_or("未知项目");
    let visible_definitions = definitions
        .iter()
        .filter(|definition| !group.hidden_params.iter().any(|key| key == &definition.key))
        .cloned()
        .collect::<Vec<_>>();
    let hidden_count = group.hidden_params.len();
    let edit_group = group.clone();
    let delete_group = group.clone();
    let dialog_group = group.clone();
    rsx! {
        section { class: "group-detail",
            div { class: "group-detail__head",
                div { h2 { "{group.name}" } p { class: "hint", "{project_name}" if !group.description.is_empty() { " · {group.description}" } } }
                crate::schemes::Schemes { task_ids, queue_busy }
                span { class: "spacer" }
                button { class: "btn btn--sm", disabled: (ui.managing)() || ui.dirty() || saving(), title: "修改组信息前请先保存或放弃组参数", onclick: move |_| on_edit.call(edit_group.clone()), Icon { width: 14, height: 14, icon: LdPencil } "编辑组" }
                button { class: "btn btn--sm btn--danger btn--icon", title: "删除任务组", disabled: (ui.managing)() || saving(), onclick: move |_| on_delete.call(delete_group.clone()), Icon { width: 14, height: 14, icon: LdTrash2 } }
            }
            fieldset { class: "group-detail__form", disabled: editing_disabled,
                BranchPicker { project: project.clone(), value: draft().branch, disabled: saving(), on_change: move |value| draft.write().branch = value }
                for definition in visible_definitions { GroupParamField { definition, draft } }
                if hidden_count > 0 { p { class: "hint group-detail__hidden-hint", "已隐藏 {hidden_count} 个参数，值保留并参与构建；可在参数配置中调整。" } }
            }
            div { class: "group-detail__presets",
                div { class: "preset-head",
                    strong { "参数预设" }
                    span { class: "spacer" }
                    button { class: "btn btn--sm", disabled: editing_disabled || ui.dirty(), onclick: move |_| config_open.set(true), Icon { width: 14, height: 14, icon: LdSettings } "参数配置" }
                }
                div { class: "preset-list",
                    for preset in group.presets.iter().cloned() {
                        { let is_applied = !ui.dirty() && !preset.params.is_empty() && preset.params.iter().all(|(key, value)| draft().params.get(key) == Some(value)); rsx! {
                            button { class: if is_applied { "preset-chip is-applied" } else { "preset-chip" }, title: "点击切换为「{preset.name}」", disabled: editing_disabled || ui.dirty(), onclick: { let preset = preset.clone(); move |event| {
                                for (key, value) in &preset.params { draft.write().params.insert(key.clone(), value.clone()); }
                                let name = preset.name.clone();
                                spawn(async move { match ui.save().await { Ok(()) => { context.success(format!("已切换为「{name}」")); on_saved.call(event); }, Err(error) => context.error(error) } });
                            } }, if is_applied { span { class: "preset-chip__check" } } "{preset.name}" }
                        } }
                    }
                }
            div { class: "group-detail__save",
                    if ui.dirty() { span { class: "tag tag--err", "未保存" } }
                    button { class: "btn", disabled: editing_disabled || !ui.dirty(), onclick: move |_| draft.set((ui.baseline)()), "放弃修改" }
                    button { class: "btn btn--primary", disabled: editing_disabled || draft().branch.trim().is_empty() || !ui.dirty(), onclick: move |event| { spawn(async move { match ui.save().await { Ok(()) => { context.success("任务组参数已保存"); on_saved.call(event); }, Err(error) => context.error(error) } }); }, if saving() { "保存中…" } else { "保存参数" } }
                }
            }
        }
        if config_open() {
            GroupConfigDialog { group: dialog_group, draft, definitions, on_close: move |_| config_open.set(false), on_saved: move |event| { config_open.set(false); on_saved.call(event); } }
        }
    }
}

#[component]
fn GroupConfigDialog(
    group: TaskGroup,
    draft: Signal<TaskGroup>,
    definitions: Vec<ParamDefinition>,
    on_close: EventHandler<MouseEvent>,
    on_saved: EventHandler<MouseEvent>,
) -> Element {
    let context = use_context::<AppContext>();
    let mut ui = use_context::<PackageUi>();
    let mut presets = use_signal(|| group.presets.clone());
    let mut hidden = use_signal(|| group.hidden_params.clone());
    let mut saving = use_signal(|| false);
    rsx! {
        div { class: "overlay is-open", onclick: move |event| on_close.call(event) }
        section { class: "modal modal--config is-open", role: "dialog", "aria-label": "参数配置",
            div { class: "drawer__head", "参数配置" span { class: "tag tag--mono", "{group.name}" } span { class: "spacer" } button { class: "btn btn--ghost btn--icon", onclick: move |event| on_close.call(event), Icon { width: 17, height: 17, icon: LdX } } }
            div { class: "drawer__body",
                div { class: "section-heading", strong { "显示参数" } span { class: "hint", "隐藏的参数保留值并继续参与构建" } }
                div { class: "visibility-list",
                    for definition in &definitions {
                        { let is_hidden = hidden().iter().any(|key| key == &definition.key); let key = definition.key.clone(); rsx! {
                            label { class: "checkbox visibility-row", input { r#type: "checkbox", checked: !is_hidden, onchange: move |event| { if event.checked() { hidden.write().retain(|hidden| hidden != &key); } else if !hidden().iter().any(|hidden| hidden == &key) { hidden.write().push(key.clone()); } } } span { "{definition.label}" } span { class: "tag tag--mono", "{definition.key}" } span { class: "tag", "{definition.kind.label()}" } }
                        } }
                    }
                    if definitions.is_empty() { p { class: "hint", "还没有参数定义，请先在设置页添加。" } }
                }
                div { class: "section-heading", strong { "参数预设" } span { class: "hint", "点击预设即一键切换组参数" } span { class: "spacer" } button { class: "btn btn--sm", onclick: move |_| { let params = draft().params.clone(); let name = format!("预设 {}", presets().len() + 1); presets.write().push(GroupParamPreset { id: String::new(), name, params }); }, Icon { width: 14, height: 14, icon: LdPlus } "添加预设" } }
                if presets().is_empty() { p { class: "hint", "暂无预设" } }
                for (index, preset) in presets().iter().cloned().enumerate() {
                    div { class: "preset-editor", key: "{preset.id}-{index}",
                        div { class: "preset-editor__head",
                            input { class: "input input--mono", value: "{preset.name}", placeholder: "预设名称", oninput: move |event| presets.write()[index].name = event.value() }
                            span { class: "tag tag--mono", "参与 {preset.params.len()} 个参数" }
                            span { class: "spacer" }
                            button { class: "btn btn--sm btn--danger btn--icon", title: "删除预设", onclick: move |_| { presets.write().remove(index); }, Icon { width: 14, height: 14, icon: LdTrash2 } }
                        }
                        for definition in &definitions {
                            PresetParamField { definition: definition.clone(), presets, index }
                        }
                    }
                }
            }
            div { class: "drawer__foot",
                button { class: "btn", onclick: move |event| on_close.call(event), "取消" }
                button { class: "btn btn--primary", disabled: saving(), onclick: move |event| {
                    let id = draft().id;
                    let request = TaskGroupRequest {
                        project_id: draft().project_id,
                        name: draft().name,
                        description: draft().description,
                        branch: (ui.baseline)().branch,
                        params: (ui.baseline)().params,
                        copy_from_group_id: None,
                        presets: Some(presets()),
                        hidden_params: Some(hidden()),
                    };
                    spawn(async move {
                        saving.set(true);
                        match api::put::<TaskGroup, _>(&format!("/api/task-groups/{id}"), &request).await {
                            Ok(group) => { ui.baseline.set(group.clone()); ui.draft.set(group); context.success("参数配置已保存"); on_saved.call(event); },
                            Err(error) => context.error(error),
                        }
                        saving.set(false);
                    });
                }, if saving() { "保存中…" } else { "保存配置" } }
            }
        }
    }
}

#[component]
fn PresetParamField(
    definition: ParamDefinition,
    presets: Signal<Vec<GroupParamPreset>>,
    index: usize,
) -> Element {
    let included = presets()[index].params.contains_key(&definition.key);
    let value = presets()[index]
        .params
        .get(&definition.key)
        .cloned()
        .unwrap_or(definition.default_value.clone());
    let rendered = value_text(&value);
    rsx! { div { class: "preset-param",
        label { class: "checkbox preset-param__include", input { r#type: "checkbox", checked: included, onchange: { let definition = definition.clone(); move |event| { if event.checked() { presets.write()[index].params.insert(definition.key.clone(), definition.default_value.clone()); } else { presets.write()[index].params.remove(&definition.key); } } } } "参与" }
        div { class: "preset-param__control",
            label { class: "field__label", "{definition.label}" span { class: "tag tag--mono", "{definition.key}" } }
            if !included { p { class: "hint", "未参与此预设，切换时保持当前值" } }
            else if definition.kind == ParamKind::Switch { label { class: "switch", input { r#type: "checkbox", checked: value.as_bool().unwrap_or(false), onchange: { let definition = definition.clone(); move |event| { presets.write()[index].params.insert(definition.key.clone(), Value::Bool(event.checked())); } } } i {} } }
            else if definition.kind == ParamKind::Select || (definition.kind == ParamKind::Identifier && !definition.options.is_empty()) { select { class: "select", value: "{rendered}", onchange: { let definition = definition.clone(); move |event| { presets.write()[index].params.insert(definition.key.clone(), Value::String(event.value())); } }, for option in definition.options.iter() { option { key: "{option}", value: "{option}", selected: option == &rendered, "{option}" } } } }
            else { input { class: "input input--mono", value: "{rendered}", oninput: { let definition = definition.clone(); move |event| { let value = if definition.kind == ParamKind::Number { parse_number_value(&event.value()) } else { Value::String(event.value()) }; presets.write()[index].params.insert(definition.key.clone(), value); } } } }
        }
    } }
}

#[component]
fn GroupParamField(definition: ParamDefinition, draft: Signal<TaskGroup>) -> Element {
    let value = draft()
        .params
        .get(&definition.key)
        .cloned()
        .unwrap_or(definition.default_value.clone());
    let rendered = value_text(&value);
    let version_key = definition.key.clone();
    let version_prefix = rendered
        .rsplit_once('.')
        .map(|(prefix, _)| prefix)
        .unwrap_or("")
        .to_owned();
    let help = format!(
        "{}{}{}{}",
        definition.kind.label(),
        if definition.required {
            " · 必填"
        } else {
            " · 可选"
        },
        match definition.kind {
            ParamKind::Version => format!(
                "\n至少两段数字，以点分隔；保留前导零。\n{version_key}_prefix：{version_prefix}"
            ),
            ParamKind::Identifier => "\n只允许英文字母和下划线；保存时自动转为小写。".into(),
            _ => String::new(),
        },
        if definition.description.is_empty() {
            String::new()
        } else {
            format!("\n{}", definition.description)
        }
    );
    rsx! { div { class: if definition.kind == ParamKind::Switch { "field field--switch" } else { "field" }, div { class: "field__label", "{definition.label}" span { class: "tag tag--mono", "{definition.key}" } crate::package_ui::FieldHelp { id: format!("param-help-{}", definition.key), text: help } }
        if definition.kind == ParamKind::Switch { label { class: "switch", input { r#type: "checkbox", checked: value.as_bool().unwrap_or(false), onchange: move |event| { draft.write().params.insert(definition.key.clone(), Value::Bool(event.checked())); } } i {} } }
        else if definition.kind == ParamKind::Select || (definition.kind == ParamKind::Identifier && !definition.options.is_empty()) { select { class: "select", value: "{rendered}", onchange: move |event| { draft.write().params.insert(definition.key.clone(), Value::String(event.value())); }, for option in definition.options { option { key: "{option}", value: "{option}", selected: option == rendered, "{option}" } } } }
        else { input { class: "input input--mono", value: "{rendered}", oninput: move |event| { let value = if definition.kind == ParamKind::Number { parse_number_value(&event.value()) } else { Value::String(event.value()) }; draft.write().params.insert(definition.key.clone(), value); } } }
    } }
}

#[component]
fn TaskTable(
    selection_locked: bool,
    tasks: Vec<PackageTask>,
    statuses: Signal<BuildStatusResponse>,
    selected: Signal<Vec<String>>,
    busy: String,
    submitting: Signal<String>,
    on_stop: EventHandler<String>,
    on_edit: EventHandler<PackageTask>,
    on_delete: EventHandler<PackageTask>,
    on_cleanup: EventHandler<PackageTask>,
    on_refresh: EventHandler<MouseEvent>,
) -> Element {
    if tasks.is_empty() {
        return rsx! { div { class: "empty-state empty-state--tasks", "这个任务组还没有打包任务" } };
    }
    let group_id = tasks[0].task_group_id.clone();
    rsx! { section { class: "table-panel task-table", div { class: "table-scroll", table { class: "table", thead { tr { th { class: "check-cell", input { r#type: "checkbox", disabled: selection_locked, checked: !tasks.is_empty() && tasks.iter().all(|task| selected().contains(&task.id)), onchange: move |event| { if event.checked() { for task in &tasks { if !selected().contains(&task.id) { selected.write().push(task.id.clone()); } } } else { selected.write().retain(|id| !tasks.iter().any(|task| &task.id == id)); } } } } th { "任务" } th { "状态" } th { "进度" } th { class: "table__actions-head", "操作" } } } tbody { for (index, task) in tasks.iter().cloned().enumerate() { TaskRow { selection_locked, index, total: tasks.len(), task, all_tasks: tasks.clone(), statuses, selected, busy: busy.clone(), submitting, on_stop, group_id: group_id.clone(), on_edit, on_delete, on_cleanup, on_refresh } } } } } } }
}

#[component]
fn TaskRow(
    selection_locked: bool,
    index: usize,
    total: usize,
    task: PackageTask,
    all_tasks: Vec<PackageTask>,
    statuses: Signal<BuildStatusResponse>,
    selected: Signal<Vec<String>>,
    busy: String,
    submitting: Signal<String>,
    on_stop: EventHandler<String>,
    group_id: String,
    on_edit: EventHandler<PackageTask>,
    on_delete: EventHandler<PackageTask>,
    on_cleanup: EventHandler<PackageTask>,
    on_refresh: EventHandler<MouseEvent>,
) -> Element {
    let context = use_context::<AppContext>();
    let (status, progress, step, error) = task_runtime(&task, &statuses());
    let running = status == TaskStatus::Running || status == TaskStatus::Canceling;
    let queued = statuses().queued_task_ids.contains(&task.id);
    let selection_task_id = task.id.clone();
    rsx! { tr { key: "{task.id}",
        td { class: "check-cell", input { r#type: "checkbox", disabled: selection_locked, checked: selected().contains(&task.id), onchange: move |event| { if event.checked() { if !selected().contains(&selection_task_id) { selected.write().push(selection_task_id.clone()); } } else { selected.write().retain(|id| id != &selection_task_id); } } } }
        td { strong { "{task.name}" } if !error.is_empty() { p { class: "task-error", "{error}" } } }
        td { span { class: "tag {status.class()}", if statuses().queued_task_ids.contains(&task.id) { "排队中" } else { "{status.label()}" } } }
        td { div { class: "progress-cell", div { class: "progress", div { style: "width:{progress}%" } } span { class: "progress__label", if step.is_empty() { "{progress}%" } else { "{step} · {progress}%" } } } }
        td { div { class: "table__actions",
            if task.enable_obfuscation { button { class: "btn btn--sm btn--icon", title: "单独执行混淆", disabled: running || !busy.is_empty(), onclick: { let id = task.id.clone(); move |_| run_obfuscation(id.clone(), context, statuses, submitting) }, Icon { width: 14, height: 14, icon: LdSparkles } } }
            if running { button { class: "btn btn--sm", disabled: status == TaskStatus::Canceling || !submitting().is_empty(), onclick: { let id = task.id.clone(); move |_| on_stop.call(id.clone()) }, Icon { width: 14, height: 14, icon: LdCircleStop } "停止整个队列" } }
            else { button { class: "btn btn--sm btn--primary", disabled: !busy.is_empty(), title: "{busy}", onclick: { let id = task.id.clone(); let name = task.name.clone(); move |_| start_tasks(vec![id.clone()], format!("task:{id}"), format!("已提交任务「{name}」"), context, statuses, submitting) }, Icon { width: 14, height: 14, icon: LdPlay } if submitting() == format!("task:{}", task.id) { "提交中…" } else { "启动" } } }
            button { class: "btn btn--sm btn--icon", title: "上移", disabled: index == 0 || running || queued, onclick: { let task_id = task.id.clone(); let tasks = all_tasks.clone(); let group_id = group_id.clone(); move |event| reorder_task(tasks.clone(), &task_id, -1, group_id.clone(), on_refresh, event, context) }, Icon { width: 14, height: 14, icon: LdChevronUp } }
            button { class: "btn btn--sm btn--icon", title: "下移", disabled: index + 1 == total || running || queued, onclick: { let task_id = task.id.clone(); let tasks = all_tasks.clone(); let group_id = group_id.clone(); move |event| reorder_task(tasks.clone(), &task_id, 1, group_id.clone(), on_refresh, event, context) }, Icon { width: 14, height: 14, icon: LdChevronDown } }
            button { class: "btn btn--sm btn--icon", title: "复制", disabled: running || queued, onclick: { let id = task.id.clone(); move |event| duplicate_task(id.clone(), on_refresh, event, context) }, Icon { width: 14, height: 14, icon: LdCopy } }
            button { class: "btn btn--sm btn--icon", title: "编辑", disabled: running || queued, onclick: { let task = task.clone(); move |_| on_edit.call(task.clone()) }, Icon { width: 14, height: 14, icon: LdPencil } }
            button { class: "btn btn--sm", disabled: running || queued, onclick: { let task = task.clone(); move |_| on_cleanup.call(task.clone()) }, "清理仓库" }
            button { class: "btn btn--sm btn--danger btn--icon", title: "删除", disabled: running || queued, onclick: move |_| on_delete.call(task.clone()), Icon { width: 14, height: 14, icon: LdTrash2 } }
        } }
    } }
}

#[component]
fn GroupEditor(
    is_new: bool,
    value: TaskGroup,
    projects: Vec<Project>,
    groups: Vec<TaskGroup>,
    definitions: Vec<ParamDefinition>,
    on_cancel: EventHandler<MouseEvent>,
    on_saved: EventHandler<TaskGroup>,
) -> Element {
    let context = use_context::<AppContext>();
    let id = value.id.clone();
    let mut draft = use_signal(|| value);
    let mut copy_from = use_signal(String::new);
    let mut saving = use_signal(|| false);
    rsx! {
        div { class: "overlay is-open", onclick: move |event| on_cancel.call(event) }
        section { class: "drawer is-open", role: "dialog", "aria-label": if is_new { "新建任务组" } else { "编辑任务组" },
            div { class: "drawer__head", if is_new { "新建任务组" } else { "编辑任务组" } span { class: "spacer" } button { class: "btn btn--ghost btn--icon", onclick: move |event| on_cancel.call(event), Icon { width: 17, height: 17, icon: LdX } } }
            div { class: "drawer__body",
                div { class: "field", label { class: "field__label", "项目" } select { class: "select", value: "{draft().project_id}", disabled: !is_new, onchange: move |event| draft.write().project_id = event.value(), for project in &projects { option { value: "{project.id}", selected: project.id == draft().project_id, "{project.name}" } } } }
                div { class: "field", label { class: "field__label", "任务组名称" } input { class: "input", value: "{draft().name}", oninput: move |event| draft.write().name = event.value() } }
                div { class: "field", label { class: "field__label", "描述" } input { class: "input", value: "{draft().description}", oninput: move |event| draft.write().description = event.value() } }
                BranchPicker { project: projects.iter().find(|project| project.id == draft().project_id).cloned(), value: draft().branch, disabled: saving(), on_change: move |value| draft.write().branch = value }
                if is_new && !groups.is_empty() { div { class: "field", label { class: "field__label", "初始参数" } select { class: "select", value: "{copy_from}", onchange: move |event| copy_from.set(event.value()), option { value: "", selected: copy_from().is_empty(), "使用参数默认值" } for group in groups { option { value: "{group.id}", selected: group.id == copy_from(), "从 {group.name} 复制" } } } } }
            }
            div { class: "drawer__foot", button { class: "btn", onclick: move |event| on_cancel.call(event), "取消" } button { class: "btn btn--primary", disabled: saving() || draft().name.trim().is_empty() || draft().branch.trim().is_empty() || draft().project_id.is_empty(), onclick: move |_| { let request = TaskGroupRequest { project_id: draft().project_id, name: draft().name, description: draft().description, branch: draft().branch, params: if is_new { definitions.iter().map(|definition| (definition.key.clone(), definition.default_value.clone())).collect::<BTreeMap<_, _>>() } else { draft().params }, copy_from_group_id: (!copy_from().is_empty()).then(&*copy_from), presets: None, hidden_params: None }; let id = id.clone(); spawn(async move { saving.set(true); let result = if is_new { api::post::<TaskGroup, _>("/api/task-groups", &request).await } else { api::put::<TaskGroup, _>(&format!("/api/task-groups/{id}"), &request).await }; match result { Ok(group) => { context.success(if is_new { "任务组已创建" } else { "任务组已保存" }); on_saved.call(group); }, Err(error) => context.error(error) } saving.set(false); }); }, if saving() { "保存中…" } else { "保存" } } }
        }
    }
}

// Task editor and prep-action controls are kept below so their state is destroyed with the drawer.
#[component]
fn TaskEditor(
    is_new: bool,
    value: PackageTask,
    groups: Vec<TaskGroup>,
    preps: Vec<PrepProject>,
    dark: bool,
    on_cancel: EventHandler<MouseEvent>,
    on_saved: EventHandler<MouseEvent>,
) -> Element {
    let context = use_context::<AppContext>();
    let id = value.id.clone();
    let pre_build_actions = value.pre_build_actions.clone();
    let post_build_actions = value.post_build_actions.clone();
    let mut draft = use_signal(|| PackageTaskRequest::from_task(&value));
    let pre_actions = use_signal(|| pre_build_actions);
    let post_actions = use_signal(|| post_build_actions);
    let mut saving = use_signal(|| false);
    let json_valid = serde_json::from_str::<Value>(&draft().build_args_json).is_ok();
    rsx! {
        div { class: "overlay is-open", onclick: move |event| on_cancel.call(event) }
        section { class: "drawer drawer--task is-open", role: "dialog", "aria-label": if is_new { "新建任务" } else { "编辑任务" },
            div { class: "drawer__head", if is_new { "新建任务" } else { "编辑任务" } span { class: "spacer" } button { class: "btn btn--ghost btn--icon", onclick: move |event| on_cancel.call(event), Icon { width: 17, height: 17, icon: LdX } } }
            div { class: "drawer__body task-editor",
                details { open: true, summary { "基础配置" } div { class: "details-body form-grid form-grid--two",
                    div { class: "field", label { class: "field__label", "任务名称" } input { class: "input", value: "{draft().name}", oninput: move |event| draft.write().name = event.value() } }
                    div { class: "field", label { class: "field__label", "任务组" } select { class: "select", value: "{draft().task_group_id}", onchange: move |event| draft.write().task_group_id = event.value(), for group in &groups { option { value: "{group.id}", selected: group.id == draft().task_group_id, "{group.name}" } } } }
                    div { class: "field", label { class: "field__label", "代码包 Git 地址" } input { class: "input input--mono", value: "{draft().code_repo_url}", oninput: move |event| draft.write().code_repo_url = event.value() } }
                    div { class: "field", label { class: "field__label", "资源包 Git 地址" } input { class: "input input--mono", value: "{draft().asset_repo_url}", oninput: move |event| draft.write().asset_repo_url = event.value() } }
                } }
                details { open: true, summary { "构建参数 JSON" } div { class: "details-body", PlaceholderHelp {} JsonEditor { value: draft().build_args_json, dark, on_change: move |value| draft.write().build_args_json = value } if !json_valid { p { class: "field-error", "JSON 格式无效，修正后才能保存" } } } }
                details { summary { "混淆与死代码" } div { class: "details-body form-grid form-grid--two",
                    label { class: "checkbox", input { r#type: "checkbox", checked: draft().enable_obfuscation, onchange: move |event| draft.write().enable_obfuscation = event.checked() } "启用混淆" }
                    if draft().enable_obfuscation { div { class: "field", label { class: "field__label", "混淆模式" } select { class: "select", value: if draft().obfuscation_mode == ObfuscationMode::Ast { "ast" } else { "classic" }, onchange: move |event| draft.write().obfuscation_mode = if event.value() == "ast" { ObfuscationMode::Ast } else { ObfuscationMode::Classic }, option { value: "classic", "经典" } option { value: "ast", "AST" } } } }
                    label { class: "checkbox", input { r#type: "checkbox", checked: draft().enable_dead_code_injection, onchange: move |event| draft.write().enable_dead_code_injection = event.checked() } "注入死代码" }
                    if draft().enable_dead_code_injection { div { class: "field", label { class: "field__label", "注入数量" } input { class: "input", r#type: "number", min: "1", value: "{draft().dead_code_injection_count}", oninput: move |event| if let Ok(value) = event.value().parse() { draft.write().dead_code_injection_count = value; } } } }
                } }
                details { summary { "打包前准备" } div { class: "details-body", PlaceholderHelp {} ActionList { actions: pre_actions, preps: preps.clone(), on_change: move |actions| draft.write().pre_build_actions = actions } } }
                details { summary { "打包后准备" } div { class: "details-body", PlaceholderHelp {} ActionList { actions: post_actions, preps, on_change: move |actions| draft.write().post_build_actions = actions } } }
            }
            div { class: "drawer__foot", button { class: "btn", onclick: move |event| on_cancel.call(event), "取消" } button { class: "btn btn--primary", disabled: saving() || draft().name.trim().is_empty() || draft().task_group_id.is_empty() || !json_valid, onclick: move |event| { let mut request = draft(); remove_system_prep_param_values(&mut request.pre_build_actions); remove_system_prep_param_values(&mut request.post_build_actions); let id = id.clone(); spawn(async move { saving.set(true); let result = if is_new { api::post::<PackageTask, _>("/api/package-tasks", &request).await } else { api::put::<PackageTask, _>(&format!("/api/package-tasks/{id}"), &request).await }; match result { Ok(_) => { context.success(if is_new { "任务已创建" } else { "任务已保存" }); on_saved.call(event); }, Err(error) => context.error(error) } saving.set(false); }); }, Icon { width: 15, height: 15, icon: LdSave } if saving() { "保存中…" } else { "保存任务" } } }
        }
    }
}

#[component]
fn JsonEditor(value: String, dark: bool, on_change: EventHandler<String>) -> Element {
    let host = "build-args-editor";
    let textarea = "build-args-source";
    let initial = value.clone();
    use_effect(move || {
        let script = format!(
            "CocosBuildLanEditor.mount({}, {}, {}, {dark});",
            serde_json::to_string(host).unwrap(),
            serde_json::to_string(textarea).unwrap(),
            serde_json::to_string(&initial).unwrap()
        );
        spawn(async move {
            let _ = document::eval(&script).await;
        });
    });
    use_drop(move || {
        spawn(async move {
            let _ = document::eval("CocosBuildLanEditor.destroy('build-args-editor');").await;
        });
    });
    rsx! { div { class: "code-editor-wrap", div { class: "code-editor-toolbar", span { class: "hint", "JSON" } span { class: "spacer" } button { class: "btn btn--sm", onclick: move |_| { spawn(async move { let _ = document::eval("CocosBuildLanEditor.format('build-args-editor');").await; }); }, "格式化" } } div { id: host, class: "code-editor-host" } textarea { id: textarea, class: "code-editor-source", value: "{value}", oninput: move |event| on_change.call(event.value()) } } }
}

#[component]
fn ActionList(
    actions: Signal<Vec<TaskPrepAction>>,
    preps: Vec<PrepProject>,
    on_change: EventHandler<Vec<TaskPrepAction>>,
) -> Element {
    let mut local = actions;
    rsx! { div { class: "action-list",
        if local().is_empty() { div { class: "empty-inline", "没有准备动作" } }
        for (index, action) in local().iter().cloned().enumerate() { ActionEditor { index, action, all: local, preps: preps.clone(), on_change } }
        button { class: "btn btn--sm", disabled: preps.is_empty(), onclick: move |_| { let prep_id = preps.first().map(|prep| prep.id.clone()).unwrap_or_default(); local.write().push(TaskPrepAction::Single { prep_project_id: prep_id, params: HashMap::new() }); on_change.call(local()); }, Icon { width: 14, height: 14, icon: LdPlus } "添加动作" }
    } }
}

#[component]
fn ActionEditor(
    index: usize,
    action: TaskPrepAction,
    all: Signal<Vec<TaskPrepAction>>,
    preps: Vec<PrepProject>,
    on_change: EventHandler<Vec<TaskPrepAction>>,
) -> Element {
    let step = index + 1;
    let action_kind = if matches!(&action, TaskPrepAction::Conditional { .. }) {
        "conditional"
    } else {
        "single"
    };
    let default_preps = preps.clone();
    rsx! { div { class: "action-item",
        div { class: "action-item__head", strong { "步骤 {step}" } select { class: "select select--compact", value: action_kind, onchange: move |event| { all.write()[index] = if event.value() == "conditional" { TaskPrepAction::Conditional { condition_source: String::new(), condition_equals: String::new(), on_match_targets: Vec::new(), on_mismatch_targets: Vec::new() } } else { TaskPrepAction::Single { prep_project_id: default_preps.first().map(|prep| prep.id.clone()).unwrap_or_default(), params: HashMap::new() } }; on_change.call(all()); }, option { value: "single", "执行项目" } option { value: "conditional", "条件分支" } } span { class: "spacer" } button { class: "btn btn--sm btn--danger btn--icon", title: "删除动作", onclick: move |_| { all.write().remove(index); on_change.call(all()); }, Icon { width: 14, height: 14, icon: LdTrash2 } } }
        match action {
            TaskPrepAction::Single { prep_project_id, params } => rsx! { PrepTargetEditor { index, target_index: 0, branch: "single", prep_project_id, params, all, preps: preps.clone(), on_change } },
            TaskPrepAction::Conditional { condition_source, condition_equals, on_match_targets, on_mismatch_targets } => rsx! {
                div { class: "form-grid form-grid--two", div { class: "field", label { class: "field__label", "条件来源" } input { class: "input input--mono", value: "{condition_source}", oninput: move |event| { if let TaskPrepAction::Conditional { condition_source, .. } = &mut all.write()[index] { *condition_source = event.value(); } on_change.call(all()); } } } div { class: "field", label { class: "field__label", "等于" } input { class: "input input--mono", value: "{condition_equals}", oninput: move |event| { if let TaskPrepAction::Conditional { condition_equals, .. } = &mut all.write()[index] { *condition_equals = event.value(); } on_change.call(all()); } } } }
                TargetBranch { action_index: index, label: "匹配时", branch: "match", targets: on_match_targets, all, preps: preps.clone(), on_change }
                TargetBranch { action_index: index, label: "不匹配时", branch: "mismatch", targets: on_mismatch_targets, all, preps: preps.clone(), on_change }
            },
        }
    } }
}

#[component]
fn TargetBranch(
    action_index: usize,
    label: &'static str,
    branch: &'static str,
    targets: Vec<TaskPrepTarget>,
    all: Signal<Vec<TaskPrepAction>>,
    preps: Vec<PrepProject>,
    on_change: EventHandler<Vec<TaskPrepAction>>,
) -> Element {
    rsx! { div { class: "target-branch", div { class: "section-heading section-heading--mini", strong { "{label}" } span { class: "spacer" } button { class: "btn btn--sm", onclick: move |_| { let target = TaskPrepTarget { prep_project_id: preps.first().map(|prep| prep.id.clone()).unwrap_or_default(), params: HashMap::new() }; if let TaskPrepAction::Conditional { on_match_targets, on_mismatch_targets, .. } = &mut all.write()[action_index] { if branch == "match" { on_match_targets.push(target); } else { on_mismatch_targets.push(target); } } on_change.call(all()); }, Icon { width: 13, height: 13, icon: LdPlus } "添加" } }
        if targets.is_empty() { p { class: "hint", "无执行项目" } }
        for (target_index, target) in targets.into_iter().enumerate() { PrepTargetEditor { index: action_index, target_index, branch, prep_project_id: target.prep_project_id, params: target.params, all, preps: preps.clone(), on_change } }
    } }
}

#[component]
fn PrepTargetEditor(
    index: usize,
    target_index: usize,
    branch: &'static str,
    prep_project_id: String,
    params: HashMap<String, Value>,
    all: Signal<Vec<TaskPrepAction>>,
    preps: Vec<PrepProject>,
    on_change: EventHandler<Vec<TaskPrepAction>>,
) -> Element {
    let selected_prep = preps
        .iter()
        .find(|prep| prep.id == prep_project_id)
        .cloned();
    rsx! { div { class: "prep-target",
        select { class: "select", value: "{prep_project_id}", onchange: move |event| { update_target(&mut all.write()[index], branch, target_index, |id, values| { *id = event.value(); values.clear(); }); on_change.call(all()); }, option { value: "", selected: prep_project_id.is_empty(), "请选择准备项目" } for prep in &preps { option { value: "{prep.id}", selected: prep.id == prep_project_id, "{prep.name}" } } }
        if branch != "single" { button { class: "btn btn--sm btn--danger btn--icon", title: "移除", onclick: move |_| { if let TaskPrepAction::Conditional { on_match_targets, on_mismatch_targets, .. } = &mut all.write()[index] { let targets = if branch == "match" { on_match_targets } else { on_mismatch_targets }; if target_index < targets.len() { targets.remove(target_index); } } on_change.call(all()); }, Icon { width: 13, height: 13, icon: LdX } } }
        if let Some(prep) = selected_prep { for parameter in prep.params.into_iter().filter(PrepParam::is_user_runtime) { ActionParam { action_index: index, target_index, branch, parameter: parameter.clone(), value: params.get(&parameter.name).cloned().unwrap_or(Value::Null), all, on_change } } }
    } }
}

#[component]
fn ActionParam(
    action_index: usize,
    target_index: usize,
    branch: &'static str,
    parameter: PrepParam,
    value: Value,
    all: Signal<Vec<TaskPrepAction>>,
    on_change: EventHandler<Vec<TaskPrepAction>>,
) -> Element {
    let text = value_text(&value);
    rsx! { div { class: "field prep-target__param", label { class: "field__label", "{parameter.name}" }
        if parameter.param_type == PrepParamType::Bool { label { class: "switch", input { r#type: "checkbox", checked: value.as_bool().unwrap_or(false), onchange: move |event| { update_target(&mut all.write()[action_index], branch, target_index, |_, params| { params.insert(parameter.name.clone(), Value::Bool(event.checked())); }); on_change.call(all()); } } i {} } }
        else if parameter.param_type == PrepParamType::Select { select { class: "select", value: "{text}", onchange: move |event| { update_target(&mut all.write()[action_index], branch, target_index, |_, params| { params.insert(parameter.name.clone(), Value::String(event.value())); }); on_change.call(all()); }, for option in parameter.options { option { value: "{option.value}", selected: option.value == text, "{option.label}" } } } }
        else { input { class: "input input--mono", value: "{text}", oninput: move |event| { let value = if parameter.param_type == PrepParamType::Int { event.value().parse::<i64>().map(Value::from).unwrap_or(Value::String(event.value())) } else { Value::String(event.value()) }; update_target(&mut all.write()[action_index], branch, target_index, |_, params| { params.insert(parameter.name.clone(), value); }); on_change.call(all()); } } }
    } }
}

#[component]
fn BatchPrepDialog(
    preps: Vec<PrepProject>,
    selected_tasks: Vec<String>,
    prep_id: Signal<String>,
    values: Signal<HashMap<String, Value>>,
    result: Signal<Option<PrepTaskRunResponse>>,
    on_close: EventHandler<MouseEvent>,
) -> Element {
    let context = use_context::<AppContext>();
    let mut running = use_signal(|| false);
    let prep = preps.iter().find(|prep| prep.id == prep_id()).cloned();
    rsx! {
        div { class: "overlay is-open", onclick: move |event| on_close.call(event) }
        section { class: "drawer is-open", role: "dialog", "aria-label": "批量执行准备项目",
            div { class: "drawer__head", "批量执行准备项目" span { class: "tag tag--mono", "{selected_tasks.len()} 个任务" } span { class: "spacer" } button { class: "btn btn--ghost btn--icon", onclick: move |event| on_close.call(event), Icon { width: 17, height: 17, icon: LdX } } }
            div { class: "drawer__body",
                div { class: "field", label { class: "field__label", "准备项目" }
                    select { class: "select", value: "{prep_id}", onchange: move |event| { prep_id.set(event.value()); if let Some(prep) = preps.iter().find(|prep| prep.id == prep_id()) { values.set(default_prep_values(prep)); } }, for prep in &preps { option { value: "{prep.id}", selected: prep.id == prep_id(), "{prep.name}" } } }
                }
                p { class: "hint mono", "project_path 由每个任务的目标项目自动注入" }
                if let Some(prep) = prep { for parameter in prep.params.into_iter().filter(PrepParam::is_user_runtime) { BatchPrepParam { parameter, values } } }
                if let Some(result) = result() {
                    div { class: if result.failed_count == 0 { "result-panel result-panel--ok" } else { "result-panel result-panel--error" },
                        strong { "共 {result.total_count} 项 · 成功 {result.success_count} / 失败 {result.failed_count}" }
                        for item in result.results {
                            details { class: "batch-result",
                                summary { span { "{item.task_name} · {item.project_name}" } span { class: if item.success { "tag tag--ok" } else { "tag tag--err" }, if item.success { "成功" } else { "失败" } } span { class: "tag tag--mono", "exit {item.exit_code}" } }
                                div { class: "batch-result__body",
                                    p { class: "hint mono", "目录 · {item.project_path}" }
                                    p { class: "mono", "{item.command}" }
                                    if !item.stdout.is_empty() { label { class: "field__label", "stdout" } pre { class: "term", "{item.stdout}" } }
                                    if !item.stderr.is_empty() { label { class: "field__label", "stderr" } pre { class: "term term--error", "{item.stderr}" } }
                                    if let Some(error) = item.error_message { p { class: "field-error", "{error}" } }
                                    span { class: "hint mono", "任务 ID · {item.task_id}" }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "drawer__foot", button { class: "btn", onclick: move |event| on_close.call(event), "关闭" } button { class: "btn btn--primary", disabled: running() || prep_id().is_empty(), onclick: move |_| { let id = prep_id(); let request = PrepRunForTasksRequest { task_ids: selected_tasks.clone(), params: values() }; spawn(async move { running.set(true); match api::post::<PrepTaskRunResponse, _>(&format!("/api/prep-projects/{id}/run-for-tasks"), &request).await { Ok(value) => result.set(Some(value)), Err(error) => context.error(error) } running.set(false); }); }, if running() { "执行中…" } else { "开始执行" } } }
        }
    }
}

#[component]
fn BatchPrepParam(parameter: PrepParam, values: Signal<HashMap<String, Value>>) -> Element {
    let value = values()
        .get(&parameter.name)
        .cloned()
        .unwrap_or(Value::Null);
    let text = value_text(&value);
    rsx! { div { class: "field", label { class: "field__label", "{parameter.name}" } if parameter.param_type == PrepParamType::Bool { label { class: "switch", input { r#type: "checkbox", checked: value.as_bool().unwrap_or(false), onchange: move |event| { values.write().insert(parameter.name.clone(), Value::Bool(event.checked())); } } i {} } } else if parameter.param_type == PrepParamType::Select { select { class: "select", value: "{text}", onchange: move |event| { values.write().insert(parameter.name.clone(), Value::String(event.value())); }, for option in parameter.options { option { value: "{option.value}", selected: option.value == text, "{option.label}" } } } } else { input { class: "input input--mono", value: "{text}", oninput: move |event| { let value = if parameter.param_type == PrepParamType::Int { event.value().parse::<i64>().map(Value::from).unwrap_or(Value::String(event.value())) } else { Value::String(event.value()) }; values.write().insert(parameter.name.clone(), value); } } } } }
}

fn selected_in_group(settings: &PublicSettings, selected: &[String], group: &str) -> Vec<String> {
    let mut tasks = settings
        .package_tasks
        .iter()
        .filter(|task| task.task_group_id == group && selected.contains(&task.id))
        .collect::<Vec<_>>();
    tasks.sort_by_key(|task| task.order);
    tasks.into_iter().map(|task| task.id.clone()).collect()
}

fn ordered_selection(settings: &PublicSettings, selected: &[String]) -> Vec<String> {
    let mut result = vec![];
    for project in &settings.projects {
        let mut groups = settings
            .task_groups
            .iter()
            .filter(|group| group.project_id == project.id)
            .collect::<Vec<_>>();
        groups.sort_by_key(|group| group.order);
        for group in groups {
            result.extend(selected_in_group(settings, selected, &group.id));
        }
    }
    result
}

fn task_runtime(
    task: &PackageTask,
    statuses: &BuildStatusResponse,
) -> (TaskStatus, u8, String, String) {
    statuses
        .package_tasks
        .iter()
        .find(|runtime| runtime.task_id == task.id)
        .map(|runtime| {
            (
                runtime.status.clone(),
                runtime.progress,
                runtime.step_label.clone(),
                runtime.last_error.clone().unwrap_or_default(),
            )
        })
        .unwrap_or((
            task.status.clone(),
            task.progress,
            String::new(),
            task.last_error.clone().unwrap_or_default(),
        ))
}

fn start_tasks(
    ids: Vec<String>,
    source: String,
    message: String,
    context: AppContext,
    mut status: Signal<BuildStatusResponse>,
    mut busy: Signal<String>,
) {
    if !busy.peek().is_empty() || status.peek().build_in_progress {
        return;
    }
    if ids.is_empty() {
        context.error("请先选择任务");
        return;
    }
    busy.set(source);
    spawn(async move {
        match api::post::<Value, _>("/api/build/start", &json!({"taskIds": ids})).await {
            Ok(_) => {
                context.success(message);
                if let Ok(value) = api::get("/api/build/status").await {
                    status.set(value);
                }
            }
            Err(error) => context.error(error),
        }
        busy.set(String::new());
    });
}

fn stop_task(
    id: String,
    context: AppContext,
    mut status: Signal<BuildStatusResponse>,
    mut busy: Signal<String>,
) {
    if !busy.peek().is_empty() {
        return;
    }
    busy.set("stop".into());
    spawn(async move {
        match api::post::<Value, _>("/api/build/stop", &json!({"taskId": id})).await {
            Ok(_) => {
                status.write().cancel_requested = true;
                context.success("已请求停止整个构建队列");
            }
            Err(error) => context.error(error),
        }
        busy.set(String::new());
    });
}

fn run_obfuscation(
    id: String,
    context: AppContext,
    mut status: Signal<BuildStatusResponse>,
    mut busy: Signal<String>,
) {
    if !busy.peek().is_empty() || status.peek().build_in_progress {
        return;
    }
    busy.set("obfuscation".into());
    spawn(async move {
        match api::post_empty(
            &format!("/api/package-tasks/{id}/run-obfuscation"),
            &json!({}),
        )
        .await
        {
            Ok(()) => {
                status.write().build_in_progress = true;
                context.success("已启动单独混淆");
            }
            Err(error) => context.error(error),
        }
        busy.set(String::new());
    });
}

fn duplicate_task(
    id: String,
    on_refresh: EventHandler<MouseEvent>,
    event: MouseEvent,
    context: AppContext,
) {
    spawn(async move {
        match api::post::<PackageTask, _>(&format!("/api/package-tasks/{id}/duplicate"), &json!({}))
            .await
        {
            Ok(_) => {
                context.success("任务副本已创建");
                on_refresh.call(event);
            }
            Err(error) => context.error(error),
        }
    });
}

fn reorder_task(
    mut tasks: Vec<PackageTask>,
    task_id: &str,
    offset: isize,
    group_id: String,
    on_refresh: EventHandler<MouseEvent>,
    event: MouseEvent,
    context: AppContext,
) {
    let Some(index) = tasks.iter().position(|task| task.id == task_id) else {
        return;
    };
    let target = index as isize + offset;
    if target < 0 || target >= tasks.len() as isize {
        return;
    }
    tasks.swap(index, target as usize);
    let ids = tasks.into_iter().map(|task| task.id).collect::<Vec<_>>();
    spawn(async move {
        match api::put::<Vec<PackageTask>, _>(
            "/api/package-tasks/reorder",
            &json!({"taskGroupId": group_id, "taskIds": ids}),
        )
        .await
        {
            Ok(_) => on_refresh.call(event),
            Err(error) => context.error(error),
        }
    });
}

fn update_target(
    action: &mut TaskPrepAction,
    branch: &str,
    index: usize,
    update: impl FnOnce(&mut String, &mut HashMap<String, Value>),
) {
    match action {
        TaskPrepAction::Single {
            prep_project_id,
            params,
        } => update(prep_project_id, params),
        TaskPrepAction::Conditional {
            on_match_targets,
            on_mismatch_targets,
            ..
        } => {
            let targets = if branch == "match" {
                on_match_targets
            } else {
                on_mismatch_targets
            };
            if let Some(target) = targets.get_mut(index) {
                update(&mut target.prep_project_id, &mut target.params);
            }
        }
    }
}

fn default_prep_values(prep: &PrepProject) -> HashMap<String, Value> {
    prep.params
        .iter()
        .filter(|parameter| parameter.is_user_runtime())
        .map(|parameter| {
            let value = match parameter.param_type {
                PrepParamType::Bool => Value::Bool(false),
                PrepParamType::Int => json!(0),
                PrepParamType::Select => parameter
                    .options
                    .first()
                    .map(|option| Value::String(option.value.clone()))
                    .unwrap_or(Value::Null),
                PrepParamType::Str => Value::String(String::new()),
            };
            (parameter.name.clone(), value)
        })
        .collect()
}

fn remove_system_prep_param_values(actions: &mut [TaskPrepAction]) {
    for action in actions {
        match action {
            TaskPrepAction::Single { params, .. } => {
                params.remove("project_path");
            }
            TaskPrepAction::Conditional {
                on_match_targets,
                on_mismatch_targets,
                ..
            } => {
                for target in on_match_targets.iter_mut().chain(on_mismatch_targets) {
                    target.params.remove("project_path");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BuildTaskStatus;

    #[test]
    fn live_build_status_overrides_persisted_task_state() {
        let task = PackageTask {
            id: "task-a".to_owned(),
            status: TaskStatus::Pending,
            ..PackageTask::default()
        };
        let statuses = BuildStatusResponse {
            package_tasks: vec![BuildTaskStatus {
                task_id: task.id.clone(),
                progress: 42,
                step_label: "拉取分支".to_owned(),
                status: TaskStatus::Running,
                ..BuildTaskStatus::default()
            }],
            ..BuildStatusResponse::default()
        };

        let (status, progress, step, error) = task_runtime(&task, &statuses);
        assert_eq!(status, TaskStatus::Running);
        assert_eq!(progress, 42);
        assert_eq!(step, "拉取分支");
        assert!(error.is_empty());
    }

    #[test]
    fn updates_only_the_selected_conditional_target() {
        let mut action = TaskPrepAction::Conditional {
            condition_source: "channel".to_owned(),
            condition_equals: "release".to_owned(),
            on_match_targets: vec![TaskPrepTarget {
                prep_project_id: "old".to_owned(),
                params: HashMap::new(),
            }],
            on_mismatch_targets: Vec::new(),
        };

        update_target(&mut action, "match", 0, |id, params| {
            *id = "new".to_owned();
            params.insert("enabled".to_owned(), Value::Bool(true));
        });

        let TaskPrepAction::Conditional {
            on_match_targets, ..
        } = action
        else {
            panic!("expected conditional action");
        };
        assert_eq!(on_match_targets[0].prep_project_id, "new");
        assert_eq!(
            on_match_targets[0].params.get("enabled"),
            Some(&json!(true))
        );
    }

    #[test]
    fn prep_defaults_and_saved_actions_exclude_project_path() {
        let prep = PrepProject {
            params: vec![
                PrepParam {
                    name: "project_path".to_owned(),
                    ..PrepParam::default()
                },
                PrepParam {
                    name: "enabled".to_owned(),
                    param_type: PrepParamType::Bool,
                    ..PrepParam::default()
                },
            ],
            ..PrepProject::default()
        };
        let defaults = default_prep_values(&prep);
        assert!(!defaults.contains_key("project_path"));
        assert_eq!(defaults.get("enabled"), Some(&Value::Bool(false)));

        let values = HashMap::from([
            ("project_path".to_owned(), json!("C:/user-selected")),
            ("enabled".to_owned(), json!(true)),
        ]);
        let mut actions = vec![
            TaskPrepAction::Single {
                prep_project_id: "prep-1".to_owned(),
                params: values.clone(),
            },
            TaskPrepAction::Conditional {
                condition_source: "channel".to_owned(),
                condition_equals: "release".to_owned(),
                on_match_targets: vec![TaskPrepTarget {
                    prep_project_id: "prep-1".to_owned(),
                    params: values,
                }],
                on_mismatch_targets: Vec::new(),
            },
        ];

        remove_system_prep_param_values(&mut actions);

        let TaskPrepAction::Single { params, .. } = &actions[0] else {
            panic!("expected single action");
        };
        assert!(!params.contains_key("project_path"));
        assert_eq!(params.get("enabled"), Some(&json!(true)));
        let TaskPrepAction::Conditional {
            on_match_targets, ..
        } = &actions[1]
        else {
            panic!("expected conditional action");
        };
        assert!(!on_match_targets[0].params.contains_key("project_path"));
        assert_eq!(
            on_match_targets[0].params.get("enabled"),
            Some(&json!(true))
        );
    }
    #[test]
    fn group_actions_never_include_hidden_selections_and_global_order_matches_sidebar() {
        let settings = PublicSettings {
            projects: vec![Project {
                id: "p".into(),
                ..Default::default()
            }],
            task_groups: vec![
                TaskGroup {
                    id: "b".into(),
                    project_id: "p".into(),
                    order: 2,
                    ..Default::default()
                },
                TaskGroup {
                    id: "a".into(),
                    project_id: "p".into(),
                    order: 1,
                    ..Default::default()
                },
            ],
            package_tasks: vec![
                PackageTask {
                    id: "b1".into(),
                    task_group_id: "b".into(),
                    ..Default::default()
                },
                PackageTask {
                    id: "a2".into(),
                    task_group_id: "a".into(),
                    order: 2,
                    ..Default::default()
                },
                PackageTask {
                    id: "a1".into(),
                    task_group_id: "a".into(),
                    order: 1,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let selected = vec![
            "b1".into(),
            "a2".into(),
            "a1".into(),
            "removed".into(),
            "a1".into(),
        ];
        assert_eq!(selected_in_group(&settings, &selected, "b"), vec!["b1"]);
        assert_eq!(
            ordered_selection(&settings, &selected),
            vec!["a1", "a2", "b1"]
        );
    }
}
