//! Интеграционные тесты оркестратора джобов: run, recover_pending, balance_view.
//!
//! Собирает клиент + хранилище + файлы в единый поток генерации. HTTP — через
//! wiremock, файлы — через tempdir, история — в файловой SQLite (второе
//! соединение читает то, что записал джоб). События собираются в
//! `Arc<Mutex<Vec<AppEvent>>>` и проверяются по порядку.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use km_core::{CiClient, CoreError, Jobs, Storage};
use km_shared::{
    AccountBalance, AppEvent, GenMode, GenParams, GenerationRequest, HistoryFilter, HistoryRecord,
    JobStatus, MediaKind,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Клиент, направленный на мок-сервер, с тестовым ключом.
fn client_for(server: &MockServer) -> CiClient {
    CiClient::with_base("ci_live_test", &server.uri())
}

/// Джоб с коллектором событий; возвращает сам джоб и коллектор.
fn jobs_with(
    client: CiClient,
    storage: Storage,
    folder: PathBuf,
) -> (Jobs, Arc<Mutex<Vec<AppEvent>>>) {
    let events: Arc<Mutex<Vec<AppEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let emit: Arc<dyn Fn(AppEvent) + Send + Sync> = Arc::new(move |event| {
        sink.lock().expect("мьютекс событий отравлен").push(event);
    });
    (Jobs::new(client, storage, folder, emit), events)
}

/// Пустой фильтр: вся история.
fn all() -> HistoryFilter {
    HistoryFilter::default()
}

/// Запрос текст → изображение.
fn image_request() -> GenerationRequest {
    GenerationRequest {
        kind: MediaKind::Image,
        mode: GenMode::TextToImage,
        model: "flux-2-pro".to_owned(),
        prompt: "кот в шляпе".to_owned(),
        params: GenParams::default(),
        references: Vec::new(),
        mask_data_url: None,
    }
}

/// Запрос текст → видео.
fn video_request() -> GenerationRequest {
    GenerationRequest {
        kind: MediaKind::Video,
        mode: GenMode::TextToVideo,
        model: "veo-3.1-generate-001".to_owned(),
        prompt: "кот плывёт на плоту".to_owned(),
        params: GenParams {
            duration_seconds: Some(6.0),
            ..GenParams::default()
        },
        references: Vec::new(),
        mask_data_url: None,
    }
}

/// Запрос озвучки.
fn speech_request() -> GenerationRequest {
    GenerationRequest {
        kind: MediaKind::Audio,
        mode: GenMode::Speech,
        model: "eleven-v4".to_owned(),
        prompt: "кот в шляпе".to_owned(),
        params: GenParams::default(),
        references: Vec::new(),
        mask_data_url: None,
    }
}

/// Базовая запись истории для recover/balance-сценариев.
fn history_record(id: &str, kind: MediaKind, mode: GenMode, status: JobStatus) -> HistoryRecord {
    HistoryRecord {
        provider: km_shared::Provider::CheaperInference,
        id: id.to_owned(),
        created_at: 1_700_000_000,
        updated_at: 1_700_000_000,
        kind,
        mode,
        model: "test-model".to_owned(),
        prompt: "кот".to_owned(),
        status,
        request_id: None,
        local_paths: Vec::new(),
        cost_usd: None,
        params_json: "{}".to_owned(),
        error: None,
    }
}

/// Принятые мок-сервером запросы.
async fn received(server: &MockServer) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .expect("мок-сервер должен записать запросы")
}

const SMALL_PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
const SMALL_WEBP: &str = "UklGRhwAAABXRUJQVlA4TA8AAAAvAUAAAAcQ0f/+ByKi/wEA";

/// Ответ в формате CI: поле content_type у изображений необязательно.
fn two_images_response() -> Value {
    json!({
        "created": 1_730_000_000,
        "data": [
            {"b64_json": SMALL_PNG},
            {"b64_json": SMALL_WEBP},
        ],
        "request_id": "8268670d-d239-4b05-99e9-aaf703977dab",
        "billing": {"status":"settled","currency":"USD","billed_cost_usd":"0.040000"},
    })
}

