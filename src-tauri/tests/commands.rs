//! Интеграционные тесты логики Tauri-команд: чистые `cmd_*` с мок-ядром.
//!
//! HTTP — wiremock, БД — файловая SQLite в tempdir, ключ — [`InMemoryKeyStore`],
//! события — коллектор. Тонкие обёртки `#[tauri::command]` не тестируются:
//! они только достают State и зовут `cmd_*`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use km_core::Storage;
use km_shared::{
    AppEvent, GenMode, GenerationRequest, HistoryFilter, HistoryRecord, JobStatus, Lang, MediaKind,
    Settings, Theme,
};
use koteyye_media_studio::commands::{
    cmd_delete_history, cmd_fetch_balance, cmd_fetch_models, cmd_generate, cmd_get_settings,
    cmd_list_history, cmd_open_file, cmd_recover_pending, cmd_set_api_key, cmd_set_settings,
    cmd_show_in_folder, cmd_test_connection, event_name,
};
use koteyye_media_studio::keychain::{InMemoryKeyStore, KeyStore};
use koteyye_media_studio::state::AppState;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Эмиттер событий ядра (в тестах — запись в коллектор).
type EventEmitter = Arc<dyn Fn(AppEvent) + Send + Sync>;

/// KeyStore с нечитаемым хранилищем: `get` всегда `Err` (заблокированный
/// keychain, отказ prompt). Запись и удаление успешны — сбой именно чтения.
struct UnreadableKeyStore;

impl KeyStore for UnreadableKeyStore {
    fn get(&self) -> Result<Option<String>, String> {
        Err("не удалось прочитать ключ из keychain: доступ запрещён".to_owned())
    }

    fn set(&self, _key: &str) -> Result<(), String> {
        Ok(())
    }

