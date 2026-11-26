//! Хранение файлов, баланс и обновления приложения.

use crate::{
    ipc,
    state::{AppState, BalanceState, updates::UpdatePhase, use_t},
};
use dioxus::prelude::*;
use km_shared::{BalanceUnavailable, BalanceView, Provider, UpdateStatus};
use km_ui_core::{format::format_usd, i18n::tr};
use wasm_bindgen_futures::spawn_local;

fn account_available_usd(view: &BalanceView) -> Option<f64> {
    match view.provider {
        Provider::CheaperInference => view.real.as_ref().map(|balance| balance.available_usd),
        Provider::OpenRouter => view
            .openrouter_credits
            .as_ref()
            .map(|credits| credits.remaining_usd()),
    }
}

fn balance_hint(provider: Provider, reason: Option<BalanceUnavailable>) -> &'static str {
    match reason {
        Some(BalanceUnavailable::MissingKey) => "ws.wallet_missing_key",
        Some(BalanceUnavailable::PermissionDenied) => match provider {
            Provider::CheaperInference => "ws.balance_scope",
            Provider::OpenRouter => "ws.wallet_management_key",
        },
        Some(BalanceUnavailable::RejectedKey) => "ws.wallet_rejected_key",
        Some(BalanceUnavailable::AccessRestricted) => "ws.wallet_access_restricted",
        Some(BalanceUnavailable::KeyStore) => "ws.key_read_failed",
        Some(BalanceUnavailable::Network) => "ws.wallet_network",
        Some(BalanceUnavailable::Other) | None => "ws.wallet_unavailable",
    }
}

#[component]
pub(super) fn StorageSettings() -> Element {
    let state = use_context::<AppState>();
    let lang = (state.lang)();
    let pick = tr(lang, "settings.pick_folder");
    let mut picking = use_signal(|| false);
    let folder = state.settings.read().local_folder.clone();
    rsx! {
        section {
            class: "sw-block sw-storage",
            h2 {

                {tr(lang, "ws.storage")}
            }
            div {
                class: "sw-row",
                span {
                    class: "sw-name",
                    {tr(lang, "settings.folder")}
                    small {
                        class: "sw-path",
                        {if folder.is_empty() { tr(lang, "settings.folder_missing") } else { &folder }}
                    }
                }
                button {
                    r#type: "button",
                    class: "btn",
                    "data-pick-folder": "",
                    disabled: picking(),
                    onclick: move |_| {
                        if *picking.peek() {
                            return;
                        }
                        picking.set(true);
                        spawn(async move {
                            match ipc::pick_folder(pick).await {
                                Ok(Some(path)) => {
                                    super::change_settings(state, move |s| s.local_folder = path)
                                }
                                Err(e) => state.set_error(Some(e)),
                                Ok(None) => {}
                            }
                            picking.set(false);
                        });
                    },
                    {pick}
                }
            }
        }
    }
}

