//! Мост для ADE, запускающих MCP-сервер через стандартные потоки.

use std::io;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use km_shared::MCP_PORT;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::Mutex,
    task::JoinSet,
};

use super::{MAX_BODY_BYTES, endpoint, transport::operation_name, valid_token};

const TOKEN_ENV: &str = "KOTEYYE_MCP_TOKEN";

pub fn run_stdio() -> Result<(), String> {
    let token = std::env::var(TOKEN_ENV)
        .ok()
        .filter(|token| valid_token(token))
        .ok_or_else(|| format!("Set {TOKEN_ENV} to the token from Settings → MCP."))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    runtime
        .block_on(bridge(token))
        .map_err(|err| err.to_string())
}

async fn bridge(token: String) -> io::Result<()> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(io::Error::other)?;
    let mut input = BufReader::new(tokio::io::stdin());
    let output = std::sync::Arc::new(Mutex::new(tokio::io::stdout()));
    let mut pending = JoinSet::new();
    let mut line = Vec::new();

    loop {
        line.clear();
        let n = (&mut input)
            .take(MAX_BODY_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .await?;
        if n == 0 {
            break;
        }
        if n > MAX_BODY_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP message exceeds the size limit",
            ));
        }
        let message: Value = match serde_json::from_slice(&line) {
            Ok(message) => message,
            Err(_) => {
                write_message(
                    &mut *output.lock().await,
                    rpc_error(Value::Null, -32700, "Parse error"),
                )
                .await?;
                continue;
            }
        };
        while pending.len() >= 8 {
            if let Some(result) = pending.join_next().await {
                result.map_err(io::Error::other)??;
            }
        }
        let client = client.clone();
        let token = token.clone();
        let output = output.clone();
        pending.spawn(async move {
            if let Some(reply) = forward(&client, &endpoint(MCP_PORT), &token, message).await {
                write_message(&mut *output.lock().await, reply).await?;
            }
            Ok::<_, io::Error>(())
        });
    }
    while let Some(result) = pending.join_next().await {
        result.map_err(io::Error::other)??;
    }
    Ok(())
}

async fn write_message(output: &mut tokio::io::Stdout, message: Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(&message)?;
    output.write_all(&bytes).await?;
    output.write_all(b"\n").await?;
    output.flush().await
}

async fn forward(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    message: Value,
) -> Option<Value> {
    let id = message.get("id").cloned();
    let mut request = client
        .post(url)
        .bearer_auth(token)
        .header("accept", "application/json, text/event-stream")
        .json(&message);
    let params = &message["params"];
    if let (Some(version), Some(method)) = (
        params["_meta"]["io.modelcontextprotocol/protocolVersion"].as_str(),
        message["method"].as_str(),
    ) {
        request = request
            .header("MCP-Protocol-Version", version)
            .header("Mcp-Method", method);
        if let Some(name) = operation_name(method, params) {
            let name = if name.bytes().all(|byte| (b' '..=b'~').contains(&byte)) {
                name.to_owned()
            } else {
                format!("=?base64?{}?=", STANDARD.encode(name))
            };
            request = request.header("Mcp-Name", name);
        }
    }
    let response = request.send().await;
    let id = id?;
    let problem = match response {
        Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
            "MCP token rejected. Copy the current token from the studio."
        }
        Ok(response) => match response.json::<Value>().await {
            Ok(reply) if reply["jsonrpc"] == "2.0" => return Some(reply),
            _ => "The studio returned an invalid MCP response.",
        },
        Err(_) => {
            "Could not reach the studio. Keep the desktop app open and enable MCP in Settings."
        }
    };
    Some(rpc_error(id, -32000, problem))
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}
