mod api;
mod models;
mod package_ui;
mod pages;
mod schemes;

use dioxus::{document, prelude::*};
use dioxus_free_icons::{
    Icon,
    icons::ld_icons::{LdFileText, LdHammer, LdMoon, LdPackage, LdSettings, LdSun},
};
use dioxus_router::*;
use gloo_timers::future::TimeoutFuture;

use crate::pages::{Logs, Package, Prep, Settings};

const APP_CSS: Asset = asset!("/assets/app.css");
const EDITOR_JS: Asset = asset!("/assets/editor.bundle.js");
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Routable, Debug, PartialEq)]
pub enum Route {
    #[layout(Shell)]
    #[route("/package")]
    Package {},
    #[route("/prep")]
    Prep {},
    #[route("/logs")]
    Logs {},
    #[route("/settings")]
    Settings {},
    #[end_layout]
    #[redirect("/", || Route::Package {})]
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

#[derive(Clone, Copy)]
pub struct AppContext {
    pub dark: Signal<bool>,
    pub toast: Signal<Option<Toast>>,
}

#[derive(Clone, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub message: String,
    pub error: bool,
}

impl AppContext {
    pub fn success(mut self, message: impl Into<String>) {
        self.show(message.into(), false);
    }

    pub fn error(mut self, message: impl Into<String>) {
        self.show(message.into(), true);
    }

    fn show(&mut self, message: String, error: bool) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        self.toast.set(Some(Toast {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            message,
            error,
        }));
    }
}

