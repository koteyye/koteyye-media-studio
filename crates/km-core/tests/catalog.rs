//! Интеграционные тесты каталога CI: методы клиента, кэш с TTL и фильтры.
//!
//! HTTP проверяется через wiremock; данные моделей — фикстура живого
//! `/public/models` из km-shared (10 моделей), без дублирования файла.

use std::time::Duration;

use km_core::{CatalogCache, CiClient, CoreError, by_kind, for_mode};
use km_shared::{CatalogModel, GenMode, MediaKind, ModelType};
use serde::Deserialize;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Фикстура живого каталога CI (`{"models":[...]}`, 10 моделей) из km-shared.
const FIXTURE: &str = include_str!("../../km-shared/tests/fixtures/public-models.json");

/// Обёртка ответа каталога для разбора фикстуры в тестах фильтров.
#[derive(Deserialize)]
struct FixtureEnvelope {
    models: Vec<CatalogModel>,
}

/// Парсит фикстуру в модели (порядок как в ответе CI).
fn fixture_models() -> Vec<CatalogModel> {
    serde_json::from_str::<FixtureEnvelope>(FIXTURE)
        .expect("фикстура живого каталога должна десериализоваться")
        .models
}

/// Тот же каталог в OpenAI-обёртке `/v1/models`: `{"data":[...]}`.
fn data_envelope() -> Value {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("фикстура должна быть валидным JSON");
    json!({ "data": fixture["models"] })
}

/// Клиент с тестовым ключом, направленный на мок-сервер.
fn client_for(server: &MockServer) -> CiClient {
    CiClient::with_base("ci_live_test", &server.uri())
}

/// Идентификаторы отфильтрованных моделей в порядке следования.
fn ids<'a>(models: &[&'a CatalogModel]) -> Vec<&'a str> {
    models.iter().map(|m| m.id.as_str()).collect()
}

/// Синтетическая модель для точечных проверок фильтров.
fn model(id: &str, model_type: &str, endpoints: &[&str], features: &[&str]) -> CatalogModel {
    serde_json::from_value(json!({
        "id": id,
        "model_type": model_type,
        "supported_endpoints": endpoints,
        "features": features,
    }))
    .expect("синтетическая модель должна десериализоваться")
}

#[tokio::test]
async fn public_models_parses_fixture_without_authorization() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FIXTURE))
        .expect(1)
        .mount(&server)
        .await;

    // Клиент без ключа: публичный каталог не требует авторизации.
    let client = CiClient::with_base("", &server.uri());
    let models = client
        .public_models()
        .await
        .expect("публичный каталог должен разобрать фикстуру");

    assert_eq!(models.len(), 10, "фикстура содержит ровно 10 моделей");
    let veo = models
        .iter()
        .find(|m| m.id == "veo-3.1-generate-001")
        .expect("veo должен быть в каталоге");
    assert_eq!(veo.price_for("720P+AUDIO"), Some(0.26));

    let requests = server
        .received_requests()
        .await
        .expect("мок-сервер должен записать запросы");
    assert_eq!(requests.len(), 1, "ожидался ровно один запрос");
    assert!(
        requests[0].headers.get("authorization").is_none(),
        "публичный запрос не должен нести Authorization"
    );
}

#[tokio::test]
async fn public_models_omits_authorization_even_with_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FIXTURE))
        .expect(1)
        .mount(&server)
        .await;

    // Ключ задан, но на публичный путь он не отправляется.
    let models = client_for(&server)
        .public_models()
        .await
        .expect("публичный каталог должен работать и с ключом в клиенте");
    assert_eq!(models.len(), 10);

    let requests = server
        .received_requests()
        .await
        .expect("мок-сервер должен записать запросы");
    assert!(
        requests[0].headers.get("authorization").is_none(),
        "ключ не должен утекать в публичный запрос"
    );
}

