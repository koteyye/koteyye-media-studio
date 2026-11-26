//! Настройка локального MCP-сервера и подключение агента.

use dioxus::prelude::*;
use km_shared::{GenMode, McpStatus, MediaKind, Provider};
use km_ui_core::i18n::tr;
use km_ui_core::studio::models_for;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::{components::error_notice::ErrorNotice, ipc, state::AppState};

const SKILL_URI: &str = "koteyye://skills/media-studio/SKILL.md";
const TOKEN_PLACEHOLDER: &str = "<KOTEYYE_MCP_TOKEN>";

#[wasm_bindgen(inline_js = r#"
export async function copyMcpText(text) {
    try {
        if (navigator.clipboard?.writeText) {
            await navigator.clipboard.writeText(text);
            return true;
        }
    } catch {}
    const focused = document.activeElement;
    const field = document.createElement("textarea");
    field.value = text;
    field.readOnly = true;
    Object.assign(field.style, { position: "fixed", opacity: "0" });
    document.body.append(field);
    field.select();
    try {
        return document.execCommand("copy");
    } finally {
        field.remove();
        focused?.focus();
    }
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = copyMcpText)]
    fn copy_mcp_text(text: &str) -> js_sys::Promise;
}

#[derive(Default)]
struct Panel {
    status: Option<McpStatus>,
    busy: bool,
    error: Option<String>,
    pending_model: Option<(Provider, GenMode, String)>,
    model_error: Option<String>,
    copied: Option<bool>,
    reveal: bool,
}

impl Panel {
    fn phase(&self) -> (&'static str, &'static str) {
        match &self.status {
            _ if self.busy && self.pending_model.is_none() => ("loading", "mcp.changing"),
            Some(status) if status.running => ("verified", "mcp.running"),
            Some(status) if status.enabled => ("error", "mcp.stopped"),
            Some(_) => ("empty", "mcp.disabled"),
            None if self.error.is_some() => ("error", "mcp.unavailable"),
            None => ("loading", "common.loading"),
        }
    }

    fn connection(&self) -> Option<(&McpStatus, &str)> {
        let status = self.status.as_ref().filter(|status| status.running)?;
        let token = status.token.as_deref().filter(|token| !token.is_empty())?;
        Some((status, token))
    }
}

enum Operation {
    Load,
    Enable(bool),
    Rotate,
    SetModel(Provider, GenMode, Option<String>),
}

fn update(mut panel: Signal<Panel>, operation: Operation) {
    let is_model = matches!(&operation, Operation::SetModel(..));
    {
        let mut current = panel.write();
        if current.busy {
            return;
        }
        current.busy = true;
        if let Operation::SetModel(provider, mode, model) = &operation {
            current.pending_model = Some((*provider, *mode, model.clone().unwrap_or_default()));
            current.model_error = None;
        } else {
            current.error = None;
            current.copied = None;
            current.reveal = false;
        }
    }
    spawn(async move {
        let result = match operation {
            Operation::Load => ipc::mcp_status().await,
            Operation::Enable(enabled) => ipc::mcp_set_enabled(enabled).await,
            Operation::Rotate => ipc::mcp_rotate_token().await,
            Operation::SetModel(provider, mode, model) => {
                ipc::mcp_set_model_default(provider, mode, model.as_deref()).await
            }
        };
        let mut current = panel.write();
        current.busy = false;
        current.pending_model = None;
        match result {
            Ok(status) => current.status = Some(status),
            Err(error) if is_model => current.model_error = Some(error),
            Err(error) => current.error = Some(error),
        }
    });
}

fn copy(mut panel: Signal<Panel>, prompt: bool) {
    let text = {
        let current = panel.peek();
        let Some((status, token)) = current.connection().filter(|_| !current.busy) else {
            return;
        };
        if prompt {
            setup_prompt(status, token)
        } else {
            token.to_owned()
        }
    };
    let result = copy_mcp_text(&text);
    spawn(async move {
        let copied = JsFuture::from(result).await.ok().and_then(|v| v.as_bool()) == Some(true);
        panel.write().copied = Some(copied);
    });
}

