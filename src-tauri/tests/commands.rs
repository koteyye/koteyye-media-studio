//! Интеграционные тесты логики Tauri-команд: чистые `cmd_*` с мок-ядром.
//!
//! HTTP — wiremock, БД — файловая SQLite в tempdir, ключ — [`InMemoryKeyStore`],
//! события — коллектор. Тонкие обёртки `#[tauri::command]` не тестируются:
//! они только достают State и зовут `cmd_*`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use km_core::{OpenRouterKeyClient, Storage};
use km_shared::{
    AppEvent, BalanceUnavailable, GenMode, GenerationRequest, HistoryFilter, HistoryRecord,
    JobStatus, Lang, MediaKind, Provider, Settings, Theme,
};
use koteyye_media_studio::commands::{
    cmd_check_provider_key, cmd_delete_history, cmd_fetch_balance, cmd_fetch_balance_for,
    cmd_fetch_models, cmd_generate, cmd_generate_for, cmd_get_settings, cmd_list_history,
    cmd_open_file, cmd_provider_has_key, cmd_recover_pending, cmd_set_api_key,
    cmd_set_provider_key, cmd_set_settings, cmd_show_in_folder, cmd_test_connection, event_name,
};
use koteyye_media_studio::keychain::{InMemoryKeyStore, KeyStore};
use koteyye_media_studio::state::AppState;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Эмиттер событий ядра (в тестах — запись в коллектор).
type EventEmitter = Arc<dyn Fn(AppEvent) + Send + Sync>;

/// KeyStore с нечитаемым хранилищем: `get` всегда `Err` (заблокированный
/// keychain, отказ prompt). Запись и удаление успешны — сбой именно чтения.
struct UnreadableKeyStore;

impl KeyStore for UnreadableKeyStore {
    fn get(&self, _provider: Provider) -> Result<Option<String>, String> {
        Err("не удалось прочитать ключ из keychain: доступ запрещён".to_owned())
    }

    fn set(&self, _provider: Provider, _key: &str) -> Result<(), String> {
        Ok(())
    }