    fn delete(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Коллектор событий для проверки порядка и состава.
type EventLog = Arc<Mutex<Vec<AppEvent>>>;

/// Коллектор событий и эмиттер для AppState.
fn collector() -> (EventEmitter, EventLog) {
    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let emit: EventEmitter = Arc::new(move |event| {
        sink.lock().expect("мьютекс событий отравлен").push(event);
    });
    (emit, events)
}

/// AppState с мок-сервером, БД в tempdir и in-memory ключом (без ключа, если `None`).
fn state_for(server: &MockServer, tmp: &TempDir, key: Option<&str>) -> (AppState, EventLog) {
    let key_store = InMemoryKeyStore::default();
    if let Some(key) = key {
        key_store.set(key).expect("ключ должен сохраниться");
    }
    let (emit, events) = collector();
    let state = AppState::new(
        &tmp.path().join("history.db"),
        Box::new(key_store),
        &server.uri(),
        emit,
    )
    .expect("AppState должен создаться");
    (state, events)
}

/// Задаёт папку сохранения в настройках и создаёт её; возвращает путь.
fn with_folder(state: &AppState, tmp: &TempDir) -> PathBuf {
    let folder = tmp.path().join("media");
    std::fs::create_dir_all(&folder).expect("папка сохранения должна создаться");
    let settings = Settings {
        local_folder: folder.to_string_lossy().into_owned(),
        lang: Lang::Ru,
        theme: Theme::System,
        show_real_balance: false,
    };
    state
        .storage
        .set_settings(&settings)
        .expect("настройки должны сохраниться");
    folder
}

/// Запрос текст → изображение.
fn image_request() -> GenerationRequest {
    GenerationRequest {
        kind: MediaKind::Image,
        mode: GenMode::TextToImage,
        model: "flux-2-pro".to_owned(),
        prompt: "кот в шляпе".to_owned(),
        params: km_shared::GenParams::default(),
        references: Vec::new(),
        mask_data_url: None,
    }
}

/// Базовая запись истории для recover/list-сценариев.
fn history_record(id: &str, status: JobStatus) -> HistoryRecord {
    HistoryRecord {
        id: id.to_owned(),
        created_at: 1_700_000_000,
        updated_at: 1_700_000_000,
        kind: MediaKind::Image,
        mode: GenMode::TextToImage,
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

/// Ответ images с base64-изображением и тарификацией строкой.
fn image_response() -> Value {
    json!({
        "data": [{"b64_json": "aGk=", "content_type": "image/png"}],
        "request_id": "img_123",
        "billing": {"billed_cost_usd": "0.040000"},
    })
}

/// Каталог: одна image- и одна speech-модель.
fn catalog_response() -> Value {
    json!({
        "data": [
            {"id": "flux-2-pro", "model_type": "image"},
            {"id": "eleven-v4", "model_type": "speech"},
        ],
    })
}

/// Принятые мок-сервером запросы.
async fn received(server: &MockServer) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .expect("мок-сервер должен записать запросы")
}

#[tokio::test]
async fn generate_writes_history_files_and_events() {
    let server = MockServer::start().await;
    let guard = Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(image_response()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (state, events) = state_for(&server, &tmp, Some("ci_live_test"));
    let folder = with_folder(&state, &tmp);

    let rec = cmd_generate(&state, image_request())
        .await
        .expect("generate должен завершиться");

    assert_eq!(rec.status, JobStatus::Completed, "статус Completed");
    assert_eq!(rec.cost_usd, Some(0.04), "стоимость из billing");
    assert_eq!(rec.local_paths.len(), 1, "один файл сохранён");
    let path = PathBuf::from(&rec.local_paths[0]);
    assert_eq!(
        path.parent(),
        Some(folder.as_path()),
        "файл в папке сохранения"
    );
    assert_eq!(std::fs::read(&path).expect("файл должен читаться"), b"hi");

    // Запись действительно в БД (второе соединение).
    let reader = Storage::open(&tmp.path().join("history.db")).expect("БД должна открыться");
    let history = reader
        .history(&HistoryFilter::default())
        .expect("история должна прочитаться");
    assert_eq!(history.len(), 1, "одна запись истории");
    assert_eq!(history[0].id, rec.id);
    assert_eq!(history[0].status, JobStatus::Completed);

    // События: Started -> Done -> BalanceUpdated (расход изменился).
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
            AppEvent::BalanceUpdated,
        ],
        "порядок событий Started -> Done -> BalanceUpdated"
    );

    drop(guard);
}

#[tokio::test]
async fn generate_without_key_returns_error_and_no_history() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let (state, events) = state_for(&server, &tmp, None);
    with_folder(&state, &tmp);

    let err = cmd_generate(&state, image_request())
        .await
        .expect_err("без ключа generate должен вернуть ошибку");
    assert!(err.contains("ключ"), "ошибка про отсутствие ключа: {err}");
    assert!(
        state
            .storage
            .history(&HistoryFilter::default())
            .expect("история")
            .is_empty(),
        "запись не создаётся без ключа"
    );
    assert!(received(&server).await.is_empty(), "запрос к CI не идёт");
    assert!(events.lock().expect("lock").is_empty(), "событий нет");
}

#[tokio::test]
async fn generate_without_folder_returns_error() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, Some("ci_live_test"));
    // Папка сохранения не задана (пустая в настройках по умолчанию).

    let err = cmd_generate(&state, image_request())
        .await
        .expect_err("без папки generate должен вернуть ошибку");
    assert!(err.contains("папка"), "ошибка про папку сохранения: {err}");
    assert!(
        state
            .storage
            .history(&HistoryFilter::default())
            .expect("история")
            .is_empty(),
        "запись не создаётся без папки"
    );
    assert!(received(&server).await.is_empty(), "запрос к CI не идёт");
}

#[tokio::test]
async fn test_connection_200_returns_true() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(catalog_response()))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, Some("ci_live_test"));

    assert!(
        cmd_test_connection(&state).await.expect("test_connection"),
        "200 от /v1/models -> подключение есть"
    );
}

#[tokio::test]
async fn test_connection_401_returns_false() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": {"code": "invalid_api_key", "message": "bad key"},
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, Some("ci_live_bad"));

    assert!(
        !cmd_test_connection(&state).await.expect("test_connection"),
        "401 -> подключения нет"
    );
}

#[tokio::test]
async fn test_connection_without_key_returns_false_without_http() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, None);

    assert!(
        !cmd_test_connection(&state).await.expect("test_connection"),
        "без ключа -> подключения нет"
    );
    assert!(received(&server).await.is_empty(), "запрос к CI не идёт");
}

