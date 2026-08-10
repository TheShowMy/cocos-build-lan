use std::collections::{BTreeMap, HashMap};

use dioxus::{document, prelude::*};
use dioxus_free_icons::{
    Icon,
    icons::ld_icons::{
        LdChevronDown, LdChevronUp, LdCircleStop, LdCopy, LdGitBranch, LdHammer, LdPencil, LdPlay,
        LdPlus, LdRefreshCw, LdSave, LdSparkles, LdTrash2, LdX,
    },
};
use gloo_timers::future::TimeoutFuture;
use serde_json::{Value, json};

use crate::{
    AppContext, ConfirmDialog, api,
    models::{
        BuildStatusResponse, ObfuscationMode, PackageTask, PackageTaskRequest, ParamDefinition,
        ParamKind, PrepParam, PrepParamType, PrepProject, PrepRunForTasksRequest,
        PrepTaskRunResponse, Project, ProjectBranchesResponse, PublicSettings, TaskGroup,
        TaskGroupParamsRequest, TaskGroupRequest, TaskPrepAction, TaskPrepTarget, TaskStatus,
        parse_number_value, value_text,
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
    let mut selected_group = use_signal(String::new);
    let mut selected_tasks = use_signal(Vec::<String>::new);
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
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            match api::get::<PublicSettings>("/api/settings").await {
                Ok(value) => {
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
                    poll_error.set(String::new());
                }
                Err(error) => poll_error.set(error),
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
    let selected_count = selected_tasks().len();

    rsx! {
        div { class: "page package-page",
            aside { class: "page__side package-tree",
                div { class: "tree-head", strong { "任务组" } span { class: "spacer" } button { class: "btn btn--sm btn--icon", title: "新建任务组", disabled: settings().projects.is_empty(), onclick: move |_| { let project_id = settings().projects.first().map(|project| project.id.clone()).unwrap_or_default(); group_editor.set(Some((true, TaskGroup { project_id, branch: "main".to_owned(), ..TaskGroup::default() }))); }, Icon { width: 15, height: 15, icon: LdPlus } } }
                if settings().projects.is_empty() { div { class: "empty-inline", "请先在设置页添加项目" } }
                for project in settings().projects {
                    ProjectGroups { project, groups: settings().task_groups, tasks: settings().package_tasks, statuses: build_status(), selected_group, on_new: move |project_id| group_editor.set(Some((true, TaskGroup { project_id, branch: "main".to_owned(), ..TaskGroup::default() }))) }
                }
            }
            main { class: "page__main package-main",
                div { class: "toolbar",
                    div { h1 { "打包任务" } p { class: "toolbar__subtitle", if let Some(group) = &current_group { "{group.name} · {current_tasks.len()} 个任务" } else { "选择或创建任务组" } } }
                    if !poll_error().is_empty() { span { class: "tag tag--err", title: "{poll_error}", "状态连接中断" } }
                    span { class: "spacer" }
                    button { class: "btn btn--icon", title: "刷新配置", disabled: loading(), onclick: move |_| refresh += 1, Icon { width: 16, height: 16, icon: LdRefreshCw } }
                    button { class: "btn btn--primary", disabled: current_group.is_none(), onclick: move |_| if let Some(group) = current_group.clone() { task_editor.set(Some((true, PackageTask { task_group_id: group.id, group: group.name, build_args_json: "{}".to_owned(), dead_code_injection_count: 200, ..PackageTask::default() }))); }, Icon { width: 15, height: 15, icon: LdPlus } "新建任务" }
                }
                if loading() && settings().task_groups.is_empty() { div { class: "empty-state", "正在加载打包配置…" } }
                else if !load_error().is_empty() { div { class: "error-state", p { "{load_error}" } button { class: "btn", onclick: move |_| refresh += 1, "重试" } } }
                else if let Some(group) = current_group.clone() {
                    GroupDetail { key: "{group.id}", group: group.clone(), project: settings().projects.into_iter().find(|project| project.id == group.project_id), definitions: settings().param_definitions, on_edit: move |group| group_editor.set(Some((false, group))), on_delete: move |group| delete_group.set(Some(group)), on_saved: move |_| refresh += 1 }
                    if selected_count > 0 {
                        div { class: "batch-bar",
                            strong { "已选择 {selected_count} 项" }
                            button { class: "btn btn--sm btn--primary", onclick: move |_| start_tasks(selected_tasks(), context, build_status), Icon { width: 14, height: 14, icon: LdPlay } "启动" }
                            button { class: "btn btn--sm", disabled: preps().is_empty(), onclick: move |_| { batch_prep_result.set(None); if let Some(prep) = preps().first() { batch_prep_id.set(prep.id.clone()); batch_prep_values.set(default_prep_values(prep)); } batch_prep_open.set(true); }, Icon { width: 14, height: 14, icon: LdHammer } "执行准备项目" }
                            button { class: "btn btn--sm btn--danger", onclick: move |_| confirm_batch_delete.set(true), Icon { width: 14, height: 14, icon: LdTrash2 } "删除" }
                            span { class: "spacer" }
                            button { class: "btn btn--sm", onclick: move |_| selected_tasks.set(Vec::new()), "取消选择" }
                        }
                    }
                    TaskTable { tasks: current_tasks, statuses: build_status, selected: selected_tasks, busy: action_busy(), on_edit: move |task| task_editor.set(Some((false, task))), on_delete: move |task| delete_task.set(Some(task)), on_cleanup: move |task| cleanup_task.set(Some(task)), on_refresh: move |_| refresh += 1 }
                } else { div { class: "empty-state", strong { "还没有任务组" } p { "先在左侧为项目创建任务组，再添加打包任务。" } } }
            }
        }

        if let Some((is_new, group)) = group_editor() { GroupEditor { is_new, value: group, projects: settings().projects, groups: settings().task_groups, definitions: settings().param_definitions, on_cancel: move |_| group_editor.set(None), on_saved: move |group: TaskGroup| { selected_group.set(group.id); group_editor.set(None); refresh += 1; } } }
        if let Some((is_new, task)) = task_editor() { TaskEditor { is_new, value: task, groups: settings().task_groups, preps: preps(), dark: (context.dark)(), on_cancel: move |_| task_editor.set(None), on_saved: move |_| { task_editor.set(None); refresh += 1; } } }

        if let Some(group) = delete_group() { ConfirmDialog { title: "删除任务组".to_owned(), message: format!("确认删除「{}」及组内 {} 个任务？任务目录也会被清理。", group.name, settings().package_tasks.iter().filter(|task| task.task_group_id == group.id).count()), confirm_label: "删除任务组".to_owned(), danger: true, on_cancel: move |_| delete_group.set(None), on_confirm: move |_| { let id = group.id.clone(); delete_group.set(None); spawn(async move { match api::delete(&format!("/api/task-groups/{id}")).await { Ok(()) => { selected_group.set(String::new()); selected_tasks.set(Vec::new()); context.success("任务组已删除"); refresh += 1; }, Err(error) => context.error(error) } }); } } }
        if let Some(task) = delete_task() { ConfirmDialog { title: "删除任务".to_owned(), message: format!("确认删除「{}」及其本地任务目录？", task.name), confirm_label: "删除任务".to_owned(), danger: true, on_cancel: move |_| delete_task.set(None), on_confirm: move |_| { let id = task.id.clone(); delete_task.set(None); spawn(async move { match api::delete(&format!("/api/package-tasks/{id}")).await { Ok(()) => { selected_tasks.write().retain(|task_id| task_id != &id); context.success("任务已删除"); refresh += 1; }, Err(error) => context.error(error) } }); } } }
        if confirm_batch_delete() { ConfirmDialog { title: "批量删除任务".to_owned(), message: format!("确认删除已选择的 {selected_count} 个任务及其任务目录？"), confirm_label: "全部删除".to_owned(), danger: true, on_cancel: move |_| confirm_batch_delete.set(false), on_confirm: move |_| { let ids = selected_tasks(); confirm_batch_delete.set(false); spawn(async move { for id in &ids { if let Err(error) = api::delete(&format!("/api/package-tasks/{id}")).await { context.error(error); return; } } selected_tasks.set(Vec::new()); context.success("已删除所选任务"); refresh += 1; }); } } }
        if let Some(task) = cleanup_task() { ConfirmDialog { title: "清理任务私有仓库".to_owned(), message: format!("确认丢弃「{}」代码包和资源包仓库内的全部本地改动？", task.name), confirm_label: "清理全部改动".to_owned(), danger: true, on_cancel: move |_| cleanup_task.set(None), on_confirm: move |_| { let id = task.id.clone(); cleanup_task.set(None); spawn(async move { match api::post::<Value, _>(&format!("/api/package-tasks/{id}/cleanup-private-repos"), &json!({})).await { Ok(_) => context.success("任务私有仓库已清理"), Err(error) => context.error(error) } }); } } }
        if batch_prep_open() { BatchPrepDialog { preps: preps(), selected_tasks: selected_tasks(), prep_id: batch_prep_id, values: batch_prep_values, result: batch_prep_result, on_close: move |_| batch_prep_open.set(false) } }
    }
}

#[component]
fn ProjectGroups(
    project: Project,
    groups: Vec<TaskGroup>,
    tasks: Vec<PackageTask>,
    statuses: BuildStatusResponse,
    selected_group: Signal<String>,
    on_new: EventHandler<String>,
) -> Element {
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
                { let group_tasks = tasks.iter().filter(|task| task.task_group_id == group.id).collect::<Vec<_>>(); let running = group_tasks.iter().any(|task| task_runtime(task, &statuses).0 == TaskStatus::Running); rsx! {
                    button { class: if selected_group() == group.id { "tree__group is-active" } else { "tree__group" }, onclick: move |_| selected_group.set(group.id.clone()),
                        span { class: if running { "status-dot status-dot--run" } else { "status-dot" } }
                        span { class: "tree__group-name", "{group.name}" }
                        span { class: "tag tag--mono", "{group_tasks.len()}" }
                    }
                } }
            }
        }
    }
}