    fn delete(&self, _provider: Provider) -> Result<(), String> {
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
        key_store
            .set(Provider::CheaperInference, key)
            .expect("ключ должен сохраниться");
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
        ..Settings::default()
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
        provider: Provider::CheaperInference,
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
            ..Settings::default()
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
    assert_eq!(view.provider, Provider::CheaperInference);
    assert_eq!(view.unavailable, Some(BalanceUnavailable::PermissionDenied));
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
        .and(wiremock::matchers::query_param("type", "image"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_string(include_str!(
            "../../crates/km-shared/tests/fixtures/v1-models.json"
        )))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().expect("tempdir");
    let (state, _events) = state_for(&server, &tmp, Some("ci_live_test"));

    let models = cmd_fetch_models(&state, MediaKind::Image)
        .await
        .expect("fetch_models");
    assert_eq!(models.len(), 1, "только image-модели");
    assert_eq!(models[0].id, "flux-2-flex");
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
        ..Settings::default()
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
        state
            .key_store
            .get(Provider::CheaperInference)
            .expect("get")
            .as_deref(),
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
        state
            .key_store
            .get(Provider::CheaperInference)
            .expect("get"),
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
async fn history_file_remains_accessible_after_folder_change() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().unwrap();
    let (state, _) = state_for(&server, &tmp, None);
    with_folder(&state, &tmp);
    let original = dunce::canonicalize(tmp.path()).unwrap().join("video-1.mp4");
    std::fs::write(&original, b"video").unwrap();
    let mut rec = history_record("previous-folder", JobStatus::Completed);
    rec.local_paths = vec![original.to_str().unwrap().to_owned()];
    state.storage.insert_history(&rec).unwrap();
    assert_eq!(
        cmd_open_file(&state, original.to_str().unwrap())
            .await
            .unwrap(),
        original
    );
}

#[tokio::test]
async fn delete_preserves_a_pending_generation_for_recovery() {
    let server = MockServer::start().await;
    let tmp = TempDir::new().unwrap();
    let (state, _) = state_for(&server, &tmp, None);
    let rec = history_record("paid-pending", JobStatus::Pending);
    state.storage.insert_history(&rec).unwrap();
    assert!(cmd_delete_history(&state, &rec.id).await.is_err());
    assert_eq!(state.storage.pending().unwrap(), vec![rec]);
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

#[tokio::test]
async fn recovery_preserves_in_flight_generation_when_settings_change() {
    use std::time::Duration;

    let server = MockServer::start().await;
    let paid = Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .and(header("authorization", "Bearer ci_live_original"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(image_response())
                .set_delay(Duration::from_millis(50)),
        )
        .expect(1)
        .mount_as_scoped(&server)
        .await;
    let tmp = TempDir::new().unwrap();
    let (state, events) = state_for(&server, &tmp, Some("ci_live_original"));
    let original = with_folder(&state, &tmp);
    let generate = cmd_generate(&state, image_request());
    tokio::pin!(generate);
    // Сервер уже принял оплачиваемый POST. Останавливаем опрос future генерации
    // до проверки гонки: даже при мгновенном ответе request_id ещё не попадёт в БД.
    tokio::select! {
        biased;
        () = paid.wait_until_satisfied() => {}
        result = &mut generate => panic!("генерация завершилась до перехвата POST: {result:?}"),
        () = tokio::time::sleep(Duration::from_secs(10)) => panic!("сервер не получил POST"),
    }
    let before = state.storage.pending().unwrap();
    assert_eq!(before.len(), 1);
    assert!(before[0].request_id.is_none());

    let recover = cmd_recover_pending(&state);
    tokio::pin!(recover);
    // Допустимы и ожидание генерации, и немедленный пропуск живой записи.
    let early = tokio::time::timeout(Duration::from_millis(100), &mut recover)
        .await
        .ok();
    let next_folder = tmp.path().join("next");
    std::fs::create_dir(&next_folder).unwrap();
    let mut changed = cmd_get_settings(&state).await.unwrap();
    changed.local_folder = next_folder.to_string_lossy().into_owned();
    changed.provider = Provider::OpenRouter;
    cmd_set_settings(&state, changed).await.unwrap();
    cmd_set_api_key(&state, "ci_live_next").await.unwrap();
    assert_eq!(state.storage.pending().unwrap(), before);

    let completed = generate.await.unwrap();
    let recovered = match early {
        Some(result) => result,
        None => recover.await,
    }
    .unwrap();
    assert!(
        recovered.is_empty(),
        "восстановление пропускает активную запись"
    );
    assert_eq!(completed.id, before[0].id);
    assert_eq!(completed.status, JobStatus::Completed);
    assert_eq!(
        state.storage.history(&HistoryFilter::default()).unwrap(),
        vec![completed.clone()]
    );
    assert_eq!(media_entries(&original).len(), 1);
    assert_eq!(
        PathBuf::from(&completed.local_paths[0]).parent(),
        Some(original.as_path())
    );
    assert!(media_entries(&next_folder).is_empty());
    assert_eq!(
        state
            .storage
            .total_spend_usd(Provider::CheaperInference)
            .unwrap(),
        0.04
    );
    assert_eq!(completed.provider, Provider::CheaperInference);
    assert_eq!(
        received(&server).await.len(),
        1,
        "единственный запрос — исходный POST"
    );
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            AppEvent::GenerationStarted {
                history_id: completed.id.clone()
            },
            AppEvent::GenerationDone {
                history_id: completed.id
            },
            AppEvent::BalanceUpdated,
        ]
    );
}

#[tokio::test]
async fn connection_checks_typed_key_without_overwriting_saved_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer new-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"object":"list","data":[]})))
        .expect(1)
        .mount(&server)
        .await;
    let tmp = TempDir::new().unwrap();
    let (state, _) = state_for(&server, &tmp, Some("old-key"));
    assert!(
        koteyye_media_studio::commands::cmd_check_key(&state, Some("  new-key  "))
            .await
            .unwrap()
    );
    assert_eq!(
        state
            .key_store
            .get(Provider::CheaperInference)
            .unwrap()
            .as_deref(),
        Some("old-key")
    );
}