#[component]
pub fn App() -> Element {
    let selected_group = use_signal(|| package_ui::read_storage::<String>("last-group", false));
    let draft = use_signal(models::TaskGroup::default);
    let baseline = use_signal(models::TaskGroup::default);
    let saving = use_signal(|| false);
    let pending = use_signal(|| None);
    let revision = use_signal(|| 0);
    let active_schemes = use_signal(|| {
        package_ui::read_storage::<std::collections::BTreeMap<String, String>>(
            "active-schemes",
            false,
        )
    });
    use_context_provider(|| schemes::ActiveSchemes(active_schemes));
    use_effect(move || package_ui::write_storage("active-schemes", false, &active_schemes()));
    let managing = use_signal(|| false);
    let ui = package_ui::PackageUi {
        active: active_schemes,
        managing,
        selected_group,
        draft,
        baseline,
        saving,
        pending,
        revision,
    };
    use_context_provider(|| ui);
    let owners = use_signal(|| {
        package_ui::read_storage::<std::collections::BTreeMap<String, String>>(
            "selected-tasks",
            true,
        )
    });
    let selected = use_signal(|| owners.peek().keys().cloned().collect::<Vec<_>>());
    use_context_provider(|| package_ui::SelectedTasks(selected));
    let saves = schemes::use_selection_saves(selected);
    use_context_provider(|| package_ui::SelectionOwners(owners));
    let branches = use_signal(|| {
        package_ui::read_storage::<std::collections::BTreeMap<String, package_ui::BranchEntry>>(
            "branches", false,
        )
    });
    use_context_provider(|| package_ui::BranchCache(branches));
    use_effect(move || package_ui::write_storage("last-group", false, &selected_group()));
    use_effect(move || package_ui::write_storage("branches", false, &branches()));
    use_effect(move || {
        let dirty = ui.dirty() || saves.pending() || managing();
        spawn(async move {
            let _ = document::eval(&format!("window.__cocosUnsaved = {dirty}; if (!window.__cocosBeforeUnload) {{ window.__cocosBeforeUnload = event => {{ if (window.__cocosUnsaved) {{ event.preventDefault(); event.returnValue = ''; }} }}; window.addEventListener('beforeunload', window.__cocosBeforeUnload); }}")).await;
        });
    });
    let mut dark = use_signal(|| false);
    let mut toast = use_signal(|| None::<Toast>);
    use_effect(move || {
        if let Some(message) = toast() {
            spawn(async move {
                TimeoutFuture::new(3600).await;
                if toast
                    .peek()
                    .as_ref()
                    .is_some_and(|current| current.id == message.id)
                {
                    toast.set(None);
                }
            });
        }
    });
    use_context_provider(|| AppContext { dark, toast });
    use_effect(move || {
        spawn(async move {
            if let Ok(value) = document::eval(
                "return localStorage.getItem('cocos-build-lan-theme') ?? (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');",
            )
            .join::<String>()
            .await
            {
                dark.set(value == "dark");
            }
        });
    });
    rsx! {
        document::Link { rel: "stylesheet", href: APP_CSS }
        document::Script { src: EDITOR_JS }
        document::Link { rel: "icon", r#type: "image/svg+xml", href: asset!("/assets/favicon.svg") }
        Router::<Route> { config: move || RouterConfig::default().on_update(move |router| {
            let next = router.current();
            if next != (Route::Package {}) && managing() { return Some(Route::Package {}.into()); }
            if next != (Route::Package {}) && ui.dirty() {
                let mut pending = ui.pending;
                pending.set(Some(package_ui::Destination::Page(next)));
                Some(Route::Package {}.into())
            } else { None }
        }) }
    }
}

#[component]
fn Shell() -> Element {
    let mut context = use_context::<AppContext>();
    let theme = if (context.dark)() { "dark" } else { "light" };
    let toggle_theme = move |_| {
        let next = !(context.dark)();
        context.dark.set(next);
        spawn(async move {
            let value = if next { "dark" } else { "light" };
            let script = format!(
                "localStorage.setItem('cocos-build-lan-theme', {});",
                serde_json::to_string(value).unwrap()
            );
            let _ = document::eval(&script).await;
        });
    };
    rsx! {
        div { class: "app-shell", "data-theme": theme,
            header { class: "topbar",
                Link { class: "topbar__brand", to: Route::Package {},
                    span { class: "brand-mark", Icon { width: 18, height: 18, icon: LdHammer } }
                    span { "Cocos Build Console" }
                    span { class: "tag tag--mono", "v{VERSION}" }
                }
                nav { class: "topbar__nav", "aria-label": "主导航",
                    NavItem { to: Route::Package {}, label: "打包", icon: "package" }
                    NavItem { to: Route::Prep {}, label: "打包准备", icon: "prep" }
                    NavItem { to: Route::Logs {}, label: "构建日志", icon: "logs" }
                    NavItem { to: Route::Settings {}, label: "设置", icon: "settings" }
                }
                div { class: "topbar__right",
                    button { class: "btn btn--ghost btn--icon", title: "切换亮暗主题", "aria-label": "切换亮暗主题", onclick: toggle_theme,
                        if (context.dark)() { Icon { width: 17, height: 17, icon: LdSun } }
                        else { Icon { width: 17, height: 17, icon: LdMoon } }
                    }
                }
            }
            if let Some(toast) = (context.toast)() {
                div { class: if toast.error { "toast toast--visible toast--error" } else { "toast toast--visible" }, role: "status", "{toast.message}" }
            }
            Outlet::<Route> {}
            package_ui::UnsavedDialog {}
        }
    }
}

#[component]
fn NavItem(to: Route, label: &'static str, icon: &'static str) -> Element {
    rsx! {
        Link { to, active_class: "is-active",
            match icon {
                "package" => rsx! { Icon { width: 16, height: 16, icon: LdPackage } },
                "prep" => rsx! { Icon { width: 16, height: 16, icon: LdHammer } },
                "logs" => rsx! { Icon { width: 16, height: 16, icon: LdFileText } },
                _ => rsx! { Icon { width: 16, height: 16, icon: LdSettings } },
            }
            span { "{label}" }
        }
    }
}

#[component]
fn NotFound(segments: Vec<String>) -> Element {
    let navigator = use_navigator();
    use_effect(move || {
        navigator.replace(Route::Package {});
    });
    rsx! { main { class: "page__main", div { class: "empty-state", "正在返回打包页…" } } }
}

#[component]
pub fn ConfirmDialog(
    title: String,
    message: String,
    confirm_label: String,
    danger: bool,
    on_cancel: EventHandler<MouseEvent>,
    on_confirm: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        div { class: "overlay is-open", onclick: move |event| on_cancel.call(event) }
        section { class: "modal modal--sm is-open", role: "alertdialog", "aria-modal": "true", "aria-label": title.clone(),
            div { class: "drawer__head", "{title}" }
            div { class: "drawer__body", p { class: "confirm-copy", "{message}" } }
            div { class: "drawer__foot",
                button { class: "btn", onclick: move |event| on_cancel.call(event), "取消" }
                button { class: if danger { "btn btn--danger" } else { "btn btn--primary" }, onclick: move |event| on_confirm.call(event), "{confirm_label}" }
            }
        }
    }
}
