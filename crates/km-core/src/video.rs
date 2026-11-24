//! Видео Cheaper Inference: генерация (t2v/i2v/r2v) и восстановление генераций.
//!
//! `POST /v1/videos/generations` — синхронный запрос: CI блокирует ответ
//! до готовности видео, поэтому он идёт через отдельный клиент с таймаутом
//! 600 секунд. Recovery: `GET /v1/generations/{request_id}` возвращает файлы
//! завершённой генерации; ссылки живут 1 час, файлы хранятся 7 дней.
//! Валидация выполняется до HTTP-запроса; опциональные параметры
//! не отправляются, если не заданы.

use km_shared::{GenMode, Generation, GenerationRequest, VideoResponse};
use serde::Serialize;

use crate::client::CiClient;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_seconds: Option<f64>,
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
    /// референсов. Опциональные `duration_seconds`, `resolution`,
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
        let (image_url, input_references) = match req.mode {
            GenMode::TextToVideo => (None, None),
            GenMode::ImageToVideo => {
                let Some(first) = req.references.first() else {
                    return Err(missing_references(req.mode));
                };
                (Some(first.data_url.as_str()), None)
            }
            GenMode::ReferenceToVideo => {
                if req.references.is_empty() {
                    return Err(missing_references(req.mode));
                }
                let urls: Vec<&str> = req.references.iter().map(|r| r.data_url.as_str()).collect();
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
            duration_seconds: req.params.duration_seconds,
            resolution: req.params.resolution.as_deref(),
            aspect_ratio: req.params.aspect_ratio.as_deref(),
            audio: req.params.audio,
            image_url,
            input_references,
            n: req.params.n,
        };
        self.post_video_json(VIDEO_PATH, &body).await
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
        if request_id.trim().is_empty() {
            return Err(CoreError::Validation("request_id пустой".to_owned()));
        }
        self.get_json(&format!("/v1/generations/{request_id}"))
            .await
    }
}

/// Ошибка валидации: режим требует референсы, но список пуст.
fn missing_references(mode: GenMode) -> CoreError {
    CoreError::Validation(format!("режим {mode:?} требует хотя бы один референс"))
}