#[component]
pub(super) fn BalanceSettings() -> Element {
    let state = use_context::<AppState>();
    let lang = (state.lang)();
    let settings = state.settings.read();
    let provider = settings.provider;
    let show = settings.show_real_balance;
    let balance = state.balance.read();
    let view = balance.for_provider(provider);
    let loading = matches!(&*balance, BalanceState::Loading);
    let scope = match provider {
        Provider::CheaperInference => "ws.balance_scope",
        Provider::OpenRouter => "ws.wallet_management_key",
    };
    rsx! {
        section { class: "sw-block sw-balance", "data-balance-provider": provider.name(),
            h2 { class: "sw-wallet-heading", {tr(lang, "ws.balance")} small { {provider.name()} } }
            div { class: "sw-row",
                span { class: "sw-name", {tr(lang, "ws.local_spend")} }
                div { class:"sw-wallet-actions",
                    if let Some(view) = view {
                        strong { class:"sw-wallet-value", "data-local-spend":"", {format_usd(view.local_spend_usd)} }
                    } else if loading {
                        span { class:"sw-note", role:"status", {tr(lang,"common.loading")} }
                    } else if let BalanceState::Failed(reason) = &*balance {
                        span { class:"sw-note", role:"status", title:"{reason}", {tr(lang,"ws.spend_unavailable")} }
                    }
                    button { r#type:"button", class:"btn", "data-refresh-balance":"", disabled:loading,
                        onclick:move |_| { spawn(async move {state.refresh_balance().await}); },
                        {tr(lang,"common.refresh")}
                    }
                }
            }
            label { class:"sw-row",
                span { class:"sw-name", {tr(lang,"ws.real_balance")}
                    if !show { small { {tr(lang,scope)} } }
                }
                input { class:"sw-switch", role:"switch", r#type:"checkbox", checked:show, "data-pref":"showRealBalance",
                    onchange:move |event| {
                        let enabled = event.checked();
                        spawn_local(async move {
                            let mut next = state.settings.peek().clone();
                            if next.provider != provider || next.show_real_balance == enabled { return; }
                            next.show_real_balance = enabled;
                            let _ = state.persist_settings(next).await;
                            if state.settings.peek().provider == provider { state.refresh_balance().await; }
                        });
                    }
                }
            }
            if show && let Some(view) = view {
                div { class:"sw-row sw-wallet-account", role:"status",
                    span { class:"sw-name", {tr(lang,"ws.account_available")} }
                    if let Some(available) = account_available_usd(view) {
                        strong { class:"sw-wallet-value", "data-account-available":"", {format_usd(available)} }
                    } else {
                        span { class:"sw-note", "data-wallet-reason":"", {tr(lang,balance_hint(provider,view.unavailable))} }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn UpdateSettings() -> Element {
    let state = use_context::<AppState>();
    let title = use_t("updates.title");
    let check = use_t("updates.check");
    let checking = use_t("updates.checking");
    let current = use_t("updates.current");
    let disabled = use_t("updates.disabled");
    let available = use_t("updates.available");
    let install = use_t("updates.install");
    let installing = use_t("updates.installing");
    let restarting = use_t("updates.restarting");
    let wait = use_t("updates.wait");
    let phase = (state.update)();
    let media_idle = state.media_idle();
    let message = match &phase {
        UpdatePhase::Idle => None,
        UpdatePhase::Checking => Some(checking.to_owned()),
        UpdatePhase::Checked(UpdateStatus::Current) => Some(current.to_owned()),
        UpdatePhase::Checked(UpdateStatus::Disabled) => Some(disabled.to_owned()),
        UpdatePhase::Checked(UpdateStatus::Available(version)) => {
            Some(format!("{available} {version}"))
        }
        UpdatePhase::Installing => Some(installing.to_owned()),
        UpdatePhase::Restarting => Some(restarting.to_owned()),
    };
    rsx! {
        section {
            class: "sw-block sw-updates",
            h2 {

                {title}
            }
            if let Some(message) = message {
                p {
                    class: "status",
                    role: "status",
                    {message}
                }
            }
            div {
                class: "field-row",
                button {
                    class: "btn",
                    r#type: "button",
                    disabled: !phase.can_check(),
                    onclick: move |_| state.check_update(),
                    {check}
                }
                if matches!(phase, UpdatePhase::Checked(UpdateStatus::Available(_))) {
                    button {
                        class: "btn btn-primary",
                        r#type: "button",
                        disabled: !media_idle,
                        onclick: move |_| state.install_update(),
                        {install}
                    }
                    if !media_idle {
                        span {
                            class: "status status-muted",
                            {wait}
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::key_status::{KeyStatus, key_status};

    #[test]
    fn connection_status_accepted_key_is_ok() {
        assert_eq!(key_status(Ok(true)), KeyStatus::Ok);
    }

    #[test]
    fn connection_status_false_is_rejected_not_error() {
        // false — ключа нет или 401, а не сетевая ошибка.
        assert_eq!(key_status(Ok(false)), KeyStatus::Rejected);
    }

    #[test]
    fn connection_status_err_is_unverified_with_reason() {
        // Сеть/5xx — «подключение не проверено», причина сохраняется.
        assert_eq!(
            key_status(Err("сеть недоступна".to_owned())),
            KeyStatus::Unverified("сеть недоступна".to_owned())
        );
    }

    #[test]
    fn each_wallet_displays_the_figure_reported_by_its_own_provider() {
        let mut view: BalanceView = serde_json::from_value(serde_json::json!({
            "provider": "ci", "local_spend_usd": 0.36,
            "real": {"currency": "USD", "balance_usd": 12.0, "available_usd": 11.0, "reserved_usd": 1.0},
            "openrouter_credits": {"total_credits": 25.0, "total_usage": 4.5}
        })).unwrap();
        assert_eq!(account_available_usd(&view), Some(11.0));
        view.provider = Provider::OpenRouter;
        assert_eq!(account_available_usd(&view), Some(20.5));
        view.openrouter_credits = None;
        assert_eq!(account_available_usd(&view), None);
        let state = BalanceState::Ready(view);
        assert!(state.for_provider(Provider::CheaperInference).is_none());
    }

    #[test]
    fn wallet_access_explains_the_selected_providers_requirement() {
        let denied = Some(BalanceUnavailable::PermissionDenied);
        assert_eq!(
            balance_hint(Provider::CheaperInference, denied),
            "ws.balance_scope"
        );
        assert_eq!(
            balance_hint(Provider::OpenRouter, denied),
            "ws.wallet_management_key"
        );
        assert_eq!(
            balance_hint(Provider::OpenRouter, Some(BalanceUnavailable::MissingKey)),
            "ws.wallet_missing_key"
        );
    }
}