#[tokio::test]
async fn run_image_completed_saves_files_cost_and_events() {
    let server = MockServer::start().await;
    let guard = Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(two_images_response()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let folder = tmp.path().join("media");
    let db = tmp.path().join("history.db");
    let storage = Storage::open(&db).expect("БД должна открыться");
    let reader = Storage::open(&db).expect("второе соединение должно открыться");
    let (jobs, events) = jobs_with(client_for(&server), storage, folder.clone());

    let rec = jobs
        .run(image_request())
        .await
        .expect("run image должен завершиться");

    assert_eq!(rec.status, JobStatus::Completed, "статус Completed");
    assert_eq!(
        rec.request_id.as_deref(),
        Some("8268670d-d239-4b05-99e9-aaf703977dab"),
        "request_id из ответа"
    );
    assert_eq!(rec.cost_usd, Some(0.04), "cost из billing строкой");
    assert_eq!(rec.local_paths.len(), 2, "два файла сохранено");
    // Обе версии сохраняются с типом фактического содержимого даже без MIME-поля.
    let expected = [("-1.png", SMALL_PNG), ("-2.webp", SMALL_WEBP)];
    for (path, (suffix, b64)) in rec.local_paths.iter().zip(expected) {
        let path = PathBuf::from(path);
        assert_eq!(
            path.parent(),
            Some(folder.as_path()),
            "файл в папке сохранения"
        );
        let name = path.file_name().expect("имя файла").to_string_lossy();
        assert!(name.ends_with(suffix), "суффикс варианта: {name}");
        assert_eq!(std::fs::read(&path).unwrap(), STANDARD.decode(b64).unwrap());
    }
    // Время в секундах (не миллисекунды): created_at/updated_at — unix seconds.
    assert!(rec.created_at > 1_700_000_000, "created_at в секундах");
    assert!(
        rec.updated_at >= rec.created_at,
        "updated_at не раньше created_at"
    );

    // История в БД действительно Completed с теми же путями и стоимостью.
    let history = reader.history(&all()).expect("history должна прочитаться");
    assert_eq!(history.len(), 1, "одна запись");
    assert_eq!(history[0].status, JobStatus::Completed);
    assert_eq!(history[0].local_paths, rec.local_paths);
    assert_eq!(history[0].cost_usd, Some(0.04));

    // События строго Started -> Done с одним и тем же history_id.
    let events = events.lock().expect("lock").clone();
    assert_eq!(
        events,
        vec![
            AppEvent::GenerationStarted {
                history_id: rec.id.clone()
            },
            AppEvent::GenerationDone {
                history_id: rec.id.clone()
            },
        ],
        "порядок событий Started -> Done"
    );

    drop(guard);
}

#[tokio::test]
async fn run_image_edit_uses_edits_endpoint() {
    let server = MockServer::start().await;
    let guard = Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(two_images_response()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let storage = Storage::open(&tmp.path().join("h.db")).expect("БД");
    let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let mut req = image_request();
    req.mode = GenMode::ImageEdit;
    req.references = vec![km_shared::Reference {
        data_url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=".to_owned(),
    }];
    let rec = jobs.run(req).await.expect("edit должен завершиться");
    assert_eq!(rec.status, JobStatus::Completed);
    assert_eq!(rec.mode, GenMode::ImageEdit);

    drop(guard);
}

#[tokio::test]
async fn run_video_completed_downloads_and_saves() {
    let server = MockServer::start().await;
    let video_url = format!("{}/signed/v.mp4", server.uri());
    let guard = Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "vid_123",
            "status": "completed",
            "videos": [{"url": video_url, "content_type": "video/mp4"}],
            "request_id": "req_video",
            "billing": {"billed_cost_usd": "0.600000"},
        })))
        .expect(1)
        .mount_as_scoped(&server)
        .await;
    let payload = b"binary video bytes";
    Mock::given(method("GET"))
        .and(path("/signed/v.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let storage = Storage::open(&tmp.path().join("h.db")).expect("БД");
    let (jobs, events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let rec = jobs
        .run(video_request())
        .await
        .expect("run video должен завершиться");
    assert_eq!(rec.status, JobStatus::Completed);
    assert_eq!(rec.request_id.as_deref(), Some("req_video"));
    assert_eq!(rec.cost_usd, Some(0.6));
    assert_eq!(rec.local_paths.len(), 1);
    let path = PathBuf::from(&rec.local_paths[0]);
    assert!(
        path.to_string_lossy().ends_with(".mp4"),
        "видео сохраняется как mp4"
    );
    assert_eq!(std::fs::read(&path).expect("файл"), payload);

    let events = events.lock().expect("lock").clone();
    assert_eq!(events.len(), 2, "Started -> Done");
    assert!(matches!(events[0], AppEvent::GenerationStarted { .. }));
    assert!(matches!(events[1], AppEvent::GenerationDone { .. }));

    drop(guard);
}

#[tokio::test]
async fn run_video_timeout_marks_failed_without_request_id() {
    let server = MockServer::start().await;
    // Мок задерживает ответ на 500 мс — дольше клиентского таймаута 100 мс.
    Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "v", "status": "completed", "videos": []}))
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let client = client_for(&server).with_timeout(Duration::from_millis(100));
    let (jobs, events) = jobs_with(client, storage, tmp.path().join("media"));

    let err = jobs
        .run(video_request())
        .await
        .expect_err("таймаут должен вернуть ошибку");
    assert!(
        matches!(err, CoreError::Timeout),
        "ожидался Timeout, получено: {err:?}"
    );

    // Запись Failed с ошибкой «таймаут» и без request_id (тело не получено).
    let history = reader.history(&all()).expect("history");
    assert_eq!(history.len(), 1);
    let rec = &history[0];
    assert_eq!(rec.status, JobStatus::Failed);
    assert!(
        rec.error.as_deref().unwrap_or_default().contains("таймаут"),
        "ошибка про таймаут"
    );
    assert_eq!(rec.request_id, None, "request_id не получен при таймауте");
    assert!(rec.local_paths.is_empty(), "файлы не сохранены");

    let events = events.lock().expect("lock").clone();
    assert_eq!(events.len(), 2, "Started -> Failed");
    assert!(matches!(events[0], AppEvent::GenerationStarted { .. }));
    assert!(matches!(events[1], AppEvent::GenerationFailed { .. }));

    // Recover нечего восстанавливать: запись Failed, а не Pending.
    let recovered = jobs.recover_pending().await.expect("recover");
    assert!(recovered.is_empty(), "Failed-запись не восстанавливается");
}

#[tokio::test]
async fn run_audio_saves_binary_with_cost() {
    let server = MockServer::start().await;
    let mp3: [u8; 4] = [0x49, 0x44, 0x33, 0x04];
    let guard = Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(mp3)
                .insert_header("content-type", "audio/mpeg")
                .insert_header("x-ci-request-id", "speech_paid")
                .insert_header("x-ci-billed-cost-usd", "0.020000"),
        )
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let storage = Storage::open(&tmp.path().join("h.db")).expect("БД");
    let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let rec = jobs
        .run(speech_request())
        .await
        .expect("run audio должен завершиться");
    assert_eq!(rec.status, JobStatus::Completed);
    assert_eq!(rec.cost_usd, Some(0.02), "cost из заголовка");
    assert_eq!(rec.request_id.as_deref(), Some("speech_paid"));
    assert_eq!(rec.local_paths.len(), 1);
    let path = PathBuf::from(&rec.local_paths[0]);
    assert!(
        path.to_string_lossy().ends_with(".mp3"),
        "аудио mpeg -> mp3"
    );
    assert_eq!(std::fs::read(&path).expect("файл"), mp3);

    drop(guard);
}

#[tokio::test]
async fn paid_audio_recovers_after_output_folder_failure_without_resubmitting() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"ID3original".as_slice())
                .insert_header("content-type", "audio/mpeg")
                .insert_header("x-ci-request-id", "paid_audio")
                .insert_header("x-ci-billed-cost-usd", "0.02"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET")).and(path("/v1/generations/paid_audio"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id":"paid_audio", "endpoint":"/v1/audio/speech",
            "created_at":"2026-11-25T00:00:00Z", "expires_at":"2026-12-02T00:00:00Z",
            "files":[{"index":0,"type":"audio","content_type":"audio/mpeg","size_bytes":12,
                "url":format!("{}/signed/audio.mp3",server.uri()),"url_expires_at":"2026-11-25T01:00:00Z"}]
        }))).expect(1).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/signed/audio.mp3"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ID3recovered".as_slice()))
        .expect(1)
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("output");
    std::fs::write(&folder, b"file instead of folder").unwrap();
    let db = dir.path().join("history.db");
    let (jobs, _) = jobs_with(
        client_for(&server),
        Storage::open(&db).unwrap(),
        folder.clone(),
    );
    assert!(jobs.run(speech_request()).await.is_err());
    let db = Storage::open(&db).unwrap();
    let pending = db.pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].request_id.as_deref(), Some("paid_audio"));
    assert_eq!(pending[0].cost_usd, Some(0.02));
    std::fs::remove_file(folder).unwrap();
    let recovered = jobs.recover_pending().await.unwrap();
    assert_eq!(recovered[0].status, JobStatus::Completed);
    assert_eq!(recovered[0].cost_usd, Some(0.02));
    assert_eq!(
        std::fs::read(&recovered[0].local_paths[0]).unwrap(),
        b"ID3recovered"
    );
}

#[tokio::test]
async fn run_validation_error_marks_failed_before_http() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let (jobs, events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let mut req = image_request();
    req.prompt = "   ".to_owned();
    let err = jobs
        .run(req)
        .await
        .expect_err("пустой промпт должен вернуть ошибку");
    assert!(
        matches!(err, CoreError::Validation(_)),
        "ожидался Validation: {err:?}"
    );

    let history = reader.history(&all()).expect("history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, JobStatus::Failed);
    assert!(received(&server).await.is_empty(), "валидация до HTTP");

    let events = events.lock().expect("lock").clone();
    assert!(matches!(events[0], AppEvent::GenerationStarted { .. }));
    assert!(matches!(events[1], AppEvent::GenerationFailed { .. }));
}

#[tokio::test]
async fn run_normalizes_content_type_with_parameters() {
    let server = MockServer::start().await;
    // CI присылает content-type с параметрами и в верхнем регистре.
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"b64_json": "aGk=", "content_type": "IMAGE/JPEG; charset=binary"}],
            "request_id": "img_ct",
        })))
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let storage = Storage::open(&tmp.path().join("h.db")).expect("БД");
    let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let rec = jobs.run(image_request()).await.expect("run");
    assert_eq!(rec.local_paths.len(), 1);
    let path = PathBuf::from(&rec.local_paths[0]);
    assert!(
        path.to_string_lossy().ends_with(".jpg"),
        "CT с параметрами нормализован до image/jpeg -> jpg: {}",
        path.display()
    );
}

