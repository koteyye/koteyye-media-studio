//! Интеграционные тесты изображений CI: генерация, редактирование, байты.
//!
//! HTTP проверяется через wiremock; валидация проверяется до запроса —
//! на мок-сервере не должно остаться ни одного принятого запроса.

use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
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

const EDIT_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
const EDIT_WEBP: &str = "data:image/webp;base64,UklGRhwAAABXRUJQVlA4TA8AAAAvAUAAAAcQ0f/+ByKi/wEA";

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
            data_url: EDIT_PNG.to_owned(),
        }],
        mask_data_url: Some(EDIT_PNG.to_owned()),
        ..image_request()
    }
}

/// Ответ CI с двумя base64-изображениями и тарификацией строкой.
fn two_images_response() -> Value {
    let png = EDIT_PNG.split_once(',').unwrap().1;
    json!({
        "created": 1_730_000_000,
        "data": [{"b64_json": png}, {"b64_json": png}],
        "request_id": "8268670d-d239-4b05-99e9-aaf703977dab",
        "billing": {"status": "settled", "currency": "USD", "billed_cost_usd": "0.040000"},
    })
}

/// Принятые мок-сервером запросы.
async fn received(server: &MockServer) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .expect("мок-сервер должен записать запросы")
}

struct SentPart<'a> {
    headers: &'a str,
    bytes: &'a [u8],
}

/// Разделяем тело по MIME boundary из HTTP, сохраняя исходные байты каждого файла.
fn multipart_fields(request: &wiremock::Request) -> BTreeMap<&str, Vec<SentPart<'_>>> {
    fn split<'a>(bytes: &'a [u8], marker: &[u8]) -> (&'a [u8], &'a [u8]) {
        let offset = bytes
            .windows(marker.len())
            .position(|w| w == marker)
            .expect("MIME boundary");
        (&bytes[..offset], &bytes[offset + marker.len()..])
    }

    let boundary = request.headers["content-type"]
        .to_str()
        .unwrap()
        .strip_prefix("multipart/form-data; boundary=")
        .expect("multipart/form-data");
    let first = format!("--{boundary}");
    let next = format!("\r\n--{boundary}");
    let mut rest = request.body.strip_prefix(first.as_bytes()).unwrap();
    let mut fields = BTreeMap::new();
    while rest != b"--\r\n" {
        let (headers, payload) = split(rest.strip_prefix(b"\r\n").unwrap(), b"\r\n\r\n");
        let (bytes, remaining) = split(payload, next.as_bytes());
        let headers = std::str::from_utf8(headers).unwrap();
        let name = headers
            .split_once("name=\"")
            .unwrap()
            .1
            .split_once('"')
            .unwrap()
            .0;
        fields
            .entry(name)
            .or_insert_with(Vec::new)
            .push(SentPart { headers, bytes });
        rest = remaining;
    }
    assert!(!request.headers.contains_key("content-encoding"));
    assert_eq!(
        request.headers["content-length"]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        request.body.len()
    );
    fields
}

/// Синтетический 8×8 JPEG. Три допустимых сегмента COM воспроизводят размер
/// локального аватара из отчёта, без использования его изображения или метаданных.
fn synthetic_avatar() -> Vec<u8> {
    let small = STANDARD.decode("/9j/4AAQSkZJRgABAgAAAQABAAD//gAQTGF2YzYyLjI4LjEwMQD/2wBDAAgEBAQEBAUFBQUFBQYGBgYGBgYGBgYGBgYHBwcICAgHBwcGBgcHCAgICAkJCQgICAgJCQoKCgwMCwsODg4RERT/xABLAAEBAAAAAAAAAAAAAAAAAAAABgEBAAAAAAAAAAAAAAAAAAAABRABAAAAAAAAAAAAAAAAAAAAABEBAAAAAAAAAAAAAAAAAAAAAP/AABEIAAgACAMBIgACEQADEQD/2gAMAwEAAhEDEQA/ALgAKSf/2Q==").unwrap();
    let app0_end = 4 + usize::from(u16::from_be_bytes([small[4], small[5]]));
    let mut jpeg = small[..app0_end].to_vec();
    for size in [60_000, 60_000, 185_606 - 120_000 - small.len()] {
        jpeg.extend_from_slice(&[0xff, 0xfe]);
        jpeg.extend_from_slice(&u16::try_from(size - 2).unwrap().to_be_bytes());
        jpeg.resize(jpeg.len() + size - 4, b'x');
    }
    jpeg.extend_from_slice(&small[app0_end..]);
    assert_eq!(jpeg.len(), 185_606);
    jpeg
}

