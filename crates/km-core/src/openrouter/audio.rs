//! OpenRouter speech, prompt-based sound effects and streamed music.

use std::time::Duration;

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use km_shared::{AudioOutput, GenMode, GenerationRequest, MediaKind};
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::client::{OpenRouterKeyClient, map_transport_error};
use crate::error::CoreError;
use crate::video::safe_request_segment;

use super::{Usage, generation_id};

#[derive(Serialize)]
struct SpeechBody<'a> {
    model: &'a str,
    input: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    voice: Option<&'a str>,
    response_format: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<f64>,
}

#[derive(Deserialize)]
struct GenerationStats {
    data: GenerationCost,
}

#[derive(Deserialize)]
struct GenerationCost {
    total_cost: Option<f64>,
}

impl OpenRouterKeyClient {
    pub async fn generate_audio(
        &self,
        key: &str,
        req: &GenerationRequest,
    ) -> Result<AudioOutput, CoreError> {
        if req.kind != MediaKind::Audio
            || req.prompt.trim().is_empty()
            || req.model.trim().is_empty()
        {
            return Err(CoreError::Validation(
                "для генерации аудио нужны модель и промпт".into(),
            ));
        }

        match req.mode {
            GenMode::Speech => self.speech(key, req, false).await,
            GenMode::SoundEffects if req.model == "bytedance-seed/seed-audio-1-0" => {
                self.speech(key, req, true).await
            }
            GenMode::Music => self.music(key, req).await,
            _ => Err(CoreError::Validation(
                "выбранная модель не поддерживает этот режим аудио OpenRouter".into(),
            )),
        }
    }

    async fn speech(
        &self,
        key: &str,
        req: &GenerationRequest,
        sound_effects: bool,
    ) -> Result<AudioOutput, CoreError> {
        let (voice, response_format, speed) = if sound_effects {
            // Seed Audio interprets input as a scene description when no voice is supplied.
            (None, "mp3", None)
        } else {
            let format = req.params.response_format.as_deref().unwrap_or("pcm");
            if !matches!(format, "mp3" | "pcm" | "wav") {
                return Err(CoreError::Validation(
                    "OpenRouter поддерживает форматы озвучки MP3, PCM и WAV".into(),
                ));
            }
            if req
                .params
                .speed
                .is_some_and(|speed| !speed.is_finite() || speed <= 0.0)
            {
                return Err(CoreError::Validation(
                    "скорость озвучки должна быть положительной".into(),
                ));
            }
            let voice = req
                .params
                .voice
                .as_deref()
                .map(str::trim)
                .filter(|voice| !voice.is_empty());
            (voice, format, req.params.speed)
        };
        let body = SpeechBody {
            model: &req.model,
            input: &req.prompt,
            voice,
            response_format,
            speed,
        };

        let response = self.post("/audio/speech", key, &body).await?;
        let request_id = generation_id(&response);
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut content_type_parts = content_type.split(';').map(str::trim);
        let mime = content_type_parts.next().unwrap_or_default();
        let pcm_rate = content_type_parts
            .clone()
            .find_map(|part| part.strip_prefix("rate="))
            .and_then(|rate| rate.parse::<u32>().ok())
            .or_else(|| pcm_rate_for_model(&req.model));
        let pcm_channels = content_type_parts
            .find_map(|part| part.strip_prefix("channels="))
            .and_then(|channels| channels.parse::<u16>().ok())
            .unwrap_or(1);

        let bytes = response
            .bytes()
            .await
            .map_err(map_transport_error)?
            .to_vec();
        let (bytes, content_type) = match mime {
            "audio/mpeg" | "audio/mp3" => ensure_audio_bytes(bytes, "audio/mpeg")?,
            "audio/wav" | "audio/x-wav" => ensure_audio_bytes(bytes, "audio/wav")?,
            "audio/pcm" => pcm_output(bytes, pcm_rate, pcm_channels)?,
            "" | "application/octet-stream" => match response_format {
                "pcm" => pcm_output(bytes, pcm_rate, pcm_channels)?,
                "wav" => ensure_audio_bytes(bytes, "audio/wav")?,
                _ => ensure_audio_bytes(bytes, "audio/mpeg")?,
            },
            _ => return Err(CoreError::Serde("OpenRouter вернул ответ без аудио".into())),
        };

        let cost_usd = self.generation_cost(key, request_id.as_deref()).await;
        Ok(AudioOutput {
            request_id,
            bytes,
            content_type,
            cost_usd,
        })
    }

    async fn music(&self, key: &str, req: &GenerationRequest) -> Result<AudioOutput, CoreError> {
        let body = json!({
            "model": req.model,
            "messages": [{"role": "user", "content": req.prompt}],
            "modalities": ["text", "audio"],
            "audio": { "format": "pcm16" },
            "stream": true
        });
        let response = self.post("/chat/completions", key, &body).await?;
        let request_id = generation_id(&response);
        let events = response.text().await.map_err(map_transport_error)?;
        let mut output = music_output(&events, request_id, key)?;
        if output.cost_usd.is_none() {
            output.cost_usd = self
                .generation_cost(key, output.request_id.as_deref())
                .await;
        }
        Ok(output)
    }

    // Generation details contain usage metadata only. A missing or delayed stats
    // record must not turn successfully received audio into a failed generation.
    async fn generation_cost(&self, key: &str, id: Option<&str>) -> Option<f64> {
        let id = id.filter(|id| safe_request_segment(id))?;
        let response = self
            .get_with_timeout(&format!("/generation?id={id}"), key, Duration::from_secs(5))
            .await
            .ok()?;
        let stats: GenerationStats = self.json(response, key).await.ok()?;
        Usage {
            cost: stats.data.total_cost,
        }
        .cost_usd()
    }
}