#[tokio::test]
async fn recover_pending_after_crash_completes_with_id_and_fails_without() {
    let server = MockServer::start().await;
    let file_url = format!("{}/signed/v.mp4", server.uri());
    let guard = Mock::given(method("GET"))
        .and(path("/v1/generations/req_crash"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_crash",
            "endpoint": "/v1/videos/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "video", "content_type": "video/mp4",
                "size_bytes": 18, "url": file_url,
            }],
        })))
        .expect(1)
        .mount_as_scoped(&server)
        .await;
    let payload = b"binary video bytes";
    Mock::given(method("GET"))
        .and(path("/signed/v.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    // «Краш»: ответ был получен (request_id есть), но приложение упало до сохранения.
    let mut with_id = history_record(
        "with-id",
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Pending,
    );
    with_id.request_id = Some("req_crash".to_owned());
    with_id.created_at = 2000;
    let mut without_id = history_record(
        "without-id",
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Pending,
    );
    without_id.created_at = 1000;
    storage.insert_history(&with_id).expect("insert with_id");
    storage
        .insert_history(&without_id)
        .expect("insert without_id");
    let (jobs, events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 2, "обе Pending-записи обработаны");

    let done = recovered
        .iter()
        .find(|r| r.id == "with-id")
        .expect("with-id");
    assert_eq!(
        done.status,
        JobStatus::Completed,
        "с request_id -> Completed"
    );
    assert_eq!(done.local_paths.len(), 1, "файл сохранён");
    assert_eq!(std::fs::read(&done.local_paths[0]).expect("файл"), payload);

    let failed = recovered
        .iter()
        .find(|r| r.id == "without-id")
        .expect("without-id");
    assert_eq!(failed.status, JobStatus::Failed, "без request_id -> Failed");
    assert!(failed.error.is_some(), "текст ошибки записан");

    // БД отражает оба исхода.
    let history = reader.history(&all()).expect("history");
    assert_eq!(history.len(), 2);
    assert!(
        history.iter().all(|r| r.status != JobStatus::Pending),
        "Pending не осталось"
    );

    // События: Done для восстановленной, Failed для невосстановимой.
    let events = events.lock().expect("lock").clone();
    assert_eq!(events.len(), 2);
    assert!(
        events.iter().any(
            |e| matches!(e, AppEvent::GenerationDone { history_id } if history_id == "with-id")
        )
    );
    assert!(events.iter().any(
        |e| matches!(e, AppEvent::GenerationFailed { history_id, .. } if history_id == "without-id")
    ));

    drop(guard);
}

#[tokio::test]
async fn recover_pending_invalid_request_id_fails_without_http() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let storage = Storage::open(&tmp.path().join("h.db")).expect("БД");
    // Служебные символы в request_id — defense-in-depth: не подставляем в URL.
    let mut rec = history_record(
        "bad",
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Pending,
    );
    rec.request_id = Some("../../etc/passwd".to_owned());
    storage.insert_history(&rec).expect("insert");
    let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, JobStatus::Failed);
    assert!(
        received(&server).await.is_empty(),
        "некорректный id не уходит в HTTP"
    );
}

#[tokio::test]
async fn recover_pending_not_found_after_retention_marks_failed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/generations/req_gone"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {"code": "generation_not_found", "message": "generation not found"},
        })))
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let storage = Storage::open(&tmp.path().join("h.db")).expect("БД");
    let mut rec = history_record(
        "gone",
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Pending,
    );
    rec.request_id = Some("req_gone".to_owned());
    storage.insert_history(&rec).expect("insert");
    let (jobs, events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].status,
        JobStatus::Failed,
        "истёкшая генерация -> Failed"
    );
    assert!(
        recovered[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("404")
    );
    let events = events.lock().expect("lock").clone();
    assert!(matches!(events[0], AppEvent::GenerationFailed { .. }));
}

/// Запись Completed с заданной стоимостью — для агрегации расхода.
fn completed_with_cost(id: &str, cost: f64) -> HistoryRecord {
    let mut rec = history_record(
        id,
        MediaKind::Image,
        GenMode::TextToImage,
        JobStatus::Completed,
    );
    rec.cost_usd = Some(cost);
    rec
}

#[tokio::test]
async fn balance_view_show_real_200_returns_real_and_local() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/account/balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currency": "USD", "balance_usd": 10.5, "available_usd": 9.0, "reserved_usd": 1.5,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let storage = Storage::open(&tmp.path().join("h.db")).expect("БД");
    storage
        .insert_history(&completed_with_cost("c1", 0.04))
        .expect("insert");
    storage
        .insert_history(&completed_with_cost("c2", 0.06))
        .expect("insert");
    let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let view = jobs.balance_view(true).await.expect("balance_view");
    assert_eq!(
        view.real,
        Some(AccountBalance {
            currency: "USD".to_owned(),
            balance_usd: 10.5,
            available_usd: 9.0,
            reserved_usd: 1.5,
        }),
        "реальный баланс из CI"
    );
    assert!(
        (view.local_spend_usd - 0.10).abs() < 1e-9,
        "локальный расход из БД"
    );
}

#[tokio::test]
async fn balance_view_403_falls_back_to_local() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/account/balance"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": {"code": "forbidden", "message": "scope account:read required"},
        })))
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let storage = Storage::open(&tmp.path().join("h.db")).expect("БД");
    storage
        .insert_history(&completed_with_cost("c1", 0.04))
        .expect("insert");
    let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let view = jobs.balance_view(true).await.expect("balance_view");
    assert_eq!(view.real, None, "403 -> реальный баланс скрыт");
    assert!(
        (view.local_spend_usd - 0.04).abs() < 1e-9,
        "фолбэк на локальный расход"
    );
}

#[tokio::test]
async fn balance_view_show_real_false_only_local() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let storage = Storage::open(&tmp.path().join("h.db")).expect("БД");
    storage
        .insert_history(&completed_with_cost("c1", 0.04))
        .expect("insert");
    let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let view = jobs.balance_view(false).await.expect("balance_view");
    assert_eq!(view.real, None, "show_real=false -> только локальный");
    assert!((view.local_spend_usd - 0.04).abs() < 1e-9);
    assert!(
        received(&server).await.is_empty(),
        "без show_real запрос баланса не идёт"
    );
}

#[tokio::test]
async fn balance_parses_200() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/account/balance"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currency": "USD", "balance_usd": 10.5, "available_usd": 9.0, "reserved_usd": 1.5,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let balance = client_for(&server)
        .balance()
        .await
        .expect("200 должен разобрать баланс");
    assert_eq!(balance.balance_usd, 10.5);
    assert_eq!(balance.currency, "USD");
}

#[tokio::test]
async fn balance_403_maps_to_scope_missing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/account/balance"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": {"code": "forbidden", "message": "scope account:read required"},
        })))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .balance()
        .await
        .expect_err("403 должен вернуть ошибку");
    assert!(
        matches!(err, CoreError::ScopeMissing),
        "ожидался ScopeMissing: {err:?}"
    );
}

