//! Типы истории генераций: запись, частичное обновление, фильтр, статус.

use serde::{Deserialize, Serialize};

use crate::requests::{GenMode, MediaKind};

/// Статус задания генерации.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    /// Запущено, ждёт завершения (в т.ч. после перезапуска приложения).
    Pending,
    /// Успешно завершено.
    Completed,
    /// Завершилось ошибкой.
    Failed,
}

/// Запись истории: одна генерация от запуска до результата.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct HistoryRecord {
    /// Локальный идентификатор записи (UUID).
    pub id: String,
    /// Сервис, принявший запрос. Прежние записи Rust-студии созданы через CI.
    #[serde(default)]
    pub provider: crate::Provider,
    /// Момент создания, Unix-время в секундах.
    pub created_at: i64,
    /// Момент последнего обновления, Unix-время в секундах.
    pub updated_at: i64,
    /// Вид медиа.
    pub kind: MediaKind,
    /// Режим генерации.
    pub mode: GenMode,
    /// Идентификатор модели.
    pub model: String,
    /// Текст промпта.
    pub prompt: String,
    /// Статус задания.
    pub status: JobStatus,
    /// Идентификатор запроса в CI, если известен.
    #[serde(default)]
    pub request_id: Option<String>,
    /// Пути сохранённых файлов.
    #[serde(default)]
    pub local_paths: Vec<String>,
    /// Стоимость в USD, если известна.
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// Параметры генерации как JSON-строка (для повторного запуска).
    pub params_json: String,
    /// Текст ошибки, если статус failed.
    #[serde(default)]
    pub error: Option<String>,
}

/// Частичное обновление записи истории: `None` означает «не менять».
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct HistoryPatch {
    /// Новый статус.
    pub status: Option<JobStatus>,
    /// Идентификатор запроса CI.
    pub request_id: Option<String>,
    /// Пути сохранённых файлов.
    pub local_paths: Option<Vec<String>>,
    /// Стоимость в USD.
    pub cost_usd: Option<f64>,
    /// Текст ошибки.
    pub error: Option<String>,
}

/// Фильтр списка истории.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct HistoryFilter {
    /// Только этот вид медиа.
    pub kind: Option<MediaKind>,
    /// Подстрока для поиска.
    pub search: Option<String>,
}

// Записи истории ходят между потоками (storage, jobs, Tauri-команды).
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<HistoryRecord>();
    assert_send_sync::<HistoryPatch>();
    assert_send_sync::<HistoryFilter>();
};
