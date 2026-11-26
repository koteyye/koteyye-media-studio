//! Loopback-only Streamable HTTP for MCP clients.

use std::{
    io,
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::Duration,
};

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::{net::TcpListener, sync::oneshot, time::timeout};

use super::{INSTRUCTIONS, MAX_BODY_BYTES, SKILL, tools};
use crate::state::AppState;

const MODERN: &str = "2026-07-28";
const LEGACY: [&str; 2] = ["2025-11-25", "2025-06-18"];
const SUPPORTED: [&str; 3] = [MODERN, LEGACY[0], LEGACY[1]];
const SKILL_URI: &str = "koteyye://skills/media-studio/SKILL.md";
const PROMPT_NAME: &str = "use_media_studio";
const BODY_DEADLINE: Duration = Duration::from_secs(15);

struct Endpoint {
    app: Arc<AppState>,
    authority: [String; 2],
    token: Arc<RwLock<Option<String>>>,
}

impl Endpoint {
    fn new(
        app: Arc<AppState>,
        address: SocketAddr,
        token: Arc<RwLock<Option<String>>>,
    ) -> io::Result<Self> {
        if !address.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "MCP requires a loopback listener",
            ));
        }
        Ok(Self {
            app,
            authority: [address.to_string(), format!("localhost:{}", address.port())],
            token,
        })
    }

    fn accepts_host_and_origin(&self, headers: &HeaderMap) -> bool {
        let Some(host) = header_text(headers, "host") else {
            return false;
        };
        if !self
            .authority
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(host))
        {
            return false;
        }
        if !headers.contains_key(header::ORIGIN) {
            return true;
        }
        header_text(headers, "origin")
            .and_then(|origin| origin.strip_prefix("http://"))
            .is_some_and(|origin| origin.eq_ignore_ascii_case(host))
    }

    fn accepts_token(&self, headers: &HeaderMap) -> bool {
        let Ok(token) = self.token.read() else {
            return false;
        };
        let Some(expected) = token.as_deref().filter(|token| !token.is_empty()) else {
            return false;
        };
        header_text(headers, "authorization")
            .and_then(|auth| auth.split_once(' '))
            .is_some_and(|(scheme, token)| {
                scheme.eq_ignore_ascii_case("bearer")
                    && bool::from(expected.as_bytes().ct_eq(token.as_bytes()))
            })
    }
}

pub(super) async fn serve(
    listener: TcpListener,
    state: Arc<AppState>,
    token: Arc<RwLock<Option<String>>>,
    stop: oneshot::Receiver<()>,
) -> io::Result<()> {
    let endpoint = Arc::new(Endpoint::new(state, listener.local_addr()?, token)?);
    let app = Router::new()
        .route("/mcp", any(handle))
        .with_state(endpoint);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = stop.await;
        })
        .await
}

async fn handle(State(endpoint): State<Arc<Endpoint>>, request: Request) -> Response {
    let headers = request.headers();
    if !endpoint.accepts_host_and_origin(headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !endpoint.accepts_token(headers) {
        return unauthorized();
    }
    if request.method() != Method::POST {
        return (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response();
    }
    if !header_text(headers, "content-type").is_some_and(|mime| {
        mime.split(';')
            .next()
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
    }) {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }

    let (parts, body) = request.into_parts();
    let bytes = match timeout(BODY_DEADLINE, to_bytes(body, MAX_BODY_BYTES)).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        Err(_) => return StatusCode::REQUEST_TIMEOUT.into_response(),
    };
    // Rotation may occur while an existing connection is still uploading.
    if !endpoint.accepts_token(&parts.headers) {
        return unauthorized();
    }
    let mut response = match serde_json::from_slice(&bytes) {
        Ok(message) => dispatch(&endpoint.app, &parts.headers, message).await,
        Err(_) => RpcError::new(StatusCode::BAD_REQUEST, -32700, "Parse error").response(None),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            "Bearer realm=\"Koteyye Media Studio\"",
        )],
    )
        .into_response()
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

#[derive(Debug)]
struct RpcError {
    status: StatusCode,
    code: i32,
    message: &'static str,
    data: Option<Value>,
}