#[tokio::test]
async fn completed_history_not_recovered_and_delete_is_local_only() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let folder = tmp.path().join("media");
    std::fs::create_dir_all(&folder).expect("папка");
    let file = folder.join("image-123.png");
    std::fs::write(&file, b"hi").expect("локальный файл");

    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    // Completed с локальным путём и request_id: ссылка могла протухнуть, но файл локально.
    let mut rec = history_record(
        "done",
        MediaKind::Image,
        GenMode::TextToImage,
        JobStatus::Completed,
    );
    rec.request_id = Some("req_old".to_owned());
    rec.local_paths = vec![file.to_string_lossy().into_owned()];
    storage.insert_history(&rec).expect("insert");
    let (jobs, _events) = jobs_with(client_for(&server), storage, folder);

    // Recover не трогает Completed и не скачивает повторно.
    let recovered = jobs.recover_pending().await.expect("recover");
    assert!(recovered.is_empty(), "Completed не восстанавливается");
    assert!(
        received(&server).await.is_empty(),
        "повторного скачивания нет"
    );

    // delete_history убирает только запись: ни remote, ни локальный файл не тронуты.
    reader.delete_history("done").expect("delete");
    assert!(
        reader.history(&all()).expect("history").is_empty(),
        "запись удалена"
    );
    assert!(file.exists(), "локальный файл остался");
    assert!(
        received(&server).await.is_empty(),
        "delete не обращается к remote"
    );
}

#[tokio::test]
async fn recover_pending_download_network_error_keeps_pending_for_retry() {
    let server = MockServer::start().await;
    // Generation OK, но ссылка на файл ведёт на закрытый порт: соединение
    // отклонено -> CoreError::Network. Оплаченная генерация жива на CI,
    // запись обязана остаться Pending и восстановиться при следующем запуске.
    let guard = Mock::given(method("GET"))
        .and(path("/v1/generations/req_net"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_net",
            "endpoint": "/v1/videos/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "video", "content_type": "video/mp4",
                "size_bytes": 2, "url": "http://127.0.0.1:1/signed/v.mp4",
            }],
        })))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let mut rec = history_record(
        "net",
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Pending,
    );
    rec.request_id = Some("req_net".to_owned());
    storage.insert_history(&rec).expect("insert");
    let (jobs, events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 1, "запись обработана");
    assert_eq!(
        recovered[0].status,
        JobStatus::Pending,
        "Network при скачивании не сжигает оплаченную генерацию"
    );
    assert_eq!(
        recovered[0].request_id.as_deref(),
        Some("req_net"),
        "request_id сохранён для следующей попытки"
    );
    let history = reader.history(&all()).expect("history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, JobStatus::Pending, "в БД запись Pending");
    assert!(
        events.lock().expect("lock").is_empty(),
        "ни Done, ни Failed при транзиентной ошибке"
    );
    drop(guard);

    // Следующий запуск: ссылка живая — та же запись восстанавливается.
    let file_url = format!("{}/signed/ok.mp4", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1/generations/req_net"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_net",
            "endpoint": "/v1/videos/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "video", "content_type": "video/mp4",
                "size_bytes": 2, "url": file_url,
            }],
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/signed/ok.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hi"))
        .mount(&server)
        .await;

    let recovered = jobs.recover_pending().await.expect("повторный recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].status,
        JobStatus::Completed,
        "повторная попытка завершает запись"
    );
    assert_eq!(
        std::fs::read(&recovered[0].local_paths[0]).expect("файл"),
        b"hi"
    );
}

#[tokio::test]
async fn recover_pending_download_timeout_keeps_pending() {
    let server = MockServer::start().await;
    let file_url = format!("{}/signed/slow.png", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1/generations/req_slow"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_slow",
            "endpoint": "/v1/images/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "image", "content_type": "image/png",
                "size_bytes": 2, "url": file_url,
            }],
        })))
        .mount(&server)
        .await;
    // Скачивание медленнее клиентского таймаута 100 мс -> CoreError::Timeout.
    let guard = Mock::given(method("GET"))
        .and(path("/signed/slow.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"hi")
                .set_delay(Duration::from_millis(500)),
        )
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let mut rec = history_record(
        "slow",
        MediaKind::Image,
        GenMode::TextToImage,
        JobStatus::Pending,
    );
    rec.request_id = Some("req_slow".to_owned());
    storage.insert_history(&rec).expect("insert");
    let client = client_for(&server).with_timeout(Duration::from_millis(100));
    let (jobs, events) = jobs_with(client, storage, tmp.path().join("media"));

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].status,
        JobStatus::Pending,
        "Timeout при скачивании не сжигает оплаченную генерацию"
    );
    assert_eq!(recovered[0].request_id.as_deref(), Some("req_slow"));
    let history = reader.history(&all()).expect("history");
    assert_eq!(history[0].status, JobStatus::Pending, "в БД запись Pending");
    assert!(events.lock().expect("lock").is_empty(), "событий нет");

    drop(guard);
}

#[tokio::test]
async fn recover_pending_io_error_keeps_pending() {
    let server = MockServer::start().await;
    let file_url = format!("{}/signed/v.mp4", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1/generations/req_io"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_io",
            "endpoint": "/v1/videos/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "video", "content_type": "video/mp4",
                "size_bytes": 2, "url": file_url,
            }],
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/signed/v.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hi"))
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let mut rec = history_record(
        "io",
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Pending,
    );
    rec.request_id = Some("req_io".to_owned());
    storage.insert_history(&rec).expect("insert");
    // Папка сохранения занята файлом: ensure_folder -> CoreError::Io.
    let folder = tmp.path().join("media");
    std::fs::write(&folder, b"not a folder").expect("файл-заглушка");
    let (jobs, events) = jobs_with(client_for(&server), storage, folder);

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].status,
        JobStatus::Pending,
        "недоступная папка не сжигает оплаченную генерацию"
    );
    assert_eq!(recovered[0].request_id.as_deref(), Some("req_io"));
    let history = reader.history(&all()).expect("history");
    assert_eq!(history[0].status, JobStatus::Pending, "в БД запись Pending");
    assert!(events.lock().expect("lock").is_empty(), "событий нет");
}

#[tokio::test]
async fn recover_pending_generation_410_marks_failed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/generations/req_expired"))
        .respond_with(ResponseTemplate::new(410).set_body_json(json!({
            "error": {"code": "gone", "message": "generation files expired"},
        })))
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let storage = Storage::open(&tmp.path().join("h.db")).expect("БД");
    let mut rec = history_record(
        "expired",
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Pending,
    );
    rec.request_id = Some("req_expired".to_owned());
    storage.insert_history(&rec).expect("insert");
    let (jobs, events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].status,
        JobStatus::Failed,
        "410 — definitive-ответ CI: файлы истекли"
    );
    assert!(
        recovered[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("410")
    );
    let events = events.lock().expect("lock").clone();
    assert!(matches!(events[0], AppEvent::GenerationFailed { .. }));
}

