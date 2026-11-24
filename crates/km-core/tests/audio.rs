//! Интеграционные тесты аудио CI: озвучка, музыка и звуковые эффекты.
//!
//! Все три режима ходят в один эндпоинт `/v1/audio/speech`; ответ бинарный,
//! стоимость — в заголовке `x-ci-billed-cost-usd`. HTTP проверяется через
//! wiremock; валидация input — до запроса (ноль принятых запросов на моке).

use km_core::{CiClient, CoreError};
use km_shared::{GenMode, GenParams, GenerationRequest, MediaKind};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Бинарное mp3-тело: ID3-заголовок и кадр sync-байтами (не UTF-8).
const MP3: [u8; 14] = [
    0x49, 0x44, 0x33, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xFB, 0x90, 0x64,
];

/// Клиент, направленный на мок-сервер, с тестовым ключом из брифа.
fn client_for(server: &MockServer) -> CiClient {
    CiClient::with_base("ci_live_test", &server.uri())
}

/// Запрос озвучки с базовым промптом.
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

/// Бинарный ответ с Content-Type и заголовком стоимости.
fn audio_response(content_type: &str, cost: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_bytes(MP3)
        .insert_header("content-type", content_type)
        .insert_header("x-ci-billed-cost-usd", cost)
}

/// Принятые мок-сервером запросы.
async fn received(server: &MockServer) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .expect("мок-сервер должен записать запросы")
}

#[tokio::test]
async fn speech_sends_full_body_and_parses_binary_with_cost() {
    let server = MockServer::start().await;
    let expected_body = json!({
        "model": "eleven-v4",
        "input": "кот в шляпе",
        "voice": "alloy",
        "response_format": "mp3",
        "speed": 1.25,
    });
    // Мок совпадает только с точным телом и заголовком Bearer; expect(1) — при сбросе guard.
    let guard = Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .and(header("authorization", "Bearer ci_live_test"))
        .and(body_json(expected_body))
        .respond_with(audio_response("audio/mpeg", "0.020000"))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let mut req = speech_request();
    req.params.voice = Some("alloy".to_owned());
    req.params.response_format = Some("mp3".to_owned());
    req.params.speed = Some(1.25);
    let output = client_for(&server)
        .speech(&req)
        .await
        .expect("озвучка с валидным телом должна разобрать бинарный ответ");

    assert_eq!(
        output.bytes,
        MP3.to_vec(),
        "байты тела должны вернуться как есть"
    );
    assert_eq!(
        output.content_type, "audio/mpeg",
        "content_type из заголовка"
    );
    assert_eq!(
        output.cost_usd,
        Some(0.02),
        "стоимость из заголовка строкой парсится в число"
    );

    drop(guard);
}

#[tokio::test]
async fn speech_defaults_format_to_mp3_and_omits_absent_params() {
    let server = MockServer::start().await;
    // Голос, скорость и формат не заданы — в теле только model, input и
    // response_format по умолчанию "mp3"; ключей voice/speed нет.
    let guard = Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .and(body_json(json!({
            "model": "eleven-v4",
            "input": "кот в шляпе",
            "response_format": "mp3",
        })))
        .respond_with(audio_response("audio/mpeg", "0.020000"))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    client_for(&server)
        .speech(&speech_request())
        .await
        .expect("озвучка без опциональных параметров должна пройти");

    drop(guard);
}

#[tokio::test]
async fn speech_passes_custom_response_format() {
    let server = MockServer::start().await;
    let guard = Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .and(body_json(json!({
            "model": "eleven-v4",
            "input": "кот в шляпе",
            "response_format": "wav",
        })))
        .respond_with(audio_response("audio/wav", "0.030000"))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let mut req = speech_request();
    req.params.response_format = Some("wav".to_owned());
    let output = client_for(&server)
        .speech(&req)
        .await
        .expect("формат wav должен пробрасываться в тело");
    assert_eq!(
        output.content_type, "audio/wav",
        "content_type следует заголовку, не mp3"
    );
    assert_eq!(output.cost_usd, Some(0.03));

    drop(guard);
}

#[tokio::test]
async fn music_omits_voice_speed_format_and_sends_duration() {
    let server = MockServer::start().await;
    // Музыка: voice/speed/response_format не уходят, даже если заданы в params;
    // duration_seconds пробрасывается как доп.свойство. Тот же эндпоинт.
    let guard = Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .and(body_json(json!({
            "model": "music-v2",
            "input": "кот в шляпе",
            "duration_seconds": 30.0,
        })))
        .respond_with(audio_response("audio/mpeg", "0.050000"))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let mut req = speech_request();
    req.mode = GenMode::Music;
    req.model = "music-v2".to_owned();
    req.params = GenParams {
        voice: Some("alloy".to_owned()),
        speed: Some(1.25),
        response_format: Some("wav".to_owned()),
        duration_seconds: Some(30.0),
        ..GenParams::default()
    };
    let output = client_for(&server)
        .speech(&req)
        .await
        .expect("музыка должна маппиться на тот же эндпоинт без voice");
    assert_eq!(output.bytes, MP3.to_vec());
    assert_eq!(output.cost_usd, Some(0.05));

    drop(guard);
}

