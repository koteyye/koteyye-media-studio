//! Корневой компонент приложения: контекст состояния, инициализация, темы, роутер.
//!
//! Редирект первого запуска: routing callback [`RouterConfig::on_update`] уводит
//! на Onboarding, пока стартовая проверка не нашла ключа (`onboarding_needed`)
//! и онбординг не завершён (`onboarding_done`). Callback вызывается только при
//! навигации, поэтому завершение стартовой проверки дополнительно запускает
//! перечитывание маршрута (эффект-триггер) — без этого редирект не сработал бы
//! до первой навигации.

use dioxus::prelude::*;
use dioxus::router::{GenericRouterContext, RouterConfig, root_router};
use km_ui_core::theme::theme_vars;

use crate::router::Route;
use crate::state::{AppState, resolve_theme};

/// Корень приложения: предоставляет [`AppState`], однократно инициализирует
/// его, применяет тему к документу и рендерит [`Router`] с редиректом онбординга.
///
/// CSS-переменные темы инжектируются в `<style>` из [`theme_vars`] — единственный
/// источник токенов; `data-theme`/`lang` на `<html>` задаёт эффект состояния.
#[component]
pub fn App() -> Element {
    let state = use_context_provider(AppState::new);

    // Однократная инициализация: внутри нет реактивных чтений — эффект не перезапускается.
    use_effect(move || state.init());
    use_effect(move || state.watch_choices());

    // Реактивное применение темы: чтения сигналов подписывают эффект на их смену.
    use_effect(move || {
        let resolved = resolve_theme((state.theme)(), (state.system_dark)());
        state.apply_document_theme(resolved);
    });

    // Триггер редиректа первого запуска: стартовая проверка завершается без
    // навигации, а routing callback вызывается только при навигации. Перечитываем
    // текущий маршрут и заменяем его тем же — callback принимает решение по
    // свежим сигналам. Проверка «уже на онбординге» внутри redirect_needed
    // не даёт эффекту зациклиться (replace нотифицирует подписчиков роутера).
    use_effect(move || {
        let needed = (state.onboarding_needed)();
        let done = (state.onboarding_done)();
        if let Some(router) = root_router() {
            let current: Route = router.current();
            if redirect_needed(needed, done, &current) {
                router.replace(current);
            }
        }
    });

    let resolved = resolve_theme((state.theme)(), (state.system_dark)());
    rsx! {
        document::Stylesheet { href: asset!("/assets/app-workspace.css") }
        document::Stylesheet { href: asset!("/assets/generator-workspace.css") }
        document::Stylesheet { href: asset!("/assets/settings-workspace.css") }
        document::Link { rel: "icon", href: asset!("/assets/icons/frame-16.png"), r#type: "image/png" }
        style { ":root {{ {theme_vars(resolved)} }}" }
        Router::<Route> {
            config: move || {
                RouterConfig::default().on_update(move |ctx| {
                    if onboarding_redirect(state, &ctx) {
                        Some(NavigationTarget::Internal(Route::Onboarding {}))
                    } else {
                        None
                    }
                })
            }
        }
    }
}

/// Решение редиректа для routing callback: читает сигналы состояния и текущий
/// маршрут, применяет [`redirect_needed`]. Чтения нереактивные (callback вне
/// компонента) — решение принимается по свежим значениям на каждую навигацию.
fn onboarding_redirect(state: AppState, ctx: &GenericRouterContext<Route>) -> bool {
    let current: Route = ctx.current();
    redirect_needed(
        (state.onboarding_needed)(),
        (state.onboarding_done)(),
        &current,
    )
}

/// Решение редиректа первого запуска (тестируется нативно).
///
/// Онбординг нужен, не завершён в этой сессии и текущий маршрут — не онбординг:
/// только тогда уводим на Onboarding. Завершение (`done`) снимает редирект даже
/// без ключа (осознанный пропуск); повторный запуск без ключа предложит онбординг
/// снова — `done` не персистится.
#[must_use]
fn redirect_needed(needed: bool, done: bool, current: &Route) -> bool {
    needed && !done && *current != Route::Onboarding {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn studio() -> Route {
        Route::Studio {
            kind: "image".to_owned(),
        }
    }

    #[test]
    fn redirect_first_launch_to_onboarding() {
        assert!(redirect_needed(true, false, &studio()));
        assert!(redirect_needed(true, false, &Route::Settings {}));
        assert!(redirect_needed(true, false, &Route::History {}));
    }

    #[test]
    fn no_redirect_when_key_present() {
        assert!(!redirect_needed(false, false, &studio()));
    }

    #[test]
    fn no_redirect_after_onboarding_done_even_without_key() {
        // Осознанный пропуск ключа: редирект снят до конца сессии.
        assert!(!redirect_needed(true, true, &studio()));
    }

    #[test]
    fn no_redirect_when_already_on_onboarding() {
        // Защита от цикла: маршрут уже онбординг — редирект не нужен.
        assert!(!redirect_needed(true, false, &Route::Onboarding {}));
    }
}
