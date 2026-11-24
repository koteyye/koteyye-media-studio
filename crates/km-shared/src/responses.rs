//! Типы ответов Cheaper Inference: изображения, видео, генерации, баланс, аудио.
//!
//! CI может не прислать опциональные поля — они получают значения по умолчанию;
//! неизвестные поля игнорируются. Стоимость приходит строкой с 6 знаками (USD),
//! а не числом с плавающей точкой.

use serde::{Deserialize, Serialize};

/// Тарификация ответа CI: стоимость строкой с 6 знаками после запятой (USD).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Billing {
    /// Стоимость генерации в USD, например `"0.040000"`; CI может прислать `null`.
    #[serde(default)]
    pub billed_cost_usd: Option<String>,
}

impl Billing {
    /// Стоимость в USD числом; `None`, если поле отсутствует, не парсится
    /// или некорректно (NaN, бесконечность, отрицательное значение).
    pub fn cost_usd(&self) -> Option<f64> {
        self.billed_cost_usd
            .as_deref()?
            .parse::<f64>()
            .ok()
            .filter(|c| c.is_finite() && *c >= 0.0)
    }
}

/// Элемент ответа images: base64 или URL — в зависимости от `response_format`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ImageItem {
    /// Изображение в base64 (без data-URL префикса), если запрошен `b64_json`.
    #[serde(default)]
    pub b64_json: Option<String>,
    /// URL изображения, если запрошен `url`.
    #[serde(default)]
    pub url: Option<String>,
    /// MIME-тип изображения, если CI его присылает.
    #[serde(default)]
    pub content_type: Option<String>,
}

/// Ответ `POST /v1/images/generations` и `/v1/images/edits`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ImageResponse {
    /// Unix-время создания ответа.
    #[serde(default)]
    pub created: Option<i64>,
    /// Сгенерированные изображения.
    #[serde(default)]
    pub data: Vec<ImageItem>,
    /// Идентификатор запроса в CI.
    #[serde(default)]
    pub request_id: Option<String>,
    /// Тарификация запроса.
    #[serde(default)]
    pub billing: Option<Billing>,
}

/// Элемент ответа videos: ссылка на готовый файл.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct VideoItem {
    /// URL видео.
    #[serde(default)]
    pub url: Option<String>,
    /// MIME-тип видео, если CI его присылает.
    #[serde(default)]
    pub content_type: Option<String>,
}

/// Ответ `POST /v1/videos/generations`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct VideoResponse {
    /// Идентификатор видео-задания в CI.
    pub id: String,
    /// Статус задания строкой, например `completed`; значения определяет CI.
    pub status: String,
    /// Готовые видео.
    #[serde(default)]
    pub videos: Vec<VideoItem>,
    /// Идентификатор запроса в CI.
    #[serde(default)]
    pub request_id: Option<String>,
    /// Тарификация запроса.
    #[serde(default)]
    pub billing: Option<Billing>,
    /// Детали тарификации (usage) как есть — формат зависит от модели.
    #[serde(default)]
    pub usage: Option<serde_json::Value>,
}

/// Файл завершённой генерации (`GET /v1/generations/{request_id}`).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct GenerationFile {
    /// Порядковый номер файла в генерации.
    pub index: u32,
    /// Тип файла (`type` в JSON), например `video` или `image`.
    #[serde(rename = "type")]
    pub kind: String,
    /// MIME-тип файла, например `video/mp4`.
    pub content_type: String,
    /// Размер файла в байтах.
    pub size_bytes: u64,
    /// URL для скачивания файла.
    pub url: String,
}

/// Завершённая генерация со списком файлов.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Generation {
    /// Идентификатор запроса в CI.
    pub request_id: String,
    /// Эндпоинт, на котором шла генерация, например `/v1/videos/generations`.
    pub endpoint: String,
    /// Момент создания (ISO 8601, строкой как в ответе CI).
    pub created_at: String,
    /// Момент истечения срока хранения файлов (ISO 8601).
    pub expires_at: String,
    /// Файлы генерации.
    #[serde(default)]
    pub files: Vec<GenerationFile>,
}

/// Баланс аккаунта (`GET /v1/account/balance`).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct AccountBalance {
    /// Код валюты, например `USD`.
    pub currency: String,
    /// Полный баланс в USD.
    pub balance_usd: f64,
    /// Доступно к трате в USD.
    pub available_usd: f64,
    /// Зарезервировано под активные генерации, USD.
    pub reserved_usd: f64,
}

/// Аудио-ответ CI: сырые байты тела плюс MIME-тип и стоимость.
///
/// Собирается из бинарного тела HTTP-ответа, а не из JSON.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct AudioOutput {
    /// Сырые байты аудио.
    pub bytes: Vec<u8>,
    /// MIME-тип из заголовка Content-Type.
    pub content_type: String,
    /// Стоимость в USD из billing, если CI её прислал.
    #[serde(default)]
    pub cost_usd: Option<f64>,
}

// Типы ответов ходят между потоками (jobs, Tauri-команды) — фиксируем Send + Sync.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Billing>();
    assert_send_sync::<ImageResponse>();
    assert_send_sync::<VideoResponse>();
    assert_send_sync::<Generation>();
    assert_send_sync::<AccountBalance>();
    assert_send_sync::<AudioOutput>();
};
