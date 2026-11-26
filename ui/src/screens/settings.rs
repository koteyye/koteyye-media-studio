//! Настройки приложения. Сохранённые ключи остаются в нативном хранилище.

use dioxus::prelude::*;
use km_shared::{MediaKind, Settings as AppSettings};
use km_ui_core::i18n::tr;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use crate::{router::Route, state::AppState};
mod application;
mod connection;
mod interface;
mod mcp;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Connection,
    Interface,
    Mcp,
}
impl Tab {
    const ALL: [Self; 3] = [Self::Connection, Self::Interface, Self::Mcp];

    fn id(self) -> &'static str {
        match self {
            Self::Connection => "connection",
            Self::Interface => "interface",
            Self::Mcp => "mcp",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Connection => "ws.connection",
            Self::Interface => "ws.interface",
            Self::Mcp => "ws.mcp",
        }
    }

    fn keyboard_target(self, key: Key) -> Option<Self> {
        let len = Self::ALL.len();
        let index = self as usize;
        match key {
            Key::ArrowRight => Some(Self::ALL[(index + 1) % len]),
            Key::ArrowLeft => Some(Self::ALL[(index + len - 1) % len]),
            Key::Home => Self::ALL.first().copied(),
            Key::End => Self::ALL.last().copied(),
            _ => None,
        }
    }
}

/// Изменяем актуальный снимок в момент старта сохранения. Запись продолжится,
/// если пользователь сразу перейдёт на другой экран.
fn change_settings(state: AppState, change: impl FnOnce(&mut AppSettings) + 'static) {
    spawn_local(async move {
        let mut next = state.settings.peek().clone();
        change(&mut next);
        if next != *state.settings.peek() {
            let _ = state.persist_settings(next).await;
        }
    });
}

fn kind_id(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "image",
        MediaKind::Video => "video",
        MediaKind::Audio => "audio",
    }
}

#[component]
pub fn Settings() -> Element {
    let state = use_context::<AppState>();
    let lang = (state.lang)();
    let nav = use_navigator();
    let mut tab = use_signal(|| Tab::Connection);
    let selected = tab();
    rsx! {
        section {
            id: "settings-workspace",
            class: "sw-workspace",
            header {
                class: "sw-head",
                h1 {

                    span {
                        class: "workspace-narrow-brand",
                        "Koteyye Media Studio v2 /"
                    }
                    {tr(lang, "settings.title")}
                }
                button {
                    class: "btn sw-back",
                    r#type: "button",
                    "data-back-studio": "",
                    onclick: move |_| {
                        if nav.can_go_back() {
                            nav.go_back();
                        } else {
                            nav.replace(Route::Studio {
                                kind: kind_id(state.settings.peek().workspace.last_kind).into(),
                            });
                        }
                    },
                    span {
                        aria_hidden: "true",
                        "←"
                    }
                    {tr(lang, "onboarding.back")}
                }
            }
            div {
                class: "sw-tabs",
                role: "tablist",
                aria_label: tr(lang, "settings.title"),
                for next in Tab::ALL {
                    button {
                        key: "{next.id()}",
                        r#type: "button",
                        role: "tab",
                        id: "sw-tab-{next.id()}",
                        "data-settings-tab": next.id(),
                        aria_selected: "{selected == next}",
                        aria_controls: "sw-pane-{next.id()}",
                        tabindex: if selected == next { "0" } else { "-1" },
                        onclick: move |_| tab.set(next),
                        onkeydown: move |e| {
                            let Some(target) = next.keyboard_target(e.key()) else { return; };
                            e.prevent_default();
                            tab.set(target);
                            if let Some(el) = web_sys::window()
                                .and_then(|w| w.document())
                                .and_then(|d| d.get_element_by_id(&format!("sw-tab-{}", target.id())))
                                .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
                            {
                                let _ = el.focus();
                            }
                        },
                        {tr(lang, next.label())}
                    }
                }
            }
            div {
                id: "sw-pane-connection",
                role: "tabpanel",
                aria_labelledby: "sw-tab-connection",
                hidden: selected != Tab::Connection,
                if selected == Tab::Connection {
                    connection::Connections {

                    }
                }
            }
            div {
                id: "sw-pane-interface",
                role: "tabpanel",
                aria_labelledby: "sw-tab-interface",
                hidden: selected != Tab::Interface,
                if selected == Tab::Interface {
                    interface::Interface {

                    }
                }
            }
            div {
                id: "sw-pane-mcp",
                role: "tabpanel",
                aria_labelledby: "sw-tab-mcp",
                hidden: selected != Tab::Mcp,
                if selected == Tab::Mcp {
                    mcp::Mcp {}
                }
            }
        }
    }
}
