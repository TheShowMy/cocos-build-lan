use std::collections::HashMap;

use dioxus::{document, prelude::*};
use dioxus_free_icons::{
    Icon,
    icons::ld_icons::{LdDownload, LdPencil, LdPlay, LdPlus, LdRefreshCw, LdTrash2, LdUpload, LdX},
};
use serde_json::{Value, json};

use crate::{
    AppContext, ConfirmDialog, api,
    models::{
        PrepExportPayload, PrepImportRequest, PrepParam, PrepParamOption, PrepParamType,
        PrepProject, PrepRunRequest, PrepRunResponse, PrepSaveRequest, PrepValueSource,
        PublicSettings,
    },
};

#[component]
pub fn Prep() -> Element {
    let context = use_context::<AppContext>();
    let mut preps = use_signal(Vec::<PrepProject>::new);
    let mut projects = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut load_error = use_signal(String::new);
    let mut refresh = use_signal(|| 0u64);
    let mut editor = use_signal(|| None::<PrepProject>);
    let mut editor_is_new = use_signal(|| false);
    let mut run_target = use_signal(|| None::<PrepProject>);
    let mut run_values = use_signal(HashMap::<String, Value>::new);
    let mut run_project_id = use_signal(String::new);
    let mut run_result = use_signal(|| None::<PrepRunResponse>);
    let mut running = use_signal(|| false);
    let mut import_open = use_signal(|| false);
    let mut import_raw = use_signal(String::new);
    let mut import_mode = use_signal(|| "create".to_owned());
    let mut import_target = use_signal(String::new);
    let mut export_data = use_signal(|| None::<(String, String)>);
    let mut delete_target = use_signal(|| None::<PrepProject>);

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            match api::get::<Vec<PrepProject>>("/api/prep-projects").await {
                Ok(value) => {
                    preps.set(value);
                    load_error.set(String::new());
                }
                Err(error) => load_error.set(error),
            }
            if projects().is_empty()
                && let Ok(settings) = api::get::<PublicSettings>("/api/settings").await
            {
                if run_project_id().is_empty()
                    && let Some(project) = settings.projects.first()
                {
                    run_project_id.set(project.id.clone());
                }
                projects.set(settings.projects);
            }
            loading.set(false);
        });
    });

    let open_new = move |_| {
        editor_is_new.set(true);
        editor.set(Some(PrepProject {
            name: "新建准备项目".to_owned(),
            ..PrepProject::default()
        }));
    };

    rsx! {
        main { class: "page__main page__main--wide",
            div { class: "toolbar",
                div { h1 { "打包准备" } p { class: "toolbar__subtitle", "维护可复用的 uv 脚本和运行参数" } }
                span { class: "spacer" }
                button { class: "btn", onclick: move |_| import_open.set(true), Icon { width: 15, height: 15, icon: LdUpload } "导入" }
                button { class: "btn btn--icon", title: "刷新", disabled: loading(), onclick: move |_| refresh += 1, Icon { width: 16, height: 16, icon: LdRefreshCw } }
                button { class: "btn btn--primary", onclick: open_new, Icon { width: 15, height: 15, icon: LdPlus } "新建准备项目" }
            }
            if loading() && preps().is_empty() { div { class: "empty-state", "正在加载准备项目…" } }
            else if !load_error().is_empty() { div { class: "error-state", p { "{load_error}" } button { class: "btn", onclick: move |_| refresh += 1, "重试" } } }
            else if preps().is_empty() { div { class: "empty-state", strong { "还没有准备项目" } p { "新建一个项目后，可在打包前后动作或批量操作中执行。" } } }
            else {
                div { class: "prep-grid", for prep in preps() {
                    article { class: "card prep-card", key: "{prep.id}",
                        div { class: "card__body",
                            div { class: "prep-card__title", strong { "{prep.name}" } span { class: "tag tag--mono", "{prep.params.iter().filter(|parameter| !parameter.is_system()).count()} 参数" } }
                            p { class: "prep-card__desc", if prep.description.is_empty() { "暂无说明" } else { "{prep.description}" } }
                            div { class: "chips", for parameter in prep.params.iter().filter(|parameter| !parameter.is_system()) { span { class: "chip", "{parameter.name}:{param_type_label(&parameter.param_type)}" } } }
                            div { class: "prep-card__path mono", "{prep.path}" }
                        }
                        div { class: "card__foot",
                            span { class: "hint mono", "{prep.create_time}" }
                            span { class: "spacer" }
                            button { class: "btn btn--sm btn--primary", onclick: { let prep = prep.clone(); move |_| { run_values.set(default_run_values(&prep)); run_result.set(None); run_target.set(Some(prep.clone())); } }, Icon { width: 14, height: 14, icon: LdPlay } "运行" }
                            button { class: "btn btn--sm", onclick: { let prep = prep.clone(); move |_| { editor_is_new.set(false); editor.set(Some(prep.clone())); } }, Icon { width: 14, height: 14, icon: LdPencil } "编辑" }
                            button { class: "btn btn--sm", onclick: { let prep = prep.clone(); move |_| export_prep(prep.clone(), export_data, context) }, Icon { width: 14, height: 14, icon: LdDownload } "导出" }
                            button { class: "btn btn--sm btn--danger", title: "删除", onclick: move |_| delete_target.set(Some(prep.clone())), Icon { width: 14, height: 14, icon: LdTrash2 } }
                        }
                    }
                } }
            }
        }
        if let Some(current) = editor() {
            PrepEditor { value: current, is_new: editor_is_new(), on_close: move |_| editor.set(None), on_saved: move |_| { editor.set(None); refresh += 1; } }
        }
        if let Some(prep) = run_target() {
            div { class: "overlay is-open", onclick: move |_| run_target.set(None) }
            section { class: "drawer is-open", role: "dialog", "aria-label": "运行准备项目",
                div { class: "drawer__head", div { strong { "运行 · {prep.name}" } span { class: "hint", "在部署机工作区执行" } } span { class: "spacer" } button { class: "btn btn--ghost btn--icon", onclick: move |_| run_target.set(None), Icon { width: 17, height: 17, icon: LdX } } }
                div { class: "drawer__body",
                    div { class: "field", label { class: "field__label", "目标项目" }
                        select { class: "select", value: "{run_project_id}", onchange: move |event| run_project_id.set(event.value()),
                            option { value: "", selected: run_project_id().is_empty(), "请选择项目" }
                            for project in projects() { option { value: "{project.id}", selected: project.id == run_project_id(), "{project.name}" } }
                        }
                    }
                    p { class: "hint mono", "project_path 由目标项目自动注入" }
                    for parameter in prep.params.iter().filter(|parameter| parameter.is_user_runtime()) {
                        RuntimeParamField { parameter: parameter.clone(), values: run_values }
                    }
                    if let Some(result) = run_result() {
                        div { class: if result.success { "result-panel result-panel--ok" } else { "result-panel result-panel--error" },
                            div { strong { if result.success { "执行成功" } else { "执行失败" } } span { class: "tag tag--mono", "exit {result.exit_code}" } }
                            p { class: "hint mono", "目录 · {result.project_path}" }
                            p { class: "mono", "{result.command}" }
                            if !result.stdout.is_empty() { label { class: "field__label", "stdout" } pre { class: "term", "{result.stdout}" } }
                            if !result.stderr.is_empty() { label { class: "field__label", "stderr" } pre { class: "term term--error", "{result.stderr}" } }
                        }
                    }
                }
                div { class: "drawer__foot", button { class: "btn", onclick: move |_| run_target.set(None), "关闭" } button { class: "btn btn--primary", disabled: running() || run_project_id().is_empty(), onclick: move |_| { let prep_id = prep.id.clone(); let request = PrepRunRequest { project_id: run_project_id(), params: run_values() }; spawn(async move { running.set(true); match api::post::<PrepRunResponse, _>(&format!("/api/prep-projects/{prep_id}/run"), &request).await { Ok(value) => run_result.set(Some(value)), Err(error) => context.error(error) } running.set(false); }); }, Icon { width: 15, height: 15, icon: LdPlay } if running() { "运行中…" } else { "开始运行" } } }
            }
        }
        if import_open() {
            div { class: "overlay is-open", onclick: move |_| import_open.set(false) }
            section { class: "drawer is-open", role: "dialog", "aria-label": "导入准备项目",
                div { class: "drawer__head", "导入准备项目" span { class: "spacer" } button { class: "btn btn--ghost btn--icon", onclick: move |_| import_open.set(false), Icon { width: 17, height: 17, icon: LdX } } }
                div { class: "drawer__body",
                    div { class: "segmented", button { class: if import_mode() == "create" { "is-active" } else { "" }, onclick: move |_| import_mode.set("create".to_owned()), "新建" } button { class: if import_mode() == "update" { "is-active" } else { "" }, onclick: move |_| import_mode.set("update".to_owned()), "覆盖更新" } }
                    if import_mode() == "update" { div { class: "field", label { class: "field__label", "目标准备项目" } select { class: "select", value: "{import_target}", onchange: move |event| import_target.set(event.value()), option { value: "", selected: import_target().is_empty(), "请选择" } for prep in preps() { option { value: "{prep.id}", selected: prep.id == import_target(), "{prep.name}" } } } } }
                    div { class: "field", label { class: "field__label", "导出 JSON" } textarea { class: "textarea textarea--mono textarea--tall", placeholder: "粘贴准备项目导出的 JSON", value: "{import_raw}", oninput: move |event| import_raw.set(event.value()) } }
                }
                div { class: "drawer__foot", button { class: "btn", onclick: move |_| import_open.set(false), "取消" } button { class: "btn btn--primary", disabled: import_raw().trim().is_empty() || (import_mode() == "update" && import_target().is_empty()), onclick: move |_| { let request = PrepImportRequest { raw_text: import_raw(), mode: import_mode(), target_prep_project_id: (import_mode() == "update").then(&*import_target) }; spawn(async move { match api::post::<PrepProject, _>("/api/prep-projects/import", &request).await { Ok(_) => { import_open.set(false); import_raw.set(String::new()); context.success("准备项目已导入"); refresh += 1; }, Err(error) => context.error(error) } }); }, "导入" } }
            }
        }
        if let Some((name, raw)) = export_data() {
            div { class: "overlay is-open", onclick: move |_| export_data.set(None) }
            section { class: "modal is-open", role: "dialog", "aria-label": "导出准备项目",
                div { class: "drawer__head", "导出 · {name}" span { class: "spacer" } button { class: "btn btn--ghost btn--icon", onclick: move |_| export_data.set(None), Icon { width: 17, height: 17, icon: LdX } } }
                div { class: "drawer__body", pre { class: "export-code", "{raw}" } }
                div { class: "drawer__foot", button { class: "btn", onclick: { let raw = raw.clone(); move |_| copy_export(raw.clone(), context) }, "复制" } button { class: "btn btn--primary", onclick: move |_| download_export(name.clone(), raw.clone()), Icon { width: 15, height: 15, icon: LdDownload } "下载 JSON" } }
            }
        }
        if let Some(prep) = delete_target() {
            ConfirmDialog { title: "删除准备项目".to_owned(), message: format!("确认删除「{}」及其脚本目录？该操作不可恢复。", prep.name), confirm_label: "删除".to_owned(), danger: true,
                on_cancel: move |_| delete_target.set(None),
                on_confirm: move |_| { let id = prep.id.clone(); delete_target.set(None); spawn(async move { match api::delete(&format!("/api/prep-projects/{id}")).await { Ok(()) => { context.success("准备项目已删除"); refresh += 1; }, Err(error) => context.error(error) } }); }
            }
        }
    }
}

