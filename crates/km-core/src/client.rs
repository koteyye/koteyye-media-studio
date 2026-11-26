//! HTTP-клиент Cheaper Inference: транспорт и разбор ошибок CI.

use std::fmt;
use std::io::Write;
use std::time::Duration;

use km_shared::{
    AccountBalance, BalanceUnavailable, BalanceView, Billing, OpenRouterCredits, Provider,
};
use reqwest::header::HeaderMap;
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

/// Читает метаданные ключа и кредиты аккаунта OpenRouter без генерации.
/// Отдельный транспорт не может использовать Bearer клиента Cheaper Inference.
pub struct OpenRouterKeyClient {
    http: reqwest::Client,
    base: String,
}

impl Default for OpenRouterKeyClient {
    fn default() -> Self {
        Self::at("https://openrouter.ai/api/v1".into())
    }
}

impl OpenRouterKeyClient {
    fn at(base: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest с rustls должен инициализироваться");
        Self { http, base }
    }

    /// Мок для интеграционных тестов: endpoint всегда остаётся на loopback.
    #[doc(hidden)]
    pub fn for_localhost(port: u16) -> Self {
        Self::at(format!("http://127.0.0.1:{port}/api/v1"))
    }

    async fn get(&self, path: &str, key: &str) -> Result<reqwest::Response, CoreError> {
        self.http
            .get(format!("{}{path}", self.base))
            .bearer_auth(key)
            .send()
            .await
            .map_err(map_transport_error)
    }

    pub async fn verify(&self, key: &str) -> Result<bool, CoreError> {
        let response = self.get("/key", key).await?;
        match response.status() {
            reqwest::StatusCode::OK => {
                let bytes = response.bytes().await.map_err(map_transport_error)?;
                let parsed = serde_json::from_slice::<Value>(&bytes).ok();
                if parsed
                    .as_ref()
                    .and_then(|value| value.get("data"))
                    .is_some_and(Value::is_object)
                {
                    Ok(true)
                } else {
                    Err(CoreError::Serde(
                        "ответ OpenRouter не содержит объект data".into(),
                    ))
                }
            }
            reqwest::StatusCode::UNAUTHORIZED => Ok(false),
            status => Err(CoreError::Http {
                status: status.as_u16(),
                code: None,
                message: "OpenRouter не смог проверить ключ".into(),
            }),
        }
    }

    /// `/credits` принимает Management key и возвращает купленные и использованные
    /// кредиты аккаунта. `/key.limit_remaining` имеет другую область действия.
    pub async fn credits(&self, key: &str) -> Result<OpenRouterCredits, CoreError> {
        let response = self.get("/credits", key).await?;
        let status = response.status();
        if status != reqwest::StatusCode::OK {
            return Err(CoreError::Http {
                status: status.as_u16(),
                code: None,
                message: "OpenRouter не предоставил баланс аккаунта".into(),
            });
        }
        let bytes = response.bytes().await.map_err(map_transport_error)?;
        let invalid = || CoreError::Serde("неверный формат кредитов аккаунта OpenRouter".into());
        let body: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let data = body
            .get("data")
            .and_then(Value::as_object)
            .ok_or_else(invalid)?;
        let credits = OpenRouterCredits {
            total_credits: data
                .get("total_credits")
                .and_then(Value::as_f64)
                .ok_or_else(invalid)?,
            total_usage: data
                .get("total_usage")
                .and_then(Value::as_f64)
                .ok_or_else(invalid)?,
        };
        if !credits.remaining_usd().is_finite() {
            return Err(invalid());
        }
        Ok(credits)
    }

    pub async fn balance_view(&self, key: &str, local_spend_usd: f64) -> BalanceView {
        let mut view = BalanceView::local(Provider::OpenRouter, local_spend_usd);
        match self.credits(key).await {
            Ok(credits) => view.openrouter_credits = Some(credits),
            Err(err) => record_balance_error(&mut view, err, key),
        }
        view
    }
}

