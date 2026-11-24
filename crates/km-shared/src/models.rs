//! Типы каталога моделей Cheaper Inference (`GET /public/models`).
//!
//! Клиенту нужны только поля для выбора модели и оценки стоимости генерации;
//! неизвестные поля ответа игнорируются, отсутствующие опциональные поля
//! получают значения по умолчанию.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Цены медиа-генерации: ключ вида `720P+AUDIO` → цена в USD за единицу
/// (секунда/символ/изображение — по `media_unit`), строкой с 6 знаками после запятой.
pub type MediaPrices = BTreeMap<String, String>;

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
    /// Тип модели (`model_type` в ответе CI).
    pub model_type: ModelType,
    /// Название провайдера, например `Google`.
    #[serde(default)]
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
    /// Скидка в процентах строкой, например `35.00`.
    #[serde(default)]
    pub discount_percent: Option<String>,
    /// Поддерживает ли модель редактирование изображений.
    #[serde(default)]
    pub supports_image_edit: bool,
    /// Альтернативные идентификаторы модели.
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl CatalogModel {
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