#[tokio::test]
async fn test_connection_non_401_error_returns_err() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "error": {"code": "internal", "message": "boom"},
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, Some("ci_live_test"));

    let err = cmd_test_connection(&state)
        .await
        .expect_err("500 — не вердикт о подключении, а ошибка");
    assert!(err.contains("500"), "ошибка содержит статус: {err}");
}

#[tokio::test]
async fn test_connection_keychain_read_error_is_err_not_false() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let (emit, _events) = collector();
    let state = AppState::new(
        &tmp.path().join("history.db"),
        Box::new(UnreadableKeyStore),
        &server.uri(),
        emit,
    )
    .expect("AppState должен создаться");

    // Ошибка чтения keychain — не вердикт о ключе: Err (UI покажет причину,
    // редиректа в онбординг нет), а не Ok(false) с ложным онбордингом.
    let err = cmd_test_connection(&state)
        .await
        .expect_err("сбой чтения keychain — ошибка, а не отсутствие ключа");
    assert!(err.contains("keychain"), "ошибка называет причину: {err}");
    assert!(
        received(&server).await.is_empty(),
        "без читаемого ключа запрос к CI не идёт"
    );
}

#[tokio::test]
async fn recover_pending_keychain_read_error_keeps_pending() {
    let server = MockServer::start().await;
    let guard = Mock::given(method("GET"))
        .and(path("/v1/generations/req_crash"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (emit, events) = collector();
    let state = AppState::new(
        &tmp.path().join("history.db"),
        Box::new(UnreadableKeyStore),
        &server.uri(),
        emit,
    )
    .expect("AppState должен создаться");
    let folder = tmp.path().join("media");
    std::fs::create_dir_all(&folder).expect("папка");
    state
        .storage
        .set_settings(&Settings {
            local_folder: folder.to_string_lossy().into_owned(),
            lang: Lang::Ru,
            theme: Theme::System,
            show_real_balance: false,
        })
        .expect("настройки");
    let mut rec = history_record("crashed", JobStatus::Pending);
    rec.request_id = Some("req_crash".to_owned());
    state.storage.insert_history(&rec).expect("запись");

    // Нечитаемый keychain не сжигает оплаченные Pending: Err, записи на месте.
    let err = cmd_recover_pending(&state)
        .await
        .expect_err("сбой чтения keychain — ошибка восстановления");
    assert!(err.contains("keychain"), "ошибка называет причину: {err}");
    let history = state
        .storage
        .history(&HistoryFilter::default())
        .expect("история");
    assert_eq!(history.len(), 1, "запись на месте");
    assert_eq!(
        history[0].status,
        JobStatus::Pending,
        "запись остаётся Pending"
    );
    assert!(received(&server).await.is_empty(), "запрос к CI не идёт");
    assert!(events.lock().expect("lock").is_empty(), "событий нет");

    drop(guard);
}

#[tokio::test]
async fn fetch_balance_403_falls_back_to_local() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/account/balance"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": {"code": "forbidden", "message": "scope account:read required"},
        })))
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, Some("ci_live_test"));
    let mut settings = state.storage.get_settings().expect("настройки");
    settings.show_real_balance = true;
    state.storage.set_settings(&settings).expect("настройки");
    let mut rec = history_record("c1", JobStatus::Completed);
    rec.cost_usd = Some(0.04);
    state.storage.insert_history(&rec).expect("запись");

    let view = cmd_fetch_balance(&state).await.expect("fetch_balance");
    assert_eq!(view.real, None, "403 -> реальный баланс скрыт");
    assert!(
        (view.local_spend_usd - 0.04).abs() < 1e-9,
        "фолбэк на локальный расход"
    );
}

#[tokio::test]
async fn fetch_balance_200_returns_real_and_local() {
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
    let (state, _events) = state_for(&server, &tmp, Some("ci_live_test"));
    let mut settings = state.storage.get_settings().expect("настройки");
    settings.show_real_balance = true;
    state.storage.set_settings(&settings).expect("настройки");

    let view = cmd_fetch_balance(&state).await.expect("fetch_balance");
    let real = view.real.expect("реальный баланс должен быть");
    assert_eq!(real.balance_usd, 10.5);
    assert_eq!(
        view.local_spend_usd, 0.0,
        "пустая история -> нулевой расход"
    );
}

