//! Запуск генераций, локальное сохранение, восстановление и баланс провайдера.
//!
//! История создаётся перед POST. Идентификатор и известная стоимость
//! сохраняются до скачивания файлов. CI восстанавливает результаты по
//! квитанции, OpenRouter позволяет продолжить опрос видео по идентификатору.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use km_shared::{
    AppEvent, BalanceView, GenMode, GenerationRequest, HistoryPatch, HistoryRecord, JobStatus,
    Lang, MediaKind, Provider,
};
use uuid::Uuid;

use crate::client::{CiClient, OpenRouterKeyClient};
use crate::error::CoreError;
use crate::files::save_media;
use crate::images::image_bytes;
use crate::openrouter::video::OpenRouterVideoJob;
use crate::storage::Storage;
use crate::video::{safe_request_segment as is_safe_request_id, video_bytes};
use km_shared::image::raster_mime;

/// Результат операций оркестратора: ошибка ядра [`CoreError`].
type Result<T> = std::result::Result<T, CoreError>;

/// Таймаут скачивания видео-файлов: крупное видео на медленном канале может
/// не успеть за дефолтные 60 секунд клиента, поэтому видео качается отдельным
/// клиентом с увеличенным таймаутом (как и синхронная видео-генерация).
const VIDEO_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const OPENROUTER_POLL_INTERVAL: Duration = Duration::from_secs(10);
const OPENROUTER_POLL_TIMEOUT: Duration = Duration::from_secs(600);

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
    client: JobClient,
    /// Ссылки на файлы скачиваются без ключа.
    download: CiClient,
    storage: Storage,
    folder: PathBuf,
    emit: Arc<dyn Fn(AppEvent) + Send + Sync>,
}

enum JobClient {
    CheaperInference(CiClient),
    OpenRouter {
        client: OpenRouterKeyClient,
        key: String,
    },
}

pub struct StartedGeneration {
    request: GenerationRequest,
    record: HistoryRecord,
}

impl StartedGeneration {
    pub fn record(&self) -> &HistoryRecord {
        &self.record
    }
}

impl JobClient {
    fn provider(&self) -> Provider {
        match self {
            Self::CheaperInference(_) => Provider::CheaperInference,
            Self::OpenRouter { .. } => Provider::OpenRouter,
        }
    }