fn setup_prompt(status: &McpStatus, token: &str) -> String {
    let endpoint = &status.endpoint;
    let bridge = status
        .command
        .as_deref()
        .map(|path| {
            let path = serde_json::Value::from(path);
            format!(
                "\nFor a stdio-only client, set the command to {path}, the argument to `--mcp-stdio`, and `KOTEYYE_MCP_TOKEN` in the process environment to the token above.\n"
            )
        })
        .unwrap_or_default();
    format!(
        "Connect the local Koteyye Media Studio as the `koteyye-media-studio` MCP server in this agent's settings. The studio runs on the same computer.\n\n\
         Transport — Streamable HTTP\nURL — {endpoint}\nHeader — Authorization: Bearer {token}\n{bridge}\n\
         Store the token in the client's local secret store or use an environment variable. If the client only accepts the header in configuration, use private local configuration. Keep the token out of chat, logs and project files.\n\
         After connecting, read the `{SKILL_URI}` MCP resource. If resources are unavailable, call `get_studio_skill`. If the client supports local skills, put the file in its skills directory as `<name>/SKILL.md`, using the name in the YAML frontmatter. Load the skill according to the client's instructions.\n\
         Verify the connection with `studio_status`. Use this read-only check for setup. When handling a task, follow the MCP model settings reported by `studio_status`."
    )
}

const MODEL_GROUPS: [(MediaKind, &str, &[(GenMode, &str)]); 3] = [
    (
        MediaKind::Image,
        "studio.image",
        &[
            (GenMode::TextToImage, "studio.tab.from_text"),
            (GenMode::ImageEdit, "studio.tab.edit"),
        ],
    ),
    (
        MediaKind::Video,
        "studio.video",
        &[
            (GenMode::TextToVideo, "studio.tab.from_text"),
            (GenMode::ImageToVideo, "studio.tab.from_image"),
            (GenMode::ReferenceToVideo, "studio.tab.references"),
        ],
    ),
    (
        MediaKind::Audio,
        "studio.audio",
        &[
            (GenMode::Speech, "studio.tab.speech"),
            (GenMode::Music, "studio.mode.music"),
            (GenMode::SoundEffects, "studio.tab.sounds"),
        ],
    ),
];

#[component]
fn ModelDefaults(panel: Signal<Panel>) -> Element {
    let state = use_context::<AppState>();
    let lang = (state.lang)();
    let provider = state.settings.read().provider;
    let current = panel.read();
    rsx! {
        section { class: "sw-block sw-separated sw-mcp-models",
            h2 { {tr(lang, "mcp.model_defaults")} }
            p { class: "sw-note", {tr(lang, "mcp.model_defaults_hint")} }
            p { class: "sw-mcp-model-provider", {provider.name()} }
            if current.pending_model.is_some() {
                p { class: "sw-note", role: "status", {tr(lang, "ws.writing")} }
            }
            if let Some(error) = current.model_error.as_deref() {
                div { class: "sw-mcp-error", role: "alert", ErrorNotice { message: error.to_owned() } }
            }
            for (kind, heading, modes) in MODEL_GROUPS {
                ModelGroup { key: "{provider:?}-{kind:?}", panel, provider, kind, heading, modes }
            }
        }
    }
}