#[tokio::test]
async fn fetch_models_with_key_uses_v1_models_and_filters_by_kind() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(catalog_response()))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, Some("ci_live_test"));

    let models = cmd_fetch_models(&state, MediaKind::Image)
        .await
        .expect("fetch_models");
    assert_eq!(models.len(), 1, "только image-модели");
    assert_eq!(models[0].id, "flux-2-pro");
}

#[tokio::test]
async fn fetch_models_without_key_uses_public_catalog_without_auth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "models": [
                {"id": "flux-2-pro", "model_type": "image"},
                {"id": "veo-3.1-generate-001", "model_type": "video"},
            ],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, None);

    let models = cmd_fetch_models(&state, MediaKind::Image)
        .await
        .expect("fetch_models без ключа");
    assert_eq!(models.len(), 1, "только image-модели");
    assert_eq!(models[0].id, "flux-2-pro");
    let requests = received(&server).await;
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].headers.get("authorization").is_none(),
        "публичный каталог без заголовка Authorization"
    );
}

#[tokio::test]
async fn fetch_models_without_key_caches_public_catalog() {
    let server = MockServer::start().await;
    let guard = Mock::given(method("GET"))
        .and(path("/public/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(catalog_response()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, None);

    let first = cmd_fetch_models(&state, MediaKind::Image)
        .await
        .expect("первый fetch_models");
    let second = cmd_fetch_models(&state, MediaKind::Image)
        .await
        .expect("второй fetch_models из кэша");
    assert_eq!(first, second, "кэш отдаёт тот же каталог");

    drop(guard); // expect(1): второй вызов не должен пойти в сеть
}

#[tokio::test]
async fn settings_roundtrip_via_commands() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, None);

    let defaults = cmd_get_settings(&state).await.expect("get_settings");
    assert_eq!(defaults.local_folder, "", "папка по умолчанию пустая");
    assert!(!defaults.show_real_balance, "реальный баланс скрыт");

    let settings = Settings {
        local_folder: "/tmp/media".to_owned(),
        lang: Lang::En,
        theme: Theme::Dark,
        show_real_balance: true,
    };
    cmd_set_settings(&state, settings.clone())
        .await
        .expect("set_settings");
    let read = cmd_get_settings(&state).await.expect("get_settings");
    assert_eq!(read, settings, "настройки сохраняются и читаются");

    // Персистентность: второе соединение видит те же настройки.
    let reader = Storage::open(&tmp.path().join("history.db")).expect("БД");
    assert_eq!(reader.get_settings().expect("настройки"), settings);
}

#[tokio::test]
async fn set_api_key_stores_in_key_store_not_in_sqlite() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, None);
    let db_path = tmp.path().join("history.db");

    cmd_set_api_key(&state, "ci_live_secret_key")
        .await
        .expect("set_api_key");
    assert_eq!(
        state.key_store.get().expect("get").as_deref(),
        Some("ci_live_secret_key"),
        "ключ в KeyStore"
    );
    let db_bytes = std::fs::read(&db_path).expect("файл БД должен читаться");
    assert!(
        !db_bytes
            .windows(b"ci_live_secret_key".len())
            .any(|w| w == b"ci_live_secret_key"),
        "ключа нет в файле SQLite"
    );

    // Пустой ключ удаляет сохранённый.
    cmd_set_api_key(&state, "   ")
        .await
        .expect("пустой set_api_key");
    assert_eq!(
        state.key_store.get().expect("get"),
        None,
        "пустой ключ удаляет запись"
    );
}

#[tokio::test]
async fn list_history_filters_and_delete_removes_record() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, None);

    let mut image = history_record("img", JobStatus::Completed);
    image.prompt = "кот в шляпе".to_owned();
    let mut video = history_record("vid", JobStatus::Completed);
    video.kind = MediaKind::Video;
    video.mode = GenMode::TextToVideo;
    video.prompt = "кот плывёт".to_owned();
    video.created_at = 1_700_000_001;
    state.storage.insert_history(&image).expect("запись");
    state.storage.insert_history(&video).expect("запись");

    let all = cmd_list_history(&state, HistoryFilter::default())
        .await
        .expect("list_history");
    assert_eq!(all.len(), 2, "вся история");

    let filter = HistoryFilter {
        kind: Some(MediaKind::Image),
        search: Some("ШЛЯПЕ".to_owned()),
    };
    let filtered = cmd_list_history(&state, filter)
        .await
        .expect("list_history с фильтром");
    assert_eq!(filtered.len(), 1, "фильтр по виду и поиску");
    assert_eq!(filtered[0].id, "img");

    cmd_delete_history(&state, "img")
        .await
        .expect("delete_history");
    let rest = cmd_list_history(&state, HistoryFilter::default())
        .await
        .expect("list_history");
    assert_eq!(rest.len(), 1, "запись удалена");
    assert_eq!(rest[0].id, "vid");
}