    fn can_recover(&self, kind: MediaKind) -> bool {
        self.provider() == Provider::CheaperInference || kind == MediaKind::Video
    }
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
        let download = client.clone();
        Jobs {
            client: JobClient::CheaperInference(client),
            download,
            storage,
            folder,
            emit,
        }
    }

    #[must_use]
    pub fn openrouter(
        client: OpenRouterKeyClient,
        key: String,
        storage: Storage,
        folder: PathBuf,
        emit: Arc<dyn Fn(AppEvent) + Send + Sync>,
    ) -> Jobs {
        Jobs {
            client: JobClient::OpenRouter { client, key },
            download: CiClient::new(""),
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
    /// Некорректный успешный ответ с квитанцией оставляет возможность восстановления.
    /// Видео в статусах `processing`/`pending`/`in_progress` с сохранённым
    /// `request_id` возвращает запись `Pending`.
    ///
    /// # Errors
    ///
    /// [`CoreError`] от клиента (валидация, HTTP, таймаут, сеть), хранилища или
    /// файловой системы; запись истории при этом либо `Failed` (definitive),
    /// либо остаётся `Pending` (транзиентный сбой после фиксации `request_id`).
    pub async fn run(&self, req: GenerationRequest) -> Result<HistoryRecord> {
        let generation = self.begin(req, Uuid::new_v4())?;
        self.finish(generation).await
    }

    /// Фиксирует начало задачи, чтобы вызывающий мог вернуть ID до платного POST.
    pub fn begin(&self, req: GenerationRequest, id: Uuid) -> Result<StartedGeneration> {
        let now = now_unix_secs()?;
        let params_json =
            serde_json::to_string(&req).map_err(|e| CoreError::Serde(e.to_string()))?;
        let record = HistoryRecord {
            provider: self.client.provider(),
            id: id.to_string(),
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
        self.storage.begin_generation(&record)?;
        (self.emit)(AppEvent::GenerationStarted {
            history_id: record.id.clone(),
        });
        Ok(StartedGeneration {
            request: req,
            record,
        })
    }

    pub async fn finish(&self, generation: StartedGeneration) -> Result<HistoryRecord> {
        let StartedGeneration {
            request: req,
            record: mut rec,
        } = generation;
        match self.produce_and_save(&mut rec, &req).await {
            Ok(record) => Ok(record),
            Err(err) => {
                // Заголовки успешного POST уже дошли, даже если чтение тела оборвалось.
                // Следующая попытка использует GET /generations/{id}, без второй оплаты.
                if self.client.can_recover(rec.kind)
                    && let CoreError::Receipt {
                        request_id,
                        cost_usd,
                        ..
                    } = &err
                    && is_safe_request_id(request_id)
                {
                    self.storage.update_history(
                        &rec.id,
                        &HistoryPatch {
                            request_id: Some(request_id.clone()),
                            cost_usd: *cost_usd,
                            error: Some("history.recovery_response".into()),
                            ..HistoryPatch::default()
                        },
                    )?;
                    return Err(err);
                }
                // Транзиентный сбой после фиксации request_id не сжигает
                // запись: оплаченная генерация жива на CI, повторит recovery.
                if self.client.can_recover(rec.kind)
                    && is_transient(&err)
                    && rec.request_id.is_some()
                {
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
    /// следующем запуске. Если текущий ключ не может получить результат,
    /// причина остаётся в истории. Ответ 410 означает, что срок хранения истёк.
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
        let pending = self.storage.pending_for(self.client.provider())?;
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
    /// `local_spend_usd` — сумма известных списаний CI из [`Storage`].
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`], если не удалось агрегировать расход; ошибки чтения удалённого кошелька
    /// сохраняются в BalanceView и не мешают показать локальный расход.
    pub async fn balance_view(&self, show_real: bool) -> Result<BalanceView> {
        let provider = self.client.provider();
        let local_spend_usd = self.storage.total_spend_usd(provider)?;
        Ok(if show_real {
            match &self.client {
                JobClient::CheaperInference(client) => client.balance_view(local_spend_usd).await,
                JobClient::OpenRouter { client, key } => {
                    client.balance_view(key, local_spend_usd).await
                }
            }
        } else {
            BalanceView::local(provider, local_spend_usd)
        })
    }

    /// Генерирует медиа и сохраняет файлы: запись `Completed` или ещё идущее видео.
    async fn produce_and_save(
        &self,
        rec: &mut HistoryRecord,
        req: &GenerationRequest,
    ) -> Result<HistoryRecord> {
        let generated = self.generate(rec, req).await?;
        self.save_generated(rec, generated)
    }

    fn save_generated(
        &self,
        rec: &mut HistoryRecord,
        generated: Generated,
    ) -> Result<HistoryRecord> {
        match generated {
            Generated::Ready {
                request_id,
                cost_usd,
                files,
            } => {
                let local_paths = self.save_files(rec.kind, &files)?;
                self.complete(rec, request_id, local_paths, cost_usd)
            }
            Generated::InProgress => {
                rec.error = Some("history.video_processing".to_owned());
                self.storage.update_history(
                    &rec.id,
                    &HistoryPatch {
                        error: rec.error.clone(),
                        ..Default::default()
                    },
                )?;
                Ok(rec.clone())
            }
        }
    }

    async fn poll_openrouter_video(
        &self,
        rec: &mut HistoryRecord,
        client: &OpenRouterKeyClient,
        key: &str,
        mut job: OpenRouterVideoJob,
    ) -> Result<Generated> {
        let cost_usd = job.cost_usd();
        self.set_request_id_and_cost(&rec.id, Some(&job.id), cost_usd)?;
        rec.request_id = Some(job.id.clone());
        rec.cost_usd = cost_usd;

        let deadline = Instant::now() + OPENROUTER_POLL_TIMEOUT;
        let mut interval = OPENROUTER_POLL_INTERVAL;
        while matches!(job.status.as_str(), "pending" | "in_progress") {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining <= interval {
                return Ok(Generated::InProgress);
            }
            tokio::time::sleep(interval).await;
            let remaining = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(remaining, client.video_status(key, &job.id)).await {
                Ok(Ok(next)) => {
                    job = next;
                    interval = OPENROUTER_POLL_INTERVAL;
                }
                Ok(Err(err)) if is_transient(&err) => {
                    interval = (interval * 2).min(Duration::from_secs(30));
                }
                Ok(Err(err)) if recovery_note(&err, rec).is_some() => {
                    return Ok(Generated::InProgress);
                }
                Ok(Err(err)) => return Err(err),
                Err(_) => return Ok(Generated::InProgress),
            }
        }
        self.openrouter_video_result(rec, client, key, job).await
    }

    async fn openrouter_video_result(
        &self,
        rec: &mut HistoryRecord,
        client: &OpenRouterKeyClient,
        key: &str,
        job: OpenRouterVideoJob,
    ) -> Result<Generated> {
        if let Some(cost) = job.cost_usd()
            && Some(cost) != rec.cost_usd
        {
            self.set_request_id_and_cost(&rec.id, None, Some(cost))?;
            rec.cost_usd = Some(cost);
        }
        match job.status.as_str() {
            "pending" | "in_progress" => return Ok(Generated::InProgress),
            "completed" => {}
            "failed" | "cancelled" | "expired" => {
                return Err(CoreError::Validation(format!(
                    "OpenRouter · {}",
                    job.error.unwrap_or(job.status).replace(key, "[скрыто]")
                )));
            }
            _ => {
                return Err(CoreError::Serde(
                    "неизвестный статус видео OpenRouter".into(),
                ));
            }
        }

        let count = job.output_count();
        let mut files = Vec::with_capacity(count);
        for index in 0..count {
            let bytes = match client.video_content(key, &job.id, index).await {
                Ok(bytes) => bytes,
                Err(CoreError::Http {
                    status: 404 | 409, ..
                }) => return Ok(Generated::InProgress),
                Err(err) => return Err(err),
            };
            files.push(MediaBytes {
                bytes,
                content_type: DEFAULT_VIDEO_CONTENT_TYPE.to_owned(),
            });
        }
        Ok(Generated::Ready {
            request_id: rec.request_id.clone(),
            cost_usd: rec.cost_usd,
            files,
        })
    }

    /// Одинаковая диагностика в ответе команды, истории и событии о сбое.
    fn explain_edit_error(&self, mut err: CoreError, model: &str) -> CoreError {
        if let CoreError::Http {
            status: 500..=599,
            message,
            ..
        } = &mut err
        {
            let system_lang = Lang::detect_system().unwrap_or(Lang::En);
            let lang = self
                .storage
                .get_settings()
                .map_or(system_lang, |s| s.lang.resolve(system_lang));
            *message = match lang {
                Lang::Ru => format!(
                    "Шлюз Cheaper Inference не смог обработать референс для модели «{model}». {message}"
                ),
                Lang::En => format!(
                    "Cheaper Inference could not process the reference with {model}. {message}"
                ),
            };
        }
        err
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
                let response = match &self.client {
                    JobClient::CheaperInference(client) if req.mode == GenMode::ImageEdit => client
                        .edit_image(req)
                        .await
                        .map_err(|err| self.explain_edit_error(err, &req.model))?,
                    JobClient::CheaperInference(client) => client.generate_image(req).await?,
                    JobClient::OpenRouter { client, key } => {
                        client.generate_image(key, req).await?
                    }
                };
                let cost_usd = response.billing.as_ref().and_then(|b| b.cost_usd());
                self.set_request_id_and_cost(&rec.id, response.request_id.as_deref(), cost_usd)?;
                rec.request_id = response.request_id.clone();
                rec.cost_usd = cost_usd;
                if response.data.is_empty() {
                    return Err(media_receipt_error(
                        rec,
                        CoreError::Serde("сервер вернул пустой результат: data".to_owned()),
                    ));
                }
                let mut files = Vec::with_capacity(response.data.len());
                for item in &response.data {
                    let bytes = image_bytes(item, &self.download)
                        .await
                        .map_err(|source| media_receipt_error(rec, source))?;
                    let content_type = raster_mime(&bytes)
                        .or(item.content_type.as_deref())
                        .unwrap_or(DEFAULT_IMAGE_CONTENT_TYPE)
                        .to_owned();
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
                let output = match &self.client {
                    JobClient::CheaperInference(client) => client.speech(req).await?,
                    JobClient::OpenRouter { client, key } => {
                        client.generate_audio(key, req).await?
                    }
                };
                self.set_request_id_and_cost(
                    &rec.id,
                    output.request_id.as_deref(),
                    output.cost_usd,
                )?;
                rec.request_id = output.request_id.clone();
                rec.cost_usd = output.cost_usd;
                Ok(Generated::Ready {
                    request_id: output.request_id,
                    cost_usd: output.cost_usd,
                    files: vec![MediaBytes {
                        bytes: output.bytes,
                        content_type: output.content_type,
                    }],
                })
            }
            MediaKind::Video => {
                let response = match &self.client {
                    JobClient::CheaperInference(client) => client.generate_video(req).await?,
                    JobClient::OpenRouter { client, key } => {
                        let job = client.start_video(key, req).await?;
                        return self.poll_openrouter_video(rec, client, key, job).await;
                    }
                };
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
                            return Err(media_receipt_error(
                                rec,
                                CoreError::Serde("CI вернул пустой результат: videos".to_owned()),
                            ));
                        }
                        let mut files = Vec::with_capacity(response.videos.len());
                        for item in &response.videos {
                            let bytes = video_bytes(item, &self.download, VIDEO_DOWNLOAD_TIMEOUT)
                                .await
                                .map_err(|source| media_receipt_error(rec, source))?;
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
                    "processing" | "pending" | "in_progress"
                        if rec.request_id.as_deref().is_some_and(is_safe_request_id) =>
                    {
                        Ok(Generated::InProgress)
                    }
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
        if !self.client.can_recover(rec.kind) {
            return self.fail_or_keep_pending(rec, "history.recovery_unavailable".into());
        }
        let Some(request_id) = rec
            .request_id
            .as_deref()
            .filter(|id| is_safe_request_id(id))
        else {
            return self.fail_or_keep_pending(
                rec,
                format!(
                    "восстановление невозможно: {} не вернул корректный идентификатор запроса",
                    rec.provider.name()
                ),
            );
        };
        match self.fetch_and_save(rec, request_id).await {
            Ok(record) => Ok(record),
            Err(err) => {
                if let Some(note) = recovery_note(&err, rec) {
                    let mut record = rec.clone();
                    if record.error.as_deref() != Some(note) {
                        record.error = Some(note.to_owned());
                        self.storage.update_history(
                            &rec.id,
                            &HistoryPatch {
                                error: record.error.clone(),
                                ..HistoryPatch::default()
                            },
                        )?;
                    }
                    Ok(record)
                } else if is_transient(&err) {
                    eprintln!("восстановление записи {} отложено: {err}", rec.id);
                    Ok(rec.clone())
                } else {
                    self.fail_or_keep_pending(rec, err.to_string())
                }
            }
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
        let client = match &self.client {
            JobClient::CheaperInference(client) => client,
            JobClient::OpenRouter { client, key } => {
                let job = client.video_status(key, request_id).await?;
                let mut record = rec.clone();
                let generated = self
                    .openrouter_video_result(&mut record, client, key, job)
                    .await?;
                return self.save_generated(&mut record, generated);
            }
        };
        let generation = client.generation(request_id).await?;
        if generation.files.is_empty() {
            return Err(CoreError::Serde(
                "CI вернул пустой результат: files".to_owned(),
            ));
        }
        let mut files = Vec::with_capacity(generation.files.len());
        for file in generation.files {
            let bytes = self
                .download_for(rec.kind, &file.url)
                .await
                .map_err(|err| match err {
                    // Срок жизни подписанной ссылки — час. Новый GET даст свежую ссылку,
                    // даже если предыдущая успела истечь в процессе загрузки нескольких файлов.
                    CoreError::Http {
                        status: 401 | 403 | 404 | 410,
                        ..
                    } => CoreError::Serde("подписанная ссылка на файл недоступна".into()),
                    other => other,
                })?;
            if bytes.is_empty() || bytes.len() as u64 != file.size_bytes {
                return Err(CoreError::Serde(
                    "размер загруженного файла не совпадает с ответом CI".into(),
                ));
            }
            files.push(MediaBytes {
                bytes,
                content_type: file.content_type,
            });
        }
        let local_paths = self.save_files(rec.kind, &files)?;
        // request_id и стоимость не меняем: оба зафиксированы в БД сразу после
        // ответа CI (recovery-эндпоинт billing не отдаёт) — восстановление
        // завершает запись с оплаченной ценой, не теряя её.
        self.complete(rec, rec.request_id.clone(), local_paths, rec.cost_usd)
    }

    /// Скачивает файл: видео — клиентом с увеличенным таймаутом, остальное — обычным.
    async fn download_for(&self, kind: MediaKind, url: &str) -> Result<Vec<u8>> {
        let timeout = (kind == MediaKind::Video).then_some(VIDEO_DOWNLOAD_TIMEOUT);
        self.download.download_with_timeout(url, timeout).await
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
            .field("provider", &self.client.provider())
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
    InProgress,
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
        CoreError::Http { status, .. } => matches!(status, 500..=599 | 408 | 425 | 429),
        CoreError::Receipt { request_id, .. } => is_safe_request_id(request_id),
        CoreError::ScopeMissing
        | CoreError::NoKey
        | CoreError::Serde(_)
        | CoreError::Validation(_) => false,
    }
}

fn media_receipt_error(record: &HistoryRecord, source: CoreError) -> CoreError {
    if is_transient(&source) {
        return source;
    }
    match record
        .request_id
        .as_ref()
        .filter(|id| is_safe_request_id(id))
    {
        Some(id) => CoreError::Receipt {
            request_id: id.clone(),
            cost_usd: record.cost_usd,
            source: Box::new(source),
        },
        None => source,
    }
}

/// Права текущего ключа и доступ к кошельку можно исправить. Один и тот же 404
/// generation_not_found также маскирует чужого владельца: исходный ключ может
/// вернуть файл в пределах объявленных семи дней. 410 окончательно означает истечение срока.
fn recovery_note(err: &CoreError, record: &HistoryRecord) -> Option<&'static str> {
    match err {
        CoreError::Serde(_) => Some("history.recovery_response"),
        CoreError::NoKey
        | CoreError::ScopeMissing
        | CoreError::Http {
            status: 401..=403, ..
        } => Some("history.recovery_access"),
        // OpenRouter ограничивает видимость задания ключом. HTTP 404 может
        // исчезнуть после возврата ключа, которым был отправлен запрос.
        CoreError::Http { status: 404, .. } if record.provider == Provider::OpenRouter => {
            Some("history.recovery_access")
        }
        CoreError::Http {
            status: 404,
            code: Some(code),
            ..
        } if code == "generation_not_found" => now_unix_secs()
            .is_ok_and(|now| now.saturating_sub(record.created_at) < 7 * 24 * 60 * 60)
            .then_some("history.recovery_not_found"),
        _ => None,
    }
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