#[tokio::test]
async fn models_sends_bearer_and_parses_data_envelope() {
    let server = MockServer::start().await;
    // Мок совпадает только с корректным Bearer; expect(1) проверяется при сбросе guard.
    let guard = Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(data_envelope()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let models = client_for(&server)
        .models()
        .await
        .expect("каталог с ключом должен разобрать обёртку data");
    assert_eq!(models.len(), 10);

    let requests = server
        .received_requests()
        .await
        .expect("мок-сервер должен записать запросы");
    assert_eq!(requests.len(), 1, "ожидался ровно один запрос");
    assert_eq!(
        requests[0]
            .headers
            .get("authorization")
            .expect("заголовок Authorization должен присутствовать")
            .to_str()
            .expect("заголовок должен быть ASCII"),
        "Bearer ci_live_test"
    );

    drop(guard);
}

#[tokio::test]
async fn both_envelope_keys_are_accepted_on_both_paths() {
    let server = MockServer::start().await;
    // CI может прислать любой из ключей на любом из путей — поддержаны оба.
    Mock::given(method("GET"))
        .and(path("/public/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(data_envelope()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FIXTURE))
        .mount(&server)
        .await;

    let client = client_for(&server);
    assert_eq!(
        client
            .public_models()
            .await
            .expect("public должен принять обёртку data")
            .len(),
        10
    );
    assert_eq!(
        client
            .models()
            .await
            .expect("v1 должен принять обёртку models")
            .len(),
        10
    );
}

#[tokio::test]
async fn catalog_without_models_or_data_maps_to_serde() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"object": "list"})))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .public_models()
        .await
        .expect_err("ответ без models и data должен вернуть ошибку");
    assert!(
        matches!(err, CoreError::Serde(_)),
        "ожидался Serde, получено: {err:?}"
    );
}