impl RpcError {
    fn new(status: StatusCode, code: i32, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            data: None,
        }
    }

    fn invalid_params(modern: bool, message: &'static str) -> Self {
        Self::new(
            if modern {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::OK
            },
            -32602,
            message,
        )
    }

    fn response(self, id: Option<&Value>) -> Response {
        let mut error = json!({"code": self.code, "message": self.message});
        if let Some(data) = self.data {
            error["data"] = data;
        }
        (
            self.status,
            Json(json!({"jsonrpc": "2.0", "id": id, "error": error})),
        )
            .into_response()
    }
}

fn header_mismatch() -> RpcError {
    RpcError::new(
        StatusCode::BAD_REQUEST,
        -32020,
        "MCP headers do not match the request",
    )
}

fn unsupported_version(requested: &str) -> RpcError {
    RpcError {
        data: Some(json!({"requested": requested, "supported": SUPPORTED})),
        ..RpcError::new(
            StatusCode::BAD_REQUEST,
            -32022,
            "Unsupported protocol version",
        )
    }
}

fn rpc_version(headers: &HeaderMap, method: &str, params: &Value) -> Result<bool, RpcError> {
    let version_header = header_text(headers, "mcp-protocol-version");
    if headers.contains_key("mcp-protocol-version") && version_header.is_none() {
        return Err(header_mismatch());
    }
    let meta = &params["_meta"];
    let version_body = meta.get("io.modelcontextprotocol/protocolVersion");
    let modern = method == "server/discover"
        || version_body.is_some()
        || version_header.is_some_and(|version| !LEGACY.contains(&version));
    if !modern {
        return Ok(false);
    }
    let header = version_header.ok_or_else(header_mismatch)?;
    if !SUPPORTED.contains(&header) {
        return Err(unsupported_version(header));
    }
    let body = version_body.and_then(Value::as_str).ok_or_else(|| {
        RpcError::invalid_params(true, "Missing or invalid MCP protocol version in _meta")
    })?;
    if header != body || header_text(headers, "mcp-method") != Some(method) {
        return Err(header_mismatch());
    }
    if header != MODERN {
        return Err(RpcError::invalid_params(
            true,
            "Use a modern protocol version for per-request metadata",
        ));
    }
    if !meta["io.modelcontextprotocol/clientCapabilities"].is_object() {
        return Err(RpcError::invalid_params(
            true,
            "Missing or invalid MCP client capabilities in _meta",
        ));
    }
    if operation_name_field(method).is_some() {
        let name = operation_name(method, params)
            .ok_or_else(|| RpcError::invalid_params(true, "Missing MCP name or URI"))?;
        if decode_name(headers).as_deref() != Some(name) {
            return Err(header_mismatch());
        }
    }
    Ok(true)
}

fn operation_name_field(method: &str) -> Option<&'static str> {
    match method {
        "tools/call" | "prompts/get" => Some("name"),
        "resources/read" => Some("uri"),
        _ => None,
    }
}

pub(super) fn operation_name<'a>(method: &str, params: &'a Value) -> Option<&'a str> {
    params[operation_name_field(method)?].as_str()
}

fn decode_name(headers: &HeaderMap) -> Option<String> {
    let name = header_text(headers, "mcp-name")?;
    match name
        .strip_prefix("=?base64?")
        .and_then(|encoded| encoded.strip_suffix("?="))
    {
        Some(encoded) => String::from_utf8(STANDARD.decode(encoded).ok()?).ok(),
        None => Some(name.to_owned()),
    }
}

