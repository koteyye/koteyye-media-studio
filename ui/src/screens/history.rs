//! Заглушка истории: число записей из сигнала состояния.
//!
//! Список, фильтры и действия появятся в Плане 3.

use dioxus::prelude::*;

use crate::state::{AppState, use_t};

/// Экран истории генераций.
#[component]
pub fn History() -> Element {
    let state = use_context::<AppState>();
    let title = use_t("history.title");
    let empty_label = use_t("history.empty");
    let recent_label = use_t("common.recent");

    let count = (state.history)().len();

    rsx! {
        h1 { {title} }
        if count == 0 {
            p { class: "muted", {empty_label} }
        } else {
            p { class: "muted", {recent_label} ": {count}" }
        }
    }
}
