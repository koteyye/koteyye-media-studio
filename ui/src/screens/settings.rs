//! Экран настроек: API-ключ и проверка соединения, папка сохранения, язык,
//! тема, реальный баланс и локальный расход.
//!
//! Настройки сохраняются через [`AppState::persist_settings`] (оптимистичное
//! применение к сигналам + откат при ошибке IPC — DOM-select не рассинхронизируется
//! с хранилищем). Ключ хранится в keychain, а не в [`km_shared::Settings`]: поле
//! password не префиливается реальным ключом, статус соединения показывает
//! [`ConnectionStatus`] (keychain-ошибка отличается от «нет ключа»). Ошибки IPC
//! попадают в глобальный баннер (`error`) либо в статус поля.

use dioxus::prelude::*;
use km_shared::{AccountBalance, BalanceView, Settings as AppSettings};
use km_ui_core::format::format_usd;

use crate::ipc;
use crate::state::{AppState, lang_name, parse_lang, parse_theme, theme_name, use_t};

/// Статус проверки подключения к Cheaper Inference (поле API-ключа).
///
/// Различает принятый ключ, отсутствие/401, непроверенное соединение (сеть/5xx)
/// и ошибку keychain при сохранении ключа — это разные причины для пользователя.
#[derive(Clone, Debug, PartialEq)]
enum ConnectionStatus {
    /// Проверка ещё не выполнялась.
    Idle,
    /// Идёт запрос проверки.
    Checking,
    /// Ключ принят (`test_connection` → `true`).
    Ok,
    /// Ключа нет или CI ответил 401 (`test_connection` → `false`).
    Rejected,
    /// Подключение не проверено: сеть/5xx (`test_connection` → `Err`).
    Unverified(String),
    /// Keychain не принял ключ (`set_api_key` → `Err`).
    KeychainError(String),
}

/// Переводит результат [`ipc::test_connection`] в статус поля.
///
/// `Ok(true)` — ключ принят; `Ok(false)` — ключа нет или 401; `Err` — соединение
/// не проверено (сеть/5xx), а не «нет ключа».
fn connection_status(result: Result<bool, String>) -> ConnectionStatus {
    match result {
        Ok(true) => ConnectionStatus::Ok,
        Ok(false) => ConnectionStatus::Rejected,
        Err(e) => ConnectionStatus::Unverified(e),
    }
}

/// Реальный баланс для показа: только если включён `show_real_balance` и получен.
///
/// Иначе UI показывает локальный расход (см. блок баланса).
fn shown_real_balance(view: &BalanceView, show_real: bool) -> Option<&AccountBalance> {
    if show_real { view.real.as_ref() } else { None }
}

