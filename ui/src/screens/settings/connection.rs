//! Редактирование ключа одного профиля. Сохранённый секрет никогда не читается в WebView.

use super::{application::BalanceSettings, change_settings};
use crate::{ipc, state::AppState};
use dioxus::prelude::*;
use km_shared::{Lang, Provider};
use km_ui_core::i18n::tr;
use wasm_bindgen_futures::spawn_local;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
enum Presence {
    #[default]
    Unknown,
    Missing,
    Present,
    Error,
}

/// Общий на время сессии учёт мутаций одного слота. Текст ключа здесь не хранится.
#[derive(Clone, Copy, Default)]
struct Slot {
    presence: Presence,
    verified: bool,
    writing: bool,
    write_error: bool,
    revision: u64,
}
static CI: GlobalSignal<Slot> = Signal::global(Slot::default);
static OR: GlobalSignal<Slot> = Signal::global(Slot::default);
fn slot_for(provider: Provider) -> Signal<Slot> {
    match provider {
        Provider::CheaperInference => CI.signal(),
        Provider::OpenRouter => OR.signal(),
    }
}
fn provider_id(provider: Provider) -> &'static str {
    match provider {
        Provider::CheaperInference => "ci",
        Provider::OpenRouter => "openrouter",
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
enum Proof {
    #[default]
    Unknown,
    Accepted,
    Rejected,
    Unavailable,
}
impl Proof {
    fn from_result(result: Result<bool, String>) -> Self {
        match result {
            Ok(true) => Self::Accepted,
            Ok(false) => Self::Rejected,
            Err(_) => Self::Unavailable,
        }
    }
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Notice {
    #[default]
    Idle,
    Checking,
    CandidateOk,
    StoredOk,
    Rejected,
    Unavailable,
    ReadError,
    WriteError,
    Removed,
}

/// При редактировании, повторной проверке или удалении старые ответы перестают
/// относиться к полю. У каждого активного профиля своя область компонента.
#[derive(Default)]
struct Entry {
    draft: String,
    revision: u64,
    proof: Proof,
    notice: Notice,
    reveal: bool,
    confirming: bool,
}
impl Entry {
    fn edit(&mut self, value: String) {
        self.revision += 1;
        self.draft = value;
        self.proof = Proof::Unknown;
        self.notice = Notice::Idle;
        self.confirming = false;
    }
    fn invalidate(&mut self) {
        self.revision += 1;
        self.notice = Notice::Idle;
    }
    fn checking(&mut self) -> u64 {
        self.invalidate();
        self.confirming = false;
        self.notice = Notice::Checking;
        self.revision
    }
    fn status(&self, slot: Slot) -> (&'static str, &'static str) {
        if slot.writing {
            return ("writing", "ws.writing");
        }
        match self.notice {
            Notice::Checking => ("checking", "settings.checking"),
            Notice::CandidateOk => ("verified", "ws.verified_draft"),
            Notice::StoredOk => ("verified", "ws.verified_saved"),
            Notice::Rejected => ("error", "ws.rejected"),
            Notice::Unavailable => ("error", "ws.unreachable"),
            Notice::ReadError => ("error", "ws.key_read_failed"),
            Notice::WriteError => ("error", "ws.key_write_failed"),
            Notice::Removed => ("empty", "ws.removed"),
            Notice::Idle if !self.draft.trim().is_empty() => ("draft", "ws.draft"),
            Notice::Idle if slot.write_error => ("error", "ws.key_write_failed"),
            Notice::Idle => match slot.presence {
                Presence::Present if slot.verified => ("verified", "ws.verified_saved"),
                Presence::Present => ("saved", "ws.saved"),
                Presence::Missing => ("empty", "settings.key_missing"),
                Presence::Unknown => ("loading", "common.loading"),
                Presence::Error => ("error", "ws.key_read_failed"),
            },
        }
    }
}

#[derive(Clone, Copy)]
struct Field {
    provider: Provider,
    slot: Signal<Slot>,
    entry: Signal<Entry>,
    state: AppState,
}
impl Field {
    fn current(self, ticket: u64, slot_revision: u64) -> bool {
        self.state.settings.peek().provider == self.provider
            && self
                .entry
                .try_peek()
                .is_ok_and(|entry| entry.revision == ticket)
            && self.slot.peek().revision == slot_revision
    }
    fn read_presence(mut self) {
        // peek: начальная проверка запускается один раз при открытии профиля.
        let slot = *self.slot.peek();
        if slot.writing {
            return;
        }
        spawn(async move {
            let read = ipc::provider_has_key(self.provider).await;
            if self.slot.peek().revision == slot.revision {
                let mut now = self.slot.write();
                now.presence = match read {
                    Ok(true) => Presence::Present,
                    Ok(false) => Presence::Missing,
                    Err(_) => Presence::Error,
                };
                if now.presence == Presence::Missing {
                    now.verified = false;
                }
            }
        });
    }
    fn check(mut self, save_after: bool) {
        if self.slot.peek().writing {
            return;
        }
        let candidate = {
            let e = self.entry.peek();
            (!e.draft.trim().is_empty()).then(|| e.draft.trim().to_owned())
        };
        if save_after && candidate.is_none() {
            return;
        }
        if save_after && self.entry.peek().proof == Proof::Accepted {
            if let Some(value) = candidate {
                self.commit(value, true);
            }
            return;
        }
        let slot_before = *self.slot.peek();
        let ticket = self.entry.write().checking();
        spawn(async move {
            if candidate.is_none() && slot_before.presence != Presence::Present {
                let present = ipc::provider_has_key(self.provider).await;
                if !self.current(ticket, slot_before.revision) {
                    return;
                }
                match present {
                    Ok(false) => {
                        self.slot.write().presence = Presence::Missing;
                        self.slot.write().verified = false;
                        self.entry.write().notice = Notice::Idle;
                        return;
                    }
                    Err(_) => {
                        self.entry.write().notice = Notice::ReadError;
                        return;
                    }
                    Ok(true) => self.slot.write().presence = Presence::Present,
                }
            }
            let proof = Proof::from_result(
                ipc::check_provider_key(self.provider, candidate.as_deref()).await,
            );
            if !self.current(ticket, slot_before.revision) {
                return;
            }
            if candidate.is_some() {
                self.entry.write().proof = proof;
            } else if proof != Proof::Unavailable {
                let mut slot = self.slot.write();
                slot.verified = proof == Proof::Accepted;
                slot.write_error = false;
            }
            self.entry.write().notice = match proof {
                Proof::Accepted if candidate.is_some() => Notice::CandidateOk,
                Proof::Accepted => Notice::StoredOk,
                Proof::Rejected => Notice::Rejected,
                Proof::Unavailable => Notice::Unavailable,
                Proof::Unknown => Notice::Idle,
            };
            if save_after
                && proof == Proof::Accepted
                && let Some(value) = candidate
            {
                self.commit(value, true);
            }
        });
    }
    /// Запись в keychain уже отправляется нативному приложению. Завершаем её и
    /// сбрасываем каталог даже после ухода со страницы. Локальный ввод очищаем
    /// только у исходного компонента; новую форму обновляет общий слот профиля.
    fn commit(mut self, value: String, verified: bool) {
        if self.slot.peek().writing {
            return;
        }
        self.entry.write().invalidate();
        {
            let mut slot = self.slot.write();
            slot.writing = true;
            slot.write_error = false;
            slot.revision += 1;
        }
        let removed = value.is_empty();
        spawn_local(async move {
            let success = ipc::set_provider_key(self.provider, &value).await.is_ok();
            drop(value);
            {
                let mut slot = self.slot.write();
                slot.writing = false;
                slot.write_error = !success;
                if success {
                    slot.presence = if removed {
                        Presence::Missing
                    } else {
                        Presence::Present
                    };
                    slot.verified = !removed && verified;
                }
            }
            if success && self.state.settings.peek().provider == self.provider {
                self.state.credential_changed();
            }
            if let Ok(mut entry) = self.entry.try_write() {
                if success {
                    entry.edit(String::new());
                    entry.reveal = false;
                    if removed {
                        entry.notice = Notice::Removed;
                    }
                } else {
                    entry.notice = Notice::WriteError;
                    entry.confirming = false;
                }
            }
        });
    }
}

#[component]
pub(super) fn Connections() -> Element {
    let state = use_context::<AppState>();
    let lang = (state.lang)();
    let active = state.settings.read().provider;
    rsx! {
        section {
            id: "connection-pane",
            class: "sw-pane",
            fieldset {
                class: "sw-providers",
                legend {

                    {tr(lang, "ws.provider")}
                }
                div {
                    class: "sw-provider-options",
                    for p in [Provider::CheaperInference, Provider::OpenRouter] {
                        label {
                            key: "{provider_id(p)}",
                            class: "sw-provider",
                            "data-chosen": "{active==p}",
                            input {
                                r#type: "radio",
                                name: "studio-provider",
                                checked: active == p,
                                value: provider_id(p),
                                aria_label: p.name(),
                                "data-provider-choice": provider_id(p),
                                onchange: move |_| change_settings(state, move |s| s.provider = p),
                            }
                            span {

                                strong {

                                    {p.name()}
                                }
                                small { {tr(lang, "ws.media_service")} }
                            }
                        }
                    }
                }
            }
            // В двух разных слотах Dioxus уничтожает черновик и задачи предыдущего
            // профиля. Простое обновление props у одного компонента сохранило бы их.
            if active == Provider::CheaperInference {
                Credentials {
                    provider: Provider::CheaperInference,
                    lang,
                }
            }
            if active == Provider::OpenRouter {
                Credentials {
                    provider: Provider::OpenRouter,
                    lang,
                }
            }
            BalanceSettings {

            }
        }
    }
}

#[component]
fn Credentials(provider: Provider, lang: Lang) -> Element {
    let state = use_context::<AppState>();
    let mut entry = use_signal(Entry::default);
    let slot = slot_for(provider);
    let field = Field {
        state,
        provider,
        entry,
        slot,
    };
    use_effect(move || field.read_presence());
    let now = *slot.read();
    let e = entry.read();
    let (status, message) = e.status(now);
    let checking = e.notice == Notice::Checking;
    let blank = e.draft.trim().is_empty();
    let t = |key| tr(lang, key);
    let key_instructions = t(match provider {
        Provider::CheaperInference => "ws.key_setup_ci",
        Provider::OpenRouter => "ws.key_setup_openrouter",
    });
    rsx! {
        section {
            class: "sw-key",
            "data-key-provider": provider_id(provider),
            div {
                class: "sw-key-heading",
                label {
                    r#for: "studio-api-key",
                    {t("settings.api_key")}
                }
                span {

                    {provider.name()}
                }
            }
            p {
                class: "sw-note",
                id: "studio-key-hint",
                {t("ws.key_hint")}
            }
            p {
                class: "sw-note",
                id: "studio-key-setup",
                {key_instructions}
                " "
                a {
                    class: "sw-link",
                    href: provider.api_keys_url(),
                    onclick: move |event| {
                        event.prevent_default();
                        spawn_local(async move {
                            if let Err(error) = ipc::open_provider_api_keys(provider).await {
                                state.set_error(Some(error));
                            }
                        });
                    },
                    {t("ws.open_api_keys")}
                }
            }
            div {
                class: "sw-secret",
                input {
                    id: "studio-api-key",
                    "data-connection-key": "",
                    r#type: if e.reveal { "text" } else { "password" },
                    autocomplete: "off",
                    autocapitalize: "none",
                    spellcheck: "false",
                    aria_describedby: "studio-key-hint studio-key-setup",
                    placeholder: t("ws.key_placeholder"),
                    value: e.draft.as_str(),
                    disabled: now.writing,
                    oninput: move |event| entry.write().edit(event.value()),
                }
                button {
                    r#type: "button",
                    "data-key-show": "",
                    aria_controls: "studio-api-key",
                    aria_pressed: "{e.reveal}",
                    aria_label: if e.reveal { t("ws.hide_key") } else { t("ws.show_key") },
                    onclick: move |_| {
                        let visible = entry.peek().reveal;
                        entry.write().reveal = !visible;
                    },
                    {if e.reveal { t("ws.hide") } else { t("ws.show") }}
                }
            }
            div {
                class: "sw-actions",
                button {
                    class: "btn btn-primary",
                    r#type: "button",
                    "data-key-save": "",
                    disabled: now.writing || checking || blank,
                    onclick: move |_| field.check(true),
                    {t("ws.save_key")}
                }
                button {
                    class: "btn",
                    r#type: "button",
                    "data-key-check": "",
                    disabled: now.writing || checking,
                    aria_busy: "{checking}",
                    onclick: move |_| field.check(false),
                    {t("settings.check_connection")}
                }
                if now.presence == Presence::Present && !e.confirming {
                    button {
                        class: "sw-link sw-delete",
                        r#type: "button",
                        "data-key-delete": "",
                        disabled: now.writing,
                        onclick: move |_| {
                            let mut e = entry.write();
                            e.invalidate();
                            e.confirming = true;
                        },
                        {t("common.delete")}
                    }
                }
            }
            if e.confirming {
                div {
                    class: "sw-confirm",
                    role: "group",
                    aria_label: t("settings.confirm_delete_key"),
                    span {

                        {t("settings.confirm_delete_key")}
                        " · {provider.name()}"
                    }
                    button {
                        class: "btn",
                        r#type: "button",
                        "data-key-cancel-delete": "",
                        disabled: now.writing,
                        onclick: move |_| entry.write().confirming = false,
                        {t("common.cancel")}
                    }
                    button {
                        class: "btn sw-danger",
                        r#type: "button",
                        "data-key-confirm-delete": "",
                        disabled: now.writing,
                        onclick: move |_| {
                            if entry.peek().confirming {
                                field.commit(String::new(), false);
                            }
                        },
                        {t("settings.delete_key")}
                    }
                }
            }
            p {
                class: "sw-key-status",
                role: "status",
                "data-connection-status": "",
                "data-state": status,
                span {
                    class: "sw-signal",
                    aria_hidden: "true",
                }
                {t(message)}
            }
            if !blank && e.proof == Proof::Unavailable && !e.confirming {
                button {
                    class: "sw-link",
                    r#type: "button",
                    "data-key-save-offline": "",
                    disabled: now.writing || checking,
                    onclick: move |_| {
                        let e = entry.peek();
                        if e.proof == Proof::Unavailable {
                            let key = e.draft.trim().to_owned();
                            drop(e);
                            field.commit(key, false);
                        }
                    },
                    {t("ws.save_unchecked")}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_edit_invalidates_the_previous_check_without_changing_the_saved_verdict() {
        let mut e = Entry::default();
        e.edit("test-one".into());
        let old = e.checking();
        e.proof = Proof::Accepted;
        e.edit("different".into());
        assert_ne!(old, e.revision);
        assert_eq!(e.proof, Proof::Unknown);
        let slot = Slot {
            presence: Presence::Present,
            verified: true,
            ..Default::default()
        };
        e.notice = Notice::Rejected;
        assert_eq!(e.status(slot).0, "error");
        e.edit(String::new());
        assert_eq!(e.status(slot).0, "verified");
    }
}