#[tokio::test]
async fn malformed_image_data_recovers_with_body_receipt_and_no_second_post() {
    const ID: &str = "135de9b7-f938-4b4a-b51d-8ff19d2bedb8";
    let server = MockServer::start().await;
    let png = STANDARD.decode(SMALL_PNG).unwrap();
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data":"wrong shape", "request_id":ID,
            "billing":{"status":"settled","currency":"USD","billed_cost_usd":"0.031250"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET")).and(path(format!("/v1/generations/{ID}")))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object":"generation","request_id":ID,"endpoint":"/v1/images/generations",
            "created_at":"2026-11-25T12:00:00Z","expires_at":"2026-12-02T12:00:00Z",
            "files":[{"index":0,"type":"image","content_type":"image/png","size_bytes":png.len(),
                "url":format!("{}/signed/image.png",server.uri()),"url_expires_at":"2026-11-26T13:00:00Z"}]
        }))).expect(1).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/signed/image.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png.clone()))
        .expect(1)
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("history.db");
    let reader = Storage::open(&db).unwrap();
    let (jobs, _) = jobs_with(
        client_for(&server),
        Storage::open(&db).unwrap(),
        dir.path().join("media"),
    );
    assert!(
        matches!(jobs.run(image_request()).await, Err(CoreError::Receipt { request_id, cost_usd:Some(0.03125), .. }) if request_id == ID)
    );
    let row = reader.history(&all()).unwrap().remove(0);
    assert_eq!(row.status, JobStatus::Pending);
    assert_eq!(row.request_id.as_deref(), Some(ID));
    assert_eq!(row.cost_usd, Some(0.03125));
    let recovered = jobs.recover_pending().await.unwrap().remove(0);
    assert_eq!(recovered.status, JobStatus::Completed);
    assert_eq!(std::fs::read(&recovered.local_paths[0]).unwrap(), png);
}

#[tokio::test]
async fn run_image_empty_data_keeps_receipt_for_recovery() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [],
            "request_id": "img_empty",
            "billing": {"billed_cost_usd": "0.040000"},
        })))
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let (jobs, events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let err = jobs
        .run(image_request())
        .await
        .expect_err("пустой data при 200 — ошибка, а не Completed");
    assert!(matches!(err, CoreError::Receipt { .. }));
    let history = reader.history(&all()).expect("history");
    assert_eq!(history[0].status, JobStatus::Pending);
    assert_eq!(history[0].request_id.as_deref(), Some("img_empty"));
    assert_eq!(history[0].cost_usd, Some(0.04));
    assert!(history[0].local_paths.is_empty());
    assert!(
        !events
            .lock()
            .expect("lock")
            .iter()
            .any(|ev| matches!(ev, AppEvent::GenerationFailed { .. }))
    );
}

#[tokio::test]
async fn run_video_empty_videos_with_receipt_keeps_pending() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "vid_empty",
            "status": "completed",
            "videos": [],
            "request_id": "req_empty",
            "billing": {"billed_cost_usd": "0.600000"},
        })))
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let err = jobs
        .run(video_request())
        .await
        .expect_err("пустой videos при 200 — ошибка");
    assert!(
        matches!(err, CoreError::Receipt { .. }),
        "ожидалась квитанция: {err:?}"
    );
    let history = reader.history(&all()).expect("history");
    assert_eq!(history[0].status, JobStatus::Pending);
    assert_eq!(history[0].request_id.as_deref(), Some("req_empty"));
    assert_eq!(
        history[0].error.as_deref(),
        Some("history.recovery_response")
    );
    assert!(history[0].local_paths.is_empty());
}

#[tokio::test]
async fn run_video_non_completed_status_marks_failed_without_download() {
    // Definitive-статусы (не «ещё идёт»): failed/cancelled сжигаются в Failed;
    // processing/pending/in_progress покрыты video_processing_keeps_pending.
    for status in ["failed", "cancelled"] {
        let server = MockServer::start().await;
        let video_url = format!("{}/signed/v.mp4", server.uri());
        Mock::given(method("POST"))
            .and(path("/v1/videos/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "vid_bad",
                "status": status,
                "videos": [{"url": video_url, "content_type": "video/mp4"}],
                "request_id": "req_bad",
            })))
            .mount(&server)
            .await;

        let tmp = TempDir::new().expect("tempdir");
        let db = tmp.path().join("h.db");
        let storage = Storage::open(&db).expect("БД");
        let reader = Storage::open(&db).expect("reader");
        let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

        let err = jobs
            .run(video_request())
            .await
            .expect_err("non-completed статус — ошибка");
        assert!(
            matches!(err, CoreError::Serde(_)),
            "ожидался Serde: {err:?}"
        );
        let history = reader.history(&all()).expect("history");
        assert_eq!(history[0].status, JobStatus::Failed, "статус {status}");
        assert!(
            history[0]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains(status),
            "ошибка содержит статус CI: {:?}",
            history[0].error
        );
        assert!(history[0].local_paths.is_empty());
        // Скачивание не начинается: статус проверен до файлов.
        let requests = received(&server).await;
        assert!(
            requests.iter().all(|r| r.url.path() != "/signed/v.mp4"),
            "при статусе {status} файл не скачивается"
        );
    }
}

#[tokio::test]
async fn run_video_status_succeeded_completes() {
    let server = MockServer::start().await;
    let video_url = format!("{}/signed/v.mp4", server.uri());
    Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "vid_ok",
            "status": "succeeded",
            "videos": [{"url": video_url, "content_type": "video/mp4"}],
            "request_id": "req_ok",
            "billing": {"billed_cost_usd": "0.600000"},
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/signed/v.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"video"))
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let storage = Storage::open(&tmp.path().join("h.db")).expect("БД");
    let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let rec = jobs
        .run(video_request())
        .await
        .expect("succeeded — успешный статус наравне с completed");
    assert_eq!(rec.status, JobStatus::Completed);
    assert_eq!(rec.local_paths.len(), 1);
    assert_eq!(std::fs::read(&rec.local_paths[0]).expect("файл"), b"video");
}

#[tokio::test]
async fn run_audio_empty_body_marks_failed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(Vec::<u8>::new())
                .insert_header("content-type", "audio/mpeg"),
        )
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let err = jobs
        .run(speech_request())
        .await
        .expect_err("пустое аудио при 200 — ошибка");
    assert!(
        matches!(err, CoreError::Serde(_)),
        "ожидался Serde: {err:?}"
    );
    let history = reader.history(&all()).expect("history");
    assert_eq!(history[0].status, JobStatus::Failed);
    assert!(
        history[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("пустой")
    );
    assert!(history[0].local_paths.is_empty());
}

