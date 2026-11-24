//! Изображения Cheaper Inference: генерация, редактирование и байты результата.
//!
//! Валидация (промпт, диапазон `n`, лимит промпта edits) выполняется
//! до HTTP-запроса. Опциональные параметры не отправляются, если не заданы.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use km_shared::{GenerationRequest, ImageItem, ImageResponse};
use serde::Serialize;

use crate::client::CiClient;
use crate::error::CoreError;

/// Допустимое количество вариантов изображения (контракт CI: `n` 1–10).
const N_RANGE: std::ops::RangeInclusive<u32> = 1..=10;

/// Лимит промпта редактирования в символах (контракт CI: не более 32000).
const MAX_EDIT_PROMPT_CHARS: usize = 32_000;

/// Тело `POST /v1/images/generations`; отсутствующие поля не сериализуются.
#[derive(Serialize)]
struct GenerateBody<'a> {
    model: &'a str,
    prompt: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quality: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'a str>,
}

/// Тело `POST /v1/images/edits`; референсы и маска — data-URL строками.
#[derive(Serialize)]
struct EditBody<'a> {
    model: &'a str,
    prompt: &'a str,
    input_references: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mask_reference: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<&'a str>,
}

impl CiClient {
    /// Генерация изображений: `POST /v1/images/generations`.
    ///
    /// В тело попадают `model`, `prompt` и заданные `n`, `size`, `quality`,
    /// `response_format`; отсутствующие параметры не отправляются.
    ///
    /// # Errors
    ///
    /// [`CoreError::Validation`] до запроса, если промпт пустой или `n`
    /// вне 1–10; ошибки [`CiClient::post_json`] при сбое запроса или разбора.
    pub async fn generate_image(
        &self,
        req: &GenerationRequest,
    ) -> Result<ImageResponse, CoreError> {
        validate(req)?;
        let body = GenerateBody {
            model: &req.model,
            prompt: &req.prompt,
            n: req.params.n,
            size: req.params.size.as_deref(),
            quality: req.params.quality.as_deref(),
            response_format: req.params.response_format.as_deref(),
        };
        self.post_json("/v1/images/generations", &body).await
    }

    /// Редактирование изображений: `POST /v1/images/edits`.
    ///
    /// Референсы уходят строками `input_references` (data-URL из
    /// `references`), маска — полем `mask_reference`; также пробрасываются
    /// заданные `n` и `size`. Качество и формат ответа в edits не уходят.
    ///
    /// # Errors
    ///
    /// [`CoreError::Validation`] до запроса, если промпт пустой, длиннее
    /// 32000 символов или `n` вне 1–10; ошибки [`CiClient::post_json`]
    /// при сбое запроса или разбора.
    pub async fn edit_image(&self, req: &GenerationRequest) -> Result<ImageResponse, CoreError> {
        validate(req)?;
        validate_edit_prompt(req)?;
        let body = EditBody {
            model: &req.model,
            prompt: &req.prompt,
            input_references: req.references.iter().map(|r| r.data_url.as_str()).collect(),
            mask_reference: req.mask_data_url.as_deref(),
            n: req.params.n,
            size: req.params.size.as_deref(),
        };
        self.post_json("/v1/images/edits", &body).await
    }
}

/// Байты изображения из элемента ответа: base64 или скачивание по URL.
///
/// Если заданы оба поля, декодируется `b64_json` — без обращения к сети.
///
/// # Errors
///
/// [`CoreError::Serde`], если base64 не декодируется или в элементе нет
/// ни `b64_json`, ни `url`; ошибки [`CiClient::download`] при сбое скачивания.
pub async fn image_bytes(item: &ImageItem, client: &CiClient) -> Result<Vec<u8>, CoreError> {
    if let Some(b64) = &item.b64_json {
        return STANDARD
            .decode(b64)
            .map_err(|e| CoreError::Serde(format!("невалидный base64 изображения: {e}")));
    }
    if let Some(url) = &item.url {
        return client.download(url).await;
    }
    Err(CoreError::Serde(
        "элемент изображения без b64_json и url".to_owned(),
    ))
}

/// Проверяет запрос до HTTP-запроса: непустой промпт и диапазон `n`.
fn validate(req: &GenerationRequest) -> Result<(), CoreError> {
    if req.prompt.trim().is_empty() {
        return Err(CoreError::Validation("промпт пустой".to_owned()));
    }
    if let Some(n) = req.params.n
        && !N_RANGE.contains(&n)
    {
        return Err(CoreError::Validation(format!(
            "n должно быть в диапазоне 1–10, получено {n}"
        )));
    }
    Ok(())
}

/// Проверяет лимит промпта редактирования: не более 32000 символов.
fn validate_edit_prompt(req: &GenerationRequest) -> Result<(), CoreError> {
    let len = req.prompt.chars().count();
    if len > MAX_EDIT_PROMPT_CHARS {
        return Err(CoreError::Validation(format!(
            "промпт редактирования длиннее {MAX_EDIT_PROMPT_CHARS} символов: {len}"
        )));
    }
    Ok(())
}
