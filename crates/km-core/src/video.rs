//! Видео Cheaper Inference: генерация (t2v/i2v/r2v) и восстановление генераций.
//!
//! `POST /v1/videos/generations` — синхронный запрос: CI блокирует ответ
//! до готовности видео, поэтому он идёт через отдельный клиент с таймаутом
//! 600 секунд. Recovery: `GET /v1/generations/{request_id}` возвращает файлы
//! завершённой генерации; ссылки живут 1 час, файлы хранятся 7 дней.
//! Валидация выполняется до HTTP-запроса; опциональные параметры
//! не отправляются, если не заданы.

use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use km_shared::video::VideoLimits;
use km_shared::{Billing, GenMode, Generation, GenerationRequest, VideoItem, VideoResponse};
use serde::Serialize;

use crate::client::{CiClient, billed_cost_from_headers, response_request_id};
use crate::error::CoreError;

/// Эндпоинт синхронной генерации видео.
const VIDEO_PATH: &str = "/v1/videos/generations";

/// Тело `POST /v1/videos/generations`; отсутствующие поля не сериализуются.
///
/// Набор полей зависит от режима: t2v шлёт только параметры, i2v добавляет
/// `image_url` (первый кадр), r2v — `input_references` (все референсы).
#[derive(Serialize)]
struct VideoBody<'a> {
    model: &'a str,
    prompt: &'a str,
    duration_seconds: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolution: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    aspect_ratio: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_references: Option<Vec<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
}

impl CiClient {
    /// Генерация видео: `POST /v1/videos/generations` (синхронно, таймаут 600 с).
    ///
    /// Маппинг режимов: [`GenMode::TextToVideo`] — без картинок;
    /// [`GenMode::ImageToVideo`] — `image_url` из первого референса (первый
    /// кадр); [`GenMode::ReferenceToVideo`] — `input_references` из всех
    /// референсов. Длительность обязательна; опциональные `resolution`,
    /// `aspect_ratio`, `audio` и `n` не отправляются, если не заданы.
    ///
    /// # Errors
    ///
    /// [`CoreError::Validation`] до запроса, если промпт пустой, режим
    /// не относится к видео или i2v/r2v без референсов; [`CoreError::Timeout`]
    /// при превышении 600 секунд; остальные ошибки — как у
    /// [`CiClient::post_json`].
    pub async fn generate_video(
        &self,
        req: &GenerationRequest,
    ) -> Result<VideoResponse, CoreError> {
        if req.prompt.trim().is_empty() {
            return Err(CoreError::Validation("промпт пустой".to_owned()));
        }
        if req.model.trim().is_empty() {
            return Err(CoreError::Validation("модель не выбрана".to_owned()));
        }
        let duration_seconds = req
            .params
            .duration_seconds
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .ok_or_else(|| {
                CoreError::Validation("требуется положительная длительность видео".into())
            })?;
        if req.params.n.is_some_and(|n| !(1..=10).contains(&n)) {
            return Err(CoreError::Validation(
                "количество видео должно быть от 1 до 10".into(),
            ));
        }
        if let Some(limits) = VideoLimits::for_model(&req.model, req.mode) {
            if !limits.supports(&req.params) {
                return Err(CoreError::Validation(format!(
                    "для {} длительность в секундах {:?}, до {} видео, формат кадра {}",
                    req.model,
                    limits.durations,
                    limits.max_outputs,
                    limits.aspect_ratios.join(" или ")
                )));
            }
            if !limits.supports_references(&req.references) {
                return Err(CoreError::Validation("Veo 3.1 и Fast принимают до трёх референсов JPEG или PNG. У Veo 3.1 Lite режим референсов недоступен.".into()));
            }
        }
        let (image_url, input_references) = match req.mode {
            GenMode::TextToVideo => (None, None),
            GenMode::ImageToVideo => {
                let Some(first) = req.references.first() else {
                    return Err(missing_references(req.mode));
                };
                (Some(video_image_url(&first.data_url)?), None)
            }
            GenMode::ReferenceToVideo => {
                if req.references.is_empty() {
                    return Err(missing_references(req.mode));
                }
                let urls = req
                    .references
                    .iter()
                    .map(|r| video_image_url(&r.data_url))
                    .collect::<Result<Vec<_>, _>>()?;
                (None, Some(urls))
            }
            other => {
                return Err(CoreError::Validation(format!(
                    "режим {other:?} не относится к видео"
                )));
            }
        };
        let body = VideoBody {
            model: &req.model,
            prompt: &req.prompt,
            duration_seconds,
            resolution: req.params.resolution.as_deref(),
            aspect_ratio: req.params.aspect_ratio.as_deref(),
            audio: req.params.audio,
            image_url,
            input_references,
            n: req.params.n,
        };
        let (mut result, headers): (VideoResponse, _) =
            self.post_video_json_with_headers(VIDEO_PATH, &body).await?;
        result.request_id = response_request_id(result.request_id.as_deref(), &headers);
        // При незавершённом расчёте billing.billed_cost_usd = null: цена пока неизвестна.
        if result.billing.is_none() {
            result.billing = billed_cost_from_headers(&headers).map(|cost| Billing {
                billed_cost_usd: Some(cost.to_string()),
            });
        }
        Ok(result)
    }