#[tokio::test]
async fn recover_pending_completes_crashed_record_and_emits_balance() {
    let server = MockServer::start().await;
    let file_url = format!("{}/signed/v.mp4", server.uri());
    let guard = Mock::given(method("GET"))
        .and(path("/v1/generations/req_crash"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_crash",
            "endpoint": "/v1/images/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "image", "content_type": "image/png",
                "size_bytes": 2, "url": file_url,
            }],
        })))
        .expect(1)
        .mount_as_scoped(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/signed/v.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hi"))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (state, events) = state_for(&server, &tmp, Some("ci_live_test"));
    let folder = with_folder(&state, &tmp);
    let mut rec = history_record("crashed", JobStatus::Pending);
    rec.request_id = Some("req_crash".to_owned());
    state.storage.insert_history(&rec).expect("запись");

    let recovered = cmd_recover_pending(&state).await.expect("recover_pending");
    assert_eq!(recovered.len(), 1, "одна Pending-запись");
    assert_eq!(recovered[0].status, JobStatus::Completed);
    assert_eq!(recovered[0].local_paths.len(), 1, "файл сохранён");
    assert!(
        PathBuf::from(&recovered[0].local_paths[0]).starts_with(&folder),
        "файл в папке сохранения"
    );

    let events = events.lock().expect("lock").clone();
    assert!(
        events.iter().any(
            |e| matches!(e, AppEvent::GenerationDone { history_id } if history_id == "crashed")
        ),
        "событие Done"
    );
    assert!(
        events.iter().any(|e| matches!(e, AppEvent::BalanceUpdated)),
        "событие BalanceUpdated после восстановления"
    );

    drop(guard);
}

#[tokio::test]
async fn parallel_recover_pending_processes_each_record_once() {
    let server = MockServer::start().await;
    let file_url = format!("{}/signed/dup.png", server.uri());
    // Стартовый проход (setup) и проход из UI (load) зовут одну команду.
    // Без сериализации оба читают одни Pending и скачивают файл дважды:
    // дубликат по timestamp, двойные Done/BalanceUpdated, сирота после complete.
    let generation_guard = Mock::given(method("GET"))
        .and(path("/v1/generations/req_dup"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req_dup",
            "endpoint": "/v1/images/generations",
            "created_at": "2026-11-24T10:00:00Z",
            "expires_at": "2026-12-01T10:00:00Z",
            "files": [{
                "index": 0, "type": "image", "content_type": "image/png",
                "size_bytes": 2, "url": file_url,
            }],
        })))
        .expect(1)
        .mount_as_scoped(&server)
        .await;
    let file_guard = Mock::given(method("GET"))
        .and(path("/signed/dup.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hi"))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (state, events) = state_for(&server, &tmp, Some("ci_live_test"));
    let folder = with_folder(&state, &tmp);
    let mut rec = history_record("dup", JobStatus::Pending);
    rec.request_id = Some("req_dup".to_owned());
    state.storage.insert_history(&rec).expect("запись");

    // Два конкурентных прохода: как при живом ключе + Pending на старте.
    let (first, second) = tokio::join!(cmd_recover_pending(&state), cmd_recover_pending(&state),);
    let first = first.expect("первый проход recover");
    let second = second.expect("второй проход recover");

    let completed = first
        .iter()
        .chain(second.iter())
        .filter(|r| r.status == JobStatus::Completed)
        .count();
    assert_eq!(completed, 1, "запись завершена ровно один раз");
    assert!(
        first.is_empty() || second.is_empty(),
        "второй проход видит только ещё-Pending: проходы сериализованы"
    );

    // Один файл на запись: без дубликата по timestamp и без сироты.
    let files: Vec<_> = std::fs::read_dir(&folder)
        .expect("папка сохранения читается")
        .collect();
    assert_eq!(files.len(), 1, "один файл на запись, без дублей");

    // События не дублируются: один Done и один BalanceUpdated.
    let events = events.lock().expect("lock").clone();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(
                e,
                AppEvent::GenerationDone { history_id } if history_id == "dup"
            ))
            .count(),
        1,
        "один GenerationDone"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AppEvent::BalanceUpdated))
            .count(),
        1,
        "один BalanceUpdated"
    );

    drop(generation_guard);
    drop(file_guard);
}

