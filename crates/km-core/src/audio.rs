//! Аудио Cheaper Inference: озвучка, музыка и звуковые эффекты.
//!
//! У трёх режимов один эндпоинт `POST /v1/audio/speech` и бинарный ответ.
//! SpeechRequest документирует model/input и необязательные voice, format,
//! speed. Длительность для моделей музыки и эффектов шлюз не публикует.

use km_shared::{AudioOutput, GenMode, GenerationRequest};
use reqwest::header::HeaderMap;
use serde::Serialize;

use crate::client::{CiClient, billed_cost_from_headers, request_id_from_headers, with_receipt};
use crate::error::CoreError;

/// Эндпоинт генерации аудио: общий для озвучки, музыки и эффектов.
const SPEECH_PATH: &str = "/v1/audio/speech";

/// Лимит input в символах (контракт CI: 1–4096).
const MAX_INPUT_CHARS: usize = 4096;

/// Тело `POST /v1/audio/speech`; отсутствующие поля не сериализуются.
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
}

impl CiClient {
    /// Генерация аудио: `POST /v1/audio/speech` для озвучки, музыки и эффектов.
    ///
    /// Настройки озвучки передаём при явном выборе. Для музыки и эффектов
    /// требуются model/input. В выборке истории могут остаться старые
    /// duration_seconds: шлюз не гарантирует их поддержку в SpeechRequest.
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
                voice: req
                    .params
                    .voice
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty()),
                response_format: req
                    .params
                    .response_format
                    .as_deref()
                    .filter(|s| !s.is_empty()),
                speed: req.params.speed,
            },
            GenMode::Music | GenMode::SoundEffects => AudioBody {
                model: &req.model,
                input: &req.prompt,
                voice: None,
                response_format: None,
                speed: None,
            },
            other => {
                return Err(CoreError::Validation(format!(
                    "режим {other:?} не относится к аудио"
                )));
            }
        };
        let (bytes, headers) = self.post_bytes(SPEECH_PATH, &body).await?;
        let content_type = audio_content_type(&headers, &bytes, body.response_format)
            .map_err(|error| with_receipt(error, &headers))?;
        Ok(AudioOutput {
            request_id: request_id_from_headers(&headers),
            bytes,
            content_type,
            cost_usd: billed_cost_from_headers(&headers),
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
    if req.mode == GenMode::Speech
        && let Some(speed) = req.params.speed
        && (!speed.is_finite() || speed <= 0.0)
    {
        return Err(CoreError::Validation(
            "скорость озвучки должна быть положительной".to_owned(),
        ));
    }
    Ok(())
}

/// OpenAPI допускает application/octet-stream. В этом случае контейнер
/// определяет расширение; для PCM без заголовка опираемся на запрошенный формат.
fn audio_content_type(
    headers: &HeaderMap,
    bytes: &[u8],
    format: Option<&str>,
) -> Result<String, CoreError> {
    if bytes.is_empty() {
        return Err(CoreError::Serde(
            "CI вернул пустой результат: аудио".to_owned(),
        ));
    }
    let mime = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        mime if mime.starts_with("audio/") => return Ok(mime.to_owned()),
        "application/ogg" => return Ok("audio/ogg".into()),
        "" | "application/octet-stream" | "binary/octet-stream" => {}
        other => return Err(CoreError::Serde(format!("CI вернул {other} вместо аудио"))),
    }
    let mime = if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        "audio/wav"
    } else if bytes.starts_with(b"OggS") {
        "audio/ogg"
    } else if bytes.starts_with(b"fLaC") {
        "audio/flac"
    } else if bytes.get(4..8) == Some(b"ftyp") {
        "audio/mp4"
    } else if matches!(bytes, [0xff,b,..] if b & 0xf6 == 0xf0) {
        "audio/aac"
    } else if bytes.starts_with(b"ID3") || matches!(bytes, [0xff,b,..] if b & 0xe0 == 0xe0) {
        "audio/mpeg"
    } else {
        match format {
            Some("wav") => "audio/wav",
            Some("opus") => "audio/opus",
            Some("aac") => "audio/aac",
            Some("flac") => "audio/flac",
            Some("pcm") => "audio/pcm",
            Some("mp3") | None => "audio/mpeg",
            _ => "application/octet-stream",
        }
    };
    Ok(mime.to_owned())
}
