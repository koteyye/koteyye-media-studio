//! Точка входа приложения Koteyye Media Studio: сборка и запуск Tauri.

/// Запускает приложение Tauri с конфигурацией из `tauri.conf.json`.
///
/// # Panics
///
/// Паникует, если приложение не удалось запустить (например, недоступен WebView).
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("не удалось запустить приложение Tauri");
}