#[tokio::test]
async fn recover_pending_empty_files_preserves_the_receipt() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/generations/req_empty"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_empty",
            "endpoint": "/v1/videos/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [],
        })))
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let mut rec = history_record(
        "empty",
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Pending,
    );
    rec.request_id = Some("req_empty".to_owned());
    storage.insert_history(&rec).expect("insert");
    let (jobs, events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, JobStatus::Pending);
    assert_eq!(
        recovered[0].error.as_deref(),
        Some("history.recovery_response")
    );
    let history = reader.history(&all()).expect("history");
    assert_eq!(history[0].status, JobStatus::Pending);
    assert_eq!(history[0].error, recovered[0].error);
    assert!(events.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn recover_pending_db_error_isolates_records_and_keeps_them_recoverable() {
    let server = MockServer::start().await;
    // rec_a: definitive 404 -> fail() упрётся в заблокированную БД.
    Mock::given(method("GET"))
        .and(path("/v1/generations/req_a"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {"code": "not_found", "message": "generation not found"},
        })))
        .mount(&server)
        .await;
    // rec_b: generation OK -> complete() упрётся в заблокированную БД.
    let file_url = format!("{}/signed/b.mp4", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1/generations/req_b"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_b",
            "endpoint": "/v1/videos/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "video", "content_type": "video/mp4",
                "size_bytes": 2, "url": file_url,
            }],
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/signed/b.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hi"))
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    for (id, request_id) in [("a", "req_a"), ("b", "req_b")] {
        let mut rec = history_record(
            id,
            MediaKind::Video,
            GenMode::TextToVideo,
            JobStatus::Pending,
        );
        rec.request_id = Some(request_id.to_owned());
        storage.insert_history(&rec).expect("insert");
    }
    let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    // Третье соединение держит write-лок (BEGIN IMMEDIATE): чтение pending
    // проходит, а обновления записей получают SQLITE_BUSY (busy timeout 5 с).
    let locker = rusqlite::Connection::open(&db).expect("соединение-лок");
    locker
        .execute_batch("BEGIN IMMEDIATE;")
        .expect("write-лок должен захватиться");

    let recovered = jobs
        .recover_pending()
        .await
        .expect("сбой БД одной записи не прерывает восстановление");
    assert_eq!(
        recovered.len(),
        2,
        "записи с ошибкой БД обработаны и возвращаются как Pending"
    );
    assert!(
        recovered.iter().all(|r| r.status == JobStatus::Pending),
        "Db-ошибка не сжигает записи: обе остаются Pending"
    );
    // Изоляция: обработаны ОБЕ записи, а не только первая.
    let requests = received(&server).await;
    assert!(
        requests
            .iter()
            .any(|r| r.url.path() == "/v1/generations/req_a"),
        "первая запись обработана"
    );
    assert!(
        requests
            .iter()
            .any(|r| r.url.path() == "/v1/generations/req_b"),
        "вторая запись обработана несмотря на ошибку БД первой"
    );
    // Записи не потеряны и не сожжены: остались Pending до следующего запуска.
    let history = reader.history(&all()).expect("history");
    assert_eq!(history.len(), 2);
    assert!(
        history.iter().all(|r| r.status == JobStatus::Pending),
        "записи остаются Pending при сбое БД"
    );

    locker.execute_batch("ROLLBACK;").expect("лок снимается");
    drop(locker);

    // Следующий запуск без лока: обе записи доходят до верного исхода.
    let recovered = jobs.recover_pending().await.expect("повторный recover");
    assert_eq!(recovered.len(), 2);
    let a = recovered.iter().find(|r| r.id == "a").expect("запись a");
    assert_eq!(a.status, JobStatus::Failed, "404 -> Failed");
    let b = recovered.iter().find(|r| r.id == "b").expect("запись b");
    assert_eq!(b.status, JobStatus::Completed, "generation OK -> Completed");
    assert_eq!(std::fs::read(&b.local_paths[0]).expect("файл"), b"hi");
}

#[tokio::test]
async fn recovery_http_5xx_keeps_pending() {
    let server = MockServer::start().await;
    // Generation OK, но CDN отдаёт 503 — транзиентный сбой, не definitive:
    // оплаченная генерация жива на CI, запись обязана остаться Pending.
    let generation_guard = Mock::given(method("GET"))
        .and(path("/v1/generations/req_5xx"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_5xx",
            "endpoint": "/v1/videos/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "video", "content_type": "video/mp4",
                "size_bytes": 2, "url": format!("{}/signed/503.mp4", server.uri()),
            }],
        })))
        .expect(2)
        .mount_as_scoped(&server)
        .await;
    let fail_guard = Mock::given(method("GET"))
        .and(path("/signed/503.mp4"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let mut rec = history_record(
        "5xx",
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Pending,
    );
    rec.request_id = Some("req_5xx".to_owned());
    storage.insert_history(&rec).expect("insert");
    let (jobs, events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 1, "запись обработана");
    assert_eq!(
        recovered[0].status,
        JobStatus::Pending,
        "HTTP 503 при скачивании не сжигает оплаченную генерацию"
    );
    assert_eq!(
        recovered[0].request_id.as_deref(),
        Some("req_5xx"),
        "request_id сохранён для следующей попытки"
    );
    let history = reader.history(&all()).expect("history");
    assert_eq!(history[0].status, JobStatus::Pending, "в БД запись Pending");
    assert!(
        events.lock().expect("lock").is_empty(),
        "ни Done, ни Failed при транзиентной ошибке"
    );
    drop(fail_guard);

    // Следующий запуск: CDN ожил — та же запись восстанавливается.
    Mock::given(method("GET"))
        .and(path("/signed/503.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hi"))
        .mount(&server)
        .await;
    let recovered = jobs.recover_pending().await.expect("повторный recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].status,
        JobStatus::Completed,
        "повторная попытка завершает запись"
    );
    assert_eq!(
        std::fs::read(&recovered[0].local_paths[0]).expect("файл"),
        b"hi"
    );
    drop(generation_guard);
}

#[tokio::test]
async fn recovery_http_429_keeps_pending() {
    let server = MockServer::start().await;
    // 429 (rate limit) — транзиентный ответ: повтор имеет смысл.
    let file_url = format!("{}/signed/429.mp4", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1/generations/req_429"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_429",
            "endpoint": "/v1/videos/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "video", "content_type": "video/mp4",
                "size_bytes": 2, "url": file_url,
            }],
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/signed/429.mp4"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let mut rec = history_record(
        "429",
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Pending,
    );
    rec.request_id = Some("req_429".to_owned());
    storage.insert_history(&rec).expect("insert");
    let (jobs, events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].status,
        JobStatus::Pending,
        "HTTP 429 не сжигает оплаченную генерацию"
    );
    assert_eq!(recovered[0].request_id.as_deref(), Some("req_429"));
    let history = reader.history(&all()).expect("history");
    assert_eq!(history[0].status, JobStatus::Pending, "в БД запись Pending");
    assert!(events.lock().expect("lock").is_empty(), "событий нет");
}

#[tokio::test]
async fn recovery_db_error_keeps_pending() {
    let server = MockServer::start().await;
    // Generation и скачивание OK, но complete() упрётся в заблокированную БД:
    // Db-ошибка записи не должна сжигать оплаченную генерацию.
    let file_url = format!("{}/signed/db.mp4", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1/generations/req_db"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_db",
            "endpoint": "/v1/videos/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "video", "content_type": "video/mp4",
                "size_bytes": 2, "url": file_url,
            }],
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/signed/db.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hi"))
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let mut rec = history_record(
        "db",
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Pending,
    );
    rec.request_id = Some("req_db".to_owned());
    storage.insert_history(&rec).expect("insert");
    let (jobs, events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    // Третье соединение держит write-лок (BEGIN IMMEDIATE): чтение pending
    // проходит, а обновление записи получает SQLITE_BUSY.
    let locker = rusqlite::Connection::open(&db).expect("соединение-лок");
    locker
        .execute_batch("BEGIN IMMEDIATE;")
        .expect("write-лок должен захватиться");

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 1, "запись обработана");
    assert_eq!(
        recovered[0].status,
        JobStatus::Pending,
        "Db-сбой complete() не сжигает оплаченную генерацию"
    );
    assert_eq!(recovered[0].request_id.as_deref(), Some("req_db"));
    let history = reader.history(&all()).expect("history");
    assert_eq!(history[0].status, JobStatus::Pending, "в БД запись Pending");
    assert!(events.lock().expect("lock").is_empty(), "событий нет");

    locker.execute_batch("ROLLBACK;").expect("лок снимается");
    drop(locker);

    // Следующий запуск без лока: запись доходит до Completed.
    let recovered = jobs.recover_pending().await.expect("повторный recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, JobStatus::Completed);
    assert_eq!(
        std::fs::read(&recovered[0].local_paths[0]).expect("файл"),
        b"hi"
    );
}

