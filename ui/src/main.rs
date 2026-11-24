//! Заглушка UI Koteyye Media Studio: Dioxus-приложение, компилируемое в WASM.

use dioxus::prelude::*;

fn main() {
    dioxus::launch(App);
}

/// Корневой компонент-заглушка: подтверждает, что каркас Tauri + Dioxus работает.
#[component]
fn App() -> Element {
    rsx! {
        h1 { "Koteyye Media Studio 2.0" }
        p { "Каркас работает." }
    }
}
