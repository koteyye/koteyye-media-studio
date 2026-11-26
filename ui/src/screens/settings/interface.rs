//! Оформление и настройки рабочего пространства.

use super::{
    application::{StorageSettings, UpdateSettings},
    change_settings, kind_id,
};
use crate::state::{AppState, parse_lang_preference, theme_name};
use dioxus::prelude::*;
use km_shared::{Lang, MediaKind, Settings, Theme, WorkspacePrefs};
use km_ui_core::i18n::tr;

#[component]
fn SelectRow(
    name: &'static str,
    label: &'static str,
    value: &'static str,
    options: Vec<(&'static str, &'static str)>,
    on_select: Callback<String>,
    hint: Option<&'static str>,
) -> Element {
    rsx! {
        label {
            class: "sw-row",
            span {
                class: "sw-name",
                {label}
                if let Some(s) = hint {
                    small {

                        {s}
                    }
                }
            }
            select {
                "data-pref": name,
                aria_label: label,
                value,
                onchange: move |e| on_select.call(e.value()),
                for (v, text) in options {
                    option {
                        key: "{v}",
                        value: v,
                        selected: v == value,
                        {text}
                    }
                }
            }
        }
    }
}

#[component]
fn Toggle(
    name: &'static str,
    label: &'static str,
    checked: bool,
    on_toggle: Callback<bool>,
    hint: Option<&'static str>,
) -> Element {
    rsx! {
        label {
            class: "sw-row",
            span {
                class: "sw-name",
                {label}
                if let Some(s) = hint {
                    small {

                        {s}
                    }
                }
            }
            input {
                class: "sw-switch",
                r#type: "checkbox",
                role: "switch",
                "data-pref": name,
                aria_label: label,
                checked,
                onchange: move |e| on_toggle.call(e.checked()),
            }
        }
    }
}

#[component]
fn Miniature(tone: &'static str, images: Vec<String>) -> Element {
    rsx! {
        span {class:"sw-mini-part","data-tone":tone,
            span {class:"sw-mini-side",img {src:asset!("/assets/icons/frame-16.png"),alt:""} i {} i {} }
            span {class:"sw-mini-work",
                i {class:"sw-mini-input"}
                span {class:"sw-mini-gallery",
                    if !images.is_empty() {
                        for path in images {crate::components::recent::Thumbnail {key:"{path}",kind:MediaKind::Image,path:Some(path.clone())}}
                    }
                }
            }
        }
    }
}
#[component]
fn ThemePicker(lang: Lang, selected: Theme) -> Element {
    let state = use_context::<AppState>();
    let images: Vec<String> = state
        .history
        .read()
        .iter()
        .filter(|row| row.kind == MediaKind::Image && row.status == km_shared::JobStatus::Completed)
        .flat_map(|row| row.local_paths.iter())
        .take(2)
        .cloned()
        .collect();
    rsx! {
        div {
            class: "sw-themes",
            role: "group",
            aria_label: tr(lang, "settings.theme"),
            for (theme, label) in [(Theme::Light, "ws.light"), (Theme::Dark, "ws.dark"), (Theme::System, "ws.system")] {
                button {
                    key: "{theme_name(theme)}",
                    class: "sw-theme",
                    r#type: "button",
                    "data-theme-choice": theme_name(theme),
                    aria_pressed: "{selected == theme}",
                    onclick: move |_| change_settings(state, move |s| s.theme = theme),
                    span {
                        class: "sw-mini",
                        aria_hidden: "true",
                        if theme != Theme::Dark {
                            Miniature {
                                tone: "light", images: images.clone(),
                            }
                        }
                        if theme != Theme::Light {
                            Miniature {
                                tone: "dark", images: images.clone(),
                            }
                        }
                    }
                    span {
                        class: "sw-theme-caption",
                        {tr(lang, label)}
                        span {
                            class: "sw-indicator",
                            aria_hidden: "true",
                            if selected == theme {
                                "✓"
                            }
                        }
                    }
                }
            }
        }
    }
}

fn reset_interface(s: &mut Settings) {
    s.theme = Theme::Light;
    s.workspace = WorkspacePrefs {
        last_kind: s.workspace.last_kind,
        model_choices: std::mem::take(&mut s.workspace.model_choices),
        openrouter_model_choices: std::mem::take(&mut s.workspace.openrouter_model_choices),
        ..WorkspacePrefs::default()
    };
}

