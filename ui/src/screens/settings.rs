//! Заглушка настроек: переключатели темы и языка для смоука каркаса.
//!
//! Полный экран (API-ключ, папка, соединение, баланс) — Task 5.
//! Смена темы/языка сохраняется через IPC и применяется без перезапуска.

use dioxus::prelude::*;
// Компонент называется Settings (имя маршрута), тип настроек — алиасом.
use km_shared::Settings as AppSettings;
use wasm_bindgen_futures::spawn_local;

use crate::state::{AppState, lang_name, parse_lang, parse_theme, theme_name, use_t};

/// Экран настроек (заглушка: только тема и язык).
#[component]
pub fn Settings() -> Element {
    let state = use_context::<AppState>();
    let title = use_t("settings.title");
    let theme_label = use_t("settings.theme");
    let dark_label = use_t("settings.dark");
    let light_label = use_t("settings.light");
    let system_label = use_t("settings.system");
    let language_label = use_t("settings.language");

    let theme = (state.theme)();
    let lang = (state.lang)();

    rsx! {
        h1 { {title} }
        div { class: "stub-controls",
            label { r#for: "theme-select", {theme_label} }
            select {
                id: "theme-select",
                value: theme_name(theme),
                onchange: move |evt| {
                    let settings = AppSettings {
                        theme: parse_theme(&evt.value()),
                        ..(state.settings)()
                    };
                    spawn_local(async move { state.persist_settings(settings).await });
                },
                option { value: "dark", {dark_label} }
                option { value: "light", {light_label} }
                option { value: "system", {system_label} }
            }
            label { r#for: "lang-select", {language_label} }
            select {
                id: "lang-select",
                value: lang_name(lang),
                onchange: move |evt| {
                    let settings = AppSettings {
                        lang: parse_lang(&evt.value()),
                        ..(state.settings)()
                    };
                    spawn_local(async move { state.persist_settings(settings).await });
                },
                // Названия языков — эндонимы, не переводятся.
                option { value: "ru", "Русский" }
                option { value: "en", "English" }
            }
        }
    }
}
