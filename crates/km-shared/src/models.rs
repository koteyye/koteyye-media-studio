//! Общие типы каталогов моделей Cheaper Inference и OpenRouter.
//!
//! Клиенту нужны только поля для выбора модели и оценки стоимости генерации;
//! неизвестные поля ответа игнорируются, отсутствующие опциональные поля
//! получают значения по умолчанию.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{GenMode, MediaKind};

/// Цены медиа-генерации: ключ вида `720P+AUDIO` → цена в USD за единицу
/// (секунда/символ/изображение — по `media_unit`), строкой с 6 знаками после запятой.
pub type MediaPrices = BTreeMap<String, String>;

/// Опубликованные ставки отдельных единиц тарификации.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogRates {
    pub images: Option<String>,
    pub characters: Option<String>,
    pub seconds: Option<String>,
    pub input_tokens: Option<String>,
    pub output_tokens: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogTier {
    #[serde(default)]
    pub rates: CatalogRates,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogPriceSelector {
    pub protocol: Option<String>,
    #[serde(rename = "pricingVariant", alias = "pricing_variant")]
    pub pricing_variant: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogPricingScope {
    pub scope: Option<CatalogPriceSelector>,
    #[serde(default)]
    pub tiers: Vec<CatalogTier>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct CatalogParameter {
    pub values: Vec<String>,
    pub min: Option<String>,
    pub max: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenRouterModelOptions {
    pub modes: Vec<GenMode>,
    pub image_parameters: BTreeMap<String, CatalogParameter>,
    pub video_durations: Vec<u32>,
    pub video_resolutions: Vec<String>,
    pub video_aspect_ratios: Vec<String>,
    pub video_sizes: Vec<String>,
    pub video_generate_audio: Option<bool>,
    pub video_can_toggle_audio: bool,
    pub video_seed: bool,
    pub tts_parameters: BTreeMap<String, CatalogParameter>,
}

/// Тип модели в каталоге CI; неизвестные типы сохраняются в [`ModelType::Unknown`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ModelType {
    /// Генерация изображений.
    Image,
    /// Генерация видео.
    Video,
    /// Озвучка и музыка (speech).
    Speech,
    /// Текстовые модели.
    Text,
    /// Векторные представления (embeddings).
    Embedding,
    /// Расшифровка аудио в текст.
    Transcription,
    /// Переранжирование результатов.
    Rerank,
    /// Модели принятия решений.
    Decision,
    /// Режим реального времени.
    Realtime,
    /// Тип, неизвестный клиенту: исходная строка каталога сохраняется как есть.
    Unknown(String),
}

impl ModelType {
    /// Каноническое имя типа в каталоге CI (lowercase).
    pub fn as_str(&self) -> &str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Speech => "speech",
            Self::Text => "text",
            Self::Embedding => "embedding",
            Self::Transcription => "transcription",
            Self::Rerank => "rerank",
            Self::Decision => "decision",
            Self::Realtime => "realtime",
            Self::Unknown(raw) => raw,
        }
    }
}

impl fmt::Display for ModelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ModelType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ModelType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "image" => Self::Image,
            "video" => Self::Video,
            "speech" => Self::Speech,
            "text" => Self::Text,
            "embedding" => Self::Embedding,
            "transcription" => Self::Transcription,
            "rerank" => Self::Rerank,
            "decision" => Self::Decision,
            "realtime" => Self::Realtime,
            // Новые типы каталога не должны ломать клиент.
            _ => Self::Unknown(raw),
        })
    }
}

/// Модель каталога CI: данные для выбора модели и оценки стоимости генерации.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogModel {
    /// Идентификатор модели, например `veo-3.1-generate-001`.
    pub id: String,
    /// Человекочитаемое имя, если провайдер его прислал.
    #[serde(default)]
    pub name: Option<String>,
    /// Тип модели (`model_type` в ответе CI).
    #[serde(alias = "type")]
    pub model_type: ModelType,
    /// Название провайдера, например `Google`.
    #[serde(default, alias = "provider")]
    pub provider_name: Option<String>,
    /// URL логотипа провайдера; CI может прислать `null`.
    #[serde(default)]
    pub logo_url: Option<String>,
    /// Возможности модели: `image-input`, `image-edit` и т.п.
    #[serde(default)]
    pub features: Vec<String>,
    /// Поддерживаемые эндпоинты API, например `/v1/videos/generations`.
    #[serde(default)]
    pub supported_endpoints: Vec<String>,
    /// Единица тарификации медиа: `second`, `image` и т.п.
    #[serde(default)]
    pub media_unit: Option<String>,
    /// Цены в USD за единицу (`media_unit`) по ключам вида `720P+AUDIO`;
    /// значения — строки с 6 знаками после запятой.
    #[serde(default)]
    pub media_prices: MediaPrices,
    /// Базовая ставка, когда для выбранных параметров нет отдельной градации.
    #[serde(default)]
    pub media_unit_price: Option<String>,
    /// У речевых моделей таблица media_prices часто пуста; тариф находится в tiers.
    #[serde(default)]
    pub pricing_scopes: Option<Vec<CatalogPricingScope>>,
    /// Скидка в процентах строкой, например `35.00`.
    #[serde(default)]
    pub discount_percent: Option<String>,
    /// Поддерживает ли модель редактирование изображений.
    #[serde(default)]
    pub supports_image_edit: bool,
    /// Альтернативные идентификаторы модели.
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openrouter: Option<OpenRouterModelOptions>,
}

