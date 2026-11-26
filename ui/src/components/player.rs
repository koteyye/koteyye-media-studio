//! Просмотр локальных результатов через asset protocol Tauri.

use dioxus::prelude::*;
use dioxus::web::WebEventExt;
use km_shared::{CatalogModel, GenMode, HistoryRecord, JobStatus, Lang, MediaKind};
use km_ui_core::format::{format_local_date_at, format_usd};
use km_ui_core::i18n::tr;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    AudioBuffer, CanvasRenderingContext2d, HtmlCanvasElement, HtmlMediaElement,
    OfflineAudioContext, Response,
};

use crate::components::error_notice::ErrorNotice;
use crate::ipc;
use crate::router::Route;
use crate::screens::uploads::accept_files;
use crate::state::{AppState, StudioDraft, use_t};

thread_local! {
    static PLAYING: std::cell::RefCell<Option<HtmlMediaElement>> = const { std::cell::RefCell::new(None) };
}

fn media_started(node: Signal<Option<HtmlMediaElement>>) {
    if let Some(current) = node.peek().as_ref() {
        PLAYING.with(|active| {
            if let Some(previous) = active.replace(Some(current.clone()))
                && previous != *current
            {
                let _ = previous.pause();
            }
        });
    }
}

fn use_media_node() -> Signal<Option<HtmlMediaElement>> {
    let node = use_signal(|| None::<HtmlMediaElement>);
    use_drop(move || {
        if let Ok(node) = node.try_peek()
            && let Some(current) = node.as_ref()
        {
            let _ = current.pause();
            PLAYING.with(|active| {
                if active.borrow().as_ref() == Some(current) {
                    active.take();
                }
            });
        }
    });
    node
}

/// В WASM системные часы предоставляет браузер.
pub(crate) fn display_date(timestamp: i64, lang: Lang) -> String {
    let record = js_sys::Date::new(&JsValue::from_f64(timestamp as f64 * 1000.0));
    let now = js_sys::Date::new_0();
    if !record.get_time().is_finite() {
        return "—".into();
    }
    format_local_date_at(
        timestamp,
        (now.get_time() / 1000.0) as i64,
        record.get_timezone_offset() as i32,
        now.get_timezone_offset() as i32,
        lang,
    )
}

pub(crate) fn display_timestamp(timestamp: i64, lang: Lang) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(timestamp as f64 * 1000.0));
    if !date.get_time().is_finite() {
        return "—".into();
    }
    let (year, month, day) = (date.get_full_year(), date.get_month() + 1, date.get_date());
    let date_text = match lang {
        Lang::Ru => format!("{day:02}.{month:02}.{year}"),
        Lang::En => format!("{year}-{month:02}-{day:02}"),
    };
    format!(
        "{date_text} {:02}:{:02}:{:02}",
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds()
    )
}

/// В студии показывает результат с метаданными; в истории они уже есть в строке.
#[component]
pub fn ResultViewer(record: HistoryRecord, #[props(default)] in_history: bool) -> Element {
    let state = use_context::<AppState>();
    let no_file = use_t("media.no_file");
    let pending = use_t("history.pending");
    let failed = use_t("history.failed");

    rsx! {
        section { class: "media-result",
            if !in_history {
                div { class: "history-meta",
                    strong { "{record.model}" }
                    span { {display_date(record.created_at, (state.lang)())} }
                    if let Some(cost) = record.cost_usd {
                        span { {format_usd(cost)} }
                    }
                }
            }
            match record.status {
                JobStatus::Pending => rsx! {
                    p { class: "muted", {pending} }
                    if let Some(reason) = &record.error {
                        div { class: "status-error", ErrorNotice { message: reason.clone() } }
                    }
                    if state.media_idle() {
                        button { class: "btn", r#type: "button", onclick: move |_| state.check_pending(),
                            {tr((state.lang)(), "studio.check_result")}
                        }
                    }
                },
                JobStatus::Failed => rsx! {
                    if let Some(reason) = &record.error {
                        div { class: "status-error", ErrorNotice { message: reason.clone() } }
                    } else {
                        p { class: "status-error", {failed} }
                    }
                },
                JobStatus::Completed if record.local_paths.is_empty() => rsx! {
                    p { class: "muted", {no_file} }
                },
                JobStatus::Completed => rsx! {
                    MediaSeries { key: "{record.id}", id: record.id.clone(), paths: record.local_paths.clone(),
                        kind: record.kind, prompt: record.prompt.clone(), single: false,
                        reference: state.settings.read().provider == record.provider }
                },
            }
            if !in_history && record.status != JobStatus::Pending {
                DeleteRecord { key: "{record.id}", id: record.id }
            }
        }
    }
}

