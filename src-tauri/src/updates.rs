//! Проверка GitHub Releases по запросу пользователя. Дескриптор из проверенного
//! источника остаётся в Rust; из интерфейса приходит только подтверждённая версия.

use std::sync::Arc;
use std::time::Duration;

use km_shared::UpdateStatus;
use tauri::async_runtime::Mutex;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::state::AppState;

#[derive(Default)]
pub(crate) struct UpdateCache(Mutex<Option<Update>>);

#[tauri::command]
pub(crate) async fn check_update(
    app: AppHandle,
    cache: State<'_, UpdateCache>,
) -> Result<UpdateStatus, String> {
    let mut pending = cache.0.lock().await;
    *pending = None;
    let enabled = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config["pubkey"].as_str())
        .is_some_and(|key| !key.trim().is_empty());
    if !enabled {
        return Ok(UpdateStatus::Disabled);
    }
    let updater = app
        .updater_builder()
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|e| e.to_string())?;
    *pending = updater.check().await.map_err(|e| e.to_string())?;
    Ok(match pending.as_ref() {
        Some(update) => UpdateStatus::Available(update.version.clone()),
        None => UpdateStatus::Current,
    })
}

#[tauri::command]
pub(crate) async fn install_update(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    cache: State<'_, UpdateCache>,
    version: String,
) -> Result<(), String> {
    // Установщик Windows может сам завершить процесс. До скачивания убеждаемся,
    // что оплаченные генерации и восстановление успели сохранить результат.
    let _idle = state
        .recovery_lock()
        .try_write()
        .map_err(|_| "Дождитесь завершения текущей генерации или восстановления".to_owned())?;
    let pending = cache.0.lock().await;
    let update = pending
        .as_ref()
        .filter(|update| update.version == version)
        .ok_or_else(|| "Проверьте наличие обновления ещё раз".to_owned())?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart()
}
