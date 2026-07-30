mod api;
mod models;
mod pages;

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
        self.toast.set(Some(Toast { message, error }));
        let mut toast = self.toast;
        spawn(async move {
            TimeoutFuture::new(3600).await;
            toast.set(None);
        });
    }
}

#[component]
pub fn App() -> Element {
    let mut dark = use_signal(|| false);
    let toast = use_signal(|| None::<Toast>);
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
        Router::<Route> {}
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
