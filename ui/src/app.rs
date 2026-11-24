//! Корневой компонент приложения: контекст состояния, инициализация, темы, роутер.

use dioxus::prelude::*;
use km_ui_core::theme::theme_vars;

use crate::router::Route;
use crate::state::{AppState, resolve_theme};

/// Корень приложения: предоставляет [`AppState`], однократно инициализирует
/// его, применяет тему к документу и рендерит [`Router`].
///
/// CSS-переменные темы инжектируются в `<style>` из [`theme_vars`] — единственный
/// источник токенов; `data-theme`/`lang` на `<html>` задаёт эффект состояния.
#[component]
pub fn App() -> Element {
    let state = use_context_provider(AppState::new);

    // Однократная инициализация: внутри нет реактивных чтений — эффект не перезапускается.
    use_effect(move || state.init());

    // Реактивное применение темы: чтения сигналов подписывают эффект на их смену.
    use_effect(move || {
        let resolved = resolve_theme((state.theme)(), (state.system_dark)());
        state.apply_document_theme(resolved);
    });

    let resolved = resolve_theme((state.theme)(), (state.system_dark)());
    rsx! {
        style { ":root {{ {theme_vars(resolved)} }}" }
        Router::<Route> {}
    }
}