    /// Восстановление генерации: `GET /v1/generations/{request_id}`.
    ///
    /// Возвращает файлы завершённой генерации после сбоя или перезапуска
    /// приложения. Ссылки на файлы живут 1 час, сами файлы хранятся 7 дней;
    /// скачивание — [`CiClient::download`] без заголовка Authorization.
    ///
    /// # Errors
    ///
    /// [`CoreError::Validation`] до запроса, если `request_id` пустой;
    /// ошибки [`CiClient::get_json`] при сбое запроса или разбора.
    pub async fn generation(&self, request_id: &str) -> Result<Generation, CoreError> {
        if !safe_request_segment(request_id) {
            return Err(CoreError::Validation("некорректный request_id".to_owned()));
        }
        let generation: Generation = self
            .get_json(&format!("/v1/generations/{request_id}"))
            .await?;
        if !generation.request_id.eq_ignore_ascii_case(request_id) {
            return Err(CoreError::Serde("получены файлы другого запроса".into()));
        }
        Ok(generation)
    }
}

fn video_image_url(value: &str) -> Result<&str, CoreError> {
    let inline = value
        .strip_prefix("data:image/")
        .and_then(|s| s.split_once(','))
        .is_some_and(|(meta, bytes)| meta.ends_with(";base64") && !bytes.is_empty());
    if inline
        || reqwest::Url::parse(value)
            .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
    {
        Ok(value)
    } else {
        Err(CoreError::Validation(
            "укажите HTTP(S)-ссылку или встроенное изображение".into(),
        ))
    }
}

/// В API идентификатор — UUID; старые записи с безопасным непрозрачным идентификатором
/// также передаём серверу, который отвечает invalid_request_id при неподдерживаемом значении.
pub(crate) fn safe_request_segment(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub(crate) async fn video_bytes(
    item: &VideoItem,
    client: &CiClient,
    timeout: Duration,
) -> Result<Vec<u8>, CoreError> {
    if let Some(encoded) = &item.b64_video
        && let Ok(bytes) = STANDARD.decode(encoded)
        && !bytes.is_empty()
    {
        return Ok(bytes);
    }
    if let Some(url) = item.url.as_deref().filter(|url| !url.trim().is_empty()) {
        let bytes = client.download_with_timeout(url, Some(timeout)).await?;
        if !bytes.is_empty() {
            return Ok(bytes);
        }
    }
    Err(CoreError::Serde(
        "элемент видео не содержит MP4 (b64_video или url)".into(),
    ))
}

/// Ошибка валидации: режим требует референсы, но список пуст.
fn missing_references(mode: GenMode) -> CoreError {
    CoreError::Validation(format!("режим {mode:?} требует хотя бы один референс"))
}