/// Экран настроек: ключ, папка, язык, тема, баланс.
#[component]
pub fn Settings() -> Element {
    let state = use_context::<AppState>();

    // Переводы — до условных ветвей rsx: use_t основан на хуке, порядок стабилен.
    let title = use_t("settings.title");
    let key_label = use_t("settings.api_key");
    let save_label = use_t("common.save");
    let check_label = use_t("settings.check_connection");
    let folder_label = use_t("settings.folder");
    let pick_label = use_t("settings.pick_folder");
    let folder_missing_label = use_t("settings.folder_missing");
    let language_label = use_t("settings.language");
    let theme_label = use_t("settings.theme");
    let dark_label = use_t("settings.dark");
    let light_label = use_t("settings.light");
    let system_label = use_t("settings.system");
    let show_real_label = use_t("settings.show_real_balance");
    let balance_label = use_t("common.balance");
    let available_label = use_t("settings.available");
    let spend_label = use_t("common.spend");
    let refresh_label = use_t("common.refresh");
    let loading_label = use_t("common.loading");
    let checking_label = use_t("settings.checking");
    let ok_label = use_t("settings.connection_ok");
    let failed_label = use_t("settings.connection_failed");
    let rejected_label = use_t("settings.connection_rejected");
    let unverified_label = use_t("settings.connection_unverified");
    let keychain_label = use_t("settings.keychain_error");

    // Локальное состояние поля ключа и диалога папки (живёт в scope экрана).
    // Signal — Copy: key_busy/connection мутируются в helper-функциях (приём mut
    // по значению), key_input/folder_busy — прямо в обработчиках (нужен mut).
    let mut key_input = use_signal(String::new);
    let key_busy = use_signal(|| false);
    let connection = use_signal(|| ConnectionStatus::Idle);
    let mut folder_busy = use_signal(|| false);

    // Проверка соединения при открытии экрана: статус «ключ задан» без раскрытия
    // ключа. Внутри эффекта нет реактивных чтений — выполняется один раз за mount.
    use_effect(move || {
        spawn(async move { check_connection(key_busy, connection).await });
    });

    let settings = (state.settings)();
    let lang = (state.lang)();
    let theme = (state.theme)();
    let balance = (state.balance)();
    let status = (connection)();
    let key_busy_value = (key_busy)();
    let folder_busy_value = (folder_busy)();
    let key_input_value = (key_input)();

    // Статус соединения — отдельный Element, чтобы не ветвить хуки внутри rsx.
    let status_view = match &status {
        ConnectionStatus::Idle => rsx! {},
        ConnectionStatus::Checking => rsx! {
            p { class: "status status-muted", {checking_label} }
        },
        ConnectionStatus::Ok => rsx! {
            p { class: "status status-ok", "✓ " {ok_label} }
        },
        ConnectionStatus::Rejected => rsx! {
            p { class: "status status-error", {failed_label} ": " {rejected_label} }
        },
        ConnectionStatus::Unverified(e) => rsx! {
            p { class: "status status-error", {unverified_label} ": " {e.as_str()} }
        },
        ConnectionStatus::KeychainError(e) => rsx! {
            p { class: "status status-error", {keychain_label} ": " {e.as_str()} }
        },
    };

    // Блок баланса — either/or: реальный available_usd, если включён и получен,
    // ИНАЧЕ локальный расход (расход всегда виден в сайдбаре, здесь не дублируем).
    let balance_view = match &balance {
        Some(view) => match shown_real_balance(view, settings.show_real_balance) {
            Some(real) => rsx! {
                p { class: "balance-line", {available_label} ": " {format_usd(real.available_usd)} }
            },
            None => rsx! {
                p { class: "balance-line", {spend_label} ": " {format_usd(view.local_spend_usd)} }
            },
        },
        None => rsx! {
            p { class: "muted", {loading_label} }
        },
    };

    let folder_value = settings.local_folder.clone();
    let folder_empty = folder_value.is_empty();

    rsx! {
        h1 { {title} }
        div { class: "settings",
            section { class: "settings-section",
                h2 { class: "section-title", {key_label} }
                div { class: "field-row",
                    input {
                        r#type: "password",
                        class: "text-input",
                        autocomplete: "off",
                        value: "{key_input_value}",
                        disabled: key_busy_value,
                        oninput: move |evt| key_input.set(evt.value()),
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: key_busy_value,
                        onclick: move |_| {
                            let key = (key_input)();
                            spawn(async move {
                                save_key(key, key_input, key_busy, connection).await;
                            });
                        },
                        {save_label}
                    }
                    button {
                        class: "btn",
                        disabled: key_busy_value,
                        onclick: move |_| {
                            spawn(async move { check_connection(key_busy, connection).await });
                        },
                        {check_label}
                    }
                }
                {status_view}
            }

            section { class: "settings-section",
                h2 { class: "section-title", {folder_label} }
                div { class: "field-row",
                    if folder_empty {
                        span { class: "folder-path muted", {folder_missing_label} }
                    } else {
                        span { class: "folder-path", {folder_value} }
                    }
                    button {
                        class: "btn",
                        disabled: folder_busy_value,
                        onclick: move |_| {
                            spawn(async move {
                                folder_busy.set(true);
                                match ipc::pick_folder().await {
                                    Ok(Some(path)) => {
                                        let next = AppSettings {
                                            local_folder: path,
                                            ..(state.settings)()
                                        };
                                        let _ = state.persist_settings(next).await;
                                    }
                                    // Отмена диалога — настройки не меняем.
                                    Ok(None) => {}
                                    Err(e) => state.set_error(Some(e)),
                                }
                                folder_busy.set(false);
                            });
                        },
                        {pick_label}
                    }
                }
            }

            section { class: "settings-section",
                h2 { class: "section-title", {language_label} }
                select {
                    class: "settings-select",
                    value: lang_name(lang),
                    onchange: move |evt| {
                        let next_lang = parse_lang(&evt.value());
                        spawn(async move {
                            // База читается в задаче (не в обработчике): видит последние
                            // оптимистичные изменения и не затирает их при быстрых правках.
                            if next_lang == (state.lang)() {
                                return;
                            }
                            let next = AppSettings { lang: next_lang, ..(state.settings)() };
                            let _ = state.persist_settings(next).await;
                        });
                    },
                    // Названия языков — эндонимы, не переводятся.
                    option { value: "ru", "Русский" }
                    option { value: "en", "English" }
                }
            }

            section { class: "settings-section",
                h2 { class: "section-title", {theme_label} }
                select {
                    class: "settings-select",
                    value: theme_name(theme),
                    onchange: move |evt| {
                        let next_theme = parse_theme(&evt.value());
                        spawn(async move {
                            if next_theme == (state.theme)() {
                                return;
                            }
                            let next = AppSettings { theme: next_theme, ..(state.settings)() };
                            let _ = state.persist_settings(next).await;
                        });
                    },
                    option { value: "dark", {dark_label} }
                    option { value: "light", {light_label} }
                    option { value: "system", {system_label} }
                }
            }

            section { class: "settings-section",
                label { class: "checkbox-row",
                    input {
                        r#type: "checkbox",
                        checked: settings.show_real_balance,
                        onchange: move |evt| {
                            let show_real = evt.value() == "true";
                            spawn(async move {
                                let next = AppSettings { show_real_balance: show_real, ..(state.settings)() };
                                // Баланс перечитываем только после успешного сохранения:
                                // иначе откат вернёт тумблер, а баннер ошибки сотрётся.
                                if state.persist_settings(next).await.is_ok() {
                                    state.refresh_balance().await;
                                }
                            });
                        },
                    }
                    {show_real_label}
                }
            }

            section { class: "settings-section",
                h2 { class: "section-title", {balance_label} }
                {balance_view}
                div { class: "field-row",
                    button {
                        class: "btn",
                        onclick: move |_| {
                            spawn(async move { state.refresh_balance().await });
                        },
                        {refresh_label}
                    }
                }
            }
        }
    }
}