#[tokio::test]
async fn balance_reports_ip_policy_denial_instead_of_a_missing_scope() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/account/balance"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error":{"code":"ip_not_allowed","message":"IP address denied by key policy"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let tmp = TempDir::new().unwrap();
    let (state, _) = state_for(&server, &tmp, Some("test"));
    let mut settings = state.storage.get_settings().unwrap();
    settings.show_real_balance = true;
    state.storage.set_settings(&settings).unwrap();
    let view = cmd_fetch_balance(&state).await.unwrap();
    assert!(view.real.is_none());
    assert_eq!(view.unavailable, Some(BalanceUnavailable::AccessRestricted));
    assert_eq!(view.local_spend_usd, 0.0);
    assert!(
        view.real_error
            .as_deref()
            .unwrap()
            .contains("ip_not_allowed")
    );
}

#[tokio::test]
async fn edit_proxy_failure_keeps_diagnostics_without_treating_trace_id_as_a_receipt() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
        .respond_with(
            ResponseTemplate::new(500)
                .insert_header("x-request-id", "req-edit-trace")
                .set_body_json(
                    json!({"error":{"code":"server_error","message":"Unexpected proxy error."}}),
                ),
        )
        .expect(1)
        .mount(&server)
        .await;
    let tmp = TempDir::new().unwrap();
    let (state, events) = state_for(&server, &tmp, Some("test-key"));
    with_folder(&state, &tmp);
    let mut request = image_request();
    request.mode = GenMode::ImageEdit;
    request.model = "minimax-h3-image".into();
    request.references.push(km_shared::Reference {
        data_url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a9qkAAAAASUVORK5CYII=".into(),
    });
    let error = cmd_generate(&state, request).await.unwrap_err();
    assert!(error.contains("HTTP 500 (server_error)"));
    assert!(error.contains("minimax-h3-image"));
    assert!(error.contains("Unexpected proxy error."));
    assert!(error.contains("req-edit-trace"));
    let records = cmd_list_history(&state, HistoryFilter::default())
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    let rec = &records[0];
    assert_eq!(rec.status, JobStatus::Failed);
    assert_eq!(rec.error.as_deref(), Some(error.as_str()));
    assert!(rec.request_id.is_none());
    assert!(rec.cost_usd.is_none());
    assert!(events.lock().unwrap().iter().any(
        |e| matches!(e, AppEvent::GenerationFailed { error: message, .. } if message == &error)
    ));
    assert!(cmd_recover_pending(&state).await.unwrap().is_empty());
    assert_eq!(received(&server).await.len(), 1);
}

