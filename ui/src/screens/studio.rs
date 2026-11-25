//! Заглушка студии: вид медиа из маршрута и число моделей каталога.
//!
//! Форма генерации и результаты появятся в Плане 3.

use dioxus::prelude::*;
use km_shared::MediaKind;

use crate::router::parse_kind;
use crate::state::{AppState, use_t};

/// Экран студии для вида медиа `kind` (`image`/`video`/`audio`).
#[component]
pub fn Studio(kind: String) -> Element {
    let state = use_context::<AppState>();
    // Переводы — до ветвления: use_t основан на хуке, порядок вызовов стабилен.
    let title = use_t("studio.title");
    let image_label = use_t("studio.image");
    let video_label = use_t("studio.video");
    let audio_label = use_t("studio.audio");
    let model_label = use_t("studio.model");

    let media_kind = parse_kind(&kind);
    let kind_label = match media_kind {
        MediaKind::Image => image_label,
        MediaKind::Video => video_label,
        MediaKind::Audio => audio_label,
    };
    let model_count = (state.models_by_kind)()
        .get(&media_kind)
        .map_or(0, Vec::len);

    rsx! {
        h1 { {title} }
        h2 { {kind_label} }
        p { class: "muted", {model_label} ": {model_count}" }
    }
}