fn record_balance_error(view: &mut BalanceView, error: CoreError, key: &str) {
    view.unavailable = Some(match &error {
        CoreError::NoKey => BalanceUnavailable::MissingKey,
        CoreError::ScopeMissing => BalanceUnavailable::PermissionDenied,
        CoreError::Http { status: 401, .. } => BalanceUnavailable::RejectedKey,
        CoreError::Http { status: 403, .. } if view.provider == Provider::OpenRouter => {
            BalanceUnavailable::PermissionDenied
        }
        CoreError::Http { status: 403, .. } => BalanceUnavailable::AccessRestricted,
        CoreError::Network(_) | CoreError::Timeout => BalanceUnavailable::Network,
        _ => BalanceUnavailable::Other,
    });
    if let CoreError::Http { status, code, .. } = error {
        // Полное тело ответа кошелька может содержать данные аккаунта.
        let code = code.filter(|c| {
            !c.is_empty()
                && c.len() <= 64
                && !c.contains(key)
                && c.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        });
        view.real_error = Some(match code {
            Some(c) => format!("HTTP {status} ({c})"),
            None => format!("HTTP {status}"),
        });
    }
}

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
        self.post_json_with_headers(path, body)
            .await
            .map(|(body, _)| body)
    }

    pub(crate) async fn post_json_with_headers<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<(T, HeaderMap), CoreError> {
        self.post_with_client(&self.http, path, body).await
    }

    pub(crate) async fn post_video_json_with_headers<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<(T, HeaderMap), CoreError> {
        self.post_with_client(&self.video_http, path, body).await
    }

    async fn post_with_client<B: Serialize, T: DeserializeOwned>(
        &self,
        http: &reqwest::Client,
        path: &str,
        body: &B,
    ) -> Result<(T, HeaderMap), CoreError> {
        let json = serde_json::to_vec(body).map_err(|error| CoreError::Serde(error.to_string()))?;
        self.post_serialized(http, path, json).await
    }

    pub(crate) async fn post_serialized_image<T: DeserializeOwned>(
        &self,
        path: &str,
        json: Vec<u8>,
    ) -> Result<(T, HeaderMap), CoreError> {
        self.post_serialized(&self.http, path, json).await
    }

    async fn post_serialized<T: DeserializeOwned>(
        &self,
        http: &reqwest::Client,
        path: &str,
        json: Vec<u8>,
    ) -> Result<(T, HeaderMap), CoreError> {
        let mut request = http
            .post(self.url(path))
            .bearer_auth(&self.key)
            .header(reqwest::header::CONTENT_TYPE, "application/json");
        let json = if json.len() > km_shared::MAX_REQUEST_WIRE_BYTES
            && matches!(path, "/v1/videos/generations" | "/v1/images/generations")
        {
            request = request.header(reqwest::header::CONTENT_ENCODING, "gzip");
            compress_media_json(&json)?
        } else {
            json
        };
        let response = request
            .body(json)
            .send()
            .await
            .map_err(map_transport_error)?;
        Self::decode_post(response, path).await
    }

    /// Multipart для изображений: источники и маска передаются байтами. Заголовок
    /// Content-Length включает также границы формы и все текстовые поля.
    pub(crate) async fn post_multipart_json_with_headers<T: DeserializeOwned>(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> Result<(T, HeaderMap), CoreError> {
        let req = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.key)
            .multipart(form)
            .build()
            .map_err(map_transport_error)?;
        let length = req
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok());
        if length.is_none_or(|size| size > km_shared::MAX_REQUEST_WIRE_BYTES) {
            return Err(CoreError::Validation("Тело запроса превышает лимит 4,5 МБ. Уменьшите изображения или используйте HTTP(S)-ссылки на исходники.".into()));
        }
        let response = self.http.execute(req).await.map_err(map_transport_error)?;
        Self::decode_post(response, path).await
    }

    async fn decode_post<T: DeserializeOwned>(
        response: reqwest::Response,
        path: &str,
    ) -> Result<(T, HeaderMap), CoreError> {
        let status = response.status();
        if !status.is_success() {
            return Err(http_error(status, path, response).await);
        }
        let headers = response.headers().clone();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| with_receipt(map_transport_error(e), &headers))?;
        let value = serde_json::from_slice(&bytes)
            .map_err(|e| receipt_from_body(CoreError::Serde(e.to_string()), &headers, &bytes))?;
        Ok((value, headers))
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
        let bytes = response
            .bytes()
            .await
            .map_err(|e| with_receipt(map_transport_error(e), &headers))?;
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

    /// Баланс аккаунта: `GET /v1/account/balance` (нужен scope `account:read`).
    ///
    /// Обычный inference-ключ баланс не читает: CI отвечает 403, который
    /// разбирается в [`CoreError::ScopeMissing`]. Вызывающий использует это
    /// как сигнал показать локальный расход вместо реального баланса.
    ///
    /// # Errors
    ///
    /// [`CoreError::ScopeMissing`] при 403 (нет scope `account:read`);
    /// остальные случаи — как у [`CiClient::get_json`].
    pub async fn balance(&self) -> Result<AccountBalance, CoreError> {
        self.get_json("/v1/account/balance").await
    }

    pub async fn balance_view(&self, local_spend_usd: f64) -> BalanceView {
        let mut view = BalanceView::local(Provider::CheaperInference, local_spend_usd);
        if self.key.is_empty() {
            view.unavailable = Some(BalanceUnavailable::MissingKey);
        } else {
            match self.balance().await {
                Ok(balance) => view.real = Some(balance),
                Err(err) => record_balance_error(&mut view, err, &self.key),
            }
        }
        view
    }

    /// Склеивает базовый URL и путь без двойных слешей.
    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }
}