#[tokio::test]
async fn provider_credentials_use_separate_entries_hosts_and_strict_validation() {
    let ci = MockServer::start().await;
    let router = MockServer::start().await;
    let tmp = TempDir::new().unwrap();
    let (mut state, _) = state_for(&ci, &tmp, None);
    state.openrouter = OpenRouterKeyClient::for_localhost(router.address().port());
    let ci_id = Provider::CheaperInference;
    let or_id = Provider::OpenRouter;
    assert!(!cmd_provider_has_key(&state, ci_id).await.unwrap());
    assert!(!cmd_check_provider_key(&state, or_id, None).await.unwrap());
    cmd_set_api_key(&state, " ci-test-value ").await.unwrap();
    assert!(!cmd_provider_has_key(&state, or_id).await.unwrap());
    cmd_set_provider_key(&state, or_id, " router-stored ")
        .await
        .unwrap();
    assert!(cmd_provider_has_key(&state, ci_id).await.unwrap());
    assert!(cmd_provider_has_key(&state, or_id).await.unwrap());
    for (wrong_slot, foreign) in [
        (ci_id, "sk-or-v1-fixture"),
        (or_id, "ci_live_fixture"),
        (or_id, "ir_live_legacy_fixture"),
    ] {
        assert!(
            !cmd_check_provider_key(&state, wrong_slot, Some(foreign))
                .await
                .unwrap()
        );
        assert!(
            cmd_set_provider_key(&state, wrong_slot, foreign)
                .await
                .is_err()
        );
    }
    assert!(received(&ci).await.is_empty());
    assert!(received(&router).await.is_empty());
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(query_param("type", "decision"))
        .and(header("authorization", "Bearer ci-test-value"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[]})))
        .expect(1)
        .mount(&ci)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/key"))
        .and(header("authorization", "Bearer router-candidate"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data":{"label":"test-only","limit_remaining":null}})),
        )
        .expect(1)
        .mount(&router)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/key"))
        .and(header("authorization", "Bearer router-stored"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&router)
        .await;
    assert!(cmd_test_connection(&state).await.unwrap());
    assert!(
        cmd_check_provider_key(&state, or_id, Some(" router-candidate "))
            .await
            .unwrap()
    );
    assert!(
        !cmd_check_provider_key(&state, or_id, Some("  "))
            .await
            .unwrap()
    );
    assert_eq!(
        state.key_store.get(or_id).unwrap().as_deref(),
        Some("router-stored")
    );

    for (status, body) in [
        (200, json!({"data":[]})),
        (200, json!({"other":true})),
        (200, json!([{}])),
        (204, Value::Null),
        (403, json!({"error":{"message":"bad test-secret"}})),
        (500, json!({"error":"test-secret"})),
        (302, Value::Null),
    ] {
        let mut response = ResponseTemplate::new(status);
        if !body.is_null() {
            response = response.set_body_json(body);
        }
        if status == 302 {
            response = response.insert_header("location", format!("{}/must-not-follow", ci.uri()));
        }
        let guard = Mock::given(method("GET"))
            .and(path("/api/v1/key"))
            .and(header("authorization", "Bearer test-secret"))
            .respond_with(response)
            .expect(1)
            .mount_as_scoped(&router)
            .await;
        let e = cmd_check_provider_key(&state, or_id, Some("test-secret"))
            .await
            .unwrap_err();
        assert!(!e.contains("test-secret"));
        if status != 200 {
            assert!(e.contains(&status.to_string()), "{e}");
        }
        drop(guard);
    }
    cmd_set_provider_key(&state, or_id, " ").await.unwrap();
    assert!(!cmd_provider_has_key(&state, or_id).await.unwrap());
    assert!(cmd_provider_has_key(&state, ci_id).await.unwrap());
    assert_eq!(received(&ci).await.len(), 1);
    assert_eq!(received(&router).await.len(), 9);
}

#[tokio::test]
async fn openrouter_workspace_never_uses_ci_keys_or_falls_back_to_ci_generation() {
    let ci = MockServer::start().await;
    let tmp = TempDir::new().unwrap();
    let (emit, _) = collector();
    let state = AppState::new(
        &tmp.path().join("test.db"),
        Box::new(UnreadableKeyStore),
        &ci.uri(),
        emit,
    )
    .unwrap();
    with_folder(&state, &tmp);
    let mut record = history_record("router-receipt", JobStatus::Pending);
    record.provider = Provider::OpenRouter;
    record.cost_usd = Some(0.125);
    state.storage.insert_history(&record).unwrap();
    let mut settings = cmd_get_settings(&state).await.unwrap();
    settings.provider = Provider::OpenRouter;
    settings.show_real_balance = true;
    cmd_set_settings(&state, settings.clone()).await.unwrap();
    assert!(
        cmd_fetch_models(&state, MediaKind::Video)
            .await
            .unwrap()
            .is_empty()
    );
    let balance = cmd_fetch_balance(&state).await.unwrap();
    assert!(balance.real.is_none());
    assert_eq!(balance.local_spend_usd, 0.125);
    assert_eq!(balance.unavailable, Some(BalanceUnavailable::KeyStore));
    assert!(
        cmd_generate(&state, image_request())
            .await
            .unwrap_err()
            .contains("OpenRouter")
    );
    assert!(
        cmd_generate_for(&state, Provider::OpenRouter, image_request())
            .await
            .unwrap_err()
            .contains("OpenRouter")
    );
    assert!(cmd_recover_pending(&state).await.unwrap().is_empty());
    settings.provider = Provider::CheaperInference;
    cmd_set_settings(&state, settings).await.unwrap();
    assert!(
        cmd_generate_for(&state, Provider::OpenRouter, image_request())
            .await
            .unwrap_err()
            .contains("OpenRouter")
    );
    assert_eq!(state.storage.pending().unwrap(), vec![record]);
    assert!(received(&ci).await.is_empty());
}

#[tokio::test]
async fn recovery_uses_each_records_origin_while_openrouter_is_active() {
    let ci = MockServer::start().await;
    let or = MockServer::start().await;
    let tmp = TempDir::new().unwrap();
    let (mut state, _) = state_for(&ci, &tmp, Some("ci-recovery-key"));
    state.openrouter = OpenRouterKeyClient::for_localhost(or.address().port());
    with_folder(&state, &tmp);
    let mut settings = cmd_get_settings(&state).await.unwrap();
    settings.provider = Provider::OpenRouter;
    cmd_set_settings(&state, settings).await.unwrap();
    cmd_set_provider_key(&state, Provider::OpenRouter, "or-recovery-key")
        .await
        .unwrap();
    let mut ours = history_record("ci", JobStatus::Pending);
    ours.request_id = Some("ci-receipt".into());
    let mut theirs = history_record("or", JobStatus::Pending);
    theirs.provider = Provider::OpenRouter;
    state.storage.insert_history(&ours).unwrap();
    state.storage.insert_history(&theirs).unwrap();
    Mock::given(method("GET")).and(path("/v1/generations/ci-receipt"))
        .and(header("authorization","Bearer ci-recovery-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id":"ci-receipt","endpoint":"/v1/images/generations","created_at":"2026-11-26T00:00:00Z","expires_at":"2026-12-03T00:00:00Z",
            "files":[{"index":0,"type":"image","content_type":"image/png","size_bytes":2,"url":format!("{}/signed/p.png",ci.uri())}]
        }))).expect(1).mount(&ci).await;
    Mock::given(method("GET"))
        .and(path("/signed/p.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hi"))
        .expect(1)
        .mount(&ci)
        .await;
    let recovered = cmd_recover_pending(&state).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].id, ours.id);
    assert_eq!(recovered[0].status, JobStatus::Completed);
    assert_eq!(recovered[0].provider, Provider::CheaperInference);
    assert_eq!(state.storage.pending().unwrap(), vec![theirs]);
    assert!(received(&or).await.is_empty());
}