impl CatalogModel {
    pub fn matches_kind(&self, kind: MediaKind) -> bool {
        matches!(
            (kind, &self.model_type),
            (MediaKind::Image, ModelType::Image)
                | (MediaKind::Video, ModelType::Video)
                | (MediaKind::Audio, ModelType::Speech)
        )
    }

    /// Единый фильтр каталога для WASM-интерфейса и нативного ядра.
    pub fn supports_mode(&self, mode: GenMode) -> bool {
        if let Some(options) = &self.openrouter {
            return options.modes.contains(&mode);
        }
        match mode {
            GenMode::TextToImage => {
                self.model_type == ModelType::Image && self.supports("/v1/images/generations")
            }
            GenMode::ImageEdit => {
                self.model_type == ModelType::Image
                    && self.supports_image_edit
                    && self.supports("/v1/images/edits")
            }
            GenMode::TextToVideo | GenMode::ImageToVideo | GenMode::ReferenceToVideo => {
                self.model_type == ModelType::Video
                    && self.supports("/v1/videos/generations")
                    && self.supports_video_input(mode)
            }
            GenMode::Speech | GenMode::Music | GenMode::SoundEffects => {
                let audio_mode = if self.id.contains("sound-effects") {
                    GenMode::SoundEffects
                } else if self.id.contains("music") {
                    GenMode::Music
                } else {
                    GenMode::Speech
                };
                self.model_type == ModelType::Speech
                    && self.supports("/v1/audio/speech")
                    && mode == audio_mode
            }
        }
    }

    fn supports_video_input(&self, mode: GenMode) -> bool {
        // Эти маршруты требуют исходный видеоклип, который студия не отправляет.
        if self.id.contains("-motion-control")
            || self.id == "proteus"
            || self.id.starts_with("starlight-precise-")
        {
            return false;
        }
        // CI публикует варианты одной модели как отдельные id. Даже t2v-вариант
        // может иметь общий для семейства флаг image-input.
        for part in self.id.split('-') {
            match part {
                "t2v" => return mode == GenMode::TextToVideo,
                "i2v" => return mode == GenMode::ImageToVideo,
                "r2v" => return mode == GenMode::ReferenceToVideo,
                _ => {}
            }
        }
        let image_input = self.has_feature("image-input");
        match mode {
            GenMode::TextToVideo => true,
            GenMode::ImageToVideo => image_input,
            // image_url (первый кадр) и input_references — разные параметры.
            // Для моделей без маркировки r2v учитываем опубликованные тарифы
            // режима и варианты с подтверждённым приёмом референсов.
            GenMode::ReferenceToVideo => {
                self.media_prices.keys().any(|key| {
                    key.split('+')
                        .any(|part| part.eq_ignore_ascii_case("reference_to_video"))
                }) || (image_input
                    && matches!(
                        self.id.as_str(),
                        "veo-3.1-generate-001"
                            | "veo-3.1-fast-generate-001"
                            | "grok-imagine-video"
                            | "grok-imagine-video-1.5"
                            | "seedance-2.0"
                    ))
            }
            _ => false,
        }
    }

    /// Поддерживает ли модель эндпоинт (точное совпадение, например `/v1/audio/speech`).
    pub fn supports(&self, endpoint: &str) -> bool {
        self.supported_endpoints.iter().any(|e| e == endpoint)
    }

    /// Есть ли у модели возможность (точное совпадение, например `image-input`).
    pub fn has_feature(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }

    /// Цена медиа-генерации в USD по ключу (`720P+AUDIO` и т.п.).
    ///
    /// Возвращает `None`, если ключа нет или значение не парсится в число.
    pub fn price_for(&self, key: &str) -> Option<f64> {
        self.media_prices.get(key)?.parse::<f64>().ok()
    }
}

// Типы каталога ходят между потоками (кэш, Tauri-команды) — фиксируем Send + Sync.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CatalogModel>();
    assert_send_sync::<ModelType>();
};
