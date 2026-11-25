//! Экран онбординга первого запуска: приветствие → ключ → папка → готово.
//!
//! Шаги — локальное состояние экрана ([`Step`]); навигация между шагами внутри
//! экрана, маршрут не меняется. Язык применяется сразу (persist), папка
//! предзаполняется из настроек и сохраняется на финальном шаге вместе с
//! остальными настройками через [`AppState::persist_settings`] (generation
//! guard отката).
//!
//! Пропуск ключа допустим (каталог доступен без ключа), пропуск папки — с
//! предупреждением: генерация требует и ключ, и папку (`cmd_generate`), задать
//! их можно позже в Настройках. Завершение ставит `onboarding_done` и уводит
//! в Studio; редирект первого запуска — в [`crate::app`].
//!
//! Экран рендерится вне [`crate::components::layout::Layout`] (без сайдбара),
//! поэтому баннер ошибки IPC показывается здесь самостоятельно.

use dioxus::prelude::*;
use km_shared::Settings as AppSettings;

use crate::ipc;
use crate::router::Route;
use crate::state::{AppState, lang_name, parse_lang, use_t};

/// Шаг онбординга; порядок фиксирован (приветствие → ключ → папка → готово).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Приветствие и выбор языка интерфейса.
    Welcome,
    /// Ввод API-ключа и проверка соединения.
    Key,
    /// Выбор папки сохранения.
    Folder,
    /// Итог и переход в Studio.
    Done,
}

impl Step {
    /// Номер шага для индикатора прогресса (1..=4).
    fn number(self) -> u8 {
        match self {
            Step::Welcome => 1,
            Step::Key => 2,
            Step::Folder => 3,
            Step::Done => 4,
        }
    }

    /// Следующий шаг; `None` — шаг последний.
    fn next(self) -> Option<Step> {
        match self {
            Step::Welcome => Some(Step::Key),
            Step::Key => Some(Step::Folder),
            Step::Folder => Some(Step::Done),
            Step::Done => None,
        }
    }

    /// Предыдущий шаг; `None` — шаг первый.
    fn prev(self) -> Option<Step> {
        match self {
            Step::Welcome => None,
            Step::Key => Some(Step::Welcome),
            Step::Folder => Some(Step::Key),
            Step::Done => Some(Step::Folder),
        }
    }
}

