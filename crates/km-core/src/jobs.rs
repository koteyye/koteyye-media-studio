//! Оркестратор джобов генерации: запуск, восстановление после сбоя, баланс.
//!
//! [`Jobs`] собирает клиент ([`CiClient`]), хранилище ([`Storage`]) и файлы
//! ([`save_media`]) в единый поток: запись истории создаётся до запроса к CI,
//! результат сохраняется в локальную папку (локальная копия первична), статус
//! и стоимость обновляются в БД, а UI получает события [`AppEvent`]. Записи
//! `Pending`, пережившие перезапуск, восстанавливаются через [`CiClient::generation`]:
//! транзиентные ошибки (сеть, таймаут, ввод-вывод, БД, HTTP 5xx/429) оставляют
//! запись `Pending` до следующего запуска, оплаченная генерация не теряется.
//! Пустой результат или definitive-статус видео при 200 — ошибка, а не тихий
//! `Completed`; ещё идущее видео (`processing`/`pending`/`in_progress`)
//! остаётся `Pending` с сохранённым `request_id`.
//! Баланс гибридный — реальный из CI при наличии scope, иначе локальный расход.

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
    /// `GenerationDone`. `request_id` и стоимость фиксируются в БД сразу после
    /// ответа CI и до скачивания файлов, поэтому краш на этапе скачивания
    /// восстановим, а оплаченная цена не теряется.
    ///
    /// Транзиентная ошибка (`is_transient`) после фиксации `request_id`
    /// оставляет запись `Pending`: оплаченная генерация жива на CI,
    /// [`Jobs::recover_pending`] повторит скачивание. Для UI возвращается
    /// [`Err`], событие `GenerationFailed` не шлётся. Если `request_id` ещё
    /// не получен (сбой до ответа CI), запись переводится в `Failed`.
    /// Definitive-ошибки (прочие 4xx, пустой результат) также переводят
    /// в `Failed` с событием `GenerationFailed`. Видео в статусах
    /// `processing`/`pending`/`in_progress` остаётся `Pending` с сохранённым
    /// `request_id` — генерация ещё идёт на CI.
    ///
    /// # Errors
    ///
    /// [`CoreError`] от клиента (валидация, HTTP, таймаут, сеть), хранилища или
    /// файловой системы; запись истории при этом либо `Failed` (definitive),
    /// либо остаётся `Pending` (транзиентный сбой после фиксации `request_id`).
    pub async fn run(&self, req: GenerationRequest) -> Result<HistoryRecord> {
        let now = now_unix_secs()?;
        let params_json =
            serde_json::to_string(&req).map_err(|e| CoreError::Serde(e.to_string()))?;
        let mut rec = HistoryRecord {
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

        match self.produce_and_save(&mut rec, &req).await {
            Ok(Produced::Completed(record)) => Ok(record),
            Ok(Produced::InProgress { status }) => {
                // request_id уже в БД: запись остаётся Pending, recovery
                // подхватит её при следующем запуске. Для UI — ошибка.
                Err(CoreError::Serde(format!(
                    "CI вернул незавершённый статус видео: {status} (генерация ещё идёт)"
                )))
            }
            Err(err) => {
                // Транзиентный сбой после фиксации request_id не сжигает
                // запись: оплаченная генерация жива на CI, повторит recovery.
                if is_transient(&err) && rec.request_id.is_some() {
                    eprintln!("генерация {} отложена: {err}", rec.id);
                    return Err(err);
                }
                self.fail(&rec, err.to_string())?;
                Err(err)
            }
        }
    }

    /// Восстанавливает записи `Pending`, пережившие перезапуск приложения.
    ///
    /// Для каждой записи: с корректным `request_id` — запрос
    /// [`CiClient::generation`], скачивание и сохранение файлов, `Completed`;
    /// без `request_id` (ответ CI не был получен) — `Failed`. Транзиентные
    /// ошибки (сеть, таймаут, ввод-вывод, БД, HTTP 5xx/429) оставляют запись
    /// `Pending`: файлы живут на CI 7 дней, генерация восстановится при
    /// следующем запуске. Definitive-ответы CI (404/410, пустой результат)
    /// переводят в `Failed`; если сам `Failed` не записался из-за сбоя БД,
    /// запись тоже остаётся `Pending`.
    /// Сбой хранилища на одной записи логируется и не прерывает остальные.
    /// Возвращает обработанные записи с итоговым статусом (оставшиеся
    /// `Pending` — как есть). События `GenerationDone`/`GenerationFailed`
    /// шлются по исходу.
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] / [`CoreError::Serde`], если не удалось прочитать
    /// хранилище; ошибки отдельных записей не прерывают восстановление.
    pub async fn recover_pending(&self) -> Result<Vec<HistoryRecord>> {
        let pending = self.storage.pending()?;
        let mut updated = Vec::with_capacity(pending.len());
        for rec in pending {
            match self.recover_one(&rec).await {
                Ok(record) => updated.push(record),
                Err(err) => {
                    // Изоляция: сбой хранилища на одной записи не прерывает
                    // остальные; запись остаётся Pending до следующего запуска.
                    eprintln!("восстановление записи {}: {err}", rec.id);
                }
            }
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

    /// Генерирует медиа и сохраняет файлы: запись `Completed` или ещё идущее видео.
    async fn produce_and_save(
        &self,
        rec: &mut HistoryRecord,
        req: &GenerationRequest,
    ) -> Result<Produced> {
        match self.generate(rec, req).await? {
            Generated::Ready {
                request_id,
                cost_usd,
                files,
            } => {
                let local_paths = self.save_files(req.kind, &files)?;
                let record = self.complete(rec, request_id, local_paths, cost_usd)?;
                Ok(Produced::Completed(record))
            }
            Generated::InProgress { status } => Ok(Produced::InProgress { status }),
        }
    }

    /// Вызывает клиент по виду медиа и собирает байты файлов со стоимостью.
    ///
    /// `request_id` и стоимость изображений и видео сохраняются в БД до
    /// скачивания файлов и до проверки статуса видео: краш после ответа CI,
    /// но до сохранения оставляет запись `Pending` с id и оплаченной ценой,
    /// которую подхватит [`Jobs::recover_pending`]. После успешной фиксации
    /// поля обновляются и в `rec` — по `request_id` [`Jobs::run`] отличает
    /// восстановимый сбой от Failed.
    async fn generate(
        &self,
        rec: &mut HistoryRecord,
        req: &GenerationRequest,
    ) -> Result<Generated> {
        match req.kind {
            MediaKind::Image => {
                let response = match req.mode {
                    GenMode::ImageEdit => self.client.edit_image(req).await?,
                    _ => self.client.generate_image(req).await?,
                };
                if response.data.is_empty() {
                    return Err(CoreError::Serde(
                        "CI вернул пустой результат: data".to_owned(),
                    ));
                }
                let cost_usd = response.billing.as_ref().and_then(|b| b.cost_usd());
                self.set_request_id_and_cost(&rec.id, response.request_id.as_deref(), cost_usd)?;
                rec.request_id = response.request_id.clone();
                rec.cost_usd = cost_usd;
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
                Ok(Generated::Ready {
                    request_id: response.request_id,
                    cost_usd,
                    files,
                })
            }
            MediaKind::Audio => {
                let output = self.client.speech(req).await?;
                if output.bytes.is_empty() {
                    return Err(CoreError::Serde(
                        "CI вернул пустой результат: аудио".to_owned(),
                    ));
                }
                Ok(Generated::Ready {
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
                let cost_usd = response.billing.as_ref().and_then(|b| b.cost_usd());
                // request_id и стоимость фиксируются до status-guard: даже
                // незавершённая генерация восстановима, оплаченные id и цена
                // не должны потеряться.
                self.set_request_id_and_cost(&rec.id, response.request_id.as_deref(), cost_usd)?;
                rec.request_id = response.request_id.clone();
                rec.cost_usd = cost_usd;
                match response.status.as_str() {
                    // CI документирует completed/succeeded как успешные статусы.
                    "completed" | "succeeded" => {
                        if response.videos.is_empty() {
                            return Err(CoreError::Serde(
                                "CI вернул пустой результат: videos".to_owned(),
                            ));
                        }
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
                        Ok(Generated::Ready {
                            request_id: response.request_id,
                            cost_usd,
                            files,
                        })
                    }
                    // Генерация ещё идёт: запись остаётся Pending с request_id,
                    // recovery заберёт файлы, когда CI завершит задание.
                    "processing" | "pending" | "in_progress" => Ok(Generated::InProgress {
                        status: response.status.clone(),
                    }),
                    // Definitive-статусы (failed, cancelled, …) — ошибка.
                    other => Err(CoreError::Serde(format!(
                        "CI вернул незавершённый статус видео: {other}"
                    ))),
                }
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
    ///
    /// Транзиентная ошибка ([`is_transient`]) оставляет запись `Pending` без
    /// события: оплаченная генерация жива на CI, повтор — при следующем запуске.
    async fn recover_one(&self, rec: &HistoryRecord) -> Result<HistoryRecord> {
        let Some(request_id) = rec
            .request_id
            .as_deref()
            .filter(|id| is_safe_request_id(id))
        else {
            return self.fail_or_keep_pending(
                rec,
                "восстановление невозможно: нет корректного request_id (ответ CI не был получен)"
                    .to_owned(),
            );
        };
        match self.fetch_and_save(rec, request_id).await {
            Ok(record) => Ok(record),
            Err(err) if is_transient(&err) => {
                eprintln!("восстановление записи {} отложено: {err}", rec.id);
                Ok(rec.clone())
            }
            Err(err) => self.fail_or_keep_pending(rec, err.to_string()),
        }
    }

    /// Переводит запись в `Failed`; транзиентный сбой самой записи оставляет `Pending`.
    ///
    /// Если БД не приняла `Failed` (например, write-лок), оплаченная генерация
    /// не сжигается: запись остаётся `Pending` до следующего запуска.
    fn fail_or_keep_pending(&self, rec: &HistoryRecord, error: String) -> Result<HistoryRecord> {
        match self.fail(rec, error) {
            Ok(record) => Ok(record),
            Err(err) if is_transient(&err) => {
                eprintln!("запись {} остаётся Pending: {err}", rec.id);
                Ok(rec.clone())
            }
            Err(err) => Err(err),
        }
    }

    /// Запрашивает файлы генерации, скачивает и сохраняет их, завершая запись.
    async fn fetch_and_save(&self, rec: &HistoryRecord, request_id: &str) -> Result<HistoryRecord> {
        let generation = self.client.generation(request_id).await?;
        if generation.files.is_empty() {
            return Err(CoreError::Serde(
                "CI вернул пустой результат: files".to_owned(),
            ));
        }
        let total = generation.files.len();
        let mut local_paths = Vec::with_capacity(total);
        for (index, file) in generation.files.iter().enumerate() {
            let bytes = self.download_for(rec.kind, &file.url).await?;
            let content_type = normalize_content_type(&file.content_type);
            let path = save_media(&self.folder, rec.kind, index, total, &bytes, &content_type)?;
            local_paths.push(path.to_string_lossy().into_owned());
        }
        // request_id и стоимость не меняем: оба зафиксированы в БД сразу после
        // ответа CI (recovery-эндпоинт billing не отдаёт) — восстановление
        // завершает запись с оплаченной ценой, не теряя её.
        self.complete(rec, rec.request_id.clone(), local_paths, rec.cost_usd)
    }

    /// Скачивает файл: видео — клиентом с увеличенным таймаутом, остальное — обычным.
    async fn download_for(&self, kind: MediaKind, url: &str) -> Result<Vec<u8>> {
        match kind {
            MediaKind::Video => self.video_download.download(url).await,
            _ => self.client.download(url).await,
        }
    }

    /// Фиксирует `request_id` и стоимость в БД сразу после ответа CI
    /// (статус остаётся `Pending`).
    ///
    /// Оба поля персистятся одним патчем до скачивания файлов: транзиентный
    /// сбой оставляет запись `Pending` с оплаченной ценой —
    /// [`Jobs::recover_pending`] завершит её без потери стоимости
    /// (recovery-эндпоинт billing не отдаёт).
    fn set_request_id_and_cost(
        &self,
        id: &str,
        request_id: Option<&str>,
        cost_usd: Option<f64>,
    ) -> Result<()> {
        if request_id.is_some() || cost_usd.is_some() {
            let patch = HistoryPatch {
                request_id: request_id.map(str::to_owned),
                cost_usd,
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
        if let Err(db_err) = self.storage.update_history(&rec.id, &patch) {
            // Исходная ошибка генерации не должна потеряться, если БД не
            // приняла Failed: логируем обе, контракт возврата прежний.
            eprintln!(
                "запись {}: не удалось сохранить Failed ({db_err}); исходная ошибка: {error}",
                rec.id
            );
            return Err(db_err);
        }
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

/// Исход генерации медиа: готовые файлы или ещё идущая генерация видео.
enum Generated {
    /// Файлы получены: идентификатор запроса CI, стоимость и байты файлов.
    Ready {
        request_id: Option<String>,
        cost_usd: Option<f64>,
        files: Vec<MediaBytes>,
    },
    /// Видео ещё генерируется на CI (статус ответа); `request_id` уже в БД.
    InProgress { status: String },
}

/// Исход [`Jobs::produce_and_save`]: запись завершена или генерация ещё идёт.
enum Produced {
    /// Файлы сохранены, запись переведена в `Completed`.
    Completed(HistoryRecord),
    /// Видео ещё генерируется (статус CI); запись остаётся `Pending`.
    InProgress { status: String },
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

/// Транзиентная ли ошибка: сеть, таймаут, ввод-вывод, БД или HTTP 5xx/429.
///
/// При таких ошибках запись остаётся `Pending` — оплаченная генерация жива
/// на CI (файлы 7 дней), повтор при следующем запуске имеет смысл. Сбой БД
/// транзиентен: `Failed` может не записаться, а терять Paid-Pending нельзя.
/// Definitive-ответы CI (прочие 4xx, [`CoreError::Serde`]), ошибки валидации
/// и ключа транзиентными не считаются: запись переводится в `Failed`.
///
/// Матч исчерпывающий по всем вариантам [`CoreError`] намеренно: денежная
/// классификация — новый вариант ошибки не должен молча стать definitive
/// (или транзиентным), добавление варианта обязывает классифицировать его явно.
fn is_transient(err: &CoreError) -> bool {
    match err {
        CoreError::Network(_) | CoreError::Timeout | CoreError::Io(_) | CoreError::Db(_) => true,
        CoreError::Http { status, .. } => matches!(status, 500..=599 | 429),
        CoreError::ScopeMissing
        | CoreError::NoKey
        | CoreError::Serde(_)
        | CoreError::Validation(_) => false,
    }
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
    fn transient_classification_covers_every_core_error_variant() {
        // Исчерпывающая классификация: каждый вариант CoreError проверен явно,
        // новый вариант не сможет молча попасть в definitive через wildcard.
        assert!(is_transient(&CoreError::Network("обрыв".to_owned())));
        assert!(is_transient(&CoreError::Timeout));
        assert!(is_transient(&CoreError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "нет прав",
        ))));
        // Сбой БД транзиентен: запись Failed может не записаться, а Paid-Pending
        // терять нельзя — оставляем Pending до следующего запуска.
        assert!(is_transient(&CoreError::Db("locked".to_owned())));
        // HTTP 5xx и 429 — транзиентные ответы CI: повтор имеет смысл.
        for status in [500, 503, 599, 429] {
            assert!(
                is_transient(&CoreError::Http {
                    status,
                    code: None,
                    message: "boom".to_owned(),
                }),
                "статус {status} должен быть транзиентным"
            );
        }
        // Прочие 4xx — definitive-ответы CI: запись переводится в Failed.
        for status in [400, 401, 403, 404, 410, 499] {
            assert!(
                !is_transient(&CoreError::Http {
                    status,
                    code: None,
                    message: "definitive".to_owned(),
                }),
                "статус {status} не должен быть транзиентным"
            );
        }
        assert!(!is_transient(&CoreError::Serde("пустой ответ".to_owned())));
        assert!(!is_transient(&CoreError::Validation("нет id".to_owned())));
        assert!(!is_transient(&CoreError::ScopeMissing));
        assert!(!is_transient(&CoreError::NoKey));
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