#[tokio::test]
async fn a_provider_change_during_keychain_access_prevents_the_paid_post() {
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::Duration;
    struct WaitingKey {
        started: Sender<()>,
        resume: Mutex<Receiver<()>>,
    }
    impl KeyStore for WaitingKey {
        fn get(&self, provider: Provider) -> Result<Option<String>, String> {
            assert_eq!(provider, Provider::CheaperInference);
            self.started.send(()).unwrap();
            self.resume
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Ok(Some("ci-held-by-dialog".into()))
        }
        fn set(&self, _provider: Provider, _key: &str) -> Result<(), String> {
            unreachable!()
        }
        fn delete(&self, _provider: Provider) -> Result<(), String> {
            unreachable!()
        }
    }
    let server = MockServer::start().await;
    let tmp = TempDir::new().unwrap();
    let (emit, events) = collector();
    let (started, wait_started) = channel();
    let (resume, wait_resume) = channel();
    let state = Arc::new(
        AppState::new(
            &tmp.path().join("h.db"),
            Box::new(WaitingKey {
                started,
                resume: Mutex::new(wait_resume),
            }),
            &server.uri(),
            emit,
        )
        .unwrap(),
    );
    with_folder(&state, &tmp);
    let task_state = state.clone();
    let run = tokio::spawn(async move {
        cmd_generate_for(&task_state, Provider::CheaperInference, image_request()).await
    });
    tokio::task::spawn_blocking(move || wait_started.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap()
        .unwrap();
    let mut prefs = cmd_get_settings(&state).await.unwrap();
    prefs.provider = Provider::OpenRouter;
    cmd_set_settings(&state, prefs).await.unwrap();
    resume.send(()).unwrap();
    let error = run.await.unwrap().unwrap_err();
    assert!(error.contains("провайдер"), "{error}");
    assert!(
        state
            .storage
            .history(&HistoryFilter::default())
            .unwrap()
            .is_empty()
    );
    assert!(received(&server).await.is_empty());
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn balance_switches_show_only_the_requested_providers_spend() {
    let ci = MockServer::start().await;
    let or = MockServer::start().await;
    let tmp = TempDir::new().unwrap();
    let (mut state, _) = state_for(&ci, &tmp, Some("ci-fixture"));
    state.openrouter = OpenRouterKeyClient::for_localhost(or.address().port());
    cmd_set_provider_key(&state, Provider::OpenRouter, "router-fixture")
        .await
        .unwrap();

    let mut old = history_record("existing-ci", JobStatus::Completed);
    old.cost_usd = Some(0.36);
    state.storage.insert_history(&old).unwrap();
    let first = cmd_fetch_balance(&state).await.unwrap();
    let legacy_payload = json!({"real": null, "local_spend_usd":0.36});
    assert_eq!(
        serde_json::from_value::<km_shared::BalanceView>(legacy_payload).unwrap(),
        first
    );
    assert_eq!(
        (first.provider, first.local_spend_usd),
        (Provider::CheaperInference, 0.36)
    );
    let mut prefs = cmd_get_settings(&state).await.unwrap();
    prefs.provider = Provider::OpenRouter;
    cmd_set_settings(&state, prefs.clone()).await.unwrap();
    assert!(
        cmd_fetch_balance_for(&state, Provider::CheaperInference)
            .await
            .is_err()
    );
    let empty = cmd_fetch_balance_for(&state, Provider::OpenRouter)
        .await
        .unwrap();
    assert_eq!(
        (empty.provider, empty.local_spend_usd),
        (Provider::OpenRouter, 0.0)
    );
    assert!(
        empty.openrouter_credits.is_none() && empty.real.is_none() && empty.unavailable.is_none()
    );
    assert_eq!(
        serde_json::to_value(&empty).unwrap()["provider"],
        "openrouter"
    );
    let mut placed = history_record("router", JobStatus::Pending);
    placed.provider = Provider::OpenRouter;
    placed.cost_usd = Some(0.08);
    state.storage.insert_history(&placed).unwrap();
    assert_eq!(
        cmd_fetch_balance(&state).await.unwrap().local_spend_usd,
        0.08
    );
    prefs.provider = Provider::CheaperInference;
    cmd_set_settings(&state, prefs).await.unwrap();
    assert_eq!(
        cmd_fetch_balance_for(&state, Provider::CheaperInference)
            .await
            .unwrap(),
        first
    );
    assert!(received(&ci).await.is_empty() && received(&or).await.is_empty());
}

#[tokio::test]
async fn openrouter_account_credits_use_only_the_selected_wallet_and_key() {
    let ci = MockServer::start().await;
    let or = MockServer::start().await;
    let tmp = TempDir::new().unwrap();
    let (mut state, _) = state_for(&ci, &tmp, Some("ci-fixture"));
    state.openrouter = OpenRouterKeyClient::for_localhost(or.address().port());
    let mut prefs = cmd_get_settings(&state).await.unwrap();
    prefs.provider = Provider::OpenRouter;
    prefs.show_real_balance = true;
    cmd_set_settings(&state, prefs).await.unwrap();
    let mut ours = history_record("router", JobStatus::Completed);
    ours.provider = Provider::OpenRouter;
    ours.cost_usd = Some(0.08);
    state.storage.insert_history(&ours).unwrap();
    let mut legacy = history_record("ci", JobStatus::Completed);
    legacy.cost_usd = Some(0.36);
    state.storage.insert_history(&legacy).unwrap();
    let no_key = cmd_fetch_balance(&state).await.unwrap();
    assert_eq!(no_key.unavailable, Some(BalanceUnavailable::MissingKey));
    assert_eq!(no_key.local_spend_usd, 0.08);
    assert!(received(&or).await.is_empty());

    // Прежнее, ошибочно заполненное хранилище также не отправит CI ключ в OpenRouter.
    state
        .key_store
        .set(Provider::OpenRouter, "ci_live_fixture")
        .unwrap();
    let foreign = cmd_fetch_balance(&state).await.unwrap();
    assert_eq!(foreign.unavailable, Some(BalanceUnavailable::RejectedKey));
    assert!(received(&or).await.is_empty());

    let cases = [
        (
            403,
            json!({"error":{"code":403,"message":"Only management keys can perform this operation"}}),
            Some(BalanceUnavailable::PermissionDenied),
        ),
        (
            401,
            json!({"error":{"code":401,"message":"expired"}}),
            Some(BalanceUnavailable::RejectedKey),
        ),
        (
            500,
            json!({"error":{"message":"sensitive wallet metadata"}}),
            Some(BalanceUnavailable::Other),
        ),
        (
            200,
            json!({"data":{"limit_remaining":9000}}),
            Some(BalanceUnavailable::Other),
        ),
        (
            200,
            json!({"data":{"total_credits":100.5,"total_usage":25.75,"limit_remaining":9000}}),
            None,
        ),
    ];
    for (index, (status, payload, expected_problem)) in cases.into_iter().enumerate() {
        let key = format!("wallet-test-{index}");
        cmd_set_provider_key(&state, Provider::OpenRouter, &key)
            .await
            .unwrap();
        Mock::given(method("GET"))
            .and(path("/api/v1/credits"))
            .and(header("authorization", format!("Bearer {key}")))
            .respond_with(ResponseTemplate::new(status).set_body_json(payload))
            .expect(1)
            .mount(&or)
            .await;
        let view = cmd_fetch_balance_for(&state, Provider::OpenRouter)
            .await
            .unwrap();
        assert_eq!(view.provider, Provider::OpenRouter);
        assert_eq!(view.local_spend_usd, 0.08);
        assert!(view.real.is_none());
        assert_eq!(view.unavailable, expected_problem);
        if expected_problem.is_none() {
            let credits = view.openrouter_credits.unwrap();
            assert_eq!(
                (
                    credits.total_credits,
                    credits.total_usage,
                    credits.remaining_usd()
                ),
                (100.5, 25.75, 74.75)
            );
        } else {
            assert!(view.openrouter_credits.is_none());
        }
    }
    assert_eq!(received(&or).await.len(), 5);
    assert!(received(&ci).await.is_empty());

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    state.openrouter = OpenRouterKeyClient::for_localhost(listener.local_addr().unwrap().port());
    let disconnect = std::thread::spawn(move || {
        let _ = listener.accept().unwrap();
    });
    let failed = cmd_fetch_balance_for(&state, Provider::OpenRouter)
        .await
        .unwrap();
    disconnect.join().unwrap();
    assert_eq!(failed.unavailable, Some(BalanceUnavailable::Network));
    assert_eq!(failed.local_spend_usd, 0.08);
}

#[tokio::test]
async fn delayed_wallet_is_discarded_after_switching_away_back_or_replacing_the_key() {
    use std::time::Duration;
    for change_key in [false, true] {
        let ci = MockServer::start().await;
        let tmp = TempDir::new().unwrap();
        let (state, _) = state_for(&ci, &tmp, Some("previous-account"));
        let active = Provider::CheaperInference;
        let mut prefs = cmd_get_settings(&state).await.unwrap();
        prefs.show_real_balance = true;
        cmd_set_settings(&state, prefs.clone()).await.unwrap();
        let pending = Mock::given(method("GET"))
            .and(path("/v1/account/balance"))
            .and(header("authorization", "Bearer previous-account"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({
                        "currency":"USD","balance_usd":90.0,"available_usd":89.0,"reserved_usd":1.0
                    }))
                    .set_delay(Duration::from_millis(80)),
            )
            .expect(1)
            .mount_as_scoped(&ci)
            .await;
        let request = cmd_fetch_balance_for(&state, active);
        tokio::pin!(request);
        tokio::select! {
            biased;
            ()=pending.wait_until_satisfied()=>{}
            _=&mut request=>panic!("wallet returned before the server received its GET"),
            ()=tokio::time::sleep(Duration::from_secs(5))=>panic!("no wallet request"),
        }
        if change_key {
            cmd_set_provider_key(&state, active, "new-account")
                .await
                .unwrap();
        } else {
            prefs.provider = Provider::OpenRouter;
            cmd_set_settings(&state, prefs.clone()).await.unwrap();
            prefs.provider = active;
            cmd_set_settings(&state, prefs).await.unwrap();
        }
        assert!(
            request.await.is_err(),
            "old wallet must never reach the new selection"
        );
    }
}
