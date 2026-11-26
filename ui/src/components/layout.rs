//! Навигация и общая область уведомлений.
use dioxus::prelude::*;
use km_shared::MediaKind;
use km_ui_core::format::format_usd;
use wasm_bindgen::{JsCast, closure::Closure};

use super::error_notice::ErrorNotice;
use crate::router::Route;
use crate::state::{AppState, use_t};

#[component]
pub fn Layout() -> Element {
    let mut state = use_context::<AppState>();
    let nav = use_navigator();
    let history_label = use_t("history.title");
    let settings_label = use_t("settings.title");
    let loading = use_t("common.loading");
    let spend = use_t("common.spend");
    let local_spend = use_t("ws.local_spend");
    let close = use_t("common.close");
    let sections = [
        (MediaKind::Image, "image", use_t("studio.image")),
        (MediaKind::Video, "video", use_t("studio.video")),
        (MediaKind::Audio, "audio", use_t("studio.audio")),
    ];

    // Документ получает клавиши и без фокуса на компоненте. Навигация должна
    // выполняться в контексте Layout, который восстанавливает Callback.
    let open_history_search = use_callback(move |()| {
        state.search_history();
        nav.push(Route::History {});
    });
    let listener = use_hook(move || {
        let document = web_sys::window()?.document()?;
        let document_key = document.clone();
        let handler =
            Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
                if e.key() != "/"
                    || e.default_prevented()
                    || e.ctrl_key()
                    || e.meta_key()
                    || e.alt_key()
                    || document_key
                        .query_selector("dialog[open], [aria-modal='true']")
                        .ok()
                        .flatten()
                        .is_some()
                {
                    return;
                }
                if document_key.active_element().is_some_and(|el| {
                    matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                        || el.has_attribute("contenteditable")
                }) {
                    return;
                }
                e.prevent_default();
                open_history_search.call(());
            });
        document
            .add_event_listener_with_callback("keydown", handler.as_ref().unchecked_ref())
            .ok()?;
        Some(std::rc::Rc::new((document, handler)))
    });
    use_drop(move || {
        if let Some(listener) = listener {
            let _ = listener.0.remove_event_listener_with_callback(
                "keydown",
                listener.1.as_ref().unchecked_ref(),
            );
        }
    });
    let error = (state.error)().or_else(|| (state.background_error)());
    rsx! {
        div { class:"app-shell",
            aside { class:"sidebar", aria_label: "Koteyye Media Studio v2",
                div { class:"sidebar-brand", title:"Koteyye Media Studio v2",
                    img {src:asset!("/assets/icons/koteyye_media_studio_logo.png"),alt:"",width:"42",height:"42"}
                    span { class:"brand-name","Koteyye" small {" Media Studio " b {"v2"}} }
                }
                nav {class:"sidebar-nav",
                    for (kind,ic,label) in sections {
                        Link {key:"{ic}",class:"nav-item",active_class:"active",to:Route::studio(kind),
                            aria_label:label,title:label,
                            span {class:"studio-icon","data-icon":ic,aria_hidden:"true"}
                            span {class:"nav-label",{label}}
                        }
                    }
                    div {class:"side-divider",aria_hidden:"true"}
                    Link {class:"nav-item",active_class:"active",to:Route::History {},aria_label:history_label,title:history_label,
                        onclick:move |_| {state.history_filter.set(Default::default());},
                        span {class:"studio-icon","data-icon":"history",aria_hidden:"true"}
                        span {class:"nav-label",{history_label}}
                    }
                }
                div {class:"sidebar-footer",
                    if (state.busy)() {p {class:"side-status",{loading}}}
                    if let Some(view)=state.balance.read().for_provider(state.settings.read().provider) {
                        p {class:"side-status side-spend", title:local_spend, "data-sidebar-spend":view.provider.name(),
                            span {{view.provider.name()}}
                            span {"{spend} · " {format_usd(view.local_spend_usd)}}
                        }
                    }
                    Link {class:"nav-item",active_class:"active",to:Route::Settings {},aria_label:settings_label,title:settings_label,
                        span {class:"studio-icon","data-icon":"settings",aria_hidden:"true"}
                        span {class:"nav-label",{settings_label}}
                    }
                }
            }
            main {class:"content",
                if let Some(message)=error {
                    div {class:"error-banner",role:"alert",
                        div {class:"error-banner-text",ErrorNotice { message }}
                        button {class:"banner-dismiss",title:close,aria_label:close,onclick:move |_|state.dismiss_error(),"×"}
                    }
                }
                Outlet::<Route> {}
            }
        }
    }
}