#[component]
fn GroupDetail(
    group: TaskGroup,
    project: Option<Project>,
    definitions: Vec<ParamDefinition>,
    on_edit: EventHandler<TaskGroup>,
    on_delete: EventHandler<TaskGroup>,
    on_saved: EventHandler<MouseEvent>,
) -> Element {
    let context = use_context::<AppContext>();
    let mut draft = use_signal(|| group.clone());
    let mut saving = use_signal(|| false);
    let mut branches = use_signal(Vec::<String>::new);
    let mut branch_loading = use_signal(|| false);
    let branch_project_id = group.project_id.clone();
    let project_name = project
        .as_ref()
        .map(|project| project.name.as_str())
        .unwrap_or("未知项目");
    let load_branches = move |_| {
        let project_id = branch_project_id.clone();
        spawn(async move {
            branch_loading.set(true);
            let path = format!(
                "/api/project-branches?projectId={}",
                api::encode_query(&project_id)
            );
            match api::get::<ProjectBranchesResponse>(&path).await {
                Ok(value) => branches.set(value.branches),
                Err(error) => context.error(error),
            }
            branch_loading.set(false);
        });
    };
    rsx! {
        section { class: "group-detail",
            div { class: "group-detail__head",
                div { h2 { "{group.name}" } p { class: "hint", "{project_name}" if !group.description.is_empty() { " · {group.description}" } } }
                span { class: "spacer" }
                button { class: "btn btn--sm", onclick: { let group = group.clone(); move |_| on_edit.call(group.clone()) }, Icon { width: 14, height: 14, icon: LdPencil } "编辑组" }
                button { class: "btn btn--sm btn--danger btn--icon", title: "删除任务组", onclick: move |_| on_delete.call(group.clone()), Icon { width: 14, height: 14, icon: LdTrash2 } }
            }
            div { class: "group-detail__form",
                div { class: "field field--span-all", label { class: "field__label", "Git 分支" }
                    div { class: "input-action", if branches().is_empty() { input { class: "input input--mono", value: "{draft().branch}", oninput: move |event| draft.write().branch = event.value() } } else { select { class: "select select--mono", value: "{draft().branch}", onchange: move |event| draft.write().branch = event.value(), for branch in branches() { option { value: "{branch}", "{branch}" } } } } button { class: "btn btn--icon", title: "拉取远程分支", disabled: branch_loading(), onclick: load_branches, Icon { width: 15, height: 15, icon: LdGitBranch } } }
                }
                for definition in definitions { GroupParamField { definition, draft } }
                div { class: "group-detail__save", button { class: "btn btn--primary", disabled: saving() || draft().branch.trim().is_empty(), onclick: move |event| { let id = draft().id; let request = TaskGroupParamsRequest { branch: draft().branch, params: draft().params }; spawn(async move { saving.set(true); match api::put::<TaskGroup, _>(&format!("/api/task-groups/{id}/params"), &request).await { Ok(_) => { context.success("任务组参数已保存"); on_saved.call(event); }, Err(error) => context.error(error) } saving.set(false); }); }, Icon { width: 15, height: 15, icon: LdSave } if saving() { "保存中…" } else { "保存组参数" } } }
            }
        }
    }
}

