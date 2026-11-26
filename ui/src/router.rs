//! Маршруты приложения: Studio (вид медиа), History, Settings, Onboarding.
//!
//! `/` открывает стартовый или последний раздел. Studio/History/Settings рендерятся
//! внутри [`Layout`] (сайдбар + контент), Onboarding — отдельный экран без сайдбара.
//! Параметр `kind` — строка маршрута (`image`/`video`/`audio`); в тип
//! [`MediaKind`] её переводит [`parse_kind`].

use dioxus::prelude::*;
use km_shared::MediaKind;

use crate::components::layout::Layout;
use crate::screens::{History, Onboarding, Settings, Studio};

/// Маршрут приложения; компонент рендерится по имени варианта.
#[derive(Clone, Debug, PartialEq, Routable)]
pub enum Route {
    #[layout(Layout)]
    #[route("/")]
    Home {},
    #[route("/studio/:kind")]
    Studio {
        /// Вид медиа: `image`, `video` или `audio`.
        kind: String,
    },
    #[route("/history")]
    History {},
    #[route("/settings")]
    Settings {},
    #[end_layout]
    #[route("/onboarding")]
    Onboarding {},
}

/// Главная точка входа ждёт выбора из SQLite. Прямые ссылки открываются как обычно.
#[component]
fn Home() -> Element {
    let state = use_context::<crate::state::AppState>();
    let nav = use_navigator();
    use_effect(move || {
        if *state.settings_ready.read() || !*state.busy.read() {
            let settings = state.settings.peek();
            let kind = settings
                .workspace
                .startup
                .unwrap_or(settings.workspace.last_kind);
            nav.replace(Route::studio(kind));
        }
    });
    let loading = crate::state::use_t("common.loading");
    rsx! { p { class: "muted", role: "status", {loading} } }
}

impl Route {
    pub fn studio(kind: MediaKind) -> Self {
        let slug = match kind {
            MediaKind::Image => "image",
            MediaKind::Video => "video",
            MediaKind::Audio => "audio",
        };
        Self::Studio { kind: slug.into() }
    }
}

/// Переводит строковый параметр маршрута в [`MediaKind`].
///
/// Неизвестное значение (в т.ч. пустое) — [`MediaKind::Image`]: студия
/// открывается на изображениях, а не падает с ошибкой маршрута.
#[must_use]
pub fn parse_kind(raw: &str) -> MediaKind {
    match raw {
        "video" => MediaKind::Video,
        "audio" => MediaKind::Audio,
        _ => MediaKind::Image,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn parse_kind_known_values() {
        assert_eq!(parse_kind("image"), MediaKind::Image);
        assert_eq!(parse_kind("video"), MediaKind::Video);
        assert_eq!(parse_kind("audio"), MediaKind::Audio);
    }

    #[test]
    fn parse_kind_unknown_falls_back_to_image() {
        assert_eq!(parse_kind(""), MediaKind::Image);
        assert_eq!(parse_kind("3d"), MediaKind::Image);
        assert_eq!(parse_kind("IMAGE"), MediaKind::Image);
    }

    #[test]
    fn route_display_and_parse_roundtrip() {
        let route = Route::Studio {
            kind: "video".to_owned(),
        };
        assert_eq!(route.to_string(), "/studio/video");
        assert_eq!(Route::from_str("/studio/video").unwrap(), route);
        assert_eq!(Route::from_str("/history").unwrap(), Route::History {});
        assert_eq!(Route::from_str("/settings").unwrap(), Route::Settings {});
        assert_eq!(
            Route::from_str("/onboarding").unwrap(),
            Route::Onboarding {}
        );
    }

    #[test]
    fn root_opens_the_saved_workspace() {
        assert_eq!(Route::from_str("/").unwrap(), Route::Home {});
    }
}
