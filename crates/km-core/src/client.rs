//! HTTP-клиент Cheaper Inference: транспорт и разбор ошибок CI.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::error::CoreError;

/// Базовый URL API Cheaper Inference.
const DEFAULT_BASE: &str = "https://api.cheaperinference.com";

/// Таймаут запроса по умолчанию.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Таймаут синхронной видео-генерации: CI блокирует ответ до готовности
/// видео и может считать до 10 минут.
const VIDEO_TIMEOUT: Duration = Duration::from_secs(600);

/// Асинхронный HTTP-клиент Cheaper Inference.
///
/// Оборачивает [`reqwest::Client`] с таймаутом 60 секунд и подставляет
/// заголовок `Authorization: Bearer <ключ>` в каждый запрос. Видео-запросы
/// идут через отдельный клиент с таймаутом 600 секунд.
/// Дёшево клонируется: внутренние клиенты reqwest разделяются через `Arc`.
#[derive(Clone)]
pub struct CiClient {
    http: reqwest::Client,
    video_http: reqwest::Client,
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
        CiClient {
            http: build_http(DEFAULT_TIMEOUT),
            video_http: build_http(VIDEO_TIMEOUT),
            base: base.trim_end_matches('/').to_owned(),
            key: key.to_owned(),
        }
    }

    /// Задаёт таймаут всех запросов клиента, включая видео-генерацию.
    ///
    /// По умолчанию обычные запросы ограничены 60 секундами, видео —
    /// 600; явное значение переопределяет оба. Нужен тестам, чтобы
    /// проверить живой таймаут на короткой задержке.
    ///
    /// # Panics
    ///
    /// Паникует, если не удалось инициализировать TLS-бэкенд rustls;
    /// при корректной сборке это невозможно.
    pub fn with_timeout(mut self, timeout: Duration) -> CiClient {
        self.http = build_http(timeout);
        self.video_http = build_http(timeout);
        self
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

    /// POST-запрос к `path` через видео-клиент с таймаутом 600 секунд.
    ///
    /// Отличается от [`CiClient::post_json`] только клиентом и таймаутом:
    /// синхронная генерация видео блокирует ответ на минуты. Разбор ответа
    /// и ошибки транспорта — те же.
    pub(crate) async fn post_video_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, CoreError> {
        let response = self
            .video_http
            .post(self.url(path))
            .bearer_auth(&self.key)
            .json(body)
            .send()
            .await
            .map_err(map_transport_error)?;
        decode(response, path).await
    }

    /// POST-запрос к `path` с JSON-телом `body`; ответ — сырые байты и заголовки.
    ///
    /// В отличие от [`CiClient::post_json`], тело ответа не разбирается как JSON:
    /// метод нужен для бинарных ответов (аудио). Заголовки возвращаются целиком,
    /// чтобы вызывающий мог прочитать MIME-тип и стоимость.
    ///
    /// # Errors
    ///
    /// Те же случаи, что и у [`CiClient::get_json`], кроме [`CoreError::Serde`]:
    /// успешное тело не разбирается как JSON.
    pub async fn post_bytes<B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<(Vec<u8>, reqwest::header::HeaderMap), CoreError> {
        let response = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.key)
            .json(body)
            .send()
            .await
            .map_err(map_transport_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(http_error(status, path, response).await);
        }
        let headers = response.headers().clone();
        let bytes = response.bytes().await.map_err(map_transport_error)?;
        Ok((bytes.to_vec(), headers))
    }

    /// Скачивает байты по signed URL без заголовка `Authorization`.
    ///
    /// Signed URL уже содержит токен доступа в query-параметрах, поэтому
    /// ключ CI в такой запрос не подставляется и не утекает в хранилище
    /// файлов. URL абсолютный; базовый URL клиента не используется.
    ///
    /// # Errors
    ///
    /// [`CoreError::Http`] при неуспешном статусе, [`CoreError::Timeout`] /
    /// [`CoreError::Network`] при сбое транспорта.
    pub async fn download(&self, url: &str) -> Result<Vec<u8>, CoreError> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(map_transport_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(http_error(status, url, response).await);
        }
        let bytes = response.bytes().await.map_err(map_transport_error)?;
        Ok(bytes.to_vec())
    }

    /// Склеивает базовый URL и путь без двойных слешей.
    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }
}

/// Собирает reqwest-клиент с заданным таймаутом запроса.
///
/// # Panics
///
/// Паникует, если не удалось инициализировать TLS-бэкенд rustls;
/// при корректной сборке это невозможно.
fn build_http(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .expect("reqwest с rustls должен инициализироваться")
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
///
/// URL в сообщении очищается от query и fragment: signed URL несут токены
/// доступа в query-параметрах, которые не должны попадать в текст ошибки.
fn map_transport_error(err: reqwest::Error) -> CoreError {
    if err.is_timeout() {
        return CoreError::Timeout;
    }
    let mut message = err.to_string();
    if let Some(url) = err.url() {
        message = message.replace(url.as_str(), &redact_url(url));
    }
    CoreError::Network(message)
}

/// URL без query и fragment — для сообщений об ошибках.
fn redact_url(url: &reqwest::Url) -> String {
    let mut clean = url.clone();
    clean.set_query(None);
    clean.set_fragment(None);
    clean.to_string()
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
    fn redact_url_strips_query_and_fragment() {
        let url = reqwest::Url::parse("https://cdn.example.com/a.mp3?sig=secret#frag")
            .expect("URL должен разобрать");
        assert_eq!(redact_url(&url), "https://cdn.example.com/a.mp3");
        let plain =
            reqwest::Url::parse("https://api.example.com/v1/models").expect("URL должен разобрать");
        assert_eq!(redact_url(&plain), "https://api.example.com/v1/models");
    }

    #[test]
    fn futures_are_send() {
        fn assert_send<T: Send>(_: T) {}
        let client = CiClient::with_base("k", "http://localhost:1");
        assert_send(client.get_json::<Value>("/v1/models"));
    }
}