#[tokio::test]
async fn sound_effects_maps_to_same_endpoint_minimal_body() {
    let server = MockServer::start().await;
    // Звуковые эффекты без длительности — тело только model и input, без voice.
    let guard = Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .and(body_json(json!({
            "model": "sound-effects-v2",
            "input": "кот в шляпе",
        })))
        .respond_with(audio_response("audio/mpeg", "0.010000"))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let mut req = speech_request();
    req.mode = GenMode::SoundEffects;
    req.model = "sound-effects-v2".to_owned();
    req.params.voice = Some("alloy".to_owned());
    client_for(&server)
        .speech(&req)
        .await
        .expect("звуковые эффекты должны маппиться на тот же эндпоинт");

    drop(guard);
}

#[tokio::test]
async fn input_over_4096_chars_fails_before_http() {
    let server = MockServer::start().await;
    let mut req = speech_request();
    // Кириллица: 4097 символов больше 4097 байт — лимит считается в символах.
    req.prompt = "к".repeat(4097);

    let err = client_for(&server)
        .speech(&req)
        .await
        .expect_err("input длиннее 4096 символов должен вернуть ошибку");
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
async fn input_of_4096_chars_is_accepted() {
    let server = MockServer::start().await;
    let prompt = "к".repeat(4096);
    let guard = Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .and(body_json(json!({
            "model": "eleven-v4",
            "input": prompt,
            "response_format": "mp3",
        })))
        .respond_with(audio_response("audio/mpeg", "0.020000"))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let mut req = speech_request();
    req.prompt = prompt;
    client_for(&server)
        .speech(&req)
        .await
        .expect("input ровно в 4096 символов допустим");

    drop(guard);
}

#[tokio::test]
async fn empty_input_fails_before_http() {
    let server = MockServer::start().await;
    let client = client_for(&server);

    for prompt in ["", "   \n\t"] {
        let mut req = speech_request();
        req.prompt = prompt.to_owned();
        let err = client
            .speech(&req)
            .await
            .expect_err("пустой input должен вернуть ошибку валидации");
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
async fn non_audio_mode_fails_before_http() {
    let server = MockServer::start().await;
    let mut req = speech_request();
    req.mode = GenMode::TextToImage;

    let err = client_for(&server)
        .speech(&req)
        .await
        .expect_err("не-аудио режим должен вернуть ошибку валидации");
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
async fn speech_http_error_envelope_propagates() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({
            "error": {"code": "insufficient_balance", "message": "not enough funds"},
        })))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .speech(&speech_request())
        .await
        .expect_err("402 должен вернуть ошибку");
    assert!(
        matches!(&err, CoreError::Http { status: 402, code: Some(code), .. }
            if code == "insufficient_balance"),
        "ожидался Http{{402, insufficient_balance}}, получено: {err:?}"
    );
}

#[tokio::test]
async fn speech_error_raw_body_falls_back_to_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(ResponseTemplate::new(500).set_body_string("upstream exploded"))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .speech(&speech_request())
        .await
        .expect_err("500 должен вернуть ошибку");
    assert!(
        matches!(&err, CoreError::Http { status: 500, code: None, message }
            if message == "upstream exploded"),
        "message должен быть сырым текстом тела, получено: {err:?}"
    );
}

#[tokio::test]
async fn missing_cost_header_yields_none() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(MP3)
                .insert_header("content-type", "audio/mpeg"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let output = client_for(&server)
        .speech(&speech_request())
        .await
        .expect("ответ без заголовка стоимости должен разобрать байты");
    assert_eq!(output.cost_usd, None, "без заголовка стоимости — None");
    assert_eq!(output.bytes, MP3.to_vec());
}

#[tokio::test]
async fn invalid_cost_header_yields_none() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(audio_response("audio/mpeg", "не-число"))
        .expect(1)
        .mount(&server)
        .await;

    let output = client_for(&server)
        .speech(&speech_request())
        .await
        .expect("нечисловая стоимость не должна ломать разбор ответа");
    assert_eq!(
        output.cost_usd, None,
        "нечисловой заголовок стоимости — None"
    );
}

#[tokio::test]
async fn missing_content_type_defaults_to_audio_mpeg() {
    let server = MockServer::start().await;
    // set_body_bytes не выставляет Content-Type — заголовка нет.
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(MP3))
        .expect(1)
        .mount(&server)
        .await;

    let output = client_for(&server)
        .speech(&speech_request())
        .await
        .expect("ответ без Content-Type должен разобрать байты");
    assert_eq!(
        output.content_type, "audio/mpeg",
        "без Content-Type — дефолт audio/mpeg для mp3"
    );
}