#[component]
fn PrepEditor(
    value: PrepProject,
    is_new: bool,
    on_close: EventHandler<MouseEvent>,
    on_saved: EventHandler<MouseEvent>,
) -> Element {
    let context = use_context::<AppContext>();
    let original_id = value.id.clone();
    let mut draft = use_signal(|| PrepProject {
        params: value
            .params
            .into_iter()
            .filter(|parameter| !parameter.is_system())
            .collect(),
        ..value
    });
    let mut saving = use_signal(|| false);
    rsx! {
        div { class: "overlay is-open", onclick: move |event| on_close.call(event) }
        section { class: "drawer drawer--wide is-open", role: "dialog", "aria-label": if is_new { "新建准备项目" } else { "编辑准备项目" },
            div { class: "drawer__head", if is_new { "新建准备项目" } else { "编辑准备项目" } span { class: "spacer" } button { class: "btn btn--ghost btn--icon", onclick: move |event| on_close.call(event), Icon { width: 17, height: 17, icon: LdX } } }
            div { class: "drawer__body",
                div { class: "form-grid form-grid--two",
                    div { class: "field", label { class: "field__label", "名称" } input { class: "input", value: "{draft().name}", oninput: move |event| draft.write().name = event.value() } }
                    div { class: "field", label { class: "field__label", "说明" } input { class: "input", value: "{draft().description}", oninput: move |event| draft.write().description = event.value() } }
                }
                div { class: "section-heading", strong { "参数定义" } span { class: "hint mono", "系统自动注入 project_path" } span { class: "spacer" } button { class: "btn btn--sm", onclick: move |_| { let next = draft().params.len() + 1; draft.write().params.push(PrepParam { name: format!("param_{next}"), ..PrepParam::default() }); }, Icon { width: 14, height: 14, icon: LdPlus } "添加参数" } }
                if draft().params.is_empty() { div { class: "empty-inline", "无参数；脚本将直接执行。" } }
                for (index, parameter) in draft().params.iter().cloned().enumerate() {
                    PrepParamRow { index, parameter, draft }
                }
            }
            div { class: "drawer__foot",
                button { class: "btn", onclick: move |event| on_close.call(event), "取消" }
                button { class: "btn btn--primary", disabled: saving() || draft().name.trim().is_empty(), onclick: move |event| { let request = PrepSaveRequest { name: draft().name, description: draft().description, params: draft().params }; let success_event = event; let id = original_id.clone(); spawn(async move { saving.set(true); let result = if is_new { api::post::<PrepProject, _>("/api/prep-projects", &request).await } else { api::put::<PrepProject, _>(&format!("/api/prep-projects/{id}"), &request).await }; match result { Ok(_) => { context.success(if is_new { "准备项目已创建" } else { "准备项目已保存" }); on_saved.call(success_event); }, Err(error) => context.error(error) } saving.set(false); }); }, if saving() { "保存中…" } else { "保存" } }
            }
        }
    }
}