#[component]
pub(super) fn Interface() -> Element {
    let state = use_context::<AppState>();
    let lang = (state.lang)();
    let settings = state.settings.read();
    let w = &settings.workspace;
    let density = if w.compact { "compact" } else { "comfortable" };
    let layout = if w.single_image { "large" } else { "grid" };
    let mut confirm_reset = use_signal(|| false);
    let t = |key| tr(lang, key);
    rsx! {
        div {
            id: "interface-pane",
            class: "sw-pane",
            section {
                class: "sw-block",
                h2 {

                    {t("ws.appearance")}
                }
                ThemePicker {
                    lang,
                    selected: settings.theme,
                }
                div {
                    class: "sw-grid sw-appearance",
                    SelectRow {
                        name: "density",
                        label: t("ws.density"),
                        value: density,
                        options: vec![("comfortable", t("ws.comfortable")), ("compact", t("ws.compact"))],
                        on_select: move |v: String| change_settings(
                            state,
                            move |s| s.workspace.compact = v == "compact",
                        ),
                    }
                    Toggle {
                        name: "motion",
                        label: t("ws.motion"),
                        checked: !w.reduce_motion,
                        on_toggle: move |v: bool| change_settings(state, move |s| s.workspace.reduce_motion = !v),
                    }
                    SelectRow {
                        name: "lang",
                        label: t("settings.language"),
                        value: settings.lang.code(),
                        options: vec![("system", t("settings.language_system")), ("ru", "Русский"), ("en", "English")],
                        on_select: move |v: String| change_settings(state, move |s| s.lang = parse_lang_preference(&v)),
                    }
                }
            }
            div {
                class: "sw-grid sw-separated",
                section {
                    class: "sw-block",
                    h2 {

                        {t("ws.results")}
                    }
                    SelectRow {
                        name: "layout",
                        label: t("ws.layout"),
                        value: layout,
                        options: vec![("grid", t("ws.grid")), ("large", t("ws.single"))],
                        on_select: move |v: String| change_settings(
                            state,
                            move |s| s.workspace.single_image = v == "large",
                        ),
                    }
                    SelectRow {
                        name: "recentCount",
                        label: t("ws.recent"),
                        hint: t("ws.recent_hint"),
                        value: match w.recent_count {
                            3 => "3",
                            8 => "8",
                            _ => "5",
                        },
                        options: vec![("3", t("ws.count3")), ("5", t("ws.count5")), ("8", t("ws.count8"))],
                        on_select: move |v: String| change_settings(
                            state,
                            move |s| {
                                s.workspace.recent_count = match v.as_str() {
                                    "3" => 3,
                                    "8" => 8,
                                    _ => 5,
                                };
                            },
                        ),
                    }
                }
                section {
                    class: "sw-block",
                    h2 {

                        {t("ws.work")}
                    }
                    SelectRow {
                        name: "startup",
                        label: t("ws.startup"),
                        value: w.startup.map_or("last", kind_id),
                        options: vec![
                            ("last", t("ws.last")),
                            ("image", t("studio.image")),
                            ("video", t("studio.video")),
                            ("audio", t("studio.audio")),
                        ],
                        on_select: move |v: String| change_settings(
                            state,
                            move |s| {
                                s.workspace.startup = match v.as_str() {
                                    "image" => Some(MediaKind::Image),
                                    "video" => Some(MediaKind::Video),
                                    "audio" => Some(MediaKind::Audio),
                                    _ => None,
                                };
                            },
                        ),
                    }
                    Toggle {
                        name: "rememberModels",
                        label: t("ws.remember"),
                        hint: t("ws.remember_hint"),
                        checked: w.remember_models,
                        on_toggle: move |v| change_settings(state, move |s| s.workspace.remember_models = v),
                    }
                    Toggle {
                        name: "shortcuts",
                        label: t("ws.shortcut"),
                        hint: "Ctrl / ⌘ + Enter",
                        checked: w.generation_shortcut,
                        on_toggle: move |v| change_settings(state, move |s| s.workspace.generation_shortcut = v),
                    }
                }
            }
            div {
                class: "sw-grid sw-separated",
                StorageSettings {

                }
                UpdateSettings {

                }
            }
            div {
                class: "sw-footer",
                if confirm_reset() {
                    span {
                        class: "sw-note",
                        {t("ws.reset_ask")}
                    }
                    div {
                        class: "sw-actions",
                        button {
                            r#type: "button",
                            class: "btn",
                            onclick: move |_| confirm_reset.set(false),
                            {t("common.cancel")}
                        }
                        button {
                            r#type: "button",
                            class: "btn",
                            "data-confirm-reset": "",
                            onclick: move |_| {
                                confirm_reset.set(false);
                                change_settings(state, reset_interface);
                            },
                            {t("ws.reset_yes")}
                        }
                    }
                } else {
                    button {
                        r#type: "button",
                        class: "sw-link",
                        "data-reset-workspace": "",
                        onclick: move |_| confirm_reset.set(true),
                        {t("ws.reset")}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use km_shared::{LangPreference, Provider};
    #[test]
    fn view_reset_preserves_provider_storage_language_and_last_section() {
        let mut settings = Settings {
            local_folder: "/existing".into(),
            lang: LangPreference::En,
            theme: Theme::Dark,
            show_real_balance: true,
            provider: Provider::OpenRouter,
            workspace: WorkspacePrefs {
                compact: true,
                last_kind: MediaKind::Video,
                recent_count: 8,
                ..Default::default()
            },
        };
        reset_interface(&mut settings);
        assert_eq!(settings.provider, Provider::OpenRouter);
        assert_eq!(settings.local_folder, "/existing");
        assert!(settings.show_real_balance);
        assert_eq!(settings.lang, LangPreference::En);
        assert_eq!(settings.workspace.last_kind, MediaKind::Video);
        assert_eq!(settings.theme, Theme::Light);
        assert_eq!(settings.workspace.recent_count, 5);
        assert!(!settings.workspace.compact);
    }
}