/// Статус проверки подключения на шаге ключа.
///
/// Семантика как в Настройках: принятый ключ, отсутствие/401, непроверенное
/// соединение (сеть/5xx) и ошибка keychain — разные причины для пользователя.
#[derive(Clone, Debug, PartialEq)]
enum KeyStatus {
    /// Ключ ещё не сохранялся.
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

/// Переводит результат [`crate::ipc::test_connection`] в статус шага ключа.
fn key_status(result: Result<bool, String>) -> KeyStatus {
    match result {
        Ok(true) => KeyStatus::Ok,
        Ok(false) => KeyStatus::Rejected,
        Err(e) => KeyStatus::Unverified(e),
    }
}

/// Можно ли перейти дальше с шага ключа.
///
/// Ключ принят (`Ok`) либо сохранён, но соединение не проверено (`Unverified`,
/// сеть/5xx) — ключ в keychain есть, пропускаем. Отклонён/не сохранён — нельзя:
/// нужен корректный ключ или осознанный пропуск.
fn key_step_can_proceed(status: &KeyStatus) -> bool {
    matches!(status, KeyStatus::Ok | KeyStatus::Unverified(_))
}

/// Можно ли перейти дальше с шага папки: папка выбрана (непустой путь).
fn folder_step_can_proceed(folder: &str) -> bool {
    !folder.is_empty()
}

/// Экран онбординга первого запуска: приветствие, ключ, папка, итог.
#[component]
pub fn Onboarding() -> Element {
    let state = use_context::<AppState>();

    // Переводы — до условных ветвей rsx: use_t основан на хуке, порядок стабилен.
    let welcome = use_t("onboarding.welcome");
    let intro = use_t("onboarding.intro");
    let step_key = use_t("onboarding.step_key");
    let key_hint = use_t("onboarding.key_hint");
    let step_folder = use_t("onboarding.step_folder");
    let folder_hint = use_t("onboarding.folder_hint");
    let done_label = use_t("onboarding.done");
    let done_text = use_t("onboarding.done_text");
    let finish_label = use_t("onboarding.finish");
    let next_label = use_t("onboarding.next");
    let back_label = use_t("onboarding.back");
    let skip_label = use_t("onboarding.skip");
    let skip_key_warning = use_t("onboarding.skip_key_warning");
    let skip_folder_warning = use_t("onboarding.skip_folder_warning");
    let language_label = use_t("settings.language");
    let key_label = use_t("settings.api_key");
    let folder_label = use_t("settings.folder");
    let pick_label = use_t("settings.pick_folder");
    let folder_missing_label = use_t("settings.folder_missing");
    let save_label = use_t("common.save");
    let check_label = use_t("settings.check_connection");
    let checking_label = use_t("settings.checking");
    let ok_label = use_t("settings.connection_ok");
    let failed_label = use_t("settings.connection_failed");
    let rejected_label = use_t("settings.connection_rejected");
    let unverified_label = use_t("settings.connection_unverified");
    let keychain_label = use_t("settings.keychain_error");
    let error_label = use_t("common.error");
    let close_label = use_t("common.close");

    // Локальное состояние шагов (живёт в scope экрана; маршрут не меняется).
    // Папка предзаполняется из настроек: повторный онбординг (например, ключ
    // отклонён) сохраняет уже выбранный путь; первый запуск — пустая папка.
    let initial_folder = (state.settings)().local_folder;
    let mut step = use_signal(|| Step::Welcome);
    let mut key_input = use_signal(String::new);
    let key_busy = use_signal(|| false);
    let status = use_signal(|| KeyStatus::Idle);
    let mut key_skipped = use_signal(|| false);
    let mut folder = use_signal(move || initial_folder);
    let mut folder_busy = use_signal(|| false);
    let finishing = use_signal(|| false);

    let current_step = (step)();
    let lang = (state.lang)();
    let error = (state.error)();
    let key_busy_value = (key_busy)();
    let folder_busy_value = (folder_busy)();
    let finishing_value = (finishing)();
    let key_input_value = (key_input)();
    let key_status_value = (status)();
    let key_skipped_value = (key_skipped)();
    let folder_value = (folder)();
    let key_can_proceed = key_step_can_proceed(&key_status_value);
    let folder_can_proceed = folder_step_can_proceed(&folder_value);

    // Индикатор прогресса: точки шагов + счётчик «N / 4» (без перевода).
    let steps_view = rsx! {
        div { class: "onboarding-steps", aria_hidden: "true",
            for s in [Step::Welcome, Step::Key, Step::Folder, Step::Done] {
                span {
                    class: if s == current_step {
                        "onboarding-dot active"
                    } else if s.number() < current_step.number() {
                        "onboarding-dot done"
                    } else {
                        "onboarding-dot"
                    },
                }
            }
            span { class: "onboarding-step-count", "{current_step.number()} / 4" }
        }
    };

    // Статус проверки ключа — отдельный Element, чтобы не ветвить хуки внутри rsx.
    let key_status_view = match &key_status_value {
        KeyStatus::Idle => rsx! {},
        KeyStatus::Checking => rsx! {
            p { class: "status status-muted", {checking_label} }
        },
        KeyStatus::Ok => rsx! {
            p { class: "status status-ok", "✓ " {ok_label} }
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
    };

    // Итог шага ключа для финального экрана: принят / не проверен / пропущен.
    let key_summary = if key_skipped_value {
        rsx! { p { class: "status status-error", {skip_key_warning} } }
    } else {
        match &key_status_value {
            KeyStatus::Ok => rsx! {
                p { class: "status status-ok", {key_label} ": ✓ " {ok_label} }
            },
            KeyStatus::Unverified(e) => rsx! {
                p { class: "status status-error", {key_label} ": " {unverified_label} ": " {e.as_str()} }
            },
            // Защита от непредусмотренного статуса: дальше с шага ключа без
            // сохранённого ключа не уйти (key_step_can_proceed), кроме пропуска.
            _ => rsx! {
                p { class: "status status-error", {key_label} ": " {rejected_label} }
            },
        }
    };

    let folder_summary = if folder_value.is_empty() {
        rsx! { p { class: "status status-error", {skip_folder_warning} } }
    } else {
        rsx! { p { class: "folder-path", {folder_label} ": " {folder_value.as_str()} } }
    };

    // Содержимое текущего шага.
    let step_view = match current_step {
        Step::Welcome => rsx! {
            h1 { {welcome} }
            p { class: "onboarding-intro", {intro} }
            div { class: "settings-section",
                span { class: "section-title", {language_label} }
                select {
                    class: "settings-select",
                    value: lang_name(lang),
                    onchange: move |evt| {
                        let next_lang = parse_lang(&evt.value());
                        spawn(async move {
                            // База читается в задаче: видит последние оптимистичные
                            // изменения и не затирает их при быстрых правках.
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
        },
        Step::Key => rsx! {
            h2 { class: "onboarding-title", {step_key} }
            p { class: "muted", {key_hint} }
            div { class: "field-row",
                input {
                    r#type: "password",
                    class: "text-input",
                    autocomplete: "off",
                    placeholder: key_label,
                    value: "{key_input_value}",
                    disabled: key_busy_value,
                    oninput: move |evt| key_input.set(evt.value()),
                }
                button {
                    class: "btn btn-primary",
                    disabled: key_busy_value || key_input_value.trim().is_empty(),
                    onclick: move |_| {
                        let key = (key_input)();
                        spawn(async move {
                            save_key(key, key_input, key_busy, status, key_skipped).await;
                        });
                    },
                    {save_label}
                }
                button {
                    class: "btn",
                    disabled: key_busy_value,
                    onclick: move |_| {
                        spawn(async move { check_connection(key_busy, status).await });
                    },
                    {check_label}
                }
            }
            {key_status_view}
            p { class: "onboarding-hint", {skip_key_warning} }
        },
        Step::Folder => rsx! {
            h2 { class: "onboarding-title", {step_folder} }
            p { class: "muted", {folder_hint} }
            if key_skipped_value {
                div { class: "onboarding-warning", {skip_key_warning} }
            }
            div { class: "field-row",
                if folder_value.is_empty() {
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
                                Ok(Some(path)) => folder.set(path),
                                // Отмена диалога — выбор не меняем.
                                Ok(None) => {}
                                Err(e) => state.set_error(Some(e)),
                            }
                            folder_busy.set(false);
                        });
                    },
                    {pick_label}
                }
            }
            p { class: "onboarding-hint", {skip_folder_warning} }
        },
        Step::Done => rsx! {
            h2 { class: "onboarding-title", {done_label} }
            p { class: "onboarding-intro", {done_text} }
            div { class: "onboarding-summary",
                {key_summary}
                {folder_summary}
            }
        },
    };

    // Кнопки навигации между шагами; «Далее» блокируется до выполнения шага.
    // Переходы вперёд/назад — через Step::next/prev (порядок шагов в одном месте).
    let actions = match current_step {
        Step::Welcome => rsx! {
            button {
                class: "btn btn-primary",
                onclick: move |_| {
                    if let Some(next) = (step)().next() {
                        step.set(next);
                    }
                },
                {next_label}
            }
        },
        Step::Key => rsx! {
            button {
                class: "btn",
                onclick: move |_| {
                    if let Some(prev) = (step)().prev() {
                        step.set(prev);
                    }
                },
                {back_label}
            }
            button {
                class: "btn",
                disabled: key_busy_value,
                onclick: move |_| {
                    key_skipped.set(true);
                    step.set(Step::Folder);
                },
                {skip_label}
            }
            button {
                class: "btn btn-primary",
                disabled: key_busy_value || !key_can_proceed,
                onclick: move |_| {
                    if let Some(next) = (step)().next() {
                        step.set(next);
                    }
                },
                {next_label}
            }
        },
        Step::Folder => rsx! {
            button {
                class: "btn",
                onclick: move |_| {
                    if let Some(prev) = (step)().prev() {
                        step.set(prev);
                    }
                },
                {back_label}
            }
            button {
                class: "btn",
                disabled: folder_busy_value,
                onclick: move |_| {
                    folder.set(String::new());
                    step.set(Step::Done);
                },
                {skip_label}
            }
            button {
                class: "btn btn-primary",
                disabled: folder_busy_value || !folder_can_proceed,
                onclick: move |_| {
                    if let Some(next) = (step)().next() {
                        step.set(next);
                    }
                },
                {next_label}
            }
        },
        Step::Done => rsx! {
            button {
                class: "btn",
                disabled: finishing_value,
                onclick: move |_| {
                    if let Some(prev) = (step)().prev() {
                        step.set(prev);
                    }
                },
                {back_label}
            }
            button {
                class: "btn btn-primary",
                disabled: finishing_value,
                onclick: move |_| {
                    spawn(async move { finish(state, folder, finishing).await });
                },
                {finish_label}
            }
        },
    };

    rsx! {
        div { class: "onboarding",
            div { class: "onboarding-card",
                div { class: "onboarding-brand", "Koteyye Media Studio" }
                {steps_view}
                // Экран вне Layout: баннер ошибки IPC показываем здесь.
                if let Some(message) = error {
                    div { class: "error-banner",
                        span { class: "error-banner-text", {error_label} ": {message}" }
                        button {
                            class: "banner-dismiss",
                            title: close_label,
                            aria_label: close_label,
                            onclick: move |_| state.set_error(None),
                            "✕"
                        }
                    }
                }
                {step_view}
                div { class: "onboarding-actions", {actions} }
            }
        }
    }
}

/// Проверяет соединение сохранившимся ключом и обновляет статус шага.
///
/// Блокирует кнопки ключа на время запроса (`key_busy`).
async fn check_connection(mut key_busy: Signal<bool>, mut status: Signal<KeyStatus>) {
    key_busy.set(true);
    status.set(KeyStatus::Checking);
    status.set(key_status(ipc::test_connection().await));
    key_busy.set(false);
}

/// Сохраняет ключ в keychain; при успехе очищает поле, снимает пропуск и проверяет.
///
/// Ошибка keychain показывается как [`KeyStatus::KeychainError`] — поле не
/// очищается, чтобы ключ можно было ввести повторно.
async fn save_key(
    key: String,
    mut key_input: Signal<String>,
    mut key_busy: Signal<bool>,
    mut status: Signal<KeyStatus>,
    mut key_skipped: Signal<bool>,
) {
    key_busy.set(true);
    match ipc::set_api_key(&key).await {
        Ok(()) => {
            key_input.set(String::new());
            key_skipped.set(false);
            check_connection(key_busy, status).await;
        }
        Err(e) => {
            status.set(KeyStatus::KeychainError(e));
            key_busy.set(false);
        }
    }
}

/// Финал онбординга: сохраняет настройки и переходит в Studio.
///
/// Папка (выбранная или пустая при пропуске) записывается вместе с текущими
/// настройками через `persist_settings` (оптимистичное применение + откат с
/// generation guard при ошибке). При успехе ставит `onboarding_done` (редирект
/// больше не срабатывает) и заменяет маршрут на Studio; при отказе остаётся
/// на экране — ошибка видна в баннере, попытку можно повторить.
async fn finish(mut state: AppState, folder: Signal<String>, mut finishing: Signal<bool>) {
    finishing.set(true);
    let next = AppSettings {
        local_folder: (folder)(),
        ..(state.settings)()
    };
    if state.persist_settings(next).await.is_ok() {
        state.onboarding_done.set(true);
        navigator().replace(Route::Studio {
            kind: "image".to_owned(),
        });
    } else {
        finishing.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_numbers_are_one_based_in_order() {
        assert_eq!(Step::Welcome.number(), 1);
        assert_eq!(Step::Key.number(), 2);
        assert_eq!(Step::Folder.number(), 3);
        assert_eq!(Step::Done.number(), 4);
    }

    #[test]
    fn step_next_walks_forward_and_stops_at_done() {
        assert_eq!(Step::Welcome.next(), Some(Step::Key));
        assert_eq!(Step::Key.next(), Some(Step::Folder));
        assert_eq!(Step::Folder.next(), Some(Step::Done));
        assert_eq!(Step::Done.next(), None);
    }

    #[test]
    fn step_prev_walks_back_and_stops_at_welcome() {
        assert_eq!(Step::Welcome.prev(), None);
        assert_eq!(Step::Key.prev(), Some(Step::Welcome));
        assert_eq!(Step::Folder.prev(), Some(Step::Key));
        assert_eq!(Step::Done.prev(), Some(Step::Folder));
    }

    #[test]
    fn key_status_accepted_is_ok() {
        assert_eq!(key_status(Ok(true)), KeyStatus::Ok);
    }

    #[test]
    fn key_status_false_is_rejected_not_error() {
        // false — ключа нет или 401, а не сетевая ошибка.
        assert_eq!(key_status(Ok(false)), KeyStatus::Rejected);
    }

    #[test]
    fn key_status_err_is_unverified_with_reason() {
        assert_eq!(
            key_status(Err("сеть недоступна".to_owned())),
            KeyStatus::Unverified("сеть недоступна".to_owned())
        );
    }

    #[test]
    fn key_step_proceeds_with_accepted_or_saved_unverified_key() {
        // Ключ в keychain есть (принят или не проверен из-за сети) — дальше можно.
        assert!(key_step_can_proceed(&KeyStatus::Ok));
        assert!(key_step_can_proceed(&KeyStatus::Unverified(
            "сеть".to_owned()
        )));
    }

    #[test]
    fn key_step_blocked_without_saved_key() {
        assert!(!key_step_can_proceed(&KeyStatus::Idle));
        assert!(!key_step_can_proceed(&KeyStatus::Checking));
        assert!(!key_step_can_proceed(&KeyStatus::Rejected));
        assert!(!key_step_can_proceed(&KeyStatus::KeychainError(
            "связка".to_owned()
        )));
    }

    #[test]
    fn folder_step_proceeds_only_with_chosen_folder() {
        assert!(folder_step_can_proceed("/media"));
        assert!(!folder_step_can_proceed(""));
    }
}