async fn dispatch(app: &Arc<AppState>, headers: &HeaderMap, message: Value) -> Response {
    let invalid = || RpcError::new(StatusCode::BAD_REQUEST, -32600, "Invalid Request");
    let id = message
        .get("id")
        .filter(|id| id.is_string() || id.is_i64() || id.is_u64());
    if message["jsonrpc"] != "2.0"
        || (message.get("id").is_some() && id.is_none())
        || message
            .get("params")
            .is_some_and(|params| !params.is_object())
    {
        return invalid().response(id);
    }
    let Some(method) = message["method"].as_str() else {
        return invalid().response(id);
    };
    let params = &message["params"];
    let Some(id) = id else {
        if matches!(
            method,
            "notifications/initialized" | "notifications/cancelled"
        ) {
            return StatusCode::ACCEPTED.into_response();
        }
        return invalid().response(None);
    };
    let modern = match rpc_version(headers, method, params) {
        Ok(modern) => modern,
        Err(error) => return error.response(Some(id)),
    };
    let mut result = match invoke(app, modern, method, params).await {
        Ok(result) => result,
        Err(error) => return error.response(Some(id)),
    };
    if modern {
        result["resultType"] = json!("complete");
        result["_meta"]["io.modelcontextprotocol/serverInfo"] = server_info();
    }
    Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

fn server_info() -> Value {
    json!({
        "name": "koteyye-media-studio",
        "title": "Koteyye Media Studio",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

fn capabilities() -> Value {
    json!({"tools": {}, "resources": {}, "prompts": {}})
}

async fn invoke(
    app: &Arc<AppState>,
    modern: bool,
    method: &str,
    params: &Value,
) -> Result<Value, RpcError> {
    match method {
        "initialize" if !modern => {
            let requested = params["protocolVersion"].as_str().ok_or_else(|| {
                RpcError::invalid_params(false, "Missing requested protocol version")
            })?;
            let version = LEGACY
                .into_iter()
                .find(|version| *version == requested)
                .unwrap_or(LEGACY[0]);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": capabilities(),
                "serverInfo": server_info(),
                "instructions": INSTRUCTIONS,
            }))
        }
        "server/discover" => Ok(json!({
            "supportedVersions": SUPPORTED,
            "capabilities": capabilities(),
            "instructions": INSTRUCTIONS,
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools::list_tools()),
        "tools/call" => {
            let name = params["name"]
                .as_str()
                .ok_or_else(|| RpcError::invalid_params(modern, "Missing tool name"))?;
            let known = tools::list_tools()["tools"]
                .as_array()
                .is_some_and(|known| known.iter().any(|tool| tool["name"] == name));
            if !known {
                return Err(RpcError::invalid_params(modern, "Unknown tool"));
            }
            let args = match params.get("arguments") {
                Some(args) if args.is_object() => args.clone(),
                Some(_) => {
                    return Err(RpcError::invalid_params(
                        modern,
                        "Tool arguments must be an object",
                    ));
                }
                None => json!({}),
            };
            Ok(tools::call_tool(app, name, args).await)
        }
        "resources/list" => Ok(json!({
            "resources": [{
                "uri": SKILL_URI,
                "name": "SKILL.md",
                "title": "Use Koteyye Media Studio",
                "description": "Agent skill for generating media and retrieving results with the local Studio",
                "mimeType": "text/markdown",
                "size": SKILL.len(),
            }]
        })),
        "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
        "resources/read" if params["uri"] == SKILL_URI => Ok(json!({
            "contents": [{"uri": SKILL_URI, "mimeType": "text/markdown", "text": SKILL}]
        })),
        "resources/read" => Err(if modern {
            RpcError::invalid_params(true, "Resource not found")
        } else {
            RpcError::new(StatusCode::OK, -32002, "Resource not found")
        }),
        "prompts/list" => Ok(json!({"prompts": [{
            "name": PROMPT_NAME,
            "title": "Use Koteyye Media Studio",
            "description": "Load the Studio workflow and select the correct tool for a media task",
        }]})),
        "prompts/get" if params["name"] == PROMPT_NAME => Ok(json!({
            "description": "Agent skill for Koteyye Media Studio",
            "messages": [{"role": "user", "content": {"type": "text", "text": SKILL}}]
        })),
        "prompts/get" => Err(RpcError::invalid_params(modern, "Unknown prompt")),
        _ => Err(RpcError::new(
            if modern {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::OK
            },
            -32601,
            "Method not found",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keychain::InMemoryKeyStore;

    #[tokio::test]
    async fn loopback_serves_legacy_and_modern_discovery_and_revokes_access() {
        let dir = tempfile::tempdir().unwrap();
        let app = Arc::new(
            AppState::new(
                &dir.path().join("studio.sqlite"),
                Box::new(InMemoryKeyStore::default()),
                "http://127.0.0.1:1",
                Arc::new(|_| ()),
            )
            .unwrap(),
        );
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let token = Arc::new(RwLock::new(Some("initial-token".to_owned())));
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve(listener, app, Arc::clone(&token), stopped));
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let post = || {
            client
                .post(&url)
                .header("accept", "application/json, text/event-stream")
        };
        let legacy = |method, params: Value| {
            post().bearer_auth("initial-token").json(&json!({
                "jsonrpc": "2.0", "id": "legacy-1", "method": method, "params": params,
            }))
        };
        let modern = |method, mut params: Value| {
            params["_meta"] = json!({
                "io.modelcontextprotocol/protocolVersion": MODERN,
                "io.modelcontextprotocol/clientCapabilities": {},
            });
            post()
                .bearer_auth("initial-token")
                .header("mcp-protocol-version", MODERN)
                .header("mcp-method", method)
                .json(&json!({
                    "jsonrpc": "2.0", "id": 2026, "method": method, "params": params,
                }))
        };

        let legacy_info: Value = legacy("initialize", json!({"protocolVersion": LEGACY[0]}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(legacy_info["id"], "legacy-1");
        assert_eq!(legacy_info["result"]["protocolVersion"], LEGACY[0]);
        assert_eq!(legacy_info["result"]["instructions"], INSTRUCTIONS);
        let initialized = post()
            .bearer_auth("initial-token")
            .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .send()
            .await
            .unwrap();
        assert_eq!(initialized.status(), StatusCode::ACCEPTED);
        assert!(initialized.bytes().await.unwrap().is_empty());

        let listed: Value = legacy("tools/list", json!({}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(!listed["result"]["tools"].as_array().unwrap().is_empty());

        let discovered: Value = modern("server/discover", json!({}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(discovered["id"], 2026);
        assert_eq!(discovered["result"]["resultType"], "complete");
        assert_eq!(discovered["result"]["supportedVersions"][0], MODERN);
        assert!(discovered["result"]["capabilities"]["resources"].is_object());

        let unsupported = post()
            .bearer_auth("initial-token")
            .header("mcp-protocol-version", "2099-01-01")
            .json(&json!({"jsonrpc":"2.0","id":2,"method":"ping"}))
            .send()
            .await
            .unwrap();
        assert_eq!(unsupported.status(), StatusCode::BAD_REQUEST);
        let error = &unsupported.json::<Value>().await.unwrap()["error"];
        assert_eq!(error["code"], -32022);
        assert_eq!(error["data"]["supported"][0], MODERN);

        let wrong_name = modern("resources/read", json!({"uri": SKILL_URI}))
            .header("mcp-name", "koteyye://elsewhere")
            .send()
            .await
            .unwrap();
        assert_eq!(wrong_name.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            wrong_name.json::<Value>().await.unwrap()["error"]["code"],
            -32020
        );

        let encoded_name = format!("=?base64?{}?=", STANDARD.encode(SKILL_URI));
        let skill: Value = modern("resources/read", json!({"uri": SKILL_URI}))
            .header("mcp-name", encoded_name)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(skill["result"]["contents"][0]["text"], SKILL);

        let prompt: Value = legacy("prompts/get", json!({"name": PROMPT_NAME}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(prompt["result"]["messages"][0]["content"]["text"], SKILL);

        let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
        assert_eq!(
            post().json(&ping).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        for bad_header in [
            ("origin", "http://localhost:1"),
            ("host", "foreign.example"),
        ] {
            assert_eq!(
                legacy("ping", json!({}))
                    .header(bad_header.0, bad_header.1)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::FORBIDDEN,
            );
        }
        assert_eq!(
            client
                .get(&url)
                .bearer_auth("initial-token")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        *token.write().unwrap() = Some("second-token".to_owned());
        assert_eq!(
            legacy("ping", json!({})).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED,
        );
        assert_eq!(
            post()
                .bearer_auth("second-token")
                .json(&ping)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK,
        );
        *token.write().unwrap() = None;
        assert_eq!(
            post()
                .bearer_auth("second-token")
                .json(&ping)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED,
        );
        stop.send(()).unwrap();
        timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