#[tokio::test]
async fn cache_second_call_hits_cache_without_http() {
    let server = MockServer::start().await;
    let guard = Mock::given(method("GET"))
        .and(path("/public/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FIXTURE))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let client = client_for(&server);
    let cache = CatalogCache::new(Duration::from_secs(60));
    let first = cache
        .get_or_fetch(&client)
        .await
        .expect("первый вызов должен загрузить каталог");
    let second = cache
        .get_or_fetch(&client)
        .await
        .expect("второй вызов должен отдать кэш");

    assert_eq!(first.len(), 10);
    assert_eq!(first, second, "кэш должен отдать те же модели");

    let requests = server
        .received_requests()
        .await
        .expect("мок-сервер должен записать запросы");
    assert_eq!(requests.len(), 1, "второй вызов не должен идти в сеть");

    drop(guard);
}

#[tokio::test]
async fn cache_zero_ttl_refetches_each_call() {
    let server = MockServer::start().await;
    let guard = Mock::given(method("GET"))
        .and(path("/public/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FIXTURE))
        .expect(2)
        .mount_as_scoped(&server)
        .await;

    let client = client_for(&server);
    let cache = CatalogCache::new(Duration::ZERO);
    cache
        .get_or_fetch(&client)
        .await
        .expect("первый вызов должен загрузить каталог");
    cache
        .get_or_fetch(&client)
        .await
        .expect("TTL=0: второй вызов должен снова пойти в сеть");

    let requests = server
        .received_requests()
        .await
        .expect("мок-сервер должен записать запросы");
    assert_eq!(requests.len(), 2, "TTL=0 означает рефетч на каждый вызов");

    drop(guard);
}

#[test]
fn by_kind_audio_includes_music_and_sound_effects() {
    let models = fixture_models();
    let audio = by_kind(&models, MediaKind::Audio);
    assert_eq!(
        ids(&audio),
        ["eleven-v4", "music-v2", "sound-effects-v2", "tts-1"],
        "Audio включает все speech-модели, в том числе music-v2 и sound-effects-v2"
    );
}

#[test]
fn by_kind_image_and_video_select_by_model_type() {
    let models = fixture_models();
    assert_eq!(
        ids(&by_kind(&models, MediaKind::Image)),
        ["flux-2-pro", "seedream-5.0-pro", "gpt-image-2"]
    );
    assert_eq!(
        ids(&by_kind(&models, MediaKind::Video)),
        ["kling-v3.0-i2v", "veo-3.1-generate-001", "wan-v2.7-r2v"]
    );
}

#[test]
fn for_mode_image_edit_only_models_with_edits_endpoint() {
    let models = fixture_models();
    assert_eq!(
        ids(&for_mode(&models, GenMode::ImageEdit)),
        ["flux-2-pro", "seedream-5.0-pro", "gpt-image-2"],
        "ImageEdit включает только image-модели с /v1/images/edits"
    );
}

#[test]
fn for_mode_text_modes_select_by_model_type() {
    let models = fixture_models();
    let t2i = for_mode(&models, GenMode::TextToImage);
    assert_eq!(ids(&t2i), ["flux-2-pro", "seedream-5.0-pro", "gpt-image-2"]);
    assert!(
        t2i.iter().all(|m| m.model_type == ModelType::Image),
        "TextToImage — только image-тип, без video/speech"
    );
    let t2v = for_mode(&models, GenMode::TextToVideo);
    assert_eq!(
        ids(&t2v),
        ["kling-v3.0-i2v", "veo-3.1-generate-001", "wan-v2.7-r2v"]
    );
    assert!(
        t2v.iter().all(|m| m.model_type == ModelType::Video),
        "TextToVideo — только video-тип"
    );
}

#[test]
fn for_mode_image_to_video_is_video_only_with_image_input() {
    let models = fixture_models();
    let got = for_mode(&models, GenMode::ImageToVideo);
    assert_eq!(
        ids(&got),
        ["kling-v3.0-i2v", "veo-3.1-generate-001", "wan-v2.7-r2v"],
        "ImageToVideo — только video-модели с image-input"
    );
    // Регрессия: image-модели тоже имеют фичу image-input, но не должны попадать в i2v.
    assert!(
        !got.iter().any(|m| m.id == "seedream-5.0-pro"),
        "seedream-5.0-pro — image-тип, не должен попадать в ImageToVideo"
    );
    assert!(
        !got.iter().any(|m| m.id == "gpt-image-2"),
        "gpt-image-2 — image-тип, не должен попадать в ImageToVideo"
    );
}

#[test]
fn for_mode_image_edit_requires_image_type() {
    // Регрессия: эндпоинт edits сам по себе не делает модель image-редактором.
    let models = vec![
        model("flux-2-pro", "image", &["/v1/images/edits"], &[]),
        model("video-with-edits", "video", &["/v1/images/edits"], &[]),
    ];
    assert_eq!(
        ids(&for_mode(&models, GenMode::ImageEdit)),
        ["flux-2-pro"],
        "ImageEdit требует image-тип, а не только эндпоинт edits"
    );
}

#[test]
fn for_mode_reference_to_video_accepts_r2v_id_or_image_input() {
    let models = vec![
        // r2v в id без feature image-input — подходит по маркеру id.
        model("wan-v2.7-r2v", "video", &["/v1/videos/generations"], &[]),
        // feature image-input — подходит по возможности.
        model(
            "kling-v3.0-i2v",
            "video",
            &["/v1/videos/generations"],
            &["image-input"],
        ),
        // Ни r2v, ни image-input — не подходит.
        model("plain-t2v", "video", &["/v1/videos/generations"], &[]),
        // Не video-тип — не подходит, даже с image-input.
        model(
            "flux-2-pro",
            "image",
            &["/v1/images/generations"],
            &["image-input"],
        ),
    ];
    assert_eq!(
        ids(&for_mode(&models, GenMode::ReferenceToVideo)),
        ["wan-v2.7-r2v", "kling-v3.0-i2v"]
    );
}

#[test]
fn for_mode_reference_to_video_on_fixture_excludes_image_models() {
    let models = fixture_models();
    let got = for_mode(&models, GenMode::ReferenceToVideo);
    assert_eq!(
        ids(&got),
        ["kling-v3.0-i2v", "veo-3.1-generate-001", "wan-v2.7-r2v"],
        "ReferenceToVideo — video-модели с image-input или r2v в id"
    );
    assert!(
        got.iter().any(|m| m.id == "wan-v2.7-r2v"),
        "wan-v2.7-r2v подходит по маркеру r2v в id"
    );
    assert!(
        got.iter().all(|m| m.model_type == ModelType::Video),
        "никаких image-моделей в ReferenceToVideo"
    );
}

#[test]
fn for_mode_audio_modes_split_speech_music_and_effects() {
    let models = fixture_models();
    let speech = for_mode(&models, GenMode::Speech);
    assert_eq!(
        ids(&speech),
        ["eleven-v4", "tts-1"],
        "Speech — озвучка, без музыки и эффектов"
    );
    assert!(
        !speech.iter().any(|m| m.id == "music-v2"),
        "music-v2 не должен попадать в Speech"
    );
    assert!(
        !speech.iter().any(|m| m.id == "sound-effects-v2"),
        "sound-effects-v2 не должен попадать в Speech"
    );
    assert_eq!(ids(&for_mode(&models, GenMode::Music)), ["music-v2"]);
    assert_eq!(
        ids(&for_mode(&models, GenMode::SoundEffects)),
        ["sound-effects-v2"]
    );
}
