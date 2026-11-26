//! Сохранение API-ключа и проверка подключения для настроек и онбординга.

use crate::{
    ipc,
    state::{AppState, use_t},
};
use dioxus::prelude::*;

/// Статус проверки подключения к Cheaper Inference (поле API-ключа).
///
/// Различает принятый ключ, отсутствие/401, непроверенное соединение (сеть/5xx)
/// и ошибку keychain при сохранении ключа — это разные причины для пользователя.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum KeyStatus {
    /// Проверка ещё не выполнялась.
    Idle,
    /// Идёт запрос проверки.
    Checking,
    /// Ключ принят (`test_connection` → `true`).
    Ok,
    /// Ключа нет или CI ответил 401 (`test_connection` → `false`).
    Rejected,
    /// Сохранённый ключ удалён пользователем.
    Removed,
    /// Подключение не проверено: сеть/5xx (`test_connection` → `Err`).
    Unverified(String),
    /// Keychain не принял ключ (`set_api_key` → `Err`).
    KeychainError(String),
}

/// Переводит результат [`ipc::test_connection`] в статус поля.
///
/// `Ok(true)` — ключ принят; `Ok(false)` — ключа нет или 401; `Err` — соединение
/// не проверено (сеть/5xx), а не «нет ключа».
pub(crate) fn key_status(result: Result<bool, String>) -> KeyStatus {
    match result {
        Ok(true) => KeyStatus::Ok,
        Ok(false) => KeyStatus::Rejected,
        Err(e) => KeyStatus::Unverified(e),
    }
}

/// Проверяет введённый ключ или, при пустом поле, ключ из хранилища.
///
/// Блокирует кнопки ключа на время запроса (`key_busy`).
pub(crate) async fn check_connection(
    candidate: Option<String>,
    mut key_busy: Signal<bool>,
    mut connection: Signal<KeyStatus>,
) {
    key_busy.set(true);
    connection.set(KeyStatus::Checking);
    connection.set(key_status(ipc::test_connection(candidate.as_deref()).await));
    key_busy.set(false);
}

/// При явном 401 сохраняем прежний ключ. При недоступной сети пользователь
/// может сохранить введённый ключ и проверить подключение позже.
pub(crate) async fn save_key(
    state: AppState,
    key: String,
    mut key_input: Signal<String>,
    mut key_busy: Signal<bool>,
    mut connection: Signal<KeyStatus>,
) -> bool {
    key_busy.set(true);
    let status = if key.trim().is_empty() {
        KeyStatus::Removed
    } else {
        connection.set(KeyStatus::Checking);
        let checked = key_status(ipc::test_connection(Some(&key)).await);
        if checked == KeyStatus::Rejected {
            connection.set(checked);
            key_busy.set(false);
            return false;
        }
        checked
    };
    match ipc::set_api_key(&key).await {
        Ok(()) => {
            key_input.set(String::new());
            state.credential_changed();
            connection.set(status);
            key_busy.set(false);
            true
        }
        Err(e) => {
            connection.set(KeyStatus::KeychainError(e));
            key_busy.set(false);
            false
        }
    }
}

#[component]
pub(crate) fn KeyStatusView(status: KeyStatus, draft: bool) -> Element {
    let checking_label = use_t("settings.checking");
    let ok_label = if draft {
        use_t("settings.draft_key_ok")
    } else {
        use_t("settings.connection_ok")
    };
    let failed_label = use_t("settings.connection_failed");
    let rejected_label = use_t("settings.connection_rejected");
    let removed_label = use_t("settings.key_removed");
    let unverified_label = use_t("settings.connection_unverified");
    let keychain_label = use_t("settings.keychain_error");
    match &status {
        KeyStatus::Idle => rsx! {},
        KeyStatus::Checking => rsx! {
            p { class: "status status-muted", {checking_label} }
        },
        KeyStatus::Ok => rsx! {
            p { class: "status status-ok", "✓ " {ok_label} }
        },
        KeyStatus::Removed => rsx! {
            p { class: "status status-muted", {removed_label} }
        },
        KeyStatus::Rejected => rsx! {
            p { class: "status status-error", {failed_label} ": " {rejected_label} }
        },
        KeyStatus::Unverified(e) => rsx! {
            p { class: "status status-error", {unverified_label} ": " {e.as_str()} }
        },
        KeyStatus::KeychainError(e) => rsx! {
            p { class: "status status-error", {keychain_label} ": " {e.as_str()} }
        },
    }
}