/// Файл для формы выдаёт нативная команда по идентификатору серии и порядковому номеру.
fn reference_model(state: AppState, draft: StudioDraft) -> Option<CatalogModel> {
    let selected = draft.model.read();
    let catalog = state.models_by_kind.read();
    catalog
        .get(&MediaKind::Image)?
        .iter()
        .find(|model| {
            (selected.is_empty() || model.id == *selected)
                && model.supports_mode(GenMode::ImageEdit)
        })
        .cloned()
}

#[component]
fn ReferenceAction(
    id: String,
    index: usize,
    #[props(default)] after: Option<Callback<()>>,
) -> Element {
    let state = use_context::<AppState>();
    let nav = use_navigator();
    let mut loading = use_signal(|| false);
    let label = use_t("gen.use");
    let unavailable = use_t("controls.model_no_references");
    let draft = (state
        .studio_for(state.settings.read().provider, MediaKind::Image)
        .active)();
    let can_attach = reference_model(state, draft).is_some();
    rsx! { button {
        class: "btn media-reference", r#type: "button", disabled: loading() || !can_attach,
        title: if can_attach { label } else { unavailable },
        onclick: move |_| {
            if *loading.peek() || !can_attach { return; }
            loading.set(true);
            let id = id.clone();
            let provider = state.settings.peek().provider;
            spawn(async move {
                let result = ipc::result_reference(&id,index).await.and_then(|url| {
                    if state.settings.peek().provider != provider { return Ok(false); }
                    let draft = *state.studio(MediaKind::Image).active.peek();
                    let lang = *state.lang.peek();
                    let model = reference_model(state, draft)
                        .ok_or_else(|| tr(lang, "controls.model_no_references").to_owned())?;
                    if draft.model.peek().is_empty() {
                        draft.select_model_for_input(MediaKind::Image, &model);
                    }
                    accept_files(draft, MediaKind::Image, &model, vec![url])
                        .map_err(|key| tr(lang, key).to_owned())?;
                    Ok(true)
                });
                match result {
                    Ok(true) => { if let Some(done) = after { done.call(()); } nav.push(Route::Studio {kind:"image".into()}); }
                    Err(message) => {
                        let lang=*state.lang.peek();
                        let key=match message.as_str(){
                            "gen.reference_missing"=>Some("gen.reference_missing"),
                            "gen.reference_format"=>Some("gen.reference_format"),
                            "controls.edit_wire_limit"=>Some("controls.edit_wire_limit"),
                            _=>None,
                        };
                        state.set_error(Some(key.map_or(message,|key|tr(lang,key).to_owned())));
                    }
                    Ok(false) => {}
                }
                if let Ok(mut value) = loading.try_write() { *value=false; }
            });
        },
        span { class: "studio-icon", "data-icon": "reference", aria_hidden: "true" }
        {label}
    } }
}

