//! Точка входа UI Koteyye Media Studio: запуск Dioxus-приложения (WASM).
//!
//! Каркас приложения (роутер, состояние, темы, i18n) — в [`km_ui::app`].

fn main() {
    dioxus::launch(km_ui::app::App);
}
