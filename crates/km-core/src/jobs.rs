//! Оркестратор джобов генерации: запуск, восстановление после сбоя, баланс.
//!
//! [`Jobs`] собирает клиент ([`CiClient`]), хранилище ([`Storage`]) и файлы
//! ([`save_media`]) в единый поток: запись истории создаётся до запроса к CI,
//! результат сохраняется в локальную папку (локальная копия первична), статус
//! и стоимость обновляются в БД, а UI получает события [`AppEvent`]. Записи
//! `Pending`, пережившие перезапуск, восстанавливаются через [`CiClient::generation`];
//! баланс гибридный — реальный из CI при наличии scope, иначе локальный расход.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use km_shared::{
    AppEvent, BalanceView, GenMode, GenerationRequest, HistoryPatch, HistoryRecord, JobStatus,
    MediaKind,
};
use uuid::Uuid;

use crate::client::CiClient;
use crate::error::CoreError;
use crate::files::save_media;
use crate::images::image_bytes;
use crate::storage::Storage;

/// Результат операций оркестратора: ошибка ядра [`CoreError`].
type Result<T> = std::result::Result<T, CoreError>;

/// Таймаут скачивания видео-файлов: крупное видео на медленном канале может
/// не успеть за дефолтные 60 секунд клиента, поэтому видео качается отдельным
/// клиентом с увеличенным таймаутом (как и синхронная видео-генерация).
const VIDEO_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// MIME-тип изображения по умолчанию, если CI не прислал `content_type`.
const DEFAULT_IMAGE_CONTENT_TYPE: &str = "image/png";

/// MIME-тип видео по умолчанию, если CI не прислал `content_type`.
const DEFAULT_VIDEO_CONTENT_TYPE: &str = "video/mp4";

/// Оркестратор джобов генерации поверх клиента, хранилища и локальной папки.
///
/// Дёргается из Tauri-команд: [`Jobs::run`] запускает генерацию,
/// [`Jobs::recover_pending`] восстанавливает незавершённые записи при старте,
/// [`Jobs::balance_view`] отдаёт гибридный баланс. События уходят в колбэк
/// `emit` (в Tauri — эмиттер событий, в тестах — коллектор).
pub struct Jobs {
    client: CiClient,
    /// Клиент с увеличенным таймаутом только для скачивания видео-файлов.
    video_download: CiClient,
    storage: Storage,
    folder: PathBuf,
    emit: Arc<dyn Fn(AppEvent) + Send + Sync>,
}

impl Jobs {
    /// Создаёт оркестратор: клиент CI, хранилище, папка сохранения и колбэк событий.
    ///
    /// Для скачивания видео строится отдельный клиент с таймаутом 600 секунд;
    /// остальные запросы идут через `client`.
    #[must_use]
    pub fn new(
        client: CiClient,
        storage: Storage,
        folder: PathBuf,
        emit: Arc<dyn Fn(AppEvent) + Send + Sync>,
    ) -> Jobs {
        let video_download = client.clone().with_timeout(VIDEO_DOWNLOAD_TIMEOUT);
        Jobs {
            client,
            video_download,
            storage,
            folder,
            emit,
        }
    }

    /// Запускает генерацию: запись `Pending` → запрос к CI → файлы → `Completed`.
    ///
    /// Порядок: вставка `Pending` (id — UUID, время в секундах), событие
    /// `GenerationStarted`, вызов клиента по виду медиа, сохранение каждого
    /// файла в локальную папку, обновление `Completed` со стоимостью, событие
    /// `GenerationDone`. `request_id` фиксируется в БД сразу после ответа CI и
    /// до скачивания файлов, поэтому краш на этапе скачивания восстановим.
    /// При ошибке или таймауте запись переводится в `Failed`, шлётся
    /// `GenerationFailed` и возвращается [`Err`] (запись `Failed` остаётся в истории).
    ///
    /// # Errors
    ///
    /// [`CoreError`] от клиента (валидация, HTTP, таймаут, сеть), хранилища или
    /// файловой системы; запись истории при этом уже переведена в `Failed`.
    pub async fn run(&self, req: GenerationRequest) -> Result<HistoryRecord> {
        let now = now_unix_secs()?;
        let params_json =
            serde_json::to_string(&req).map_err(|e| CoreError::Serde(e.to_string()))?;
        let rec = HistoryRecord {
            id: Uuid::new_v4().to_string(),
            created_at: now,
            updated_at: now,
            kind: req.kind,
            mode: req.mode,
            model: req.model.clone(),
            prompt: req.prompt.clone(),
            status: JobStatus::Pending,
            request_id: None,
            local_paths: Vec::new(),
            cost_usd: None,
            params_json,
            error: None,
        };
        self.storage.insert_history(&rec)?;
        (self.emit)(AppEvent::GenerationStarted {
            history_id: rec.id.clone(),
        });

        match self.produce_and_save(&rec, &req).await {
            Ok(record) => Ok(record),
            Err(err) => {
                self.fail(&rec, err.to_string())?;
                Err(err)
            }
        }
    }

