//! Типизированный IPC-биндинг UI к Tauri: команды ядра и события.
//!
//! 13 обёрток над командами `src-tauri` (типы [`km_shared`] на входе и выходе)
//! и подписка [`on_event`] на `generation:started/done/failed`, `balance:updated`.
//! Аргументы — serde JSON с camelCase-ключами (`settings`, `key`, `kind`, `req`,
//! `filter`, `id`, `path`); ошибки IPC и разбора приводятся к `String` для UI.
//!
//! ## Свой glue вместо tauri-sys
//!
//! `tauri-sys` не опубликован на crates.io (только git-зависимость), а его
//! JS-прослойка — тривиальная передача в `window.__TAURI_INTERNALS__`.
//! Поэтому здесь минимальный wasm-bindgen glue (допустимо spec): `invoke` и
//! `listen` из `__TAURI_INTERNALS__`, без npm и бандлера. JSON пересекает
//! границу строками, поэтому вся сериализация — чистый `serde_json` на стороне
//! Rust и тестируется нативно (`cargo test -p km-ui`).

use std::rc::Rc;

use km_shared::{
    AppEvent, BalanceView, CatalogModel, GenerationRequest, HistoryFilter, HistoryRecord,
    MediaKind, Settings,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// Минимальный JS-glue к `window.__TAURI_INTERNALS__` (без npm/бандлера).
mod glue {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(inline_js = r#"
export async function invoke(cmd, argsJson) {
    const internals = window.__TAURI_INTERNALS__;
    if (!internals) {
        throw new Error("Tauri IPC недоступен: страница открыта не в окне Tauri");
    }
    const result = await internals.invoke(cmd, JSON.parse(argsJson));
    return JSON.stringify(result);
}

export function listen(event, callback) {
    const internals = window.__TAURI_INTERNALS__;
    if (!internals) {
        console.error("Tauri IPC недоступен: подписка на событие невозможна", event);
        return;
    }
    const handler = (e) => callback(JSON.stringify(e.payload));
    const handlerId = internals.transformCallback(handler);
    internals
        .invoke("plugin:event|listen", {
            event,
            target: { kind: "Any" },
            handler: handlerId,
        })
        .catch((e) => console.error("не удалось подписаться на событие", event, e));
}
"#)]
    extern "C" {
        /// Вызов Tauri-команды: ответ — JSON-строка, reject promise — ошибка.
        pub(super) fn invoke(cmd: &str, args_json: &str) -> js_sys::Promise;
        /// Подписка на событие ядра; callback получает payload JSON-строкой.
        pub(super) fn listen(event: &str, callback: &Closure<dyn Fn(String)>);
    }
}

/// Вызывает Tauri-команду и десериализует JSON-ответ в тип `T`.
///
/// Reject promise (ошибка команды) и сбой разбора ответа приводятся к строке.
async fn invoke_command<T: DeserializeOwned>(cmd: &str, args: Value) -> Result<T, String> {
    let result = JsFuture::from(glue::invoke(cmd, &args.to_string()))
        .await
        .map_err(js_error_to_string)?;
    let json = result
        .as_string()
        .ok_or_else(|| format!("команда {cmd} вернула не JSON-строку"))?;
    serde_json::from_str(&json)
        .map_err(|e| format!("не удалось разобрать ответ команды {cmd}: {e}"))
}

/// Приводит ошибку JS (reject promise) к строке для UI.
fn js_error_to_string(value: JsValue) -> String {
    if let Some(text) = value.as_string() {
        return text;
    }
    if let Some(error) = value.dyn_ref::<js_sys::Error>() {
        return error.message().into();
    }
    format!("{value:?}")
}

// --- Аргументы команд: camelCase-ключи, значения — serde JSON ---
// Чистые функции: сериализация тестируется нативно (cargo test -p km-ui).

/// Аргументы `set_settings`: настройки под ключом `settings`.
fn settings_args(settings: &Settings) -> Value {
    json!({ "settings": settings })
}

/// Аргументы `set_api_key`: ключ под ключом `key`.
fn api_key_args(key: &str) -> Value {
    json!({ "key": key })
}

/// Аргументы `fetch_models`: вид медиа под ключом `kind` (lowercase).
fn models_args(kind: MediaKind) -> Value {
    json!({ "kind": kind })
}

/// Аргументы `generate`: запрос под ключом `req`.
fn generate_args(req: &GenerationRequest) -> Value {
    json!({ "req": req })
}

/// Аргументы `list_history`: фильтр под ключом `filter`.
fn history_args(filter: &HistoryFilter) -> Value {
    json!({ "filter": filter })
}

/// Аргументы `delete_history`: идентификатор под ключом `id`.
fn id_args(id: &str) -> Value {
    json!({ "id": id })
}

/// Аргументы `open_file`/`show_in_folder`: путь под ключом `path`.
fn path_args(path: &str) -> Value {
    json!({ "path": path })
}

// --- Обёртки над командами Tauri (src-tauri/src/commands.rs) ---

/// Настройки приложения из SQLite.
///
/// # Errors
///
/// Строка с причиной при сбое IPC или хранилища.
pub async fn get_settings() -> Result<Settings, String> {
    invoke_command("get_settings", json!({})).await
}

/// Сохраняет настройки в SQLite (ключ API здесь не хранится).
///
/// # Errors
///
/// Строка с причиной при сбое IPC или записи.
pub async fn set_settings(s: &Settings) -> Result<(), String> {
    invoke_command("set_settings", settings_args(s)).await
}

/// Сохраняет API-ключ в системный keychain; пустой ключ удаляет сохранённый.
///
/// # Errors
///
/// Строка с причиной, если keychain не принял ключ.
pub async fn set_api_key(key: &str) -> Result<(), String> {
    invoke_command("set_api_key", api_key_args(key)).await
}

/// Проверка подключения к Cheaper Inference.
///
/// `true` — ключ принят, `false` — ключа нет или CI ответил 401.
///
/// # Errors
///
/// Строка с причиной при ошибках, отличных от 401 (сеть, 5xx).
pub async fn test_connection() -> Result<bool, String> {
    invoke_command("test_connection", json!({})).await
}

/// Каталог моделей по виду медиа; без ключа — публичный каталог.
///
/// # Errors
///
/// Строка с причиной при сбое запроса или разбора ответа.
pub async fn fetch_models(kind: MediaKind) -> Result<Vec<CatalogModel>, String> {
    invoke_command("fetch_models", models_args(kind)).await
}

/// Запуск генерации; после успеха ядро шлёт событие `balance:updated`.
///
/// # Errors
///
/// Строка с причиной: нет ключа, не задана папка сохранения, ошибки ядра.
pub async fn generate(req: &GenerationRequest) -> Result<HistoryRecord, String> {
    invoke_command("generate", generate_args(req)).await
}

/// Список истории с фильтром, новые первыми.
///
/// # Errors
///
/// Строка с причиной, если хранилище не читается.
pub async fn list_history(filter: &HistoryFilter) -> Result<Vec<HistoryRecord>, String> {
    invoke_command("list_history", history_args(filter)).await
}

/// Удаляет запись истории; файлы и remote не трогает.
///
/// # Errors
///
/// Строка с причиной, если удаление не удалось.
pub async fn delete_history(id: &str) -> Result<(), String> {
    invoke_command("delete_history", id_args(id)).await
}

/// Восстанавливает `Pending`-записи, пережившие перезапуск.
///
/// # Errors
///
/// Строка с причиной при сбое хранилища; ошибки CI отдельных записей
/// переводят их в `Failed`, а не прерывают восстановление.
pub async fn recover_pending() -> Result<Vec<HistoryRecord>, String> {
    invoke_command("recover_pending", json!({})).await
}

/// Гибридный баланс: реальный из CI (если включён) плюс локальный расход.
///
/// # Errors
///
/// Строка с причиной при сбое хранилища или запроса баланса.
pub async fn fetch_balance() -> Result<BalanceView, String> {
    invoke_command("fetch_balance", json!({})).await
}

/// Выбор папки сохранения в нативном диалоге; `None` — пользователь отменил.
///
/// # Errors
///
/// Строка с причиной, если диалог не завершился.
pub async fn pick_folder() -> Result<Option<String>, String> {
    invoke_command("pick_folder", json!({})).await
}

/// Открывает файл в приложении ОС; путь только внутри папки сохранения.
///
/// # Errors
///
/// Строка с причиной: папка не задана, файл не существует, путь вне папки.
pub async fn open_file(path: &str) -> Result<(), String> {
    invoke_command("open_file", path_args(path)).await
}

/// Показывает файл в папке (Finder/Explorer); путь только внутри папки сохранения.
///
/// # Errors
///
/// Те же случаи, что у [`open_file`].
pub async fn show_in_folder(path: &str) -> Result<(), String> {
    invoke_command("show_in_folder", path_args(path)).await
}

// --- События ядра ---

/// События ядра, на которые подписывается UI (имена из `src-tauri`).
const EVENT_NAMES: [&str; 4] = [
    "generation:started",
    "generation:done",
    "generation:failed",
    "balance:updated",
];

/// Разбирает JSON-payload Tauri-события в [`AppEvent`].
///
/// Payload externally-tagged snake_case: `{"generation_started":{"history_id":…}}`;
/// `balance:updated` несёт строку `"balance_updated"` (unit-вариант).
fn parse_event_payload(payload: &str) -> Result<AppEvent, String> {
    serde_json::from_str(payload).map_err(|e| format!("не удалось разобрать payload события: {e}"))
}

/// Подписывается на события ядра и вызывает `cb` с разобранным [`AppEvent`].
///
/// Подписка живёт всё время приложения (отписка не предусмотрена). Ошибки
/// разбора payload логируются в консоль webview и не прерывают подписку.
pub fn on_event<F: Fn(AppEvent) + 'static>(cb: F) {
    let cb = Rc::new(cb);
    for name in EVENT_NAMES {
        let cb = Rc::clone(&cb);
        let callback: Closure<dyn Fn(String)> =
            Closure::new(move |payload: String| match parse_event_payload(&payload) {
                Ok(event) => cb(event),
                Err(e) => web_sys::console::error_1(&format!("{name}: {e}").into()),
            });
        glue::listen(name, &callback);
        // Подписка постоянная — closure не должна быть уничтожена.
        callback.forget();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use km_shared::{GenMode, Lang, Theme};

    fn settings() -> Settings {
        Settings {
            local_folder: "/media".to_owned(),
            lang: Lang::Ru,
            theme: Theme::Dark,
            show_real_balance: true,
        }
    }

    #[test]
    fn models_args_serializes_kind_lowercase() {
        assert_eq!(models_args(MediaKind::Image), json!({"kind": "image"}));
        assert_eq!(models_args(MediaKind::Video), json!({"kind": "video"}));
        assert_eq!(models_args(MediaKind::Audio), json!({"kind": "audio"}));
    }

    #[test]
    fn settings_args_uses_camel_case_key_and_lowercase_enums() {
        let args = settings_args(&settings());
        assert_eq!(args["settings"]["local_folder"], json!("/media"));
        assert_eq!(args["settings"]["lang"], json!("ru"));
        assert_eq!(args["settings"]["theme"], json!("dark"));
        assert_eq!(args["settings"]["show_real_balance"], json!(true));
    }

    #[test]
    fn generate_args_wraps_req_with_snake_case_mode() {
        let req = GenerationRequest {
            kind: MediaKind::Image,
            mode: GenMode::TextToImage,
            model: "gpt-image-1".to_owned(),
            prompt: "кот".to_owned(),
            params: Default::default(),
            references: vec![],
            mask_data_url: None,
        };
        let args = generate_args(&req);
        assert_eq!(args["req"]["kind"], json!("image"));
        assert_eq!(args["req"]["mode"], json!("text_to_image"));
        assert_eq!(args["req"]["model"], json!("gpt-image-1"));
        assert_eq!(args["req"]["prompt"], json!("кот"));
    }

    #[test]
    fn history_args_wraps_filter() {
        let filter = HistoryFilter {
            kind: Some(MediaKind::Video),
            search: Some("кот".to_owned()),
        };
        let args = history_args(&filter);
        assert_eq!(args["filter"]["kind"], json!("video"));
        assert_eq!(args["filter"]["search"], json!("кот"));
        // Пустой фильтр сериализуется целиком (Option -> null).
        assert_eq!(
            history_args(&HistoryFilter::default()),
            json!({"filter": {"kind": null, "search": null}})
        );
    }

    #[test]
    fn scalar_args_use_command_keys() {
        assert_eq!(api_key_args("sk-123"), json!({"key": "sk-123"}));
        assert_eq!(id_args("abc"), json!({"id": "abc"}));
        assert_eq!(path_args("/media/a.png"), json!({"path": "/media/a.png"}));
    }

    #[test]
    fn parses_generation_started_payload() {
        let event = parse_event_payload(r#"{"generation_started":{"history_id":"h1"}}"#).unwrap();
        assert_eq!(
            event,
            AppEvent::GenerationStarted {
                history_id: "h1".to_owned()
            }
        );
    }

    #[test]
    fn parses_generation_done_and_failed_payloads() {
        let done = parse_event_payload(r#"{"generation_done":{"history_id":"h2"}}"#).unwrap();
        assert_eq!(
            done,
            AppEvent::GenerationDone {
                history_id: "h2".to_owned()
            }
        );
        let failed = parse_event_payload(
            r#"{"generation_failed":{"history_id":"h3","error":"сеть недоступна"}}"#,
        )
        .unwrap();
        assert_eq!(
            failed,
            AppEvent::GenerationFailed {
                history_id: "h3".to_owned(),
                error: "сеть недоступна".to_owned()
            }
        );
    }

    #[test]
    fn parses_balance_updated_unit_payload() {
        // balance:updated несёт JSON-строку "balance_updated" (unit-вариант).
        let event = parse_event_payload(r#""balance_updated""#).unwrap();
        assert_eq!(event, AppEvent::BalanceUpdated);
    }

    #[test]
    fn invalid_payload_is_error_not_panic() {
        assert!(parse_event_payload("{}").is_err());
        assert!(parse_event_payload("not json").is_err());
        assert!(parse_event_payload(r#"{"generation_started":{}}"#).is_err());
        assert!(parse_event_payload(r#"{"unknown_event":{}}"#).is_err());
    }

    #[test]
    fn roundtrip_app_event_payloads() {
        // Контракт с src-tauri: serde AppEvent -> JSON -> parse_event_payload.
        let events = [
            AppEvent::GenerationStarted {
                history_id: "h1".to_owned(),
            },
            AppEvent::GenerationDone {
                history_id: "h2".to_owned(),
            },
            AppEvent::GenerationFailed {
                history_id: "h3".to_owned(),
                error: "сеть".to_owned(),
            },
            AppEvent::BalanceUpdated,
        ];
        for event in events {
            let payload = serde_json::to_string(&event).unwrap();
            assert_eq!(
                parse_event_payload(&payload).unwrap(),
                event,
                "payload {payload}"
            );
        }
    }
}
