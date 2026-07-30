use dioxus::{document, prelude::*};
use dioxus_free_icons::{
    Icon,
    icons::ld_icons::{
        LdChevronLeft, LdChevronRight, LdClipboard, LdEye, LdRefreshCw, LdSearch, LdTrash2, LdX,
    },
};

use crate::{
    AppContext, ConfirmDialog, api,
    models::{LogFile, LogPageResponse},
};

#[component]
pub fn Logs() -> Element {
    let context = use_context::<AppContext>();
    let mut logs = use_signal(LogPageResponse::default);
    let mut loading = use_signal(|| true);
    let mut load_error = use_signal(String::new);
    let mut page = use_signal(|| 1usize);
    let mut query = use_signal(String::new);
    let mut sort_by = use_signal(|| "createdAt".to_owned());
    let mut sort_order = use_signal(|| "desc".to_owned());
    let mut refresh = use_signal(|| 0u64);
    let mut active = use_signal(|| None::<LogFile>);
    let mut content = use_signal(String::new);
    let mut content_loading = use_signal(|| false);
    let mut content_query = use_signal(String::new);
    let mut delete_target = use_signal(|| None::<LogFile>);
    let mut confirm_clear = use_signal(|| false);

    use_effect(move || {
        let request_page = page();
        let request_query = query();
        let request_sort = sort_by();
        let request_order = sort_order();
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            let path = format!(
                "/api/logs?page={request_page}&pageSize=20&query={}&sortBy={request_sort}&sortOrder={request_order}",
                api::encode_query(&request_query)
            );
            match api::get::<LogPageResponse>(&path).await {
                Ok(value) => {
                    logs.set(value);
                    load_error.set(String::new());
                }
                Err(error) => load_error.set(error),
            }
            loading.set(false);
        });
    });

    let total_pages = logs().total.div_ceil(20).max(1);
    let mut load_log = move |log: LogFile| {
        active.set(Some(log.clone()));
        content.set(String::new());
        content_query.set(String::new());
        spawn(async move {
            content_loading.set(true);
            let path = format!("/api/log-content?path={}", api::encode_query(&log.path));
            match api::get_text(&path).await {
                Ok(value) => content.set(value),
                Err(error) => context.error(error),
            }
            content_loading.set(false);
        });
    };

    rsx! {
        main { class: "page__main page__main--wide",
            div { class: "toolbar",
                div { h1 { "构建日志" } p { class: "toolbar__subtitle", "查看、检索和清理真实构建输出" } }
                span { class: "spacer" }
                div { class: "search-box", Icon { width: 15, height: 15, icon: LdSearch }
                    input { class: "input", placeholder: "按文件名过滤", value: "{query}", oninput: move |event| { page.set(1); query.set(event.value()); } }
                }
                select { class: "select select--compact", value: "{sort_by}", onchange: move |event| { page.set(1); sort_by.set(event.value()); },
                    option { value: "createdAt", "按时间" }
                    option { value: "name", "按名称" }
                    option { value: "size", "按大小" }
                }
                button { class: "btn btn--icon", title: "切换排序方向", "aria-label": "切换排序方向", onclick: move |_| { sort_order.set(if sort_order() == "asc" { "desc".to_owned() } else { "asc".to_owned() }); },
                    if sort_order() == "asc" { "↑" } else { "↓" }
                }
                button { class: "btn btn--icon", title: "刷新日志", "aria-label": "刷新日志", disabled: loading(), onclick: move |_| refresh += 1,
                    Icon { width: 16, height: 16, icon: LdRefreshCw }
                }
                button { class: "btn btn--danger", disabled: logs().total == 0, onclick: move |_| confirm_clear.set(true),
                    Icon { width: 15, height: 15, icon: LdTrash2 } "清空全部"
                }
            }
            section { class: "table-panel",
                if loading() && logs().items.is_empty() {
                    div { class: "empty-state", "正在加载日志…" }
                } else if !load_error().is_empty() {
                    div { class: "error-state", p { "{load_error}" } button { class: "btn", onclick: move |_| refresh += 1, "重试" } }
                } else if logs().items.is_empty() {
                    div { class: "empty-state", "没有符合条件的日志" }
                } else {
                    div { class: "table-scroll",
                        table { class: "table",
                            thead { tr { th { "日志文件" } th { "创建时间" } th { "大小" } th { class: "table__actions-head", "操作" } } }
                            tbody { for log in logs().items {
                                tr { key: "{log.path}",
                                    td { strong { "{log.name}" } }
                                    td { class: "mono", "{log.created_at}" }
                                    td { class: "mono", "{format_size(log.size)}" }
                                    td { div { class: "table__actions",
                                        button { class: "btn btn--sm", title: "查看日志", onclick: { let log = log.clone(); move |_| load_log(log.clone()) }, Icon { width: 14, height: 14, icon: LdEye } "查看" }
                                        button { class: "btn btn--sm btn--danger", title: "删除日志", onclick: move |_| delete_target.set(Some(log.clone())), Icon { width: 14, height: 14, icon: LdTrash2 } }
                                    } }
                                }
                            } }
                        }
                    }
                    div { class: "pagination",
                        span { class: "hint", "共 {logs().total} 条 · 第 {page()} / {total_pages} 页" }
                        span { class: "spacer" }
                        button { class: "btn btn--sm btn--icon", title: "上一页", disabled: page() <= 1, onclick: move |_| page -= 1, Icon { width: 15, height: 15, icon: LdChevronLeft } }
                        button { class: "btn btn--sm btn--icon", title: "下一页", disabled: page() >= total_pages, onclick: move |_| page += 1, Icon { width: 15, height: 15, icon: LdChevronRight } }
                    }
                }
            }
        }
        if let Some(log) = active() {
            div { class: "overlay is-open", onclick: move |_| active.set(None) }
            section { class: "modal modal--log is-open", role: "dialog", "aria-modal": "true", "aria-label": "日志详情",
                div { class: "drawer__head",
                    div { strong { "{log.name}" } span { class: "hint mono", "{format_size(log.size)} · {log.created_at}" } }
                    span { class: "spacer" }
                    button { class: "btn btn--sm", disabled: content().is_empty(), onclick: move |_| copy_text(&content(), context), Icon { width: 14, height: 14, icon: LdClipboard } "复制全文" }
                    button { class: "btn btn--ghost btn--icon", title: "关闭", onclick: move |_| active.set(None), Icon { width: 17, height: 17, icon: LdX } }
                }
                div { class: "logview__search",
                    div { class: "search-box", Icon { width: 14, height: 14, icon: LdSearch }
                        input { class: "input", placeholder: "在日志中搜索", value: "{content_query}", oninput: move |event| content_query.set(event.value()), onkeydown: move |event| { if event.key() == Key::Enter { find_in_page(&content_query(), event.modifiers().shift()); } } }
                    }
                    span { class: "hint", if content_query().is_empty() { "输入关键词后按 Enter" } else { "Enter 下一个 · Shift+Enter 上一个" } }
                    span { class: "spacer" }
                    button { class: "btn btn--sm btn--icon", title: "上一个匹配", disabled: content_query().is_empty(), onclick: move |_| find_in_page(&content_query(), true), "↑" }
                    button { class: "btn btn--sm btn--icon", title: "下一个匹配", disabled: content_query().is_empty(), onclick: move |_| find_in_page(&content_query(), false), "↓" }
                }
                div { class: "logview",
                    if content_loading() { div { class: "empty-state", "正在读取日志…" } }
                    else { pre { id: "active-log-content", "{content}" } }
                }
            }
        }
        if let Some(log) = delete_target() {
            ConfirmDialog { title: "删除日志".to_owned(), message: format!("确认删除「{}」？该操作不可恢复。", log.name), confirm_label: "删除".to_owned(), danger: true,
                on_cancel: move |_| delete_target.set(None),
                on_confirm: move |_| { let path = log.path.clone(); delete_target.set(None); spawn(async move { match api::delete(&format!("/api/log?path={}", api::encode_query(&path))).await { Ok(()) => { context.success("日志已删除"); refresh += 1; }, Err(error) => context.error(error) } }); }
            }
        }
        if confirm_clear() {
            ConfirmDialog { title: "清空全部日志".to_owned(), message: "确认删除所有构建日志？该操作不可恢复。".to_owned(), confirm_label: "清空全部".to_owned(), danger: true,
                on_cancel: move |_| confirm_clear.set(false),
                on_confirm: move |_| { confirm_clear.set(false); spawn(async move { match api::delete("/api/logs").await { Ok(()) => { active.set(None); context.success("日志已清空"); refresh += 1; }, Err(error) => context.error(error) } }); }
            }
        }
    }
}

fn format_size(size: u64) -> String {
    if size < 1024 {
        format!("{size} B")
    } else if size < 1024 * 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else {
        format!("{:.1} MB", size as f64 / 1024.0 / 1024.0)
    }
}

fn copy_text(value: &str, context: AppContext) {
    let encoded = serde_json::to_string(value).unwrap();
    spawn(async move {
        let script = format!("return navigator.clipboard.writeText({encoded}).then(() => true);");
        match document::eval(&script).join::<bool>().await {
            Ok(true) => context.success("日志全文已复制"),
            _ => context.error("浏览器拒绝访问剪贴板"),
        }
    });
}

fn find_in_page(query: &str, backwards: bool) {
    let query = serde_json::to_string(query).unwrap();
    spawn(async move {
        let script = format!("window.find({query}, false, {backwards}, true, false, true, false);");
        let _ = document::eval(&script).await;
    });
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[test]
    fn formats_log_sizes_at_unit_boundaries() {
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
    }
}
