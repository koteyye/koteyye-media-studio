//! Точка входа приложения Koteyye Media Studio: сборка и запуск Tauri.
//!
//! Команды ядра ([`commands`]), состояние ([`state::AppState`]) и keychain
//! ([`keychain`]) регистрируются здесь; при старте фоном восстанавливаются
//! незавершённые генерации. События ядра уходят в окно через [`tauri::Emitter`]
//! с именами [`commands::event_name`] и payload — serde [`km_shared::AppEvent`].

pub mod commands;
pub mod keychain;
pub mod result_reference;
pub mod state;
mod updates;

use std::sync::Arc;

use km_shared::AppEvent;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

use crate::commands::event_name;
use crate::keychain::KeychainStore;
use crate::state::AppState;

/// Базовый URL API Cheaper Inference (продакшен).
const CI_BASE_URL: &str = "https://api.cheaperinference.com";

/// Имя файла SQLite в `app_data_dir`.
const DB_FILE: &str = "history.db";

pub(crate) fn apply_native_theme(app: &AppHandle, theme: km_shared::Theme) {
    app.set_theme(match theme {
        km_shared::Theme::Light => Some(tauri::Theme::Light),
        km_shared::Theme::Dark => Some(tauri::Theme::Dark),
        km_shared::Theme::System => None,
    });
}

fn show_studio(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Запускает приложение Tauri с конфигурацией из `tauri.conf.json`.
///
/// # Panics
///
/// Паникует, если приложение не удалось запустить (например, недоступен WebView).
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_studio(app)
        }))
        .manage(updates::UpdateCache::default())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && let Some(state) = window.try_state::<Arc<AppState>>()
                && state.recovery_lock().try_write().is_err()
            {
                // Закрываем окно, сохраняя живой оплаченный запрос до записи файлов.
                // Повторный запуск показывает это же окно и отменяет фоновый выход.
                api.prevent_close();
                if window.hide().is_ok() {
                    let state = state.inner().clone();
                    let window = window.clone();
                    tauri::async_runtime::spawn(async move {
                        let _idle = state.recovery_lock().write().await;
                        if matches!(window.is_visible(), Ok(false)) {
                            window.app_handle().exit(0);
                        }
                    });
                }
            }
        })
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
            if let Ok(settings) = state.storage.get_settings() {
                apply_native_theme(app.handle(), settings.theme);
            }
            app.manage(Arc::clone(&state));

            // Восстановление незавершённых генераций — фоном, не блокируя запуск.
            // Та же команда зовётся из UI (load); проходы сериализованы локом
            // AppState::recovery_lock, поэтому дублей файлов и событий нет.
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
            commands::provider_has_key,
            commands::set_provider_key,
            commands::check_provider_key,
            commands::test_connection,
            commands::fetch_models,
            commands::generate,
            commands::list_history,
            result_reference::result_reference,
            commands::delete_history,
            commands::recover_pending,
            commands::fetch_balance,
            commands::pick_folder,
            commands::open_file,
            commands::show_in_folder,
            updates::check_update,
            updates::install_update,
        ])
        .build(tauri::generate_context!())
        .expect("не удалось запустить приложение Tauri")
        .run(|_app, _event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } = _event
            {
                show_studio(_app);
            }
        });
}