    /// Восстанавливает записи `Pending`, пережившие перезапуск приложения.
    ///
    /// Для каждой записи: с корректным `request_id` — запрос
    /// [`CiClient::generation`], скачивание и сохранение файлов, `Completed`;
    /// без `request_id` (ответ CI не был получен) или при ошибке восстановления —
    /// `Failed`. Возвращает обновлённые записи; сбой одной записи не прерывает
    /// остальные. События `GenerationDone`/`GenerationFailed` шлются по исходу.
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] / [`CoreError::Serde`], если не удалось прочитать или
    /// обновить хранилище; ошибки CI отдельных записей переводят их в `Failed`.
    pub async fn recover_pending(&self) -> Result<Vec<HistoryRecord>> {
        let pending = self.storage.pending()?;
        let mut updated = Vec::with_capacity(pending.len());
        for rec in pending {
            updated.push(self.recover_one(&rec).await?);
        }
        Ok(updated)
    }

    /// Гибридный баланс: реальный из CI (если `show_real`) плюс локальный расход.
    ///
    /// При `show_real = true` запрашивается [`CiClient::balance`]; 403
    /// ([`CoreError::ScopeMissing`]) или любая ошибка CI дают `real = None` —
    /// фолбэк на локальный расход. При `show_real = false` запрос не идёт.
    /// `local_spend_usd` — сумма стоимостей по всей истории из [`Storage`].
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`], если не удалось агрегировать расход; ошибка запроса
    /// баланса не пробрасывается (скрывает реальный баланс, оставляя локальный).
    pub async fn balance_view(&self, show_real: bool) -> Result<BalanceView> {
        let local_spend_usd = self.storage.total_spend_usd()?;
        let real = if show_real {
            self.client.balance().await.ok()
        } else {
            None
        };
        Ok(BalanceView {
            real,
            local_spend_usd,
        })
    }

    /// Генерирует медиа и сохраняет файлы, возвращая запись `Completed`.
    async fn produce_and_save(
        &self,
        rec: &HistoryRecord,
        req: &GenerationRequest,
    ) -> Result<HistoryRecord> {
        let generated = self.generate(&rec.id, req).await?;
        let local_paths = self.save_files(req.kind, &generated.files)?;
        self.complete(rec, generated.request_id, local_paths, generated.cost_usd)
    }

    /// Вызывает клиент по виду медиа и собирает байты файлов со стоимостью.
    ///
    /// `request_id` изображений и видео сохраняется в БД до скачивания файлов:
    /// краш после ответа CI, но до сохранения оставляет запись `Pending` с id,
    /// которую подхватит [`Jobs::recover_pending`].
    async fn generate(&self, id: &str, req: &GenerationRequest) -> Result<Generated> {
        match req.kind {
            MediaKind::Image => {
                let response = match req.mode {
                    GenMode::ImageEdit => self.client.edit_image(req).await?,
                    _ => self.client.generate_image(req).await?,
                };
                self.set_request_id(id, response.request_id.as_deref())?;
                let mut files = Vec::with_capacity(response.data.len());
                for item in &response.data {
                    let bytes = image_bytes(item, &self.client).await?;
                    let content_type = item
                        .content_type
                        .clone()
                        .unwrap_or_else(|| DEFAULT_IMAGE_CONTENT_TYPE.to_owned());
                    files.push(MediaBytes {
                        bytes,
                        content_type,
                    });
                }
                Ok(Generated {
                    request_id: response.request_id,
                    cost_usd: response.billing.as_ref().and_then(|b| b.cost_usd()),
                    files,
                })
            }
            MediaKind::Audio => {
                let output = self.client.speech(req).await?;
                Ok(Generated {
                    request_id: None,
                    cost_usd: output.cost_usd,
                    files: vec![MediaBytes {
                        bytes: output.bytes,
                        content_type: output.content_type,
                    }],
                })
            }
            MediaKind::Video => {
                let response = self.client.generate_video(req).await?;
                self.set_request_id(id, response.request_id.as_deref())?;
                let mut files = Vec::with_capacity(response.videos.len());
                for item in &response.videos {
                    let Some(url) = &item.url else {
                        return Err(CoreError::Serde("элемент видео без url".to_owned()));
                    };
                    let bytes = self.video_download.download(url).await?;
                    let content_type = item
                        .content_type
                        .clone()
                        .unwrap_or_else(|| DEFAULT_VIDEO_CONTENT_TYPE.to_owned());
                    files.push(MediaBytes {
                        bytes,
                        content_type,
                    });
                }
                Ok(Generated {
                    request_id: response.request_id,
                    cost_usd: response.billing.as_ref().and_then(|b| b.cost_usd()),
                    files,
                })
            }
        }
    }

    /// Сохраняет байты файлов в локальную папку, возвращая пути как строки.
    ///
    /// Content-type нормализуется перед [`save_media`] (CI может прислать
    /// MIME с параметрами или в другом регистре); индекс варианта — `index + 1`.
    fn save_files(&self, kind: MediaKind, files: &[MediaBytes]) -> Result<Vec<String>> {
        let total = files.len();
        let mut paths = Vec::with_capacity(total);
        for (index, file) in files.iter().enumerate() {
            let content_type = normalize_content_type(&file.content_type);
            let path = save_media(&self.folder, kind, index, total, &file.bytes, &content_type)?;
            paths.push(path.to_string_lossy().into_owned());
        }
        Ok(paths)
    }

    /// Восстанавливает одну запись `Pending`: с id — генерация, без id — `Failed`.
    async fn recover_one(&self, rec: &HistoryRecord) -> Result<HistoryRecord> {
        let Some(request_id) = rec
            .request_id
            .as_deref()
            .filter(|id| is_safe_request_id(id))
        else {
            return self.fail(
                rec,
                "восстановление невозможно: нет корректного request_id (ответ CI не был получен)"
                    .to_owned(),
            );
        };
        match self.fetch_and_save(rec, request_id).await {
            Ok(record) => Ok(record),
            Err(err) => self.fail(rec, err.to_string()),
        }
    }

    /// Запрашивает файлы генерации, скачивает и сохраняет их, завершая запись.
    async fn fetch_and_save(&self, rec: &HistoryRecord, request_id: &str) -> Result<HistoryRecord> {
        let generation = self.client.generation(request_id).await?;
        let total = generation.files.len();
        let mut local_paths = Vec::with_capacity(total);
        for (index, file) in generation.files.iter().enumerate() {
            let bytes = self.download_for(rec.kind, &file.url).await?;
            let content_type = normalize_content_type(&file.content_type);
            let path = save_media(&self.folder, rec.kind, index, total, &bytes, &content_type)?;
            local_paths.push(path.to_string_lossy().into_owned());
        }
        // request_id и стоимость не меняем: id уже в записи, а recovery-эндпоинт
        // не отдаёт billing — стоимость восстановленной записи остаётся прежней.
        self.complete(rec, rec.request_id.clone(), local_paths, rec.cost_usd)
    }

    /// Скачивает файл: видео — клиентом с увеличенным таймаутом, остальное — обычным.
    async fn download_for(&self, kind: MediaKind, url: &str) -> Result<Vec<u8>> {
        match kind {
            MediaKind::Video => self.video_download.download(url).await,
            _ => self.client.download(url).await,
        }
    }

    /// Фиксирует `request_id` в БД сразу после ответа CI (статус остаётся `Pending`).
    fn set_request_id(&self, id: &str, request_id: Option<&str>) -> Result<()> {
        if let Some(request_id) = request_id {
            let patch = HistoryPatch {
                request_id: Some(request_id.to_owned()),
                ..HistoryPatch::default()
            };
            self.storage.update_history(id, &patch)?;
        }
        Ok(())
    }

    /// Переводит запись в `Completed`, шлёт `GenerationDone`, возвращает запись.
    ///
    /// `request_id` и `cost_usd` — итоговые значения: `None` означает «не менять»
    /// (как в [`HistoryPatch`]), поэтому для восстановления передаются текущие.
    fn complete(
        &self,
        base: &HistoryRecord,
        request_id: Option<String>,
        local_paths: Vec<String>,
        cost_usd: Option<f64>,
    ) -> Result<HistoryRecord> {
        let mut rec = base.clone();
        rec.status = JobStatus::Completed;
        rec.request_id = request_id;
        rec.local_paths = local_paths;
        rec.cost_usd = cost_usd;
        rec.error = None;
        rec.updated_at = now_unix_secs()?;
        let patch = HistoryPatch {
            status: Some(JobStatus::Completed),
            request_id: rec.request_id.clone(),
            local_paths: Some(rec.local_paths.clone()),
            cost_usd: rec.cost_usd,
            error: None,
        };
        self.storage.update_history(&rec.id, &patch)?;
        (self.emit)(AppEvent::GenerationDone {
            history_id: rec.id.clone(),
        });
        Ok(rec)
    }

    /// Переводит запись в `Failed`, шлёт `GenerationFailed`, возвращает запись.
    fn fail(&self, base: &HistoryRecord, error: String) -> Result<HistoryRecord> {
        let mut rec = base.clone();
        rec.status = JobStatus::Failed;
        rec.error = Some(error.clone());
        rec.updated_at = now_unix_secs()?;
        let patch = HistoryPatch {
            status: Some(JobStatus::Failed),
            error: Some(error.clone()),
            ..HistoryPatch::default()
        };
        self.storage.update_history(&rec.id, &patch)?;
        (self.emit)(AppEvent::GenerationFailed {
            history_id: rec.id.clone(),
            error,
        });
        Ok(rec)
    }
}

/// Debug без колбэка событий (замыкание не печатается).
impl fmt::Debug for Jobs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Jobs")
            .field("folder", &self.folder)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

/// Результат генерации: идентификатор запроса CI, стоимость и байты файлов.
struct Generated {
    request_id: Option<String>,
    cost_usd: Option<f64>,
    files: Vec<MediaBytes>,
}

/// Байты одного медиа-файла и его MIME-тип до нормализации.
struct MediaBytes {
    bytes: Vec<u8>,
    content_type: String,
}

/// Нормализует content-type: без параметров, в нижнем регистре и без пробелов.
///
/// CI может прислать `image/png; charset=binary` или `IMAGE/PNG`; [`ext_for`]
/// сравнивает строки точно, поэтому параметры и регистр убираются заранее.
fn normalize_content_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase()
}

/// Допустим ли `request_id` для подстановки в URL recovery-эндпоинта.
///
/// Defense-in-depth: только непустые ASCII-буквы/цифры, `-` и `_` (UUID и id
/// CI); служебные символы (`/`, `?`, `..`) не попадают в путь запроса.
fn is_safe_request_id(request_id: &str) -> bool {
    !request_id.is_empty()
        && request_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// Текущий момент как Unix-время в секундах — единица `created_at`/`updated_at`.
fn now_unix_secs() -> Result<i64> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| CoreError::Db(format!("системное время раньше UNIX_EPOCH: {e}")))?
        .as_secs();
    i64::try_from(secs).map_err(|_| CoreError::Db("момент времени вне диапазона i64".to_owned()))
}

// Оркестратор живёт в Tauri-состоянии и ходит между потоками — фиксируем Send + Sync.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Jobs>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_normalizes_parameters_case_and_spaces() {
        assert_eq!(
            normalize_content_type("image/png; charset=binary"),
            "image/png"
        );
        assert_eq!(normalize_content_type(" IMAGE/JPEG "), "image/jpeg");
        assert_eq!(normalize_content_type("audio/mpeg"), "audio/mpeg");
        assert_eq!(normalize_content_type(""), "");
    }

    #[test]
    fn request_id_sanitization_allows_uuid_and_rejects_specials() {
        assert!(is_safe_request_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_safe_request_id("req_123"));
        assert!(!is_safe_request_id(""));
        assert!(!is_safe_request_id("../../etc/passwd"));
        assert!(!is_safe_request_id("a/b"));
        assert!(!is_safe_request_id("id?x=1"));
        assert!(!is_safe_request_id("id space"));
    }
}