#[tokio::test]
async fn run_transient_download_keeps_pending() {
    let server = MockServer::start().await;
    // generate_video 200: request_id фиксируется до скачивания. Ссылка ведёт
    // на закрытый порт -> Network. Запись обязана остаться Pending с id.
    let guard = Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "vid_net",
            "status": "completed",
            "videos": [{"url": "http://127.0.0.1:1/signed/v.mp4", "content_type": "video/mp4"}],
            "request_id": "req_run_net",
            "billing": {"billed_cost_usd": "0.600000"},
        })))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let (jobs, events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let err = jobs
        .run(video_request())
        .await
        .expect_err("сбой скачивания — ошибка для UI");
    assert!(
        matches!(err, CoreError::Network(_)),
        "ожидался Network: {err:?}"
    );

    let history = reader.history(&all()).expect("history");
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].status,
        JobStatus::Pending,
        "транзиентный сбой после фиксации request_id не сжигает Paid-Pending"
    );
    assert_eq!(
        history[0].request_id.as_deref(),
        Some("req_run_net"),
        "request_id сохранён до скачивания"
    );
    assert!(history[0].local_paths.is_empty(), "файлы не сохранены");
    let events = events.lock().expect("lock").clone();
    assert_eq!(events.len(), 1, "только Started: запись не Failed");
    assert!(matches!(events[0], AppEvent::GenerationStarted { .. }));
    drop(guard);

    // Recovery подхватит запись: generation отдаёт живую ссылку.
    let file_url = format!("{}/signed/ok.mp4", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1/generations/req_run_net"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_run_net",
            "endpoint": "/v1/videos/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "video", "content_type": "video/mp4",
                "size_bytes": 2, "url": file_url,
            }],
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/signed/ok.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hi"))
        .mount(&server)
        .await;

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 1, "Pending-запись подхвачена recovery");
    assert_eq!(recovered[0].status, JobStatus::Completed);
    assert_eq!(
        std::fs::read(&recovered[0].local_paths[0]).expect("файл"),
        b"hi"
    );
}

#[tokio::test]
async fn run_transient_download_persists_cost_for_recovery() {
    let server = MockServer::start().await;
    // generate_video 200 с billing; ссылка на закрытый порт -> Network.
    // Оплаченная стоимость обязана персиститься сразу после ответа CI:
    // recovery-эндпоинт billing не отдаёт, иначе цена потеряется навсегда.
    let guard = Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "vid_cost",
            "status": "completed",
            "videos": [{"url": "http://127.0.0.1:1/signed/v.mp4", "content_type": "video/mp4"}],
            "request_id": "req_run_cost",
            "billing": {"billed_cost_usd": "0.600000"},
        })))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let err = jobs
        .run(video_request())
        .await
        .expect_err("сбой скачивания — ошибка для UI");
    assert!(
        matches!(err, CoreError::Network(_)),
        "ожидался Network: {err:?}"
    );

    // Стоимость уже в Pending-записи — тем же патчем, что и request_id.
    let history = reader.history(&all()).expect("history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, JobStatus::Pending);
    assert_eq!(
        history[0].request_id.as_deref(),
        Some("req_run_cost"),
        "request_id зафиксирован до скачивания"
    );
    assert_eq!(
        history[0].cost_usd,
        Some(0.6),
        "оплаченная стоимость персистится сразу после ответа CI, до скачивания"
    );
    drop(guard);

    // Recovery завершает запись, не теряя цену; расход учитывает оплату.
    let file_url = format!("{}/signed/ok.mp4", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1/generations/req_run_cost"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_run_cost",
            "endpoint": "/v1/videos/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "video", "content_type": "video/mp4",
                "size_bytes": 2, "url": file_url,
            }],
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/signed/ok.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hi"))
        .mount(&server)
        .await;

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, JobStatus::Completed);
    assert_eq!(
        recovered[0].cost_usd,
        Some(0.6),
        "recovery сохраняет оплаченную стоимость"
    );
    let history = reader.history(&all()).expect("history");
    assert_eq!(
        history[0].cost_usd,
        Some(0.6),
        "стоимость в БД после recovery"
    );
    let view = jobs.balance_view(false).await.expect("balance_view");
    assert!(
        (view.local_spend_usd - 0.6).abs() < 1e-9,
        "оплаченная стоимость попала в total_spend_usd: {}",
        view.local_spend_usd
    );
}

#[tokio::test]
async fn run_image_transient_download_persists_cost_for_recovery() {
    let server = MockServer::start().await;
    // images 200 с billing; элемент с url (не b64) на закрытый порт -> Network
    // после ответа CI. Стоимость изображений персистится так же, как у видео.
    let guard = Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"url": "http://127.0.0.1:1/signed/i.png", "content_type": "image/png"}],
            "request_id": "img_cost",
            "billing": {"billed_cost_usd": "0.040000"},
        })))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("h.db");
    let storage = Storage::open(&db).expect("БД");
    let reader = Storage::open(&db).expect("reader");
    let (jobs, _events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

    let err = jobs
        .run(image_request())
        .await
        .expect_err("сбой скачивания — ошибка для UI");
    assert!(
        matches!(err, CoreError::Network(_)),
        "ожидался Network: {err:?}"
    );

    let history = reader.history(&all()).expect("history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, JobStatus::Pending);
    assert_eq!(history[0].request_id.as_deref(), Some("img_cost"));
    assert_eq!(
        history[0].cost_usd,
        Some(0.04),
        "оплаченная стоимость изображения персистится до скачивания"
    );
    drop(guard);

    // Recovery завершает запись с сохранённой ценой; расход учитывает оплату.
    let file_url = format!("{}/signed/ok.png", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1/generations/img_cost"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "img_cost",
            "endpoint": "/v1/images/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "image", "content_type": "image/png",
                "size_bytes": 2, "url": file_url,
            }],
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/signed/ok.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hi"))
        .mount(&server)
        .await;

    let recovered = jobs.recover_pending().await.expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, JobStatus::Completed);
    assert_eq!(recovered[0].cost_usd, Some(0.04));
    let view = jobs.balance_view(false).await.expect("balance_view");
    assert!(
        (view.local_spend_usd - 0.04).abs() < 1e-9,
        "оплаченная стоимость попала в total_spend_usd: {}",
        view.local_spend_usd
    );
}

