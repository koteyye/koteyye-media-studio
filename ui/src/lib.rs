//! UI-крейт Koteyye Media Studio: IPC-биндинг к Tauri и каркас Dioxus-приложения.
//!
//! Библиотечная часть: [`ipc`] — типизированные обёртки над командами и событиями
//! ядра; [`app`]/[`router`]/[`state`]/[`components`]/[`screens`] — каркас приложения
//! (роутер, глобальное состояние, темы, i18n, сайдбар). Бинарная часть (`main.rs`) —
//! точка входа Dioxus (WASM).

pub mod app;
pub mod components;
pub mod ipc;
// expect: dioxus-router-macro 0.7.10 генерирует для динамических сегментов
// (Err = Infallible у String) код с ложным unreachable_code; подавляем только его.
#[expect(unreachable_code, reason = "ложный unreachable в коде derive Routable")]
pub mod router;
pub mod screens;
pub mod state;
