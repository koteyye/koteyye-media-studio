use dioxus::prelude::*;
use km_ui_core::i18n::{localize_error, tr};

use crate::state::AppState;

#[component]
pub(crate) fn ErrorNotice(message: String) -> Element {
    let lang = (use_context::<AppState>().lang)();
    let localized = localize_error(lang, &message);
    let summary = localized.as_deref().unwrap_or_else(|| {
        let key = km_ui_core::studio_results::failure_explanation(Some(&message));
        tr(
            lang,
            if key == "gen.failure.other" {
                "common.action_failed"
            } else {
                key
            },
        )
    });
    let technical =
        localized.is_none() || message.contains(": ") && !message.starts_with("ошибка валидации: ");
    rsx! {
        div { class: "localized-error",
            span { {summary} }
            if technical && message.trim() != summary {
                details {
                    summary { {tr(lang, "gen.failure.diagnostic")} }
                    pre { translate: "no", "{message}" }
                }
            }
        }
    }
}
