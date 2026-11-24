//! Интеграционные тесты видео CI: генерация (t2v/i2v/r2v), recovery, скачивание.
//!
//! HTTP проверяется через wiremock; валидация — до запроса (на мок-сервере
//! не должно остаться принятых запросов). Таймаут проверяется живо: мок
//! задерживает ответ дольше короткого клиентского таймаута.

use std::time::Duration;

use km_core::{CiClient, CoreError};
use km_shared::{GenMode, GenParams, GenerationRequest, MediaKind, Reference};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Клиент, направленный на мок-сервер, с тестовым ключом из брифа.
fn client_for(server: &MockServer) -> CiClient {
    CiClient::with_base("ci_live_test", &server.uri())
}

/// Запрос текст → видео с базовым промптом.
fn video_request() -> GenerationRequest {
    GenerationRequest {
        kind: MediaKind::Video,
        mode: GenMode::TextToVideo,
        model: "veo-3.1-generate-001".to_owned(),
        prompt: "кот плывёт на плоту".to_owned(),
        params: GenParams::default(),
        references: Vec::new(),
        mask_data_url: None,
    }
}

/// Референс-изображение как data-URL.
fn reference(data_url: &str) -> Reference {
    Reference {
        data_url: data_url.to_owned(),
    }
}

/// Ответ CI с готовым видео и тарификацией строкой.
fn completed_video_response() -> Value {
    json!({
        "id": "vid_123",
        "status": "completed",
        "videos": [{"url": "https://cdn.example.com/v.mp4", "content_type": "video/mp4"}],
        "request_id": "req_123",
        "billing": {"billed_cost_usd": "0.600000"},
    })
}

