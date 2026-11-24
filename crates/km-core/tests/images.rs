//! Интеграционные тесты изображений CI: генерация, редактирование, байты.
//!
//! HTTP проверяется через wiremock; валидация проверяется до запроса —
//! на мок-сервере не должно остаться ни одного принятого запроса.

use km_core::{CiClient, CoreError, image_bytes};
use km_shared::{GenMode, GenParams, GenerationRequest, ImageItem, MediaKind, Reference};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Клиент, направленный на мок-сервер, с тестовым ключом из брифа.
fn client_for(server: &MockServer) -> CiClient {
    CiClient::with_base("ci_live_test", &server.uri())
}

/// Запрос текст → изображение с базовым промптом.
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

/// Запрос редактирования с одним референсом и PNG-маской.
fn edit_request() -> GenerationRequest {
    GenerationRequest {
        mode: GenMode::ImageEdit,
        prompt: "добавь коту шляпу".to_owned(),
        params: GenParams {
            n: Some(1),
            size: Some("1024x1024".to_owned()),
            ..GenParams::default()
        },
        references: vec![Reference {
            data_url: "data:image/png;base64,aGk=".to_owned(),
        }],
        mask_data_url: Some("data:image/png;base64,bWFzaw==".to_owned()),
        ..image_request()
    }
}