#[tokio::test]
async fn minimax_avatar_sends_one_binary_jpeg_and_propagates_proxy_error_without_retry() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "error":{"code":"server_error","message":"Unexpected proxy error"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let jpeg = synthetic_avatar();
    let mut req = edit_request();
    req.model = "minimax-h3-image".into();
    req.prompt = "Поменяй фон".into();
    req.references[0].data_url = format!("data:image/jpeg;base64,{}", STANDARD.encode(&jpeg));
    req.mask_data_url = None;
    let err = client_for(&server).edit_image(&req).await.unwrap_err();
    assert!(matches!(err, CoreError::Http { status: 500, .. }));
    let requests = received(&server).await;
    assert_eq!(requests.len(), 1);
    let form = multipart_fields(&requests[0]);
    assert_eq!(
        form.keys().copied().collect::<Vec<_>>(),
        ["image", "model", "n", "prompt", "size"]
    );
    for (name, value) in [
        ("model", "minimax-h3-image"),
        ("prompt", "Поменяй фон"),
        ("size", "1024x1024"),
        ("n", "1"),
    ] {
        assert_eq!(form[name].len(), 1);
        assert_eq!(form[name][0].bytes, value.as_bytes());
    }
    assert_eq!(form["image"].len(), 1);
    let image = &form["image"][0];
    assert!(image.headers.contains("filename=\"source-1.jpg\""));
    assert!(image.headers.contains("Content-Type: image/jpeg"));
    assert_eq!(image.bytes, jpeg);
}

