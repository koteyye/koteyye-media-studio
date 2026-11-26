//! Последние попытки одного вида и провайдера, включая ожидание и ошибки.

use dioxus::prelude::*;
use dioxus::web::WebEventExt;
use km_shared::{JobStatus, MediaKind};
use wasm_bindgen::JsCast;

use crate::components::player::display_date;
use crate::state::{AppState, use_t};
use crate::{ipc, router::Route};
use km_ui_core::i18n::tr;

#[component]
pub fn RecentStrip(
    kind: MediaKind,
    selected: Option<String>,
    on_select: Callback<String>,
    on_play: Callback<String>,
) -> Element {
    let state = use_context::<AppState>();
    let nav = use_navigator();
    let lang = (state.lang)();
    let recent = use_t("common.recent");
    let settings = state.settings.read();
    let history = state.history.read();
    let records: Vec<_> = history
        .iter()
        .filter(|record| record.kind == kind && record.provider == settings.provider)
        .take(usize::from(settings.workspace.recent_count))
        .collect();

    rsx! {
        section { class: "recent-strip generator-recent", aria_label: recent,
            header { h2 { {recent} }
                button { class:"generator-link",r#type:"button",onclick:move |_|{
                    state.show_history(kind);nav.push(Route::History {});}, {tr(lang,"gen.history")} " →" }
            }
            if records.is_empty() {p {class:"muted", {tr(lang,"gen.empty")} }}
            div { class: "recent-items generator-recent-list",
                for record in records {
                    div { key:"{record.id}",class:"generator-recent-row",
                        button {
                            r#type:"button", class:"recent-item",title:"{record.prompt}",
                            aria_label:format!("{} · {}",tr(lang,"gen.select"), record.prompt),
                            aria_pressed:"{selected.as_ref()==Some(&record.id)}",
                            onclick:{let id=record.id.clone();move |_|on_select.call(id.clone())},
                            Thumbnail { kind, path:record.local_paths.first().cloned(), video_frame:true }
                            span { class:"generator-recent-copy",strong { "{record.prompt}" }
                                small { "{record.model} · "
                                    {tr(lang,match record.status { JobStatus::Completed=>"history.completed", JobStatus::Failed=>"history.failed",JobStatus::Pending=>"history.pending" })}
                                } }
                            time { {display_date(record.created_at,lang)} }
                        }
                        if kind!=MediaKind::Image && record.status==JobStatus::Completed && !record.local_paths.is_empty() {
                            button { r#type:"button",class:"generator-icon-button",aria_label:tr(lang,"gen.play"),title:tr(lang,"gen.play"),
                                onclick:{let id=record.id.clone();move |_|on_play.call(id.clone())},
                                span {class:"studio-icon","data-icon":"play",aria_hidden:"true"}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Кадр из видео читают только видимые строки ограниченной ленты. В полной истории
/// видео и аудио остаются значками и не открывают файл до выбора пользователем.
#[component]
pub(crate) fn Thumbnail(
    kind: MediaKind,
    path: Option<String>,
    #[props(default)] video_frame: bool,
) -> Element {
    let mut failed = use_signal(|| None::<String>);
    let icon = match kind {
        MediaKind::Image => "image",
        MediaKind::Video => "video",
        MediaKind::Audio => "audio",
    };
    rsx! {
        span { class: "history-thumb", aria_hidden: "true",
            if let Some(path) = path.filter(|path| (kind == MediaKind::Image || (kind==MediaKind::Video&&video_frame)) && failed.read().as_ref() != Some(path)) {
                if kind==MediaKind::Image {
                    img {src:ipc::media_url(&path),loading:"lazy",alt:"",
                        onerror:move |_|failed.set(Some(path.clone()))}
                } else {
                    video {src:ipc::media_url(&path),preload:"metadata",muted:true,playsinline:true,tabindex:"-1",
                        onmounted:move |e|{
                            if let Ok(media)=e.as_web_event().dyn_into::<web_sys::HtmlMediaElement>() {media.set_muted(true);}
                        },
                        onloadedmetadata:move |e|{
                            if let Some(media)=e.as_web_event().target().and_then(|t|t.dyn_into::<web_sys::HtmlMediaElement>().ok()){
                                media.set_muted(true);
                                let duration=media.duration();
                                if duration.is_finite()&&duration>0.0 {media.set_current_time((duration/2.0).min(0.01));}
                            }
                        },
                        onerror:move |_|failed.set(Some(path.clone()))
                    }
                }
            } else {
                span {class:"studio-icon","data-icon":icon}
            }
        }
    }
}