/// Ответ CI с двумя base64-изображениями и тарификацией строкой.
fn two_images_response() -> Value {
    json!({
        "created": 1_730_000_000,
        "data": [{"b64_json": "aGk="}, {"b64_json": "aGk="}],
        "request_id": "img_123",
        "billing": {"billed_cost_usd": "0.040000"},
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
async fn generate_sends_model_prompt_n_size_and_parses_two_items() {
    let server = MockServer::start().await;
    let expected_body = json!({
        "model": "flux-2-pro",
        "prompt": "кот в шляпе",
        "n": 2,
        "size": "1024x1024",
    });
    // Мок совпадает только с точным телом; expect(1) проверяется при сбросе guard.
    let guard = Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .and(header("authorization", "Bearer ci_live_test"))
        .and(body_json(expected_body))
        .respond_with(ResponseTemplate::new(200).set_body_json(two_images_response()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let mut req = image_request();
    req.params.n = Some(2);
    req.params.size = Some("1024x1024".to_owned());
    let response = client_for(&server)
        .generate_image(&req)
        .await
        .expect("генерация с валидным телом должна разобрать ответ");

    assert_eq!(response.data.len(), 2, "ответ содержит два изображения");
    assert_eq!(response.request_id.as_deref(), Some("img_123"));
    assert_eq!(
        response.billing.as_ref().and_then(|b| b.cost_usd()),
        Some(0.04),
        "billing приходит строкой и парсится в число"
    );

    drop(guard);
}

#[tokio::test]
async fn generate_omits_absent_optional_params() {
    let server = MockServer::start().await;
    // Параметры не заданы — в теле только model и prompt, без null-полей.
    let guard = Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .and(body_json(
            json!({"model": "flux-2-pro", "prompt": "кот в шляпе"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(two_images_response()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    client_for(&server)
        .generate_image(&image_request())
        .await
        .expect("запрос без опциональных параметров должен пройти");

    drop(guard);
}

#[tokio::test]
async fn generate_passes_quality_and_response_format() {
    let server = MockServer::start().await;
    let guard = Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .and(body_json(json!({
            "model": "flux-2-pro",
            "prompt": "кот в шляпе",
            "quality": "high",
            "response_format": "url",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"url": "https://cdn.example.com/img.png"}],
        })))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let mut req = image_request();
    req.params.quality = Some("high".to_owned());
    req.params.response_format = Some("url".to_owned());
    let response = client_for(&server)
        .generate_image(&req)
        .await
        .expect("quality и response_format должны пробрасываться в тело");
    assert_eq!(
        response.data[0].url.as_deref(),
        Some("https://cdn.example.com/img.png")
    );

    drop(guard);
}

#[tokio::test]
async fn edit_sends_references_and_mask() {
    let server = MockServer::start().await;
    let expected_body = json!({
        "model": "flux-2-pro",
        "prompt": "добавь коту шляпу",
        "input_references": ["data:image/png;base64,aGk="],
        "mask_reference": "data:image/png;base64,bWFzaw==",
        "n": 1,
        "size": "1024x1024",
    });
    let guard = Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
        .and(header("authorization", "Bearer ci_live_test"))
        .and(body_json(expected_body))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"b64_json": "aGk="}],
            "billing": {"billed_cost_usd": "0.020000"},
        })))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let response = client_for(&server)
        .edit_image(&edit_request())
        .await
        .expect("редактирование с референсом и маской должно разобрать ответ");
    assert_eq!(response.data.len(), 1);

    drop(guard);
}

#[tokio::test]
async fn edit_without_mask_omits_mask_reference_and_extra_params() {
    let server = MockServer::start().await;
    // Маски нет — ключ mask_reference отсутствует; quality/response_format
    // в edits не пробрасываются (маппинг брифа).
    let guard = Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
        .and(body_json(json!({
            "model": "flux-2-pro",
            "prompt": "добавь коту шляпу",
            "input_references": ["data:image/png;base64,aGk="],
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(two_images_response()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let mut req = edit_request();
    req.mask_data_url = None;
    req.params = GenParams {
        quality: Some("high".to_owned()),
        response_format: Some("b64_json".to_owned()),
        ..GenParams::default()
    };
    client_for(&server)
        .edit_image(&req)
        .await
        .expect("редактирование без маски должно пройти");

    drop(guard);
}

#[tokio::test]
async fn generate_http_error_propagates() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({
            "error": {"code": "insufficient_balance", "message": "not enough funds"},
        })))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .generate_image(&image_request())
        .await
        .expect_err("402 должен вернуть ошибку");
    assert!(
        matches!(&err, CoreError::Http { status: 402, code: Some(code), .. }
            if code == "insufficient_balance"),
        "ожидался Http{{402, insufficient_balance}}, получено: {err:?}"
    );
}

#[tokio::test]
async fn image_bytes_decodes_b64_without_http() {
    let server = MockServer::start().await;
    let item = ImageItem {
        b64_json: Some("aGk=".to_owned()),
        url: None,
        content_type: None,
    };

    let bytes = image_bytes(&item, &client_for(&server))
        .await
        .expect("base64 должен декодироваться без сети");
    assert_eq!(bytes, b"hi");
    assert!(
        received(&server).await.is_empty(),
        "декодирование base64 не должно ходить в сеть"
    );
}

#[tokio::test]
async fn image_bytes_downloads_url_without_authorization() {
    let server = MockServer::start().await;
    let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    Mock::given(method("GET"))
        .and(path("/signed/img.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png))
        .expect(1)
        .mount(&server)
        .await;

    let item = ImageItem {
        b64_json: None,
        url: Some(format!("{}/signed/img.png?sig=abc", server.uri())),
        content_type: Some("image/png".to_owned()),
    };
    let bytes = image_bytes(&item, &client_for(&server))
        .await
        .expect("url должен скачиваться через мок");
    assert_eq!(bytes, png);

    let requests = received(&server).await;
    assert_eq!(requests.len(), 1, "ожидался ровно один запрос");
    assert!(
        requests[0].headers.get("authorization").is_none(),
        "signed URL не должен нести Authorization"
    );
}

#[tokio::test]
async fn image_bytes_prefers_b64_over_url() {
    let server = MockServer::start().await;
    // Оба поля заданы — base64 выигрывает, сеть не нужна.
    let item = ImageItem {
        b64_json: Some("aGk=".to_owned()),
        url: Some(format!("{}/signed/img.png", server.uri())),
        content_type: None,
    };

    let bytes = image_bytes(&item, &client_for(&server))
        .await
        .expect("при обоих полях должен декодироваться base64");
    assert_eq!(bytes, b"hi");
    assert!(
        received(&server).await.is_empty(),
        "base64 не должен ходить в сеть при заданном url"
    );
}

#[tokio::test]
async fn image_bytes_invalid_base64_maps_to_serde() {
    let server = MockServer::start().await;
    let item = ImageItem {
        b64_json: Some("!!!не base64!!!".to_owned()),
        url: None,
        content_type: None,
    };

    let err = image_bytes(&item, &client_for(&server))
        .await
        .expect_err("невалидный base64 должен вернуть ошибку");
    assert!(
        matches!(err, CoreError::Serde(_)),
        "ожидался Serde, получено: {err:?}"
    );
}

#[tokio::test]
async fn image_bytes_empty_item_maps_to_serde() {
    let server = MockServer::start().await;
    let item = ImageItem {
        b64_json: None,
        url: None,
        content_type: None,
    };

    let err = image_bytes(&item, &client_for(&server))
        .await
        .expect_err("элемент без b64_json и url должен вернуть ошибку");
    assert!(
        matches!(err, CoreError::Serde(_)),
        "ожидался Serde, получено: {err:?}"
    );
}

#[tokio::test]
async fn download_fetches_signed_url_without_authorization() {
    let server = MockServer::start().await;
    let payload = b"binary image bytes";
    Mock::given(method("GET"))
        .and(path("/files/img.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload))
        .expect(1)
        .mount(&server)
        .await;

    let url = format!("{}/files/img.png?expires=123&sig=abc", server.uri());
    let bytes = client_for(&server)
        .download(&url)
        .await
        .expect("signed URL должен отдавать байты");
    assert_eq!(bytes, payload);

    let requests = received(&server).await;
    assert_eq!(requests.len(), 1, "ожидался ровно один запрос");
    assert!(
        requests[0].headers.get("authorization").is_none(),
        "ключ не должен утекать в запрос к signed URL"
    );
}

#[tokio::test]
async fn download_error_status_maps_to_http() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/files/expired.png"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let url = format!("{}/files/expired.png", server.uri());
    let err = client_for(&server)
        .download(&url)
        .await
        .expect_err("протухший signed URL должен вернуть ошибку");
    assert!(
        matches!(err, CoreError::Http { status: 404, .. }),
        "ожидался Http{{404}}, получено: {err:?}"
    );
}

#[tokio::test]
async fn empty_prompt_fails_before_http() {
    let server = MockServer::start().await;
    let client = client_for(&server);

    for prompt in ["", "   \n\t"] {
        let mut req = image_request();
        req.prompt = prompt.to_owned();
        let err = client
            .generate_image(&req)
            .await
            .expect_err("пустой prompt должен вернуть ошибку валидации");
        assert!(
            matches!(err, CoreError::Validation(_)),
            "ожидался Validation для prompt {prompt:?}, получено: {err:?}"
        );

        let mut edit = edit_request();
        edit.prompt = prompt.to_owned();
        let err = client
            .edit_image(&edit)
            .await
            .expect_err("пустой prompt в edits должен вернуть ошибку валидации");
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
async fn n_out_of_range_fails_before_http() {
    let server = MockServer::start().await;
    let client = client_for(&server);

    for n in [0, 11] {
        let mut req = image_request();
        req.params.n = Some(n);
        let err = client
            .generate_image(&req)
            .await
            .expect_err("n вне 1–10 должен вернуть ошибку валидации");
        assert!(
            matches!(err, CoreError::Validation(_)),
            "ожидался Validation для n={n}, получено: {err:?}"
        );

        let mut edit = edit_request();
        edit.params.n = Some(n);
        let err = client
            .edit_image(&edit)
            .await
            .expect_err("n вне 1–10 в edits должен вернуть ошибку валидации");
        assert!(
            matches!(err, CoreError::Validation(_)),
            "ожидался Validation для n={n}, получено: {err:?}"
        );
    }

    assert!(
        received(&server).await.is_empty(),
        "валидация должна срабатывать до HTTP-запроса"
    );
}

#[tokio::test]
async fn n_bounds_1_and_10_are_accepted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(two_images_response()))
        .expect(2)
        .mount(&server)
        .await;

    let client = client_for(&server);
    for n in [1, 10] {
        let mut req = image_request();
        req.params.n = Some(n);
        client
            .generate_image(&req)
            .await
            .expect("граничные значения n=1 и n=10 допустимы");
    }
    assert_eq!(received(&server).await.len(), 2);
}

#[tokio::test]
async fn edit_prompt_over_32000_chars_fails_before_http() {
    let server = MockServer::start().await;
    let mut req = edit_request();
    // Кириллица: 32001 символ — это больше 32001 байта, лимит считается в символах.
    req.prompt = "к".repeat(32_001);

    let err = client_for(&server)
        .edit_image(&req)
        .await
        .expect_err("prompt длиннее 32000 символов должен вернуть ошибку");
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
async fn edit_prompt_of_32000_chars_is_accepted() {
    let server = MockServer::start().await;
    let prompt = "к".repeat(32_000);
    let guard = Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
        .and(body_json(json!({
            "model": "flux-2-pro",
            "prompt": prompt,
            "input_references": ["data:image/png;base64,aGk="],
            "mask_reference": "data:image/png;base64,bWFzaw==",
            "n": 1,
            "size": "1024x1024",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(two_images_response()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let mut req = edit_request();
    req.prompt = prompt;
    client_for(&server)
        .edit_image(&req)
        .await
        .expect("prompt ровно в 32000 символов допустим");

    drop(guard);
}