#[tokio::test]
async fn url_only_edit_forwards_the_exact_url_in_json_without_downloading_it() {
    let api = MockServer::start().await;
    let cdn = MockServer::start().await;
    let source = format!("{}/original.jpg?sig=one%2Btwo", cdn.uri());
    let mut req = edit_request();
    req.references[0].data_url = source.clone();
    req.mask_data_url = None;
    req.params = GenParams::default();
    Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
        .and(header("content-type", "application/json"))
        .and(body_json(
            json!({"model":req.model,"prompt":req.prompt,"input_references":[source]}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(two_images_response()))
        .expect(1)
        .mount(&api)
        .await;
    client_for(&api).edit_image(&req).await.unwrap();
    let arrived = received(&api).await;
    assert_eq!(arrived.len(), 1);
    assert!(!arrived[0].headers.contains_key("content-encoding"));
    assert!(received(&cdn).await.is_empty());
}

#[tokio::test]
async fn edits_forward_ten_remote_and_inline_sources_in_order() {
    let server = MockServer::start().await;
    let mut req = edit_request();
    let mut sources = vec![
        "http://assets.example/test.jpg".to_owned(),
        EDIT_PNG.to_owned(),
    ];
    sources.extend((2..10).map(|index| format!("https://cdn.example/image-{index}.webp?sig=x")));
    req.references = sources
        .iter()
        .map(|s| Reference {
            data_url: s.clone(),
        })
        .collect();
    req.params.n = Some(10);
    Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
        .and(body_json(json!({
            "model": req.model, "prompt": req.prompt, "size": "1024x1024", "n": 10,
            "input_references": sources, "mask_reference": EDIT_PNG,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(two_images_response()))
        .expect(1)
        .mount(&server)
        .await;
    client_for(&server).edit_image(&req).await.unwrap();
    assert_eq!(received(&server).await.len(), 1);
}

#[tokio::test]
async fn edit_validates_reference_count_scheme_types_and_png_mask_before_http() {
    let server = MockServer::start().await;
    let client = client_for(&server);
    let mut req = edit_request();
    req.references.clear();
    assert!(matches!(
        client.edit_image(&req).await,
        Err(CoreError::Validation(_))
    ));
    req.references = vec![
        Reference {
            data_url: EDIT_PNG.into()
        };
        11
    ];
    assert!(matches!(
        client.edit_image(&req).await,
        Err(CoreError::Validation(_))
    ));
    for reference in [
        "",
        "file:///image.png",
        "data:image/gif;base64,R0lGODlh",
        "data:image/jpeg;base64,aGk=",
        "data:image/png;base64,!",
    ] {
        req.references = vec![Reference {
            data_url: reference.into(),
        }];
        assert!(
            matches!(client.edit_image(&req).await, Err(CoreError::Validation(_))),
            "{reference}"
        );
    }
    req.references = vec![Reference {
        data_url: EDIT_PNG.into(),
    }];
    for mask in [
        "https://cdn.example/mask.png",
        "data:image/webp;base64,UklGRgAAAABXRUJQ",
        "data:image/png;base64,/9j/",
    ] {
        req.mask_data_url = Some(mask.into());
        assert!(
            matches!(client.edit_image(&req).await, Err(CoreError::Validation(_))),
            "{mask}"
        );
    }
    assert!(received(&server).await.is_empty());
}

/// Размер нужен для проверки предела HTTP независимо от степени сжатия PNG.
fn padded_png(size: usize) -> String {
    let mut bytes = STANDARD
        .decode(EDIT_PNG.split_once(',').unwrap().1)
        .unwrap();
    bytes.resize(size, 0);
    format!("data:image/png;base64,{}", STANDARD.encode(bytes))
}

#[tokio::test]
async fn edits_send_binary_multipart_when_base64_json_exceeds_the_gateway_limit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(two_images_response()))
        .expect(1)
        .mount(&server)
        .await;
    let mut req = edit_request();
    req.references.push(Reference {
        data_url: padded_png(3_400_000),
    });
    req.params.quality = Some("high".into());
    req.params.response_format = Some("b64_json".into());
    client_for(&server).edit_image(&req).await.unwrap();
    let requests = received(&server).await;
    let request = &requests[0];
    assert!(
        request.headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("multipart/form-data; boundary=")
    );
    assert_eq!(
        request.headers["content-length"]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        request.body.len()
    );
    assert!((3_400_000..4_500_000).contains(&request.body.len()));
    let body = String::from_utf8_lossy(&request.body);
    assert_eq!(body.matches("name=\"image\"").count(), 2);
    assert_eq!(body.matches("name=\"mask\"").count(), 1);
    for (key, value) in [
        ("size", "1024x1024"),
        ("quality", "high"),
        ("response_format", "b64_json"),
        ("n", "1"),
    ] {
        assert!(
            body.contains(&format!("name=\"{key}\"\r\n\r\n{value}\r\n")),
            "{key}"
        );
    }
}

#[tokio::test]
async fn edits_count_multipart_framing_mixed_urls_and_decoded_limit_before_post() {
    let server = MockServer::start().await;
    let client = client_for(&server);
    let mut req = edit_request();
    req.mask_data_url = None;
    req.references[0].data_url = padded_png(4_500_000);
    assert!(
        matches!(client.edit_image(&req).await, Err(CoreError::Validation(e)) if e.contains("4,5"))
    );
    req.references[0].data_url = padded_png(3_400_000);
    req.references.push(Reference {
        data_url: "https://cdn.example/image.png".into(),
    });
    assert!(
        matches!(client.edit_image(&req).await, Err(CoreError::Validation(e)) if e.contains("4,5"))
    );
    req.references = vec![
        Reference {
            data_url: padded_png(2_100_000)
        };
        10
    ];
    assert!(
        matches!(client.edit_image(&req).await, Err(CoreError::Validation(e)) if e.contains("20"))
    );
    assert!(received(&server).await.is_empty());
}

#[tokio::test]
async fn image_receipt_uses_headers_when_absent_and_keeps_an_explicit_pending_cost() {
    const ID: &str = "739ac917-97b1-4b66-9269-db0f4ef130ea";
    let server = MockServer::start().await;
    let with_headers = ResponseTemplate::new(200)
        .insert_header("x-ci-request-id", ID)
        .insert_header("x-ci-billed-cost-usd", "0.030000");
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(with_headers.clone().set_body_json(json!({"data": []})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
        .respond_with(with_headers.set_body_json(json!({
            "data": [], "request_id": "bb5f30d0-b303-46d5-96b9-1032c442c135",
            "billing": {"status":"pending","currency":"USD","billed_cost_usd":null}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = client_for(&server);
    let generated = client.generate_image(&image_request()).await.unwrap();
    assert_eq!(generated.request_id.as_deref(), Some(ID));
    assert_eq!(generated.billing.unwrap().cost_usd(), Some(0.03));
    let edited = client.edit_image(&edit_request()).await.unwrap();
    assert_eq!(
        edited.request_id.as_deref(),
        Some("bb5f30d0-b303-46d5-96b9-1032c442c135")
    );
    assert_eq!(edited.billing.unwrap().cost_usd(), None);
}

#[tokio::test]
async fn broken_inline_result_falls_back_to_signed_url_without_credentials() {
    let server = MockServer::start().await;
    let png = STANDARD
        .decode(EDIT_PNG.split_once(',').unwrap().1)
        .unwrap();
    Mock::given(method("GET"))
        .and(path("/cdn/result"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png.clone()))
        .expect(1)
        .mount(&server)
        .await;
    let item = ImageItem {
        b64_json: Some("!".into()),
        url: Some(format!("{}/cdn/result?ticket=x", server.uri())),
        content_type: None,
    };
    assert_eq!(image_bytes(&item, &client_for(&server)).await.unwrap(), png);
    assert!(
        !received(&server).await[0]
            .headers
            .contains_key("authorization")
    );
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
    assert_eq!(
        response.request_id.as_deref(),
        Some("8268670d-d239-4b05-99e9-aaf703977dab")
    );
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
async fn edit_sends_ordered_png_and_webp_as_repeated_image_parts_and_separate_png_mask() {
    let server = MockServer::start().await;
    let guard = Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(two_images_response()))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let mut request = edit_request();
    request.references.push(Reference {
        data_url: EDIT_WEBP.into(),
    });
    let response = client_for(&server).edit_image(&request).await.unwrap();
    assert_eq!(response.data.len(), 2);
    let arrived = received(&server).await;
    assert_eq!(arrived.len(), 1);
    let form = multipart_fields(&arrived[0]);
    assert_eq!(form["image"].len(), 2);
    assert_eq!(form["mask"].len(), 1);
    for (part, data, filename, mime) in [
        (&form["image"][0], EDIT_PNG, "source-1.png", "image/png"),
        (&form["image"][1], EDIT_WEBP, "source-2.webp", "image/webp"),
        (&form["mask"][0], EDIT_PNG, "mask.png", "image/png"),
    ] {
        assert!(part.headers.contains(&format!("filename=\"{filename}\"")));
        assert!(part.headers.contains(&format!("Content-Type: {mime}")));
        assert_eq!(
            part.bytes,
            STANDARD.decode(data.split_once(',').unwrap().1).unwrap()
        );
    }
    drop(guard);
}

#[tokio::test]
async fn edit_omits_absent_mask_but_sends_quality_and_response_format() {
    let server = MockServer::start().await;
    let guard = Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
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
    let arrived = received(&server).await;
    let form = multipart_fields(&arrived[0]);
    assert_eq!(
        form.keys().copied().collect::<Vec<_>>(),
        ["image", "model", "prompt", "quality", "response_format"]
    );
    assert_eq!(form["quality"][0].bytes, b"high");
    assert_eq!(form["response_format"][0].bytes, b"b64_json");
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
    let arrived = received(&server).await;
    assert_eq!(
        multipart_fields(&arrived[0])["prompt"][0].bytes,
        req.prompt.as_bytes()
    );
    drop(guard);
}