#[tokio::test]
async fn video_processing_keeps_pending() {
    for status in ["processing", "pending", "in_progress"] {
        let server = MockServer::start().await;
        let video_url = format!("{}/signed/v.mp4", server.uri());
        Mock::given(method("POST"))
            .and(path("/v1/videos/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "vid_proc",
                "status": status,
                "videos": [{"url": video_url, "content_type": "video/mp4"}],
                "request_id": "req_proc",
            })))
            .mount(&server)
            .await;

        let tmp = TempDir::new().expect("tempdir");
        let db = tmp.path().join("h.db");
        let storage = Storage::open(&db).expect("БД");
        let reader = Storage::open(&db).expect("reader");
        let (jobs, events) = jobs_with(client_for(&server), storage, tmp.path().join("media"));

        let pending = jobs
            .run(video_request())
            .await
            .expect("CI принял задание и прислал квитанцию");
        assert_eq!(pending.status, JobStatus::Pending);
        assert_eq!(pending.error.as_deref(), Some("history.video_processing"));

        let history = reader.history(&all()).expect("history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].error, pending.error);
        assert_eq!(
            history[0].status,
            JobStatus::Pending,
            "статус {status}: генерация ещё идёт, запись не сжигается"
        );
        assert_eq!(
            history[0].request_id.as_deref(),
            Some("req_proc"),
            "request_id сохранён до status-guard (статус {status})"
        );
        assert!(history[0].local_paths.is_empty(), "файлы не сохранены");
        // Скачивание не начинается: статус проверен до файлов.
        let requests = received(&server).await;
        assert!(
            requests.iter().all(|r| r.url.path() != "/signed/v.mp4"),
            "при статусе {status} файл не скачивается"
        );
        // Событие Failed не шлётся: запись остаётся Pending.
        let events = events.lock().expect("lock").clone();
        assert_eq!(events.len(), 1, "только Started при статусе {status}");
        assert!(matches!(events[0], AppEvent::GenerationStarted { .. }));
    }
}

#[test]
fn run_future_is_send() {
    fn assert_send<T: Send>(_: T) {}
    let storage = Storage::open_in_memory().expect("БД");
    let events: Arc<Mutex<Vec<AppEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let emit: Arc<dyn Fn(AppEvent) + Send + Sync> = Arc::new(move |e| {
        sink.lock().expect("lock").push(e);
    });
    let jobs = Jobs::new(
        CiClient::with_base("k", "http://localhost:1"),
        storage,
        PathBuf::from("/tmp/media"),
        emit,
    );
    assert_send(jobs.run(image_request()));
    assert_send(jobs.recover_pending());
    assert_send(jobs.balance_view(true));
}

#[tokio::test]
async fn malformed_video_response_recovers_without_a_second_paid_request() {
    const ID: &str = "78c93671-645f-40b9-bff3-9d120038a71a";
    let server = MockServer::start().await;
    let generation_path = format!("/v1/generations/{ID}");
    Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-ci-request-id", ID)
                .insert_header("x-ci-billed-cost-usd", "0.125000")
                .set_body_string("{"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("history.db");
    let reader = Storage::open(&db).unwrap();
    let (jobs, events) = jobs_with(
        client_for(&server),
        Storage::open(&db).unwrap(),
        tmp.path().join("media"),
    );
    let err = jobs.run(video_request()).await.unwrap_err();
    assert!(
        matches!(&err, CoreError::Receipt {request_id, cost_usd: Some(cost), ..} if request_id == ID && *cost == 0.125)
    );
    let row = reader.history(&all()).unwrap().remove(0);
    assert_eq!(row.status, JobStatus::Pending);
    assert_eq!(row.request_id.as_deref(), Some(ID));
    assert_eq!(row.cost_usd, Some(0.125));
    assert_eq!(row.error.as_deref(), Some("history.recovery_response"));

    // CI скрывает присутствие чужих файлов за generation_not_found. Правильный
    // ключ всё ещё может вернуть тот же результат в течение семи дней.
    for (status, code, note) in [
        (403, "insufficient_scope", "history.recovery_access"),
        (404, "generation_not_found", "history.recovery_not_found"),
    ] {
        let guard = Mock::given(method("GET"))
            .and(path(&generation_path))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({
                "error": {"code":code, "message":"unavailable"}
            })))
            .expect(1)
            .mount_as_scoped(&server)
            .await;
        let pending = jobs.recover_pending().await.unwrap().remove(0);
        assert_eq!(pending.status, JobStatus::Pending);
        assert_eq!(
            reader.history(&all()).unwrap()[0].error.as_deref(),
            Some(note)
        );
        drop(guard);
    }

    Mock::given(method("GET")).and(path(&generation_path))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object":"generation", "request_id": ID, "endpoint":"/v1/videos/generations",
            "created_at":"2026-11-24T10:00:00Z", "expires_at":"2026-12-01T10:00:00Z",
            "files":[{"index":0,"type":"video","content_type":"video/mp4","size_bytes":2,
                "url":format!("{}/signed/output.mp4",server.uri()),"url_expires_at":"2026-11-26T11:00:00Z"}]
        })))
        .expect(2).mount(&server).await;
    let expired_link = Mock::given(method("GET"))
        .and(path("/signed/output.mp4"))
        .respond_with(ResponseTemplate::new(410))
        .expect(1)
        .mount_as_scoped(&server)
        .await;
    let pending = jobs.recover_pending().await.unwrap().remove(0);
    assert_eq!(pending.status, JobStatus::Pending);
    assert_eq!(pending.error.as_deref(), Some("history.recovery_response"));
    drop(expired_link);
    Mock::given(method("GET"))
        .and(path("/signed/output.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hi"))
        .expect(1)
        .mount(&server)
        .await;
    let recovered = jobs.recover_pending().await.unwrap().remove(0);
    assert_eq!(recovered.status, JobStatus::Completed);
    assert_eq!(recovered.id, row.id);
    assert_eq!(std::fs::read(&recovered.local_paths[0]).unwrap(), b"hi");
    let stored = reader.history(&all()).unwrap();
    assert_eq!(stored.len(), 1);
    assert!(stored[0].error.is_none());
    assert_eq!(
        reader
            .total_spend_usd(km_shared::Provider::CheaperInference)
            .unwrap(),
        0.125
    );
    assert!(matches!(
        events.lock().unwrap().as_slice(),
        [
            AppEvent::GenerationStarted { .. },
            AppEvent::GenerationDone { .. }
        ]
    ));
}

#[tokio::test]
async fn inline_video_is_saved_when_billing_is_pending() {
    const ID: &str = "0d2fb997-922c-4602-9feb-307eb73ef209";
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-request-id", ID)
                .set_body_json(json!({
                    "id":"provider-video-id", "status":"succeeded",
                    "videos":[{"b64_video":"aGk="}],
                    "billing":{"status":"pending","currency":"USD","billed_cost_usd":null}
                })),
        )
        .expect(1)
        .mount(&server)
        .await;
    let tmp = TempDir::new().unwrap();
    let (jobs, _) = jobs_with(
        client_for(&server),
        Storage::open_in_memory().unwrap(),
        tmp.path().join("media"),
    );
    let rec = jobs.run(video_request()).await.unwrap();
    assert_eq!(rec.status, JobStatus::Completed);
    assert_eq!(rec.request_id.as_deref(), Some(ID));
    assert_eq!(rec.cost_usd, None);
    assert_eq!(std::fs::read(&rec.local_paths[0]).unwrap(), b"hi");
    assert_eq!(received(&server).await.len(), 1);
}
