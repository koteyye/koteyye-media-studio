//! Типы запросов на генерацию: вид медиа, режим, параметры, референсы.
//!
//! Запрос приходит из UI (Dioxus → Tauri-команда); опциональные параметры
//! могут отсутствовать. Enum'ы сериализуются в lowercase-строки
//! (многословные режимы — snake_case, например `text_to_image`).

use serde::{Deserialize, Serialize};

/// Вид генерируемого медиа.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    /// Изображение.
    Image,
    /// Видео.
    Video,
    /// Аудио (озвучка, музыка, эффекты).
    Audio,
}

/// Режим генерации: определяет эндпоинт CI и набор параметров.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenMode {
    /// Текст → изображение (`/v1/images/generations`).
    TextToImage,
    /// Редактирование изображения (`/v1/images/edits`).
    ImageEdit,
    /// Текст → видео (`/v1/videos/generations`).
    TextToVideo,
    /// Изображение → видео.
    ImageToVideo,
    /// Референсы → видео.
    ReferenceToVideo,
    /// Озвучка текста (TTS).
    Speech,
    /// Музыка.
    Music,
    /// Звуковые эффекты.
    SoundEffects,
}

impl GenMode {
    pub const fn kind(self) -> MediaKind {
        match self {
            Self::TextToImage | Self::ImageEdit => MediaKind::Image,
            Self::TextToVideo | Self::ImageToVideo | Self::ReferenceToVideo => MediaKind::Video,
            Self::Speech | Self::Music | Self::SoundEffects => MediaKind::Audio,
        }
    }
}

/// Параметры генерации: набор зависит от режима, лишние CI игнорирует.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct GenParams {
    /// Размер изображения, например `1024x1024`.
    pub size: Option<String>,
    /// Разрешение видео, например `720P`.
    pub resolution: Option<String>,
    /// Соотношение сторон, например `16:9`.
    pub aspect_ratio: Option<String>,
    /// Длительность видео/аудио в секундах.
    pub duration_seconds: Option<f64>,
    /// Количество вариантов.
    pub n: Option<u32>,
    /// Seed для воспроизводимости.
    pub seed: Option<i64>,
    /// Голос озвучки.
    pub voice: Option<String>,
    /// Формат ответа, например `b64_json` или `url`.
    pub response_format: Option<String>,
    /// Скорость озвучки.
    pub speed: Option<f64>,
    /// Звук в видео (вкл/выкл).
    pub audio: Option<bool>,
    /// Качество, например `high`.
    pub quality: Option<String>,
}

/// Референс для генерации: изображение как data-URL.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Reference {
    /// Изображение в формате data-URL (`data:image/png;base64,...`).
    pub data_url: String,
}

/// Запрос на генерацию из UI: всё, что нужно jobs для запуска.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct GenerationRequest {
    /// Вид медиа.
    pub kind: MediaKind,
    /// Режим генерации.
    pub mode: GenMode,
    /// Идентификатор модели каталога CI.
    pub model: String,
    /// Текст промпта.
    pub prompt: String,
    /// Параметры режима; могут отсутствовать целиком.
    #[serde(default)]
    pub params: GenParams,
    /// Референсы-изображения; могут отсутствовать.
    #[serde(default)]
    pub references: Vec<Reference>,
    /// PNG-маска для редактирования, data-URL.
    #[serde(default)]
    pub mask_data_url: Option<String>,
}

// Запросы ходят между потоками (Tauri-команды, jobs) — фиксируем Send + Sync.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GenerationRequest>();
    assert_send_sync::<GenParams>();
    assert_send_sync::<MediaKind>();
    assert_send_sync::<GenMode>();
};