/// Проверяет соединение сохранившимся ключом и обновляет статус поля.
///
/// Блокирует кнопки ключа на время запроса (`key_busy`).
async fn check_connection(mut key_busy: Signal<bool>, mut connection: Signal<ConnectionStatus>) {
    key_busy.set(true);
    connection.set(ConnectionStatus::Checking);
    connection.set(connection_status(ipc::test_connection().await));
    key_busy.set(false);
}

/// Сохраняет ключ в keychain; при успехе очищает поле и проверяет соединение.
///
/// Пустой ключ удаляет сохранённый (семантика команды). Ошибка keychain
/// показывается как [`ConnectionStatus::KeychainError`] — поле не очищается,
/// чтобы ключ можно было ввести повторно.
async fn save_key(
    key: String,
    mut key_input: Signal<String>,
    mut key_busy: Signal<bool>,
    mut connection: Signal<ConnectionStatus>,
) {
    key_busy.set(true);
    match ipc::set_api_key(&key).await {
        Ok(()) => {
            key_input.set(String::new());
            check_connection(key_busy, connection).await;
        }
        Err(e) => {
            connection.set(ConnectionStatus::KeychainError(e));
            key_busy.set(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn balance(real: Option<AccountBalance>) -> BalanceView {
        BalanceView {
            real,
            local_spend_usd: 0.04,
        }
    }

    fn account(available: f64) -> AccountBalance {
        AccountBalance {
            currency: "USD".to_owned(),
            balance_usd: available,
            available_usd: available,
            reserved_usd: 0.0,
        }
    }

    #[test]
    fn connection_status_accepted_key_is_ok() {
        assert_eq!(connection_status(Ok(true)), ConnectionStatus::Ok);
    }

    #[test]
    fn connection_status_false_is_rejected_not_error() {
        // false — ключа нет или 401, а не сетевая ошибка.
        assert_eq!(connection_status(Ok(false)), ConnectionStatus::Rejected);
    }

    #[test]
    fn connection_status_err_is_unverified_with_reason() {
        // Сеть/5xx — «подключение не проверено», причина сохраняется.
        assert_eq!(
            connection_status(Err("сеть недоступна".to_owned())),
            ConnectionStatus::Unverified("сеть недоступна".to_owned())
        );
    }

    #[test]
    fn real_balance_shown_only_when_enabled_and_present() {
        let view = balance(Some(account(12.5)));
        assert_eq!(shown_real_balance(&view, true), Some(&account(12.5)));
        assert_eq!(shown_real_balance(&view, false), None);
    }

    #[test]
    fn real_balance_absent_falls_back_to_local() {
        let view = balance(None);
        assert_eq!(shown_real_balance(&view, true), None);
        assert_eq!(shown_real_balance(&view, false), None);
    }
}
