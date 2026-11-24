//! HTTP-клиент Cheaper Inference: транспорт и разбор ошибок CI.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::error::CoreError;

/// Базовый URL API Cheaper Inference.
const DEFAULT_BASE: &str = "https://api.cheaperinference.com";

/// Таймаут запроса по умолчанию; видео-генерация переопределит его в Task 8.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Асинхронный HTTP-клиент Cheaper Inference.
///
/// Оборачивает [`reqwest::Client`] с таймаутом 60 секунд и подставляет
/// заголовок `Authorization: Bearer <ключ>` в каждый запрос.
/// Дёшево клонируется: внутренний клиент reqwest разделяется через `Arc`.
#[derive(Clone)]
pub struct CiClient {
    http: reqwest::Client,
    base: String,
    key: String,
}

impl CiClient {
    /// Создаёт клиент с базовым URL `https://api.cheaperinference.com`.
    ///
    /// Пустой ключ допустим: наличие ключа проверяет вызывающий
    /// ([`CoreError::NoKey`]), а не конструктор.
    ///
    /// # Panics
    ///
    /// Паникует, если не удалось инициализировать TLS-бэкенд rustls;
    /// при корректной сборке это невозможно.
    pub fn new(key: &str) -> CiClient {
        CiClient::with_base(key, DEFAULT_BASE)
    }

    /// Создаёт клиент с произвольным базовым URL (для тестов).
    ///
    /// # Panics
    ///
    /// Паникует, если не удалось инициализировать TLS-бэкенд rustls;
    /// при корректной сборке это невозможно.
    pub fn with_base(key: &str, base: &str) -> CiClient {
        let http = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .expect("reqwest с rustls должен инициализироваться");
        CiClient {
            http,
            base: base.trim_end_matches('/').to_owned(),
            key: key.to_owned(),
        }
    }

    /// GET-запрос к `path` (например `/v1/models`) с разбором JSON-ответа в `T`.
    ///
    /// # Errors
    ///
    /// [`CoreError::Http`] при неуспешном статусе, [`CoreError::ScopeMissing`]
    /// при 403 на account-путях, [`CoreError::Serde`] при невалидном JSON,
    /// [`CoreError::Timeout`] / [`CoreError::Network`] при сбое транспорта.
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, CoreError> {
        let response = self
            .http
            .get(self.url(path))
            .bearer_auth(&self.key)
            .send()
            .await
            .map_err(map_transport_error)?;
        decode(response, path).await
    }

    /// GET-запрос к публичному пути CI без заголовка `Authorization`.
    ///
    /// Публичные пути (например `/public/models`) не требуют ключа; ключ,
    /// даже если задан в клиенте, в такой запрос не попадает.
    ///
    /// # Errors
    ///
    /// Те же случаи, что и у [`CiClient::get_json`].
    pub(crate) async fn get_public_json<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, CoreError> {
        let response = self
            .http
            .get(self.url(path))
            .send()
            .await
            .map_err(map_transport_error)?;
        decode(response, path).await
    }

    /// POST-запрос к `path` с JSON-телом `body` и разбором ответа в `T`.
    ///
    /// # Errors
    ///
    /// Те же случаи, что и у [`CiClient::get_json`].
    pub async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, CoreError> {
        let response = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.key)
            .json(body)
            .send()
            .await
            .map_err(map_transport_error)?;
        decode(response, path).await
    }

    /// Склеивает базовый URL и путь без двойных слешей.
    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }
}

/// Debug без утечки API-ключа.
impl fmt::Debug for CiClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CiClient")
            .field("base", &self.base)
            .field("key_set", &!self.key.is_empty())
            .finish_non_exhaustive()
    }
}

// Клиент должен быть Send для использования в асинхронных Tauri-командах.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<CiClient>();
};

/// OpenAI-конверт ошибки CI: `{"error":{"code":...,"message":...}}`.
#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

/// Поля ошибки CI; `code` приходит строкой или числом.
#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    code: Option<Value>,
    #[serde(default)]
    message: Option<String>,
}

/// Разбирает ответ: успешный статус — JSON в `T`, иначе — [`CoreError`].
async fn decode<T: DeserializeOwned>(
    response: reqwest::Response,
    path: &str,
) -> Result<T, CoreError> {
    let status = response.status();
    if !status.is_success() {
        return Err(http_error(status, path, response).await);
    }
    let body = response.text().await.map_err(map_transport_error)?;
    serde_json::from_str(&body).map_err(|e| CoreError::Serde(e.to_string()))
}

/// Собирает [`CoreError`] из ошибочного HTTP-ответа CI.
async fn http_error(
    status: reqwest::StatusCode,
    path: &str,
    response: reqwest::Response,
) -> CoreError {
    // 403 на account-эндпоинтах — отдельный случай: ключ без scope account:read.
    if status == reqwest::StatusCode::FORBIDDEN && is_account_path(path) {
        return CoreError::ScopeMissing;
    }
    // Тело ошибки читаем по возможности: при сбое чтения остаётся статус.
    let raw = response.text().await.unwrap_or_default();
    let envelope: Option<ErrorEnvelope> = serde_json::from_str(&raw).ok();
    let code = envelope
        .as_ref()
        .and_then(|e| normalize_code(e.error.code.as_ref()));
    let message = envelope
        .and_then(|e| e.error.message)
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| fallback_message(&raw, status));
    CoreError::Http {
        status: status.as_u16(),
        code,
        message,
    }
}

/// Сообщение по умолчанию: сырое тело, либо текст статуса, если тело пустое.
fn fallback_message(raw: &str, status: reqwest::StatusCode) -> String {
    if raw.trim().is_empty() {
        status
            .canonical_reason()
            .unwrap_or("HTTP-ошибка")
            .to_owned()
    } else {
        raw.to_owned()
    }
}

/// Нормализует `code` CI (строка или число) в `Option<String>`.
fn normalize_code(code: Option<&Value>) -> Option<String> {
    match code {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// Путь относится к account-эндпоинтам (`/v1/account/...`).
fn is_account_path(path: &str) -> bool {
    let path = path.trim_start_matches('/');
    path == "v1/account" || path.starts_with("v1/account/")
}

/// Ошибка транспорта reqwest: таймаут отдельно, остальное — сеть.
fn map_transport_error(err: reqwest::Error) -> CoreError {
    if err.is_timeout() {
        CoreError::Timeout
    } else {
        CoreError::Network(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn url_joins_without_double_slashes() {
        let client = CiClient::with_base("k", "http://localhost:8080/");
        assert_eq!(client.url("/v1/models"), "http://localhost:8080/v1/models");
        assert_eq!(client.url("v1/models"), "http://localhost:8080/v1/models");
    }

    #[test]
    fn account_paths_are_recognized() {
        assert!(is_account_path("/v1/account/balance"));
        assert!(is_account_path("/v1/account"));
        assert!(!is_account_path("/v1/images/generations"));
        assert!(!is_account_path("/v1/accounting"));
    }

    #[test]
    fn code_normalizes_strings_and_numbers() {
        assert_eq!(
            normalize_code(Some(&json!("invalid_api_key"))),
            Some("invalid_api_key".to_owned())
        );
        assert_eq!(normalize_code(Some(&json!(429))), Some("429".to_owned()));
        assert_eq!(normalize_code(Some(&Value::Null)), None);
        assert_eq!(normalize_code(None), None);
    }

    #[test]
    fn futures_are_send() {
        fn assert_send<T: Send>(_: T) {}
        let client = CiClient::with_base("k", "http://localhost:1");
        assert_send(client.get_json::<Value>("/v1/models"));
    }
}