#[component]
fn ModelGroup(
    panel: Signal<Panel>,
    provider: Provider,
    kind: MediaKind,
    heading: &'static str,
    modes: &'static [(GenMode, &'static str)],
) -> Element {
    let state = use_context::<AppState>();
    let lang = (state.lang)();
    let mut reloading = use_signal(|| false);
    let catalog = state.models_by_kind.read();
    let models = catalog.get(&kind);
    let current = panel.read();
    let defaults = current
        .status
        .as_ref()
        .and_then(|status| status.model_defaults.get(&provider));
    let waiting_for_catalog = *state.busy.read() || reloading();
    rsx! {
        div { class: "sw-mcp-model-group",
            h3 { {tr(lang, heading)} }
            for (mode, label) in modes.iter().copied() {
                {
                    let options = models_for(models.map_or(&[], Vec::as_slice), kind, mode);
                    let stored = defaults.and_then(|choices| choices.get(&mode)).map_or("", String::as_str);
                    let pending = current.pending_model.as_ref().filter(|(active, pending, _)| {
                        *active == provider && *pending == mode
                    });
                    let selected = pending.map_or(stored, |(_, _, value)| value.as_str());
                    let missing = !selected.is_empty() && !options.iter().any(|model| model.id == selected);
                    let disabled = current.busy || (options.is_empty() && selected.is_empty());
                    let label = tr(lang, label);
                    let aria_label = format!("{} — {label}", tr(lang, heading));
                    rsx! {
                        label { key: "{mode:?}", class: "sw-row",
                            span { class: "sw-name", {label} }
                            select {
                                "data-mcp-mode": "{mode:?}",
                                aria_label,
                                value: selected,
                                disabled,
                                onchange: move |event| {
                                    let model = event.value();
                                    update(panel, Operation::SetModel(provider, mode, (!model.is_empty()).then_some(model)));
                                },
                                option { value: "", selected: selected.is_empty(), {tr(lang, "mcp.model_auto")} }
                                if missing {
                                    option { value: selected, selected: true, disabled: true,
                                        "{selected}"
                                        if models.is_some() { " · " {tr(lang, "mcp.model_unavailable")} }
                                    }
                                }
                                for model in options {
                                    option { key: "{model.id}", value: model.id.as_str(), selected: model.id == selected,
                                        {model.name.as_deref().unwrap_or(&model.id)}
                                        if model.name.as_deref().is_some_and(|name| name != model.id) {
                                            " · {model.id}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if models.is_none() {
                div { class: "sw-actions sw-mcp-catalog",
                    p { class: "sw-note", role: "status", {
                        tr(lang, if waiting_for_catalog { "common.loading" } else { "mcp.model_catalog_unavailable" })
                    } }
                    if !waiting_for_catalog {
                        button { class: "btn", r#type: "button",
                            onclick: move |_| {
                                reloading.set(true);
                                spawn(async move {
                                    state.refresh_models(kind).await;
                                    reloading.set(false);
                                });
                            },
                            {tr(lang, "common.retry")}
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn Mcp() -> Element {
    let lang = (use_context::<AppState>().lang)();
    let mut panel = use_signal(Panel::default);
    use_effect(move || update(panel, Operation::Load));
    let current = panel.read();
    let status = current.status.as_ref();
    let enabled = status.is_some_and(|status| status.enabled);
    let busy = current.busy;
    let (phase, label) = current.phase();
    let error = current
        .error
        .as_deref()
        .or_else(|| status.and_then(|status| status.error.as_deref()));
    let prompt = current.connection().map(|(status, token)| {
        setup_prompt(
            status,
            if current.reveal {
                token
            } else {
                TOKEN_PLACEHOLDER
            },
        )
    });
    let reveal = current.reveal;
    rsx! {
        div { class: "sw-pane sw-mcp",
            section { class: "sw-block",
                h2 { {tr(lang, "mcp.server")} }
                label { class: "sw-row",
                    span { class: "sw-name",
                        {tr(lang, "mcp.enable")}
                        small { {tr(lang, "mcp.local_hint")} }
                    }
                    input {
                        class: "sw-switch",
                        "data-pref": "mcp-enabled",
                        r#type: "checkbox",
                        role: "switch",
                        aria_label: tr(lang, "mcp.enable"),
                        checked: enabled,
                        disabled: busy || status.is_none(),
                        onchange: move |event| update(panel, Operation::Enable(event.checked())),
                    }
                }
                div { class: "sw-actions sw-mcp-status",
                    p { class: "sw-key-status", role: "status", "data-state": phase,
                        span { class: "sw-signal", aria_hidden: "true" }
                        {tr(lang, label)}
                    }
                    if !busy && error.is_some() && phase == "error" {
                        button { class: "btn", r#type: "button",
                            onclick: move |_| update(panel, if enabled { Operation::Enable(true) } else { Operation::Load }),
                            {tr(lang, "common.retry")}
                        }
                    }
                }
                if let Some(error) = error {
                    div { class: "sw-mcp-error", role: "alert", ErrorNotice { message: error.to_owned() } }
                }
                if let Some(status) = status {
                    label { class: "sw-mcp-endpoint",
                        span { class: "sw-name", {tr(lang, "mcp.endpoint")} }
                        input { class: "text-input", r#type: "text", readonly: true, value: status.endpoint.clone() }
                    }
                }
            }
            if let Some(prompt) = prompt {
                section { class: "sw-block sw-separated",
                    h2 { {tr(lang, "mcp.connect")} }
                    p { class: "sw-note", {tr(lang, "mcp.prompt_hint")} }
                    div { class: "sw-mcp-code",
                        pre {
                            tabindex: 0,
                            role: "region",
                            aria_label: tr(lang, "mcp.prompt"),
                            code { lang: "en", dir: "ltr", translate: "no", {prompt} }
                        }
                        button {
                            class: "btn sw-mcp-copy",
                            r#type: "button",
                            disabled: busy,
                            aria_label: tr(lang, "mcp.copy_prompt"),
                            title: tr(lang, "mcp.copy_prompt"),
                            onclick: move |_| copy(panel, true),
                            span { class: "sw-mcp-copy-icon", aria_hidden: "true" }
                        }
                    }
                    div { class: "sw-actions",
                        button { class: "btn", r#type: "button", disabled: busy,
                            onclick: move |_| copy(panel, false),
                            {tr(lang, "mcp.copy_token")}
                        }
                        button { class: "btn", r#type: "button", disabled: busy,
                            title: tr(lang, "mcp.rotate_hint"),
                            onclick: move |_| update(panel, Operation::Rotate),
                            {tr(lang, "mcp.rotate")}
                        }
                    }
                    if let Some(copied) = current.copied {
                        div { class: "sw-actions sw-mcp-copy-feedback",
                            p { class: "sw-note", role: "status",
                                {tr(lang, if copied { "mcp.copied" } else { "mcp.copy_failed" })}
                            }
                            if !copied || reveal {
                                button { class: "sw-link", r#type: "button",
                                    onclick: move |_| panel.write().reveal = !reveal,
                                    {tr(lang, if reveal { "mcp.hide_full_prompt" } else { "mcp.show_full_prompt" })}
                                }
                            }
                        }
                    }
                }
            }
            if status.is_some() {
                ModelDefaults { panel }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_prompt_carries_connection_and_skill_instructions() {
        let status = McpStatus {
            enabled: true,
            running: true,
            endpoint: "http://127.0.0.1:29731/mcp".into(),
            token: Some("fixture-private-token".into()),
            command: Some("/Applications/Koteyye Media Studio.app/Contents/MacOS/studio".into()),
            error: None,
            model_defaults: Default::default(),
        };
        let preview = setup_prompt(&status, TOKEN_PLACEHOLDER);
        assert!(preview.contains(TOKEN_PLACEHOLDER));
        assert!(!preview.contains("fixture-private-token"));
        let prompt = setup_prompt(&status, status.token.as_deref().unwrap());
        assert!(prompt.starts_with("Connect the local Koteyye Media Studio"));
        assert!(prompt.contains(&status.endpoint));
        assert!(prompt.contains("Authorization: Bearer fixture-private-token"));
        assert!(prompt.contains(SKILL_URI));
        assert!(prompt.contains("get_studio_skill"));
        assert!(prompt.contains("studio_status"));
        assert!(prompt.contains("--mcp-stdio"));
        assert!(prompt.contains(status.command.as_deref().unwrap()));
    }
}
