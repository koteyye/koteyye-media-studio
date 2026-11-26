//! Чистая UI-логика Koteyye Media Studio: i18n, токены тем и форматтеры.
//!
//! Крейт нативный (без dioxus/web): вся логика — чистые функции над
//! [`km_shared`] типами, поэтому тестируется обычным `cargo test`.
//! UI-компоненты (Dioxus) и оболочка Tauri используют эти функции,
//! не дублируя словари, цвета и форматы.

mod error_i18n;
pub mod format;
mod generator_i18n;
pub mod i18n;
mod media_i18n;
mod settings_workspace_i18n;
pub mod studio;
pub mod studio_controls_i18n;
mod studio_i18n;
pub mod studio_results;
pub mod theme;
