//! Интеграционные тесты HTTP-клиента CI: реальное поведение через wiremock.

use km_core::{CiClient, CoreError};
use km_shared::AccountBalance;
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Клиент, направленный на мок-сервер, с тестовым ключом из брифа.
fn client_for(server: &MockServer) -> CiClient {
    CiClient::with_base("ci_live_test", &server.uri())
}

/// Ответ CI с OpenAI-конвертом ошибки `{"error":{code,message}}`.
fn error_response(status: u16, code: Value, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .set_body_json(json!({"error": {"code": code, "message": message}}))
}

#[tokio::test]
async fn http_401_maps_to_http_error_with_code() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(error_response(401, json!("invalid_api_key"), "bad key"))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .get_json::<Value>("/v1/models")
        .await
        .expect_err("401 должен вернуть ошибку");

    assert!(
        matches!(&err, CoreError::Http { status: 401, code: Some(code), message }
            if code == "invalid_api_key" && message == "bad key"),
        "ожидался Http{{401, invalid_api_key, bad key}}, получено: {err:?}"
    );
}

#[tokio::test]
async fn http_402_insufficient_balance_maps_to_http_402() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/account/balance"))
        .respond_with(error_response(
            402,
            json!("insufficient_balance"),
            "not enough funds",
        ))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .get_json::<AccountBalance>("/v1/account/balance")
        .await
        .expect_err("402 должен вернуть ошибку");

    assert!(
        matches!(&err, CoreError::Http { status: 402, code: Some(code), .. }
            if code == "insufficient_balance"),
        "ожидался Http{{402, insufficient_balance}}, получено: {err:?}"
    );
}

#[tokio::test]
async fn http_403_on_account_path_maps_to_scope_missing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/account/balance"))
        .respond_with(error_response(
            403,
            json!("forbidden"),
            "account scope required",
        ))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .get_json::<AccountBalance>("/v1/account/balance")
        .await
        .expect_err("403 на account-пути должен вернуть ошибку");

    assert!(
        matches!(err, CoreError::ScopeMissing),
        "ожидался ScopeMissing, получено: {err:?}"
    );
}

#[tokio::test]
async fn http_403_on_inference_path_stays_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/images/generations"))
        .respond_with(error_response(403, json!("forbidden"), "model not allowed"))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .get_json::<Value>("/v1/images/generations")
        .await
        .expect_err("403 на inference-пути должен вернуть ошибку");

    assert!(
        matches!(&err, CoreError::Http { status: 403, .. }),
        "ожидался обычный Http{{403}}, а не ScopeMissing, получено: {err:?}"
    );
}

#[tokio::test]
async fn invalid_json_on_200_maps_to_serde() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/account/balance"))
        .respond_with(ResponseTemplate::new(200).set_body_string("это не JSON"))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .get_json::<AccountBalance>("/v1/account/balance")
        .await
        .expect_err("невалидный JSON должен вернуть ошибку");

    assert!(
        matches!(err, CoreError::Serde(_)),
        "ожидался Serde, получено: {err:?}"
    );
}

#[tokio::test]
async fn request_carries_bearer_authorization_header() {
    let server = MockServer::start().await;
    // Мок совпадает только при корректном заголовке; expect(1) проверяется при сбросе guard.
    let guard = Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer ci_live_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let value: Value = client_for(&server)
        .get_json("/v1/models")
        .await
        .expect("запрос с заголовком Bearer должен совпасть с моком");
    assert_eq!(value, json!({"ok": true}));

    // Явная проверка заголовка по фактически принятому запросу.
    let requests = server
        .received_requests()
        .await
        .expect("мок-сервер должен записать запросы");
    assert_eq!(requests.len(), 1, "ожидался ровно один запрос");
    let auth = requests[0]
        .headers
        .get("authorization")
        .expect("заголовок Authorization должен присутствовать");
    assert_eq!(
        auth.to_str().expect("заголовок должен быть ASCII"),
        "Bearer ci_live_test"
    );

    drop(guard);
}

#[tokio::test]
async fn get_json_deserializes_account_balance() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/account/balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currency": "USD",
            "balance_usd": 10.5,
            "available_usd": 9.0,
            "reserved_usd": 1.5,
        })))
        .mount(&server)
        .await;

    let balance: AccountBalance = client_for(&server)
        .get_json("/v1/account/balance")
        .await
        .expect("200 с валидным JSON должен разобрать баланс");

    assert_eq!(
        balance,
        AccountBalance {
            currency: "USD".to_owned(),
            balance_usd: 10.5,
            available_usd: 9.0,
            reserved_usd: 1.5,
        }
    );
}

#[tokio::test]
async fn post_json_sends_body_and_deserializes_response() {
    let server = MockServer::start().await;
    let body = json!({"model": "flux-schnell", "prompt": "кот"});
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .and(body_json(body.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"request_id": "gen_123"})))
        .expect(1)
        .mount(&server)
        .await;

    let value: Value = client_for(&server)
        .post_json("/v1/images/generations", &body)
        .await
        .expect("POST с валидным телом должен разобрать ответ");
    assert_eq!(value, json!({"request_id": "gen_123"}));

    // Тело запроса действительно сериализовано и отправлено.
    let requests = server
        .received_requests()
        .await
        .expect("мок-сервер должен записать запросы");
    assert_eq!(requests.len(), 1, "ожидался ровно один запрос");
    assert_eq!(
        requests[0].body,
        serde_json::to_vec(&body).expect("тело должно сериализоваться"),
        "отправленное тело должно совпадать с переданным"
    );
}

#[tokio::test]
async fn numeric_error_code_is_normalized_to_string() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(error_response(429, json!(429), "rate limited"))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .get_json::<Value>("/v1/models")
        .await
        .expect_err("429 должен вернуть ошибку");

    assert!(
        matches!(&err, CoreError::Http { status: 429, code: Some(code), .. } if code == "429"),
        "числовой code должен нормализоваться в строку \"429\", получено: {err:?}"
    );
}

#[tokio::test]
async fn non_json_error_body_falls_back_to_raw_text() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(500).set_body_string("upstream exploded"))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .get_json::<Value>("/v1/models")
        .await
        .expect_err("500 должен вернуть ошибку");

    assert!(
        matches!(&err, CoreError::Http { status: 500, code: None, message }
            if message == "upstream exploded"),
        "message должен быть сырым текстом тела, получено: {err:?}"
    );
}

#[tokio::test]
async fn empty_error_body_falls_back_to_status_reason() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .get_json::<Value>("/v1/models")
        .await
        .expect_err("503 должен вернуть ошибку");

    assert!(
        matches!(&err, CoreError::Http { status: 503, code: None, message }
            if message == "Service Unavailable"),
        "message должен быть статус-текстом, получено: {err:?}"
    );
}

#[tokio::test]
async fn connection_refused_maps_to_network() {
    // Порт 1 на localhost заведомо закрыт: соединение отклоняется сразу.
    let client = CiClient::with_base("ci_live_test", "http://127.0.0.1:1");

    let err = client
        .get_json::<Value>("/v1/models")
        .await
        .expect_err("закрытый порт должен вернуть ошибку");

    assert!(
        matches!(err, CoreError::Network(_)),
        "ожидался Network, получено: {err:?}"
    );
}
