//! Каркас экрана: сайдбар с навигацией и контентная область с [`Outlet`].
//!
//! Активный пункт подсвечивается акцентом (`active_class` роутера). В подвале
//! сайдбара — локальный расход и индикатор загрузки; ошибки IPC показываются
//! баннером над контентом.

use dioxus::prelude::*;
use km_ui_core::format::format_usd;

use crate::router::Route;
use crate::state::{AppState, use_t};

/// Сайдбар (Изображения/Видео/Аудио → Studio, История, Настройки) + контент.
#[component]
pub fn Layout() -> Element {
    let state = use_context::<AppState>();
    // Переводы — до условных ветвей rsx: use_t основан на хуке.
    let image_label = use_t("studio.image");
    let video_label = use_t("studio.video");
    let audio_label = use_t("studio.audio");
    let history_label = use_t("history.title");
    let settings_label = use_t("settings.title");
    let loading_label = use_t("common.loading");
    let spend_label = use_t("common.spend");
    let error_label = use_t("common.error");

    let error = (state.error)();
    let balance = (state.balance)();
    let busy = (state.busy)();

    rsx! {
        div { class: "app-shell",
            aside { class: "sidebar",
                div { class: "sidebar-brand", "Koteyye Media Studio" }
                nav { class: "sidebar-nav",
                    Link {
                        class: "nav-item",
                        active_class: "active",
                        to: Route::Studio { kind: "image".to_owned() },
                        span { class: "nav-icon", "🖼" }
                        {image_label}
                    }
                    Link {
                        class: "nav-item",
                        active_class: "active",
                        to: Route::Studio { kind: "video".to_owned() },
                        span { class: "nav-icon", "🎬" }
                        {video_label}
                    }
                    Link {
                        class: "nav-item",
                        active_class: "active",
                        to: Route::Studio { kind: "audio".to_owned() },
                        span { class: "nav-icon", "🎵" }
                        {audio_label}
                    }
                    Link {
                        class: "nav-item",
                        active_class: "active",
                        to: Route::History {},
                        span { class: "nav-icon", "🕘" }
                        {history_label}
                    }
                    Link {
                        class: "nav-item",
                        active_class: "active",
                        to: Route::Settings {},
                        span { class: "nav-icon", "⚙" }
                        {settings_label}
                    }
                }
                div { class: "sidebar-footer",
                    if busy {
                        p { class: "muted", {loading_label} }
                    }
                    if let Some(view) = balance {
                        p { class: "muted", {spend_label} ": " {format_usd(view.local_spend_usd)} }
                    }
                }
            }
            main { class: "content",
                if let Some(message) = error {
                    div { class: "error-banner", {error_label} ": {message}" }
                }
                Outlet::<Route> {}
            }
        }
    }
}
