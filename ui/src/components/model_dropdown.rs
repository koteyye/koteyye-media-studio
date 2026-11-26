//! Выбор совместимой модели по имени, идентификатору или провайдеру.

use dioxus::prelude::*;
use km_shared::{GenMode, MediaKind};
use km_ui_core::i18n::tr;
use km_ui_core::studio::{
    PriceUnit, format_price, group_by_provider, models_for, search_models, speech_token_rates,
    starting_rate_for_mode,
};

use crate::state::{AppState, use_t};

#[component]
pub fn ModelDropdown(
    kind: MediaKind,
    mode: GenMode,
    selected: String,
    on_select: Callback<String>,
) -> Element {
    let state = use_context::<AppState>();
    let lang = (state.lang)();
    let label = use_t("studio.model");
    let choose = use_t("controls.choose");
    let other = use_t("controls.other");
    let search = use_t("controls.search");
    let empty = use_t("controls.no_models");
    let close = use_t("common.close");
    let from = use_t("controls.from");
    let mut open = use_signal(|| false);
    let mut query = use_signal(String::new);
    let catalog = state.models_by_kind.read();
    let available = models_for(catalog.get(&kind).map_or(&[], Vec::as_slice), kind, mode);
    let selected_name = available
        .iter()
        .find(|m| m.id == selected)
        .map(|m| m.name.as_deref().unwrap_or(&m.id))
        .unwrap_or(if selected.is_empty() {
            choose
        } else {
            &selected
        });
    let matches = search_models(&available, &query.read());
    let groups = group_by_provider(&matches);

    rsx! {
        div {
            class: "model-picker",
            onkeydown: move |event| {
                if event.key() == Key::Escape { open.set(false); }
            },
            button {
                class: "btn model-trigger", r#type: "button", aria_expanded: open(),
                onclick: move |_| { query.set(String::new()); open.toggle(); },
                span { class: "model-trigger-label", {label} }
                strong { "{selected_name}" }
                span { aria_hidden: "true", "▾" }
            }
            if open() {
                button {
                    class: "model-backdrop", r#type: "button", tabindex: "-1", aria_label: close,
                    onclick: move |_| open.set(false),
                }
                div { class: "model-menu",
                    input {
                        class: "model-search", r#type: "search", aria_label: search,
                        placeholder: search, value: "{query}",
                        onmounted: move |event| async move { let _ = event.set_focus(true).await; },
                        oninput: move |event| query.set(event.value()),
                    }
                    if groups.is_empty() { p { class: "muted model-empty", {empty} } }
                    for (provider, models) in groups {
                        section { key: "{provider}", class: "model-group",
                            h3 { if provider.is_empty() { {other} } else { "{provider}" } }
                            for model in models {
                                {
                                    let id = model.id.clone();
                                    let name = model.name.as_deref().unwrap_or(&model.id);
                                    let features = model.features.join(" · ");
                                    let price = starting_rate_for_mode(model, mode).map(|(unit, price)| {
                                        let unit = tr(lang, match unit {
                                            PriceUnit::Image => "controls.per_image",
                                            PriceUnit::Second => "controls.per_second",
                                            PriceUnit::Character => "controls.per_character",
                                        });
                                        format!("{from} {} / {unit}", format_price(price))
                                    }).or_else(|| speech_token_rates(model).map(|(input, output)| format!(
                                        "{} / {} · {}",
                                        format_price(input), format_price(output), tr(lang, "studio.token_rates")
                                    )));
                                    rsx! {
                                        button {
                                            key: "{id}", class: "model-option", r#type: "button", aria_pressed: id == selected,
                                            onclick: move |_| { open.set(false); on_select.call(id.clone()); },
                                            span { class: "model-option-title", "{name}" }
                                            if let Some(price) = price { span { class: "model-rate", "{price}" } }
                                            if !features.is_empty() { small { class: "model-features", "{features}" } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