/// Для поддерживающих gzip медиа-эндпоинтов CI проверяет оба размера.
fn compress_media_json(json: &[u8]) -> Result<Vec<u8>, CoreError> {
    if json.len() > 20_000_000 {
        return Err(CoreError::Validation(
            "JSON превышает 20 МБ. Уменьшите вложения или используйте HTTP(S)-ссылки.".into(),
        ));
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(json)?;
    let compressed = encoder.finish()?;
    if compressed.len() > km_shared::MAX_REQUEST_WIRE_BYTES {
        return Err(CoreError::Validation("Даже после сжатия запрос превышает 4,5 МБ. Уменьшите вложения или используйте HTTP(S)-ссылки.".into()));
    }
    Ok(compressed)
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
    #[serde(default)]
    request_id: Option<Value>,
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
    decode_body(response).await
}

async fn decode_body<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, CoreError> {
    let body = response.bytes().await.map_err(map_transport_error)?;
    serde_json::from_slice(&body).map_err(|e| CoreError::Serde(e.to_string()))
}

pub(crate) fn request_id_from_headers(headers: &HeaderMap) -> Option<String> {
    [
        "x-ci-request-id",
        "x-request-id",
        "x-cheaper-inference-request-id",
    ]
    .into_iter()
    .find_map(|name| {
        let id = headers.get(name)?.to_str().ok()?.trim();
        crate::video::safe_request_segment(id).then(|| id.to_owned())
    })
}

pub(crate) fn response_request_id(body_id: Option<&str>, headers: &HeaderMap) -> Option<String> {
    body_id
        .map(str::trim)
        .filter(|id| crate::video::safe_request_segment(id))
        .map(str::to_owned)
        .or_else(|| request_id_from_headers(headers))
}

/// Даже при нарушении схемы media payload у успешной операции может быть
/// квитанция. Повторно читаем только её поля, пропуская крупное поле data/videos.
fn receipt_from_body(source: CoreError, headers: &HeaderMap, bytes: &[u8]) -> CoreError {
    #[derive(Deserialize)]
    struct MediaReceipt {
        request_id: Option<Value>,
        billing: Option<Value>,
    }
    let Ok(receipt) = serde_json::from_slice::<MediaReceipt>(bytes) else {
        return with_receipt(source, headers);
    };
    let body_id = receipt.request_id.as_ref().and_then(Value::as_str);
    let Some(request_id) = response_request_id(body_id, headers) else {
        return source;
    };
    let cost_usd = match receipt.billing {
        Some(billing) => serde_json::from_value::<Billing>(billing)
            .ok()
            .and_then(|b| b.cost_usd()),
        None => billed_cost_from_headers(headers),
    };
    CoreError::Receipt {
        request_id,
        cost_usd,
        source: Box::new(source),
    }
}

pub(crate) fn billed_cost_from_headers(headers: &HeaderMap) -> Option<f64> {
    let cost = headers
        .get("x-ci-billed-cost-usd")?
        .to_str()
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()?;
    (cost.is_finite() && cost >= 0.0).then_some(cost)
}

pub(crate) fn with_receipt(source: CoreError, headers: &HeaderMap) -> CoreError {
    match request_id_from_headers(headers) {
        Some(request_id) => CoreError::Receipt {
            request_id,
            cost_usd: billed_cost_from_headers(headers),
            source: Box::new(source),
        },
        None => source,
    }
}

/// Собирает [`CoreError`] из ошибочного HTTP-ответа CI.
async fn http_error(
    status: reqwest::StatusCode,
    path: &str,
    response: reqwest::Response,
) -> CoreError {
    // Идентификатор неуспешного запроса полезен для поддержки. Это не квитанция
    // об успешной генерации, поэтому не передаём его в механизм восстановления.
    let ci_headers = path
        .trim_start_matches('/')
        .starts_with("v1/")
        .then(|| response.headers().clone());
    // Тело ошибки читаем по возможности: при сбое чтения остаётся статус.
    let raw = response.text().await.unwrap_or_default();
    let envelope: Option<ErrorEnvelope> = serde_json::from_str(&raw).ok();
    let code = envelope
        .as_ref()
        .and_then(|e| normalize_code(e.error.code.as_ref()));
    let support_id = ci_headers.as_ref().and_then(|headers| {
        let body_id = envelope
            .as_ref()
            .and_then(|e| e.request_id.as_ref())
            .and_then(Value::as_str);
        response_request_id(body_id, headers)
    });
    let mut message = envelope
        .and_then(|e| e.error.message)
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| fallback_message(&raw, status));
    if status == reqwest::StatusCode::FORBIDDEN
        && is_account_path(path)
        && (matches!(
            code.as_deref(),
            Some("insufficient_scope" | "missing_scope" | "scope_required" | "insufficient_scopes")
        ) || message.to_ascii_lowercase().contains("scope")
            || message.contains("account:read"))
    {
        return CoreError::ScopeMissing;
    }
    if let Some(id) = support_id {
        message.push_str(&format!(" [request_id: {id}]"));
    }
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
