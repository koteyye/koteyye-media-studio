//! Аудио Cheaper Inference: озвучка, музыка и звуковые эффекты.
//!
//! Все три режима ([`GenMode::Speech`], [`GenMode::Music`],
//! [`GenMode::SoundEffects`]) ходят в один эндпоинт `POST /v1/audio/speech`;
//! различается только набор параметров тела. Ответ бинарный (mp3/wav),
//! стоимость приходит в заголовке `x-ci-billed-cost-usd`. Валидация input
//! выполняется до HTTP-запроса; опциональные параметры не отправляются,
//! если не заданы.

use km_shared::{AudioOutput, GenMode, GenerationRequest};
use reqwest::header::HeaderMap;
use serde::Serialize;

use crate::client::CiClient;
use crate::error::CoreError;

/// Эндпоинт генерации аудио: общий для озвучки, музыки и эффектов.
const SPEECH_PATH: &str = "/v1/audio/speech";

/// Лимит input в символах (контракт CI: 1–4096).
const MAX_INPUT_CHARS: usize = 4096;

/// Формат ответа озвучки по умолчанию (контракт CI).
const DEFAULT_RESPONSE_FORMAT: &str = "mp3";

/// MIME-тип по умолчанию, если CI не прислал заголовок Content-Type.
const DEFAULT_CONTENT_TYPE: &str = "audio/mpeg";

/// Заголовок CI со стоимостью генерации: строка USD с 6 знаками.
const BILLED_COST_HEADER: &str = "x-ci-billed-cost-usd";

/// Тело `POST /v1/audio/speech`; отсутствующие поля не сериализуются.
///
/// Набор полей зависит от режима: озвучка шлёт `voice`, `response_format`
/// и `speed`; музыка и эффекты — `duration_seconds`. Лишние поля не уходят.
#[derive(Serialize)]
struct AudioBody<'a> {
    model: &'a str,
    input: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    voice: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_seconds: Option<f64>,
}

impl CiClient {
    /// Генерация аудио: `POST /v1/audio/speech` для озвучки, музыки и эффектов.
    ///
    /// Промпт уходит полем `input`. Для озвучки добавляются `voice`,
    /// `response_format` (по умолчанию `mp3`) и `speed`; для музыки и
    /// эффектов — `duration_seconds`, если задан. Голос, скорость и формат
    /// в режимах музыки/эффектов не отправляются. Ответ — бинарные байты,
    /// MIME-тип из заголовка Content-Type и стоимость из
    /// `x-ci-billed-cost-usd`.
    ///
    /// # Errors
    ///
    /// [`CoreError::Validation`] до запроса, если input пустой, длиннее
    /// 4096 символов или режим не относится к аудио; ошибки
    /// [`CiClient::post_bytes`] при сбое запроса.
    pub async fn speech(&self, req: &GenerationRequest) -> Result<AudioOutput, CoreError> {
        validate_input(req)?;
        let body = match req.mode {
            GenMode::Speech => AudioBody {
                model: &req.model,
                input: &req.prompt,
                voice: req.params.voice.as_deref(),
                response_format: Some(
                    req.params
                        .response_format
                        .as_deref()
                        .unwrap_or(DEFAULT_RESPONSE_FORMAT),
                ),
                speed: req.params.speed,
                duration_seconds: None,
            },
            GenMode::Music | GenMode::SoundEffects => AudioBody {
                model: &req.model,
                input: &req.prompt,
                voice: None,
                response_format: None,
                speed: None,
                duration_seconds: req.params.duration_seconds,
            },
            other => {
                return Err(CoreError::Validation(format!(
                    "режим {other:?} не относится к аудио"
                )));
            }
        };
        let (bytes, headers) = self.post_bytes(SPEECH_PATH, &body).await?;
        Ok(AudioOutput {
            bytes,
            content_type: content_type(&headers),
            cost_usd: billed_cost(&headers),
        })
    }
}

/// Проверяет input до HTTP-запроса: непустой и не длиннее 4096 символов.
fn validate_input(req: &GenerationRequest) -> Result<(), CoreError> {
    if req.prompt.trim().is_empty() {
        return Err(CoreError::Validation("input пустой".to_owned()));
    }
    let len = req.prompt.chars().count();
    if len > MAX_INPUT_CHARS {
        return Err(CoreError::Validation(format!(
            "input длиннее {MAX_INPUT_CHARS} символов: {len}"
        )));
    }
    Ok(())
}

/// MIME-тип из заголовка Content-Type; без него — дефолт для mp3.
fn content_type(headers: &HeaderMap) -> String {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or(DEFAULT_CONTENT_TYPE)
        .to_owned()
}

/// Стоимость из заголовка `x-ci-billed-cost-usd`; `None`, если нет или не парсится.
fn billed_cost(headers: &HeaderMap) -> Option<f64> {
    headers
        .get(BILLED_COST_HEADER)?
        .to_str()
        .ok()?
        .parse::<f64>()
        .ok()
}