/// Завершённая генерация с одним видео-файлом (recovery-ответ CI).
fn generation_json() -> Value {
    json!({
        "request_id": "uuid",
        "endpoint": "/v1/videos/generations",
        "created_at": "2026-11-24T10:00:00Z",
        "expires_at": "2026-12-01T10:00:00Z",
        "files": [{
            "index": 0,
            "type": "video",
            "content_type": "video/mp4",
            "size_bytes": 1_048_576,
            "url": "https://cdn.example.com/signed/v.mp4?sig=abc",
        }],
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
async fn generate_t2v_sends_params_and_parses_completed_response() {
    let server = MockServer::start().await;
    let expected_body = json!({
        "model": "veo-3.1-generate-001",
        "prompt": "кот плывёт на плоту",
        "duration_seconds": 8.0,
        "resolution": "720P",
        "aspect_ratio": "16:9",
        "audio": true,
        "n": 1,
    });
    // Мок совпадает только с точным телом; expect(1) проверяется при сбросе guard.
    let guard = Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .and(header("authorization", "Bearer ci_live_test"))
        .and(body_json(expected_body))
        .respond_with(ResponseTemplate::new(200).set_body_json(completed_video_response()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let mut req = video_request();
    req.params = GenParams {
        duration_seconds: Some(8.0),
        resolution: Some("720P".to_owned()),
        aspect_ratio: Some("16:9".to_owned()),
        audio: Some(true),
        n: Some(1),
        ..GenParams::default()
    };
    let response = client_for(&server)
        .generate_video(&req)
        .await
        .expect("генерация с валидным телом должна разобрать ответ");

    assert_eq!(response.id, "vid_123");
    assert_eq!(response.status, "completed");
    assert_eq!(response.videos.len(), 1, "ответ содержит одно видео");
    assert_eq!(
        response.videos[0].url.as_deref(),
        Some("https://cdn.example.com/v.mp4")
    );
    assert_eq!(response.request_id.as_deref(), Some("req_123"));
    assert_eq!(
        response.billing.as_ref().and_then(|b| b.cost_usd()),
        Some(0.6),
        "billing приходит строкой и парсится в число"
    );

    drop(guard);
}

#[tokio::test]
async fn generate_t2v_omits_absent_params_and_image_fields() {
    let server = MockServer::start().await;
    // Параметры не заданы — в теле только model и prompt: без null-полей
    // и без image_url/input_references (t2v не использует картинки).
    let guard = Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .and(body_json(json!({
            "model": "veo-3.1-generate-001",
            "prompt": "кот плывёт на плоту",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(completed_video_response()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    client_for(&server)
        .generate_video(&video_request())
        .await
        .expect("запрос без опциональных параметров должен пройти");

    drop(guard);
}

#[tokio::test]
async fn generate_i2v_sends_first_reference_as_image_url() {
    let server = MockServer::start().await;
    // Первый референс — первый кадр (image_url); второй в тело не попадает,
    // ключ input_references отсутствует.
    let guard = Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .and(header("authorization", "Bearer ci_live_test"))
        .and(body_json(json!({
            "model": "kling-v3.0-i2v",
            "prompt": "кот плывёт на плоту",
            "image_url": "data:image/png;base64,aGk=",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(completed_video_response()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let mut req = video_request();
    req.mode = GenMode::ImageToVideo;
    req.model = "kling-v3.0-i2v".to_owned();
    req.references = vec![
        reference("data:image/png;base64,aGk="),
        reference("data:image/png;base64,b2k="),
    ];
    client_for(&server)
        .generate_video(&req)
        .await
        .expect("i2v должен отправить первый референс как image_url");

    drop(guard);
}

#[tokio::test]
async fn generate_r2v_sends_all_references_as_input_references() {
    let server = MockServer::start().await;
    // Все референсы уходят строками input_references; image_url отсутствует.
    let guard = Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .and(body_json(json!({
            "model": "wan-v2.7-r2v",
            "prompt": "кот плывёт на плоту",
            "input_references": [
                "data:image/png;base64,aGk=",
                "data:image/png;base64,b2k=",
            ],
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(completed_video_response()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let mut req = video_request();
    req.mode = GenMode::ReferenceToVideo;
    req.model = "wan-v2.7-r2v".to_owned();
    req.references = vec![
        reference("data:image/png;base64,aGk="),
        reference("data:image/png;base64,b2k="),
    ];
    client_for(&server)
        .generate_video(&req)
        .await
        .expect("r2v должен отправить все референсы как input_references");

    drop(guard);
}

#[tokio::test]
async fn generation_fetches_files_with_authorization() {
    let server = MockServer::start().await;
    let guard = Mock::given(method("GET"))
        .and(path("/v1/generations/uuid"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(generation_json()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let generation = client_for(&server)
        .generation("uuid")
        .await
        .expect("recovery должен разобрать файлы генерации");

    assert_eq!(generation.request_id, "uuid");
    assert_eq!(generation.endpoint, "/v1/videos/generations");
    assert_eq!(generation.files.len(), 1, "генерация содержит один файл");
    let file = &generation.files[0];
    assert_eq!(file.index, 0);
    assert_eq!(file.kind, "video");
    assert_eq!(file.content_type, "video/mp4");
    assert_eq!(file.size_bytes, 1_048_576);
    assert_eq!(file.url, "https://cdn.example.com/signed/v.mp4?sig=abc");

    // Явная проверка заголовка по фактически принятому запросу.
    let requests = received(&server).await;
    assert_eq!(requests.len(), 1, "ожидался ровно один запрос");
    let auth = requests[0]
        .headers
        .get("authorization")
        .expect("recovery-запрос должен нести Authorization");
    assert_eq!(
        auth.to_str().expect("заголовок должен быть ASCII"),
        "Bearer ci_live_test"
    );

    drop(guard);
}

#[tokio::test]
async fn download_signed_video_url_without_authorization() {
    let server = MockServer::start().await;
    let payload = b"binary video bytes";
    Mock::given(method("GET"))
        .and(path("/signed/v.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload))
        .expect(1)
        .mount(&server)
        .await;

    let url = format!("{}/signed/v.mp4?expires=123&sig=abc", server.uri());
    let bytes = client_for(&server)
        .download(&url)
        .await
        .expect("signed URL должен отдавать байты видео");
    assert_eq!(bytes, payload);

    let requests = received(&server).await;
    assert_eq!(requests.len(), 1, "ожидался ровно один запрос");
    assert!(
        requests[0].headers.get("authorization").is_none(),
        "ключ не должен утекать в запрос к signed URL"
    );
}

#[tokio::test]
async fn generate_video_times_out_on_slow_response() {
    let server = MockServer::start().await;
    // Мок задерживает ответ на 500 мс — дольше клиентского таймаута 100 мс.
    Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(completed_video_response())
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&server)
        .await;

    let client = client_for(&server).with_timeout(Duration::from_millis(100));
    let err = client
        .generate_video(&video_request())
        .await
        .expect_err("медленный ответ должен превысить таймаут");
    assert!(
        matches!(err, CoreError::Timeout),
        "ожидался Timeout, получено: {err:?}"
    );
}

#[tokio::test]
async fn generation_times_out_on_slow_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/generations/uuid"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(generation_json())
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&server)
        .await;

    let client = client_for(&server).with_timeout(Duration::from_millis(100));
    let err = client
        .generation("uuid")
        .await
        .expect_err("медленный recovery-ответ должен превысить таймаут");
    assert!(
        matches!(err, CoreError::Timeout),
        "ожидался Timeout, получено: {err:?}"
    );
}

#[tokio::test]
async fn empty_prompt_fails_before_http() {
    let server = MockServer::start().await;
    let client = client_for(&server);

    for prompt in ["", "   \n\t"] {
        let mut req = video_request();
        req.prompt = prompt.to_owned();
        let err = client
            .generate_video(&req)
            .await
            .expect_err("пустой prompt должен вернуть ошибку валидации");
        assert!(
            matches!(err, CoreError::Validation(_)),
            "ожидался Validation для prompt {prompt:?}, получено: {err:?}"
        );
    }

    assert!(
        received(&server).await.is_empty(),
        "валидация должна срабатывать до HTTP-запроса"
    );
}

#[tokio::test]
async fn empty_request_id_fails_before_http() {
    let server = MockServer::start().await;
    let client = client_for(&server);

    for request_id in ["", "   "] {
        let err = client
            .generation(request_id)
            .await
            .expect_err("пустой request_id должен вернуть ошибку валидации");
        assert!(
            matches!(err, CoreError::Validation(_)),
            "ожидался Validation для request_id {request_id:?}, получено: {err:?}"
        );
    }

    assert!(
        received(&server).await.is_empty(),
        "валидация должна срабатывать до HTTP-запроса"
    );
}

#[tokio::test]
async fn i2v_and_r2v_without_references_fail_before_http() {
    let server = MockServer::start().await;
    let client = client_for(&server);

    for mode in [GenMode::ImageToVideo, GenMode::ReferenceToVideo] {
        let mut req = video_request();
        req.mode = mode;
        let err = client
            .generate_video(&req)
            .await
            .expect_err("i2v/r2v без референсов должен вернуть ошибку валидации");
        assert!(
            matches!(err, CoreError::Validation(_)),
            "ожидался Validation для режима {mode:?}, получено: {err:?}"
        );
    }

    assert!(
        received(&server).await.is_empty(),
        "валидация должна срабатывать до HTTP-запроса"
    );
}

#[tokio::test]
async fn non_video_mode_fails_before_http() {
    let server = MockServer::start().await;
    let mut req = video_request();
    req.mode = GenMode::TextToImage;

    let err = client_for(&server)
        .generate_video(&req)
        .await
        .expect_err("не-видео режим должен вернуть ошибку валидации");
    assert!(
        matches!(err, CoreError::Validation(_)),
        "ожидался Validation, получено: {err:?}"
    );
    assert!(
        received(&server).await.is_empty(),
        "валидация должна срабатывать до HTTP-запроса"
    );
}

#[tokio::test]
async fn generate_video_http_error_propagates() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/videos/generations"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({
            "error": {"code": "insufficient_balance", "message": "not enough funds"},
        })))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .generate_video(&video_request())
        .await
        .expect_err("402 должен вернуть ошибку");
    assert!(
        matches!(&err, CoreError::Http { status: 402, code: Some(code), .. }
            if code == "insufficient_balance"),
        "ожидался Http{{402, insufficient_balance}}, получено: {err:?}"
    );
}

#[tokio::test]
async fn generation_unknown_request_id_maps_to_http_404() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/generations/unknown"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {"code": "not_found", "message": "generation not found"},
        })))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .generation("unknown")
        .await
        .expect_err("неизвестный request_id должен вернуть ошибку");
    assert!(
        matches!(err, CoreError::Http { status: 404, .. }),
        "ожидался Http{{404}}, получено: {err:?}"
    );
}
