//! Запуск MCP в терминале с теми же данными и ключами, что и у Tauri.

use std::sync::Arc;

use crate::{APP_IDENTIFIER, AppState, CI_BASE_URL, DB_FILE, KeychainStore, mcp};

pub fn run_mcp_server(show_token: bool) -> Result<(), String> {
    let data_dir = dirs::data_dir()
        .ok_or("Could not resolve the Studio data directory")?
        .join(APP_IDENTIFIER);
    std::fs::create_dir_all(&data_dir).map_err(|err| err.to_string())?;
    let state = Arc::new(AppState::new(
        &data_dir.join(DB_FILE),
        Box::new(KeychainStore::new()?),
        CI_BASE_URL,
        Arc::new(|_| {}),
    )?);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    runtime.block_on(async {
        let signal = shutdown_signal().map_err(|err| err.to_string())?;
        let stop = async {
            signal.await;
            eprintln!("Stopping MCP. Waiting for accepted jobs to finish.");
        };
        mcp::serve_headless(&state, stop, |status| {
            println!("MCP listening on {}", status.endpoint);
            if show_token && let Some(token) = &status.token {
                println!("Authorization: Bearer {token}");
            }
        })
        .await
    })
}

#[cfg(unix)]
fn shutdown_signal() -> std::io::Result<impl Future<Output = ()> + Send> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
    })
}

#[cfg(not(unix))]
fn shutdown_signal() -> std::io::Result<impl Future<Output = ()> + Send> {
    Ok(async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            eprintln!("Could not handle Ctrl+C: {err}");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_uses_the_same_data_identifier_as_desktop() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"].as_str(), Some(APP_IDENTIFIER));
        assert!(config["app"].get("appDirectoriesOverride").is_none());
    }
}