#[tokio::test]
async fn recover_pending_without_key_keeps_records_pending() {
    let server = MockServer::start().await;
    // Без ключа запросов к CI быть не должно: оплаченная генерация могла
    // завершиться на сервере, запись должна дождаться запуска с ключом.
    let guard = Mock::given(method("GET"))
        .and(path("/v1/generations/req_crash"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (state, events) = state_for(&server, &tmp, None);
    with_folder(&state, &tmp);
    let mut rec = history_record("crashed", JobStatus::Pending);
    rec.request_id = Some("req_crash".to_owned());
    state.storage.insert_history(&rec).expect("запись");

    let recovered = cmd_recover_pending(&state)
        .await
        .expect("recover_pending без ключа не ошибка");
    assert!(
        recovered.is_empty(),
        "без ключа ничего не восстанавливается"
    );

    let history = state
        .storage
        .history(&HistoryFilter::default())
        .expect("история");
    assert_eq!(history.len(), 1, "запись на месте");
    assert_eq!(
        history[0].status,
        JobStatus::Pending,
        "запись не сжигается в Failed без ключа"
    );
    assert!(received(&server).await.is_empty(), "запрос к CI не идёт");
    assert!(events.lock().expect("lock").is_empty(), "событий нет");

    drop(guard);
}

#[tokio::test]
async fn recover_pending_without_folder_keeps_pending_and_no_cwd_files() {
    let server = MockServer::start().await;
    // Без папки сохранения запросов к CI быть не должно: пустой путь записи
    // файлов увёл бы в working directory, а записи должны дождаться папки.
    let guard = Mock::given(method("GET"))
        .and(path("/v1/generations/req_crash"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount_as_scoped(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (state, events) = state_for(&server, &tmp, Some("ci_live_test"));
    // Папка сохранения не задана (пустая в настройках по умолчанию).
    let mut rec = history_record("crashed", JobStatus::Pending);
    rec.request_id = Some("req_crash".to_owned());
    state.storage.insert_history(&rec).expect("запись");

    let cwd = std::env::current_dir().expect("cwd");
    let before = media_entries(&cwd);

    let recovered = cmd_recover_pending(&state)
        .await
        .expect("recover без папки не ошибка");
    assert!(
        recovered.is_empty(),
        "без папки ничего не восстанавливается"
    );

    let history = state
        .storage
        .history(&HistoryFilter::default())
        .expect("история");
    assert_eq!(history.len(), 1, "запись на месте");
    assert_eq!(
        history[0].status,
        JobStatus::Pending,
        "запись не сжигается без папки сохранения"
    );
    assert_eq!(
        history[0].request_id.as_deref(),
        Some("req_crash"),
        "request_id сохранён"
    );
    assert!(received(&server).await.is_empty(), "запрос к CI не идёт");
    assert!(events.lock().expect("lock").is_empty(), "событий нет");
    assert_eq!(
        media_entries(&cwd),
        before,
        "файлы не создаются в working directory"
    );

    drop(guard);
}

/// Имена медиа-файлов (`image-*`/`video-*`/`audio-*`) в директории — для
/// проверки, что recovery с пустой папкой не пишет файлы в CWD.
fn media_entries(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("директория читается")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name.starts_with("image-") || name.starts_with("video-") || name.starts_with("audio-")
        })
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn open_file_inside_folder_returns_canonical_path() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, None);
    let folder = with_folder(&state, &tmp);
    let file = folder.join("image-1.png");
    std::fs::write(&file, b"hi").expect("файл");

    let resolved = cmd_open_file(&state, &file.to_string_lossy())
        .await
        .expect("файл внутри папки должен быть разрешён");
    assert_eq!(
        resolved,
        std::fs::canonicalize(&file).expect("canonicalize"),
        "возвращается канонический путь"
    );
}

#[tokio::test]
async fn open_file_outside_folder_returns_error() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, None);
    with_folder(&state, &tmp);
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).expect("папка");
    let secret = outside.join("secret.png");
    std::fs::write(&secret, b"secret").expect("файл");

    let err = cmd_open_file(&state, &secret.to_string_lossy())
        .await
        .expect_err("путь вне папки сохранения должен быть запрещён");
    assert!(err.contains("вне"), "ошибка про путь вне папки: {err}");
}

