//! Заглушка UI Koteyye Media Studio: Dioxus-приложение, компилируемое в WASM.
//!
//! При старте выполняет смоук IPC-биндинга (настройки + публичный каталог
//! моделей): результат — в консоль webview и диагностической строкой ниже.
//! Экраны поверх биндинга появятся в следующих задачах Плана 2.

use dioxus::prelude::*;
use km_shared::MediaKind;
use km_ui::ipc;
use wasm_bindgen_futures::spawn_local;

fn main() {
    dioxus::launch(App);
}

/// Корневой компонент-заглушка: подтверждает, что каркас Tauri + Dioxus работает.
#[component]
fn App() -> Element {
    let mut smoke = use_signal(|| "IPC: проверка…".to_owned());
    use_effect(move || {
        // Смоук подписки: ошибки listen glue логирует в консоль webview.
        ipc::on_event(|event| {
            web_sys::console::log_1(&format!("IPC event: {event:?}").into());
        });
        spawn_local(async move {
            let settings = ipc::get_settings().await;
            let models = ipc::fetch_models(MediaKind::Image).await;
            let line = match (settings, models) {
                (Ok(s), Ok(m)) => format!(
                    "IPC: настройки получены (lang={:?}, theme={:?}), моделей image: {}",
                    s.lang,
                    s.theme,
                    m.len()
                ),
                (Err(e), _) => format!("IPC: get_settings — ошибка: {e}"),
                (_, Err(e)) => format!("IPC: fetch_models — ошибка: {e}"),
            };
            web_sys::console::log_1(&line.clone().into());
            smoke.set(line);
        });
    });
    rsx! {
        h1 { "Koteyye Media Studio 2.0" }
        p { "Каркас работает." }
        p { "{smoke}" }
    }
}