#[component]
fn PrepParamRow(index: usize, parameter: PrepParam, draft: Signal<PrepProject>) -> Element {
    let fixed_text = parameter
        .fixed_value
        .as_ref()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| value.to_string())
        })
        .unwrap_or_default();
    rsx! {
        div { class: "param-row",
            div { class: "field", label { class: "field__label", "key" } input { class: "input input--mono", value: "{parameter.name}", oninput: move |event| draft.write().params[index].name = event.value() } }
            div { class: "field", label { class: "field__label", "类型" } select { class: "select", value: "{param_type_value(&parameter.param_type)}", onchange: move |event| set_param_type(&mut draft.write().params[index], parse_param_type(&event.value())), option { value: "str", "文本" } option { value: "int", "整数" } option { value: "bool", "开关" } option { value: "select", "选择" } } }
            div { class: "field", label { class: "field__label", "传参方式" } select { class: "select", value: if parameter.value_source == PrepValueSource::Runtime { "runtime" } else { "fixed" }, onchange: move |event| set_value_source(&mut draft.write().params[index], if event.value() == "fixed" { PrepValueSource::Fixed } else { PrepValueSource::Runtime }), option { value: "runtime", "运行时传入" } option { value: "fixed", "固定值" } } }
            label { class: "checkbox checkbox--compact", input { r#type: "checkbox", checked: parameter.optional, onchange: move |event| draft.write().params[index].optional = event.checked() } "可选" }
            button { class: "btn btn--sm btn--danger btn--icon", title: "删除参数", onclick: move |_| { draft.write().params.remove(index); }, Icon { width: 14, height: 14, icon: LdTrash2 } }
            if parameter.param_type == PrepParamType::Select {
                div { class: "field param-row__wide", label { class: "field__label", "预设选项" }
                    div { class: "select-options",
                        if parameter.options.is_empty() { div { class: "empty-inline", "请至少添加一个选项" } }
                        for (option_index, option) in parameter.options.iter().cloned().enumerate() {
                            div { class: "select-option-row", key: "{option_index}",
                                input { class: "input", placeholder: "显示文案", value: "{option.label}", oninput: move |event| update_select_option_label(&mut draft.write().params[index], option_index, event.value()) }
                                input { class: "input input--mono", placeholder: "实际值", value: "{option.value}", oninput: move |event| update_select_option_value(&mut draft.write().params[index], option_index, event.value()) }
                                button { class: "btn btn--sm btn--danger btn--icon", title: "删除选项", onclick: move |_| remove_select_option(&mut draft.write().params[index], option_index), Icon { width: 14, height: 14, icon: LdTrash2 } }
                            }
                        }
                        button { class: "btn btn--sm select-options__add", onclick: move |_| add_select_option(&mut draft.write().params[index]), Icon { width: 14, height: 14, icon: LdPlus } "添加选项" }
                    }
                }
            }
            if parameter.value_source == PrepValueSource::Fixed {
                div { class: "field param-row__wide", label { class: "field__label", "固定值" }
                    if parameter.param_type == PrepParamType::Select {
                        select { class: "select", value: "{fixed_text}", onchange: move |event| draft.write().params[index].fixed_value = Some(Value::String(event.value())),
                            option { value: "", disabled: true, selected: fixed_text.is_empty(), "请选择固定值" }
                            for option in parameter.options.iter().filter(|option| !option.value.is_empty()) {
                                option { value: "{option.value}", selected: option.value == fixed_text, if option.label.is_empty() { "未命名选项" } else { "{option.label}" } }
                            }
                        }
                    } else {
                        input { class: "input input--mono", value: "{fixed_text}", oninput: move |event| { let kind = draft().params[index].param_type.clone(); draft.write().params[index].fixed_value = Some(parse_param_value(&kind, &event.value())); } }
                    }
                }
            }
        }
    }
}

#[component]
fn RuntimeParamField(parameter: PrepParam, values: Signal<HashMap<String, Value>>) -> Element {
    let current = values()
        .get(&parameter.name)
        .cloned()
        .unwrap_or(Value::Null);
    let current_text = current
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| current.to_string());
    rsx! {
        div { class: "field", label { class: "field__label", "{parameter.name} · {param_type_label(&parameter.param_type)}" }
            if parameter.param_type == PrepParamType::Bool {
                label { class: "switch", input { r#type: "checkbox", checked: current.as_bool().unwrap_or(false), onchange: move |event| { values.write().insert(parameter.name.clone(), Value::Bool(event.checked())); } } i {} }
            } else if parameter.param_type == PrepParamType::Select {
                select { class: "select", value: "{current_text}", onchange: move |event| { values.write().insert(parameter.name.clone(), Value::String(event.value())); }, for option in parameter.options { option { value: "{option.value}", selected: option.value == current_text, "{option.label}" } } }
            } else {
                input { class: "input input--mono", value: "{current_text}", oninput: move |event| { values.write().insert(parameter.name.clone(), parse_param_value(&parameter.param_type, &event.value())); } }
            }
        }
    }
}

fn default_run_values(prep: &PrepProject) -> HashMap<String, Value> {
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

fn set_param_type(parameter: &mut PrepParam, param_type: PrepParamType) {
    parameter.param_type = param_type;
    if parameter.param_type == PrepParamType::Select {
        if parameter.options.is_empty() {
            parameter.options.push(PrepParamOption::default());
        }
    } else {
        parameter.options.clear();
    }

    if parameter.value_source == PrepValueSource::Fixed {
        match parameter.param_type {
            PrepParamType::Bool => parameter.fixed_value = Some(Value::Bool(false)),
            PrepParamType::Select => reconcile_fixed_select_value(parameter),
            PrepParamType::Str | PrepParamType::Int => parameter.fixed_value = None,
        }
    }
}

fn set_value_source(parameter: &mut PrepParam, value_source: PrepValueSource) {
    parameter.value_source = value_source;
    if parameter.value_source == PrepValueSource::Runtime {
        parameter.fixed_value = None;
        return;
    }

    match parameter.param_type {
        PrepParamType::Bool => parameter.fixed_value = Some(Value::Bool(false)),
        PrepParamType::Select => reconcile_fixed_select_value(parameter),
        PrepParamType::Str | PrepParamType::Int => parameter.fixed_value = None,
    }
}

fn add_select_option(parameter: &mut PrepParam) {
    parameter.options.push(PrepParamOption::default());
    reconcile_fixed_select_value(parameter);
}

fn update_select_option_label(parameter: &mut PrepParam, index: usize, label: String) {
    if let Some(option) = parameter.options.get_mut(index) {
        option.label = label;
    }
}

fn update_select_option_value(parameter: &mut PrepParam, index: usize, value: String) {
    let selected = parameter.options.get(index).is_some_and(|option| {
        parameter.fixed_value.as_ref().and_then(Value::as_str) == Some(option.value.as_str())
    });
    if let Some(option) = parameter.options.get_mut(index) {
        option.value = value.clone();
    }
    if selected {
        parameter.fixed_value = (!value.is_empty()).then_some(Value::String(value));
    }
    reconcile_fixed_select_value(parameter);
}

fn remove_select_option(parameter: &mut PrepParam, index: usize) {
    if index < parameter.options.len() {
        parameter.options.remove(index);
    }
    reconcile_fixed_select_value(parameter);
}

fn reconcile_fixed_select_value(parameter: &mut PrepParam) {
    if parameter.param_type != PrepParamType::Select
        || parameter.value_source != PrepValueSource::Fixed
    {
        return;
    }

    let current = parameter.fixed_value.as_ref().and_then(Value::as_str);
    if current.is_some_and(|current| {
        !current.is_empty()
            && parameter
                .options
                .iter()
                .any(|option| option.value == current)
    }) {
        return;
    }

    parameter.fixed_value = parameter
        .options
        .iter()
        .find(|option| !option.value.is_empty())
        .map(|option| Value::String(option.value.clone()));
}

fn parse_param_value(kind: &PrepParamType, raw: &str) -> Value {
    match kind {
        PrepParamType::Int => raw
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or(Value::String(raw.to_owned())),
        PrepParamType::Bool => Value::Bool(matches!(raw, "true" | "1" | "是")),
        PrepParamType::Str | PrepParamType::Select => Value::String(raw.to_owned()),
    }
}

fn parse_param_type(value: &str) -> PrepParamType {
    match value {
        "int" => PrepParamType::Int,
        "bool" => PrepParamType::Bool,
        "select" => PrepParamType::Select,
        _ => PrepParamType::Str,
    }
}

fn param_type_value(value: &PrepParamType) -> &'static str {
    match value {
        PrepParamType::Str => "str",
        PrepParamType::Int => "int",
        PrepParamType::Bool => "bool",
        PrepParamType::Select => "select",
    }
}

fn param_type_label(value: &PrepParamType) -> &'static str {
    match value {
        PrepParamType::Str => "文本",
        PrepParamType::Int => "整数",
        PrepParamType::Bool => "开关",
        PrepParamType::Select => "选择",
    }
}

fn export_prep(
    prep: PrepProject,
    mut export_data: Signal<Option<(String, String)>>,
    context: AppContext,
) {
    spawn(async move {
        match api::get::<PrepExportPayload>(&format!("/api/prep-projects/{}/export", prep.id)).await
        {
            Ok(payload) => match serde_json::to_string_pretty(&payload) {
                Ok(raw) => export_data.set(Some((prep.name, raw))),
                Err(error) => context.error(error.to_string()),
            },
            Err(error) => context.error(error),
        }
    });
}

fn copy_export(raw: String, context: AppContext) {
    spawn(async move {
        let script = format!(
            "return navigator.clipboard.writeText({}).then(() => true);",
            serde_json::to_string(&raw).unwrap()
        );
        match document::eval(&script).join::<bool>().await {
            Ok(true) => context.success("导出 JSON 已复制"),
            _ => context.error("浏览器拒绝访问剪贴板"),
        }
    });
}

fn download_export(name: String, raw: String) {
    spawn(async move {
        let script = format!(
            "const b=new Blob([{}],{{type:'application/json'}});const u=URL.createObjectURL(b);const a=document.createElement('a');a.href=u;a.download={};a.click();URL.revokeObjectURL(u);",
            serde_json::to_string(&raw).unwrap(),
            serde_json::to_string(&format!("{name}.json")).unwrap()
        );
        let _ = document::eval(&script).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_type_initializes_and_clears_options() {
        let mut parameter = PrepParam::default();

        set_param_type(&mut parameter, PrepParamType::Select);
        assert_eq!(parameter.options, vec![PrepParamOption::default()]);

        set_param_type(&mut parameter, PrepParamType::Str);
        assert!(parameter.options.is_empty());
    }

    #[test]
    fn structured_select_option_edits_keep_fixed_value_consistent() {
        let mut parameter = PrepParam {
            param_type: PrepParamType::Select,
            value_source: PrepValueSource::Fixed,
            options: vec![
                PrepParamOption {
                    label: "正式".to_owned(),
                    value: "release".to_owned(),
                    ..PrepParamOption::default()
                },
                PrepParamOption {
                    label: "测试".to_owned(),
                    value: "test".to_owned(),
                    ..PrepParamOption::default()
                },
            ],
            fixed_value: Some(json!("test")),
            ..PrepParam::default()
        };

        update_select_option_label(&mut parameter, 1, "验收".to_owned());
        update_select_option_value(&mut parameter, 1, "qa".to_owned());
        assert_eq!(parameter.options[1].label, "验收");
        assert_eq!(parameter.fixed_value, Some(json!("qa")));

        add_select_option(&mut parameter);
        assert_eq!(parameter.options.len(), 3);

        remove_select_option(&mut parameter, 1);
        assert_eq!(parameter.fixed_value, Some(json!("release")));
    }

    #[test]
    fn fixed_source_uses_type_appropriate_default() {
        let mut parameter = PrepParam {
            param_type: PrepParamType::Bool,
            ..PrepParam::default()
        };

        set_value_source(&mut parameter, PrepValueSource::Fixed);
        assert_eq!(parameter.fixed_value, Some(Value::Bool(false)));

        set_param_type(&mut parameter, PrepParamType::Select);
        assert_eq!(parameter.options, vec![PrepParamOption::default()]);
        assert_eq!(parameter.fixed_value, None);

        update_select_option_value(&mut parameter, 0, "stable".to_owned());
        assert_eq!(parameter.fixed_value, Some(json!("stable")));

        set_value_source(&mut parameter, PrepValueSource::Runtime);
        assert_eq!(parameter.fixed_value, None);
    }

    #[test]
    fn creates_runtime_defaults_without_fixed_parameters() {
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
                PrepParam {
                    name: "channel".to_owned(),
                    param_type: PrepParamType::Select,
                    options: vec![PrepParamOption {
                        value: "stable".to_owned(),
                        ..PrepParamOption::default()
                    }],
                    ..PrepParam::default()
                },
                PrepParam {
                    name: "fixed".to_owned(),
                    value_source: PrepValueSource::Fixed,
                    ..PrepParam::default()
                },
            ],
            ..PrepProject::default()
        };

        let values = default_run_values(&prep);
        assert_eq!(values.get("enabled"), Some(&Value::Bool(false)));
        assert_eq!(values.get("channel"), Some(&json!("stable")));
        assert!(!values.contains_key("project_path"));
        assert!(!values.contains_key("fixed"));
    }

    #[test]
    fn preserves_invalid_integer_text_for_server_validation() {
        assert_eq!(
            parse_param_value(&PrepParamType::Int, "not-a-number"),
            json!("not-a-number")
        );
    }
}