#[tokio::test]
async fn open_file_traversal_outside_folder_returns_error() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, None);
    let folder = with_folder(&state, &tmp);
    let outside = tmp.path().join("outside.png");
    std::fs::write(&outside, b"secret").expect("файл");
    let traversal = format!("{}/../outside.png", folder.display());

    let err = cmd_open_file(&state, &traversal)
        .await
        .expect_err("обход через .. должен быть запрещён");
    assert!(err.contains("вне"), "ошибка про путь вне папки: {err}");
}

#[cfg(unix)]
#[tokio::test]
async fn open_file_symlink_escaping_folder_returns_error() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, None);
    let folder = with_folder(&state, &tmp);
    let outside = tmp.path().join("outside.png");
    std::fs::write(&outside, b"secret").expect("файл");
    let link = folder.join("link.png");
    std::os::unix::fs::symlink(&outside, &link).expect("симлинк");

    let err = cmd_open_file(&state, &link.to_string_lossy())
        .await
        .expect_err("симлинк наружу должен быть запрещён");
    assert!(err.contains("вне"), "ошибка про путь вне папки: {err}");
}

#[tokio::test]
async fn open_file_missing_or_unset_folder_returns_error() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, None);
    let folder = with_folder(&state, &tmp);

    let missing = folder.join("nope.png");
    assert!(
        cmd_open_file(&state, &missing.to_string_lossy())
            .await
            .is_err(),
        "несуществующий файл -> ошибка"
    );

    // Папка сохранения не задана.
    let (bare, _) = state_for(&server, &tmp, None);
    assert!(
        cmd_open_file(&bare, &missing.to_string_lossy())
            .await
            .is_err(),
        "без папки сохранения -> ошибка"
    );
}

#[tokio::test]
async fn show_in_folder_checks_same_path_safety() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, None);
    let folder = with_folder(&state, &tmp);
    let inside = folder.join("image-1.png");
    std::fs::write(&inside, b"hi").expect("файл");
    let outside = tmp.path().join("outside.png");
    std::fs::write(&outside, b"secret").expect("файл");

    let resolved = cmd_show_in_folder(&state, &inside.to_string_lossy())
        .await
        .expect("файл внутри папки разрешён");
    assert_eq!(
        resolved,
        std::fs::canonicalize(&inside).expect("canonicalize")
    );

    assert!(
        cmd_show_in_folder(&state, &outside.to_string_lossy())
            .await
            .is_err(),
        "путь вне папки сохранения запрещён"
    );
}

#[test]
fn event_names_and_payload_shape_are_pinned() {
    assert_eq!(
        event_name(&AppEvent::GenerationStarted {
            history_id: "h".to_owned()
        }),
        "generation:started"
    );
    assert_eq!(
        event_name(&AppEvent::GenerationDone {
            history_id: "h".to_owned()
        }),
        "generation:done"
    );
    assert_eq!(
        event_name(&AppEvent::GenerationFailed {
            history_id: "h".to_owned(),
            error: "e".to_owned()
        }),
        "generation:failed"
    );
    assert_eq!(event_name(&AppEvent::BalanceUpdated), "balance:updated");

    // Payload — serde AppEvent: externally tagged, snake_case (форма из Task 3).
    let payload = serde_json::to_value(AppEvent::GenerationStarted {
        history_id: "h1".to_owned(),
    })
    .expect("сериализация");
    assert_eq!(payload, json!({"generation_started": {"history_id": "h1"}}));
    let payload = serde_json::to_value(AppEvent::BalanceUpdated).expect("сериализация");
    assert_eq!(payload, json!("balance_updated"));
}