#[component]
fn GroupParamField(definition: ParamDefinition, draft: Signal<TaskGroup>) -> Element {
    let value = draft()
        .params
        .get(&definition.key)
        .cloned()
        .unwrap_or(definition.default_value.clone());
    let rendered = value_text(&value);
    rsx! { div { class: "field", label { class: "field__label", "{definition.label}" span { class: "tag tag--mono", "{definition.key}" } }
        if definition.kind == ParamKind::Switch { label { class: "switch", input { r#type: "checkbox", checked: value.as_bool().unwrap_or(false), onchange: move |event| { draft.write().params.insert(definition.key.clone(), Value::Bool(event.checked())); } } i {} } }
        else if definition.kind == ParamKind::Select { select { class: "select", value: "{rendered}", onchange: move |event| { draft.write().params.insert(definition.key.clone(), Value::String(event.value())); }, for option in definition.options { option { key: "{option}", value: "{option}", selected: option == rendered, "{option}" } } } }
        else { input { class: "input input--mono", value: "{rendered}", oninput: move |event| { let value = if definition.kind == ParamKind::Number { parse_number_value(&event.value()) } else { Value::String(event.value()) }; draft.write().params.insert(definition.key.clone(), value); } } }
        if !definition.description.is_empty() { p { class: "hint", "{definition.description}" } }
    } }
}

#[component]
fn TaskTable(
    tasks: Vec<PackageTask>,
    statuses: Signal<BuildStatusResponse>,
    selected: Signal<Vec<String>>,
    busy: String,
    on_edit: EventHandler<PackageTask>,
    on_delete: EventHandler<PackageTask>,
    on_cleanup: EventHandler<PackageTask>,
    on_refresh: EventHandler<MouseEvent>,
) -> Element {
    if tasks.is_empty() {
        return rsx! { div { class: "empty-state empty-state--tasks", "这个任务组还没有打包任务" } };
    }
    let group_id = tasks[0].task_group_id.clone();
    rsx! { section { class: "table-panel task-table", div { class: "table-scroll", table { class: "table", thead { tr { th { class: "check-cell", input { r#type: "checkbox", checked: !tasks.is_empty() && tasks.iter().all(|task| selected().contains(&task.id)), onchange: move |event| { if event.checked() { selected.set(tasks.iter().map(|task| task.id.clone()).collect()); } else { selected.set(Vec::new()); } } } } th { "任务" } th { "状态" } th { "进度" } th { class: "table__actions-head", "操作" } } } tbody { for (index, task) in tasks.iter().cloned().enumerate() { TaskRow { index, total: tasks.len(), task, all_tasks: tasks.clone(), statuses, selected, busy: busy.clone(), group_id: group_id.clone(), on_edit, on_delete, on_cleanup, on_refresh } } } } } } }
}

#[component]
fn TaskRow(
    index: usize,
    total: usize,
    task: PackageTask,
    all_tasks: Vec<PackageTask>,
    statuses: Signal<BuildStatusResponse>,
    selected: Signal<Vec<String>>,
    busy: String,
    group_id: String,
    on_edit: EventHandler<PackageTask>,
    on_delete: EventHandler<PackageTask>,
    on_cleanup: EventHandler<PackageTask>,
    on_refresh: EventHandler<MouseEvent>,
) -> Element {
    let context = use_context::<AppContext>();
    let (status, progress, step, error) = task_runtime(&task, &statuses());
    let running = status == TaskStatus::Running || status == TaskStatus::Canceling;
    let selection_task_id = task.id.clone();
    rsx! { tr { key: "{task.id}",
        td { class: "check-cell", input { r#type: "checkbox", checked: selected().contains(&task.id), onchange: move |event| { if event.checked() { if !selected().contains(&selection_task_id) { selected.write().push(selection_task_id.clone()); } } else { selected.write().retain(|id| id != &selection_task_id); } } } }
        td { strong { "{task.name}" } if !error.is_empty() { p { class: "task-error", "{error}" } } }
        td { span { class: "tag {status.class()}", "{status.label()}" } }
        td { div { class: "progress-cell", div { class: "progress", div { style: "width:{progress}%" } } span { class: "progress__label", if step.is_empty() { "{progress}%" } else { "{step} · {progress}%" } } } }
        td { div { class: "table__actions",
            if task.enable_obfuscation { button { class: "btn btn--sm btn--icon", title: "单独执行混淆", disabled: running || busy == task.id, onclick: { let id = task.id.clone(); move |_| run_obfuscation(id.clone(), context) }, Icon { width: 14, height: 14, icon: LdSparkles } } }
            if running { button { class: "btn btn--sm", disabled: status == TaskStatus::Canceling, onclick: { let id = task.id.clone(); move |_| stop_task(id.clone(), context) }, Icon { width: 14, height: 14, icon: LdCircleStop } "停止" } }
            else { button { class: "btn btn--sm btn--primary", onclick: { let id = task.id.clone(); move |_| start_tasks(vec![id.clone()], context, statuses) }, Icon { width: 14, height: 14, icon: LdPlay } "启动" } }
            button { class: "btn btn--sm btn--icon", title: "上移", disabled: index == 0 || running, onclick: { let task_id = task.id.clone(); let tasks = all_tasks.clone(); let group_id = group_id.clone(); move |event| reorder_task(tasks.clone(), &task_id, -1, group_id.clone(), on_refresh, event, context) }, Icon { width: 14, height: 14, icon: LdChevronUp } }
            button { class: "btn btn--sm btn--icon", title: "下移", disabled: index + 1 == total || running, onclick: { let task_id = task.id.clone(); let tasks = all_tasks.clone(); let group_id = group_id.clone(); move |event| reorder_task(tasks.clone(), &task_id, 1, group_id.clone(), on_refresh, event, context) }, Icon { width: 14, height: 14, icon: LdChevronDown } }
            button { class: "btn btn--sm btn--icon", title: "复制", disabled: running, onclick: { let id = task.id.clone(); move |event| duplicate_task(id.clone(), on_refresh, event, context) }, Icon { width: 14, height: 14, icon: LdCopy } }
            button { class: "btn btn--sm btn--icon", title: "编辑", disabled: running, onclick: { let task = task.clone(); move |_| on_edit.call(task.clone()) }, Icon { width: 14, height: 14, icon: LdPencil } }
            button { class: "btn btn--sm", disabled: running, onclick: { let task = task.clone(); move |_| on_cleanup.call(task.clone()) }, "清理仓库" }
            button { class: "btn btn--sm btn--danger btn--icon", title: "删除", disabled: running, onclick: move |_| on_delete.call(task.clone()), Icon { width: 14, height: 14, icon: LdTrash2 } }
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
    let mut branches = use_signal(Vec::<String>::new);
    let mut loading_branches = use_signal(|| false);
    let mut saving = use_signal(|| false);
    let mut branch_refresh = use_signal(|| 0u64);
    use_effect(move || {
        let project_id = draft().project_id;
        let _ = branch_refresh();
        if project_id.is_empty() {
            return;
        }
        spawn(async move {
            loading_branches.set(true);
            match api::get::<ProjectBranchesResponse>(&format!(
                "/api/project-branches?projectId={}",
                api::encode_query(&project_id)
            ))
            .await
            {
                Ok(value) => branches.set(value.branches),
                Err(error) => context.error(error),
            }
            loading_branches.set(false);
        });
    });
    rsx! {
        div { class: "overlay is-open", onclick: move |event| on_cancel.call(event) }
        section { class: "drawer is-open", role: "dialog", "aria-label": if is_new { "新建任务组" } else { "编辑任务组" },
            div { class: "drawer__head", if is_new { "新建任务组" } else { "编辑任务组" } span { class: "spacer" } button { class: "btn btn--ghost btn--icon", onclick: move |event| on_cancel.call(event), Icon { width: 17, height: 17, icon: LdX } } }
            div { class: "drawer__body",
                div { class: "field", label { class: "field__label", "项目" } select { class: "select", value: "{draft().project_id}", disabled: !is_new, onchange: move |event| draft.write().project_id = event.value(), for project in projects { option { value: "{project.id}", "{project.name}" } } } }
                div { class: "field", label { class: "field__label", "任务组名称" } input { class: "input", value: "{draft().name}", oninput: move |event| draft.write().name = event.value() } }
                div { class: "field", label { class: "field__label", "描述" } input { class: "input", value: "{draft().description}", oninput: move |event| draft.write().description = event.value() } }
                div { class: "field", label { class: "field__label", "Git 分支" } div { class: "input-action", if branches().is_empty() { input { class: "input input--mono", value: "{draft().branch}", oninput: move |event| draft.write().branch = event.value() } } else { select { class: "select select--mono", value: "{draft().branch}", onchange: move |event| draft.write().branch = event.value(), for branch in branches() { option { value: "{branch}", "{branch}" } } } } button { class: "btn btn--icon", title: "重新拉取分支", disabled: loading_branches(), onclick: move |_| branch_refresh += 1, Icon { width: 15, height: 15, icon: LdRefreshCw } } } }
                if is_new && !groups.is_empty() { div { class: "field", label { class: "field__label", "初始参数" } select { class: "select", value: "{copy_from}", onchange: move |event| copy_from.set(event.value()), option { value: "", "使用参数默认值" } for group in groups { option { value: "{group.id}", "从 {group.name} 复制" } } } } }
            }
            div { class: "drawer__foot", button { class: "btn", onclick: move |event| on_cancel.call(event), "取消" } button { class: "btn btn--primary", disabled: saving() || draft().name.trim().is_empty() || draft().branch.trim().is_empty() || draft().project_id.is_empty(), onclick: move |_| { let request = TaskGroupRequest { project_id: draft().project_id, name: draft().name, description: draft().description, branch: draft().branch, params: if is_new { definitions.iter().map(|definition| (definition.key.clone(), definition.default_value.clone())).collect::<BTreeMap<_, _>>() } else { draft().params }, copy_from_group_id: (!copy_from().is_empty()).then(&*copy_from) }; let id = id.clone(); spawn(async move { saving.set(true); let result = if is_new { api::post::<TaskGroup, _>("/api/task-groups", &request).await } else { api::put::<TaskGroup, _>(&format!("/api/task-groups/{id}"), &request).await }; match result { Ok(group) => { context.success(if is_new { "任务组已创建" } else { "任务组已保存" }); on_saved.call(group); }, Err(error) => context.error(error) } saving.set(false); }); }, if saving() { "保存中…" } else { "保存" } } }
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
                    div { class: "field", label { class: "field__label", "任务组" } select { class: "select", value: "{draft().task_group_id}", onchange: move |event| draft.write().task_group_id = event.value(), for group in &groups { option { value: "{group.id}", "{group.name}" } } } }
                    div { class: "field", label { class: "field__label", "代码包 Git 地址" } input { class: "input input--mono", value: "{draft().code_repo_url}", oninput: move |event| draft.write().code_repo_url = event.value() } }
                    div { class: "field", label { class: "field__label", "资源包 Git 地址" } input { class: "input input--mono", value: "{draft().asset_repo_url}", oninput: move |event| draft.write().asset_repo_url = event.value() } }
                } }
                details { open: true, summary { "构建参数 JSON" } div { class: "details-body", JsonEditor { value: draft().build_args_json, dark, on_change: move |value| draft.write().build_args_json = value } if !json_valid { p { class: "field-error", "JSON 格式无效，修正后才能保存" } } } }
                details { summary { "混淆与死代码" } div { class: "details-body form-grid form-grid--two",
                    label { class: "checkbox", input { r#type: "checkbox", checked: draft().enable_obfuscation, onchange: move |event| draft.write().enable_obfuscation = event.checked() } "启用混淆" }
                    if draft().enable_obfuscation { div { class: "field", label { class: "field__label", "混淆模式" } select { class: "select", value: if draft().obfuscation_mode == ObfuscationMode::Ast { "ast" } else { "classic" }, onchange: move |event| draft.write().obfuscation_mode = if event.value() == "ast" { ObfuscationMode::Ast } else { ObfuscationMode::Classic }, option { value: "classic", "经典" } option { value: "ast", "AST" } } } }
                    label { class: "checkbox", input { r#type: "checkbox", checked: draft().enable_dead_code_injection, onchange: move |event| draft.write().enable_dead_code_injection = event.checked() } "注入死代码" }
                    if draft().enable_dead_code_injection { div { class: "field", label { class: "field__label", "注入数量" } input { class: "input", r#type: "number", min: "1", value: "{draft().dead_code_injection_count}", oninput: move |event| if let Ok(value) = event.value().parse() { draft.write().dead_code_injection_count = value; } } } }
                } }
                details { summary { "打包前准备" } div { class: "details-body", ActionList { actions: pre_actions, preps: preps.clone(), on_change: move |actions| draft.write().pre_build_actions = actions } } }
                details { summary { "打包后准备" } div { class: "details-body", ActionList { actions: post_actions, preps, on_change: move |actions| draft.write().post_build_actions = actions } } }
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
        select { class: "select", value: "{prep_project_id}", onchange: move |event| { update_target(&mut all.write()[index], branch, target_index, |id, values| { *id = event.value(); values.clear(); }); on_change.call(all()); }, option { value: "", "请选择准备项目" } for prep in &preps { option { value: "{prep.id}", "{prep.name}" } } }
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
        else if parameter.param_type == PrepParamType::Select { select { class: "select", value: "{text}", onchange: move |event| { update_target(&mut all.write()[action_index], branch, target_index, |_, params| { params.insert(parameter.name.clone(), Value::String(event.value())); }); on_change.call(all()); }, for option in parameter.options { option { value: "{option.value}", "{option.label}" } } } }
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
                    select { class: "select", value: "{prep_id}", onchange: move |event| { prep_id.set(event.value()); if let Some(prep) = preps.iter().find(|prep| prep.id == prep_id()) { values.set(default_prep_values(prep)); } }, for prep in &preps { option { value: "{prep.id}", "{prep.name}" } } }
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
    rsx! { div { class: "field", label { class: "field__label", "{parameter.name}" } if parameter.param_type == PrepParamType::Bool { label { class: "switch", input { r#type: "checkbox", checked: value.as_bool().unwrap_or(false), onchange: move |event| { values.write().insert(parameter.name.clone(), Value::Bool(event.checked())); } } i {} } } else if parameter.param_type == PrepParamType::Select { select { class: "select", value: "{text}", onchange: move |event| { values.write().insert(parameter.name.clone(), Value::String(event.value())); }, for option in parameter.options { option { value: "{option.value}", "{option.label}" } } } } else { input { class: "input input--mono", value: "{text}", oninput: move |event| { let value = if parameter.param_type == PrepParamType::Int { event.value().parse::<i64>().map(Value::from).unwrap_or(Value::String(event.value())) } else { Value::String(event.value()) }; values.write().insert(parameter.name.clone(), value); } } } } }
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

fn start_tasks(ids: Vec<String>, context: AppContext, mut status: Signal<BuildStatusResponse>) {
    if ids.is_empty() {
        context.error("请先选择任务");
        return;
    }
    spawn(async move {
        match api::post::<Value, _>("/api/build/start", &json!({"taskIds": ids})).await {
            Ok(_) => {
                context.success("构建任务已启动");
                if let Ok(value) = api::get("/api/build/status").await {
                    status.set(value);
                }
            }
            Err(error) => context.error(error),
        }
    });
}

fn stop_task(id: String, context: AppContext) {
    spawn(async move {
        match api::post::<Value, _>("/api/build/stop", &json!({"taskId": id})).await {
            Ok(_) => context.success("已请求停止当前构建队列"),
            Err(error) => context.error(error),
        }
    });
}

fn run_obfuscation(id: String, context: AppContext) {
    spawn(async move {
        match api::post_empty(
            &format!("/api/package-tasks/{id}/run-obfuscation"),
            &json!({}),
        )
        .await
        {
            Ok(()) => context.success("已启动单独混淆"),
            Err(error) => context.error(error),
        }
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
}