#[derive(Deserialize)]
struct MusicChunk {
    id: Option<String>,
    usage: Option<Usage>,
    error: Option<StreamError>,
    #[serde(default)]
    choices: Vec<MusicChoice>,
}

#[derive(Deserialize)]
struct StreamError {
    message: String,
}

#[derive(Deserialize)]
struct MusicChoice {
    delta: MusicDelta,
}

#[derive(Deserialize)]
struct MusicDelta {
    audio: Option<MusicAudio>,
}

#[derive(Deserialize)]
struct MusicAudio {
    data: Option<String>,
}

fn music_output(
    events: &str,
    mut request_id: Option<String>,
    key: &str,
) -> Result<AudioOutput, CoreError> {
    let mut bytes = Vec::new();
    let mut encoded = String::new();
    let mut cost_usd = None;
    let mut done = false;
    for data in events.lines().filter_map(|line| line.strip_prefix("data:")) {
        let data = data.trim();
        if data == "[DONE]" {
            done = true;
            break;
        }
        let chunk: MusicChunk = serde_json::from_str(data).map_err(|_| {
            CoreError::Serde("некорректный фрагмент музыкального потока OpenRouter".into())
        })?;
        if let Some(error) = chunk.error {
            let message = if key.is_empty() {
                error.message
            } else {
                error.message.replace(key, "[API key]")
            };
            return Err(CoreError::Http {
                status: 502,
                code: None,
                message,
            });
        }
        if request_id.is_none() {
            request_id = chunk.id.filter(|id| safe_request_segment(id));
        }
        if let Some(cost) = chunk.usage.and_then(|usage| usage.cost_usd()) {
            cost_usd = Some(cost);
        }
        if let Some(data) = chunk
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.delta.audio)
            .and_then(|audio| audio.data)
        {
            encoded.push_str(&data);
            // Some transports frame each audio block separately with base64 padding.
            if encoded.ends_with('=') {
                decode_audio(&encoded, &mut bytes)?;
                encoded.clear();
            }
        }
    }
    if !done {
        return Err(CoreError::Serde(
            "музыкальный поток OpenRouter прервался".into(),
        ));
    }
    decode_audio(&encoded, &mut bytes)?;

    let (bytes, content_type) = if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        ensure_audio_bytes(bytes, "audio/wav")?
    } else if bytes.starts_with(b"ID3") {
        ensure_audio_bytes(bytes, "audio/mpeg")?
    } else if bytes.starts_with(b"fLaC") {
        ensure_audio_bytes(bytes, "audio/flac")?
    } else if bytes.starts_with(b"OggS") {
        ensure_audio_bytes(bytes, "audio/ogg")?
    } else {
        // Lyria's audio is 16-bit, 48 kHz stereo. Preserve this layout in a playable file.
        let wav = wav_from_pcm(&bytes, 48_000, 2)?;
        (wav, "audio/wav".into())
    };

    Ok(AudioOutput {
        request_id,
        bytes,
        content_type,
        cost_usd,
    })
}

fn decode_audio(encoded: &str, bytes: &mut Vec<u8>) -> Result<(), CoreError> {
    let engine = if encoded.ends_with('=') {
        &STANDARD
    } else {
        &STANDARD_NO_PAD
    };
    engine
        .decode_vec(encoded, bytes)
        .map_err(|_| CoreError::Serde("некорректная кодировка аудио OpenRouter".into()))
}

fn ensure_audio_bytes(bytes: Vec<u8>, mime: &str) -> Result<(Vec<u8>, String), CoreError> {
    if bytes.is_empty() {
        return Err(CoreError::Serde("OpenRouter вернул пустое аудио".into()));
    }
    Ok((bytes, mime.into()))
}

fn pcm_rate_for_model(model: &str) -> Option<u32> {
    if model.starts_with("heygen/") {
        Some(44_100)
    } else if ["openai/", "google/", "microsoft/"]
        .iter()
        .any(|prefix| model.starts_with(prefix))
    {
        Some(24_000)
    } else {
        None
    }
}

fn pcm_output(
    bytes: Vec<u8>,
    rate: Option<u32>,
    channels: u16,
) -> Result<(Vec<u8>, String), CoreError> {
    if let Some(rate) = rate {
        let bytes = wav_from_pcm(&bytes, rate, channels)?;
        Ok((bytes, "audio/wav".into()))
    } else {
        ensure_audio_bytes(bytes, "audio/pcm")
    }
}

fn wav_from_pcm(samples: &[u8], rate: u32, channels: u16) -> Result<Vec<u8>, CoreError> {
    let block_align = channels
        .checked_mul(2)
        .filter(|align| *align != 0)
        .ok_or_else(|| {
            CoreError::Serde("некорректное число каналов PCM-аудио OpenRouter".into())
        })?;
    let data_len = u32::try_from(samples.len())
        .ok()
        .filter(|length| *length > 0 && length % u32::from(block_align) == 0)
        .ok_or_else(|| CoreError::Serde("некорректная длина PCM-аудио OpenRouter".into()))?;
    let riff_len = data_len
        .checked_add(36)
        .ok_or_else(|| CoreError::Serde("слишком большое PCM-аудио OpenRouter".into()))?;
    let byte_rate = rate
        .checked_mul(u32::from(block_align))
        .filter(|rate| *rate > 0)
        .ok_or_else(|| CoreError::Serde("некорректная частота PCM-аудио OpenRouter".into()))?;
    let mut wav = Vec::with_capacity(samples.len() + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_len.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(samples);
    Ok(wav)
}