#[component]
pub(crate) fn MediaSeries(
    id: String,
    paths: Vec<String>,
    kind: MediaKind,
    prompt: String,
    single: bool,
    #[props(default = 1.5)] aspect: f64,
    #[props(default)] reference: bool,
    #[props(default)] play: bool,
) -> Element {
    let mut index = use_signal(|| 0usize);
    let mut viewing = use_signal(|| None::<usize>);
    let total = paths.len();
    let active = index().min(total.saturating_sub(1));
    let no_file = use_t("media.no_file");
    let open = use_t("gen.open_image");
    let prev = use_t("gen.prev");
    let next = use_t("gen.next");
    if total == 0 {
        return rsx! {p { class:"muted", {no_file} }};
    }
    if kind != MediaKind::Image {
        return rsx! { div { class:"generator-av-series",
            for (i,path) in paths.iter().enumerate() {
                MediaFile { key:"{id}-{i}-{play}", path:path.clone(), kind, prompt:prompt.clone(), play:play && i==0 }
            }
        } };
    }
    rsx! {
        div { class:"generator-pictures",style:"--generator-aspect:{aspect}",
            div { class:if single || total==1 {"generator-shots one"} else {"generator-shots"},
                for (i,path) in paths.iter().enumerate().filter(|(i,_)| !single||*i==active) {
                    div { key:"{id}-{i}", class:"generator-shot",
                        button { class:"generator-shot-open",r#type:"button",title:open,aria_label:"{open} {i+1}",
                            onclick:move |_| viewing.set(Some(i)),
                            LocalImage { key:"{path}", path:path.clone(), alt:prompt.clone() }
                        }
                        if reference { ReferenceAction { id:id.clone(), index:i } }
                    }
                }
            }
            if single && total>1 { nav { class:"generator-pager", aria_label:open,
                button { r#type:"button",aria_label:prev,onclick:move |_|index.set((active+total-1)%total), "‹" }
                span { "{active+1} / {total}" }
                button { r#type:"button",aria_label:next,onclick:move |_|index.set((active+1)%total), "›" }
            }}
            if let Some(i)=viewing().filter(|i|*i<total) {
                ImageDialog { paths:paths.clone(), id, index:i, prompt, reference,
                    on_index:move |i|viewing.set(Some(i)), on_close:move |_|viewing.set(None) }
            }
        }
    }
}

#[component]
fn LocalImage(path: String, alt: String) -> Element {
    let mut failed = use_signal(|| false);
    let message = use_t("media.preview_failed");
    rsx! { if failed() { span { class:"status-error", {message} }}
    else { img { src:ipc::media_url(&path),alt, onerror:move |_|failed.set(true) }} }
}

#[component]
fn ImageDialog(
    paths: Vec<String>,
    id: String,
    index: usize,
    prompt: String,
    reference: bool,
    on_index: Callback<usize>,
    on_close: Callback<()>,
) -> Element {
    let total = paths.len();
    let path = &paths[index];
    let prev = use_t("gen.prev");
    let next = use_t("gen.next");
    let close = use_t("common.close");
    let open = use_t("gen.open_image");
    rsx! {
        dialog { class:"generator-dialog",aria_label:open,
            onmounted:move |e| { if let Ok(d)=e.as_web_event().dyn_into::<web_sys::HtmlDialogElement>() { let _=d.show_modal(); } },
            oncancel:move |e| {e.prevent_default();on_close.call(());},
            onkeydown:move |e| { match e.key(){
                Key::ArrowLeft if total>1 => {e.prevent_default();on_index.call((index+total-1)%total);}
                Key::ArrowRight if total>1 => {e.prevent_default();on_index.call((index+1)%total);}
                _=>{}
            } },
            header { span { "{index+1} / {total}" }
                button {class:"btn",r#type:"button",aria_label:close, onclick:move |_|on_close.call(()),"×"} }
            div { class:"generator-dialog-stage", LocalImage { key:"{path}",path:path.clone(),alt:prompt } }
            footer {
                if total>1 {
                    button {class:"btn",r#type:"button",aria_label:prev,onclick:move |_|on_index.call((index+total-1)%total),"‹"}
                    button {class:"btn",r#type:"button",aria_label:next,onclick:move |_|on_index.call((index+1)%total),"›"}
                }
                if reference { ReferenceAction { id,index,after:on_close } }
                FileActions { path:path.clone() }
            }
        }
    }
}

/// Приложение авторизует путь перед передачей файла оболочке ОС.
#[component]
pub(crate) fn FileActions(path: String, #[props(default)] compact: bool) -> Element {
    let state = use_context::<AppState>();
    let open = use_t("media.open_file");
    let reveal = use_t("media.reveal");
    let reveal_path = path.clone();

    rsx! {
        button {
            r#type: "button", class: "btn", aria_label: open, title: open,
            onclick: move |_| {
                let path = path.clone();
                async move {
                    if let Err(message) = ipc::open_file(&path).await {
                        state.set_error(Some(message));
                    }
                }
            },
            if compact { "↗" } else { {open} }
        }
        button {
            r#type: "button", class: "btn", aria_label: reveal, title: reveal,
            onclick: move |_| {
                let path = reveal_path.clone();
                async move {
                    if let Err(message) = ipc::show_in_folder(&path).await {
                        state.set_error(Some(message));
                    }
                }
            },
            if compact { "▣" } else { {reveal} }
        }
    }
}

/// Удаление записи SQLite с подтверждением; файлы принадлежат пользователю.
#[component]
pub(crate) fn DeleteRecord(id: String, #[props(default)] compact: bool) -> Element {
    let state = use_context::<AppState>();
    let mut confirming = use_signal(|| false);
    let mut deleting = use_signal(|| false);
    let remove = use_t("history.remove");
    let confirm = use_t("history.remove_confirm");
    let accept = use_t("common.delete");
    let cancel = use_t("history.cancel");

    rsx! {
        if confirming() {
            div { class: "history-confirm", role: "group", aria_label: confirm,
                span { {confirm} }
                button {
                    r#type: "button", class: "btn", disabled: deleting(),
                    onclick: move |_| confirming.set(false), {cancel}
                }
                button {
                    r#type: "button", class: "btn", disabled: deleting(),
                    onclick: move |_| {
                        if *deleting.peek() {
                            return;
                        }
                        deleting.set(true);
                        let id = id.clone();
                        // Обновляем общее состояние и при переходе на другой экран во время IPC.
                        spawn_local(async move {
                            match ipc::delete_history(&id).await {
                                Ok(()) => {
                                    state.refresh_history().await;
                                    state.refresh_balance().await;
                                }
                                Err(message) => state.set_error(Some(message)),
                            }
                            if let Ok(mut flag) = deleting.try_write() {
                                *flag = false;
                            }
                        });
                    },
                    {accept}
                }
            }
        } else {
            button {
                r#type: "button", class: "btn", aria_label: remove, title: remove,
                onclick: move |_| confirming.set(true),
                if compact { "×" } else { {remove} }
            }
        }
    }
}

#[component]
fn MediaFile(
    path: String,
    kind: MediaKind,
    prompt: String,
    #[props(default)] play: bool,
) -> Element {
    let mut failed = use_signal(|| false);
    let mut video = use_media_node();
    let unavailable = use_t("media.preview_failed");
    let url = ipc::media_url(&path);
    let filename = path.rsplit(['/', '\\']).next().unwrap_or(&path);

    rsx! {
        figure { class: "media-file",
            if failed() {
                p { class: "status-error", role: "status", {unavailable} }
            } else {
                match kind {
                    MediaKind::Image => rsx! {
                        img {
                            class: "media-stage", src: url, alt: prompt,
                            onerror: move |_| failed.set(true),
                        }
                    },
                    MediaKind::Video => rsx! {
                        video {
                            class: "media-stage", src: url, aria_label: prompt,
                            controls: true, preload: "metadata", playsinline: true, autoplay:play,
                            onmounted:move |event|video.set(event.as_web_event().dyn_into::<HtmlMediaElement>().ok()),
                            onplay:move |_|media_started(video),
                            onerror: move |_| failed.set(true),
                        }
                    },
                    MediaKind::Audio => rsx! {
                        AudioPlayer { url, label: prompt, play, on_error: move |_| failed.set(true) }
                    },
                }
            }
            figcaption { class: "media-actions",
                span { class: "media-filename", title: "{path}", "{filename}" }
                FileActions { path: path.clone() }
            }
        }
    }
}

/// Нативные play/seek/таймер дополняет интерактивная волна из самого файла.
#[component]
fn AudioPlayer(url: String, label: String, play: bool, on_error: Callback<()>) -> Element {
    let state = use_context::<AppState>();
    let mut audio = use_media_node();
    let mut canvas = use_signal(|| None::<HtmlCanvasElement>);
    let mut progress = use_signal(|| 0.0);
    let peaks = use_resource(use_reactive((&url,), |(url,)| async move {
        // Отказ декодера волны не должен мешать нативному плееру.
        waveform_peaks(&url).await.ok()
    }));
    use_effect(move || {
        (state.theme)();
        (state.system_dark)();
        if let Some(canvas) = canvas.read().as_ref()
            && let Some(Some(peaks)) = peaks.read().as_ref()
        {
            draw_waveform(canvas, peaks, progress());
        }
    });
    let has_wave = peaks.read().as_ref().is_some_and(Option::is_some);

    rsx! {
        if has_wave {
            canvas {
                class: "audio-wave", width: "960", height: "80", aria_hidden: "true",
                onmounted: move |event| {
                    canvas.set(event.as_web_event().dyn_into::<HtmlCanvasElement>().ok());
                },
                onclick: move |event| {
                    if let (Some(canvas), Some(audio)) = (canvas.peek().as_ref(), audio.peek().as_ref()) {
                        let width = f64::from(canvas.client_width());
                        let duration = audio.duration();
                        if width > 0.0 && duration.is_finite() && duration > 0.0 {
                            audio.set_current_time((event.element_coordinates().x / width).clamp(0.0, 1.0) * duration);
                        }
                    }
                },
            }
        }
        audio {
            class: "media-audio", src: url, aria_label: label, controls: true, preload: "metadata", autoplay:play,
            onmounted: move |event| audio.set(event.as_web_event().dyn_into::<HtmlMediaElement>().ok()),
            onplay:move |_|media_started(audio),
            onerror: move |_| on_error.call(()),
            ontimeupdate: move |_| {
                if let Some(audio) = audio.peek().as_ref() {
                    let duration = audio.duration();
                    if duration > 0.0 && duration.is_finite() {
                        progress.set((audio.current_time() / duration).clamp(0.0, 1.0));
                    }
                }
            },
        }
    }
}

/// Низкая частота дискретизации достаточна для столбиков и экономит память.
async fn waveform_peaks(url: &str) -> Result<Vec<f32>, JsValue> {
    const BARS: usize = 120;
    let window = web_sys::window().ok_or(JsValue::NULL)?;
    let response = JsFuture::from(window.fetch_with_str(url))
        .await?
        .dyn_into::<Response>()?;
    if !response.ok() {
        return Err(JsValue::NULL);
    }
    let bytes = JsFuture::from(response.array_buffer()?).await?.dyn_into()?;
    let decoder =
        OfflineAudioContext::new_with_number_of_channels_and_length_and_sample_rate(1, 1, 8_000.0)?;
    let buffer = JsFuture::from(decoder.decode_audio_data(&bytes)?)
        .await?
        .dyn_into::<AudioBuffer>()?;
    let count = BARS.min(buffer.length() as usize);
    if count == 0 {
        return Err(JsValue::NULL);
    }
    let mut peaks = vec![0.0_f32; count];
    for channel in 0..buffer.number_of_channels() {
        let samples = buffer.get_channel_data(channel)?;
        let step = samples.len() as f64 / count as f64;
        for (index, peak) in peaks.iter_mut().enumerate() {
            let start = (index as f64 * step) as usize;
            let end = (((index + 1) as f64 * step) as usize).min(samples.len());
            *peak = samples[start..end]
                .iter()
                .fold(*peak, |p, sample| p.max(sample.abs()));
        }
    }
    Ok(peaks)
}

fn draw_waveform(canvas: &HtmlCanvasElement, peaks: &[f32], progress: f64) {
    let Ok(Some(context)) = canvas.get_context("2d") else {
        return;
    };
    let Ok(context) = context.dyn_into::<CanvasRenderingContext2d>() else {
        return;
    };
    let width = f64::from(canvas.width());
    let height = f64::from(canvas.height());
    let step = width / peaks.len() as f64;
    let loudest = peaks.iter().copied().fold(f32::EPSILON, f32::max);
    context.clear_rect(0.0, 0.0, width, height);
    context.set_fill_style_str("currentColor");
    for (index, peak) in peaks.iter().enumerate() {
        let x = index as f64 * step;
        let bar = (f64::from(peak / loudest) * height).max(1.0);
        context.set_global_alpha(if x < progress * width { 1.0 } else { 0.45 });
        context.fill_rect(x, (height - bar) / 2.0, (step - 2.0).max(1.0), bar);
    }
}
