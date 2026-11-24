//! Точка входа приложения Koteyye Media Studio: сборка и запуск Tauri.
//!
//! Команды ядра ([`commands`]), состояние ([`state::AppState`]) и keychain
//! ([`keychain`]) регистрируются здесь; при старте фоном восстанавливаются
//! незавершённые генерации. События ядра уходят в окно через [`tauri::Emitter`]
//! с именами [`commands::event_name`] и payload — serde [`km_shared::AppEvent`].

pub mod commands;
pub mod keychain;
pub mod state;

use std::sync::Arc;

use km_shared::AppEvent;
use tauri::{Emitter, Manager};

use crate::commands::event_name;
use crate::keychain::KeychainStore;
use crate::state::AppState;

/// Базовый URL API Cheaper Inference (продакшен).
const CI_BASE_URL: &str = "https://api.cheaperinference.com";

/// Имя файла SQLite в `app_data_dir`.
const DB_FILE: &str = "history.db";

/// Запускает приложение Tauri с конфигурацией из `tauri.conf.json`.
///
/// # Panics
///
/// Паникует, если приложение не удалось запустить (например, недоступен WebView).
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            // Эмиттер: событие ядра -> событие окна (payload — serde AppEvent).
            let handle = app.handle().clone();
            let emit: Arc<dyn Fn(AppEvent) + Send + Sync> = Arc::new(move |event| {
                if let Err(e) = handle.emit(event_name(&event), &event) {
                    eprintln!("не удалось отправить событие {}: {e}", event_name(&event));
                }
            });

            let state = Arc::new(AppState::new(
                &data_dir.join(DB_FILE),
                Box::new(KeychainStore::new()?),
                CI_BASE_URL,
                emit,
            )?);
            app.manage(Arc::clone(&state));

            // Восстановление незавершённых генераций — фоном, не блокируя запуск.
            tauri::async_runtime::spawn(async move {
                if let Err(err) = commands::cmd_recover_pending(&state).await {
                    eprintln!("восстановление при старте не удалось: {err}");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::set_settings,
            commands::set_api_key,
            commands::test_connection,
            commands::fetch_models,
            commands::generate,
            commands::list_history,
            commands::delete_history,
            commands::recover_pending,
            commands::fetch_balance,
            commands::pick_folder,
            commands::open_file,
            commands::show_in_folder,
        ])
        .run(tauri::generate_context!())
        .expect("не удалось запустить приложение Tauri");
}
