//! Заглушка онбординга: приветствие.
//!
//! Шаги (ключ → папка → готово) и редирект первого запуска — Task 6.

use dioxus::prelude::*;

use crate::state::use_t;

/// Экран онбординга первого запуска.
#[component]
pub fn Onboarding() -> Element {
    let welcome = use_t("onboarding.welcome");
    rsx! {
        h1 { {welcome} }
    }
}
