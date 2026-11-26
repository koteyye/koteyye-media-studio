//! Рабочая область Студии: формы восьми режимов, результат и недавние работы.

use dioxus::prelude::*;
use dioxus::web::WebFileExt;
use km_shared::video::VideoLimits;
use km_shared::{GenMode, GenParams, JobStatus, Lang, MediaKind, Provider};
use km_ui_core::i18n::tr;
use km_ui_core::studio::{
    PriceUnit, estimate_generation_price, format_price, models_for, starting_rate,
};
use km_ui_core::studio_results::{self, StudioResult, Submission};

use crate::components::{
    chips::ParamChips,
    model_dropdown::ModelDropdown,
    player::{MediaSeries, display_date, display_timestamp},
    recent::RecentStrip,
    upload::{FilePicker, is_file_drag, read_batch},
};
use crate::router::{Route, parse_kind};
use crate::state::{AppState, StudioDraft};

#[path = "studio_uploads.rs"]
mod uploads;
use uploads::StudioUploads;

/// Параметр маршрута выбирает вид медиа, черновики принадлежат AppState.
#[component]
pub fn Studio(kind: String) -> Element {
    let kind = parse_kind(&kind);
    rsx! { for section_kind in [kind] { Workspace { key: "{section_kind:?}", kind:section_kind } } }
}

#[component]
fn Workspace(kind: MediaKind) -> Element {
    let state = use_context::<AppState>();
    use_effect(move || state.visit_media(kind));
    let mut section = state.studio(kind);
    let draft = (section.active)();
    let lang = (state.lang)();
    let mode = draft.mode;
    let title = match kind {
        MediaKind::Image => tr(lang, "studio.image"),
        MediaKind::Video => tr(lang, "studio.video"),
        MediaKind::Audio => tr(lang, "studio.audio"),
    };
    let provider = state.settings.read().provider;
    let nav = use_navigator();
    let drafts = section.drafts.read();
    rsx! {
        section { class: "studio generator-workspace",
            header {class:"generator-heading",
                h1 {span {class:"workspace-narrow-brand","Koteyye Media Studio v2 /"} {title} }
                Link {to:Route::Settings{},class:"generator-provider",title:tr(lang,"gen.open_settings"),{provider.name()}
                    span {class:"studio-icon","data-icon":"chevron",aria_hidden:"true"}
                }
                button {class:"generator-find",r#type:"button",title:tr(lang,"gen.find"),aria_label:tr(lang,"gen.find"),
                    onclick:move |_|{state.search_history();nav.push(Route::History{});},
                    span {class:"studio-icon","data-icon":"search",aria_hidden:"true"} span { {tr(lang,"gen.find")} }
                }
            }
            if provider==Provider::CheaperInference {
              div {class:"generator-composer",
               div { class: "mode-tabs", role: "group", aria_label: title,
                for tab in drafts.iter().copied() {
                    button {
                        key: "{tab.mode:?}", r#type: "button", class: "mode-tab",
                        aria_pressed: "{mode == tab.mode}",
                        onclick: move |_| section.active.set(tab),
                        {tab_label(lang, tab.mode)}
                    }
                }
               }
               for active in [draft] {
                   StudioForm { key: "{active.mode:?}", kind,draft:active }
               }
              }
              StudioResults {kind, provider }
            } else {
                div { class:"generator-planned",
                    h2 {{tr(lang,"gen.or_title")}} p {{tr(lang,"gen.or_note")}}
                    button {class:"btn",r#type:"button",disabled:!state.media_idle(),
                        onclick:move |_|{
                            let mut settings=state.settings.peek().clone();
                            settings.provider=Provider::CheaperInference;
                            wasm_bindgen_futures::spawn_local(async move{let _=state.persist_settings(settings).await;});
                        }, {tr(lang,"gen.select_ci")}
                    }
                    Link {to:Route::Settings{},class:"btn", {tr(lang,"gen.open_settings")} }
                }
            }
        }
    }
}

fn tab_label(lang: Lang, mode: GenMode) -> &'static str {
    tr(
        lang,
        match mode {
            GenMode::TextToImage | GenMode::TextToVideo => "studio.tab.from_text",
            GenMode::ImageEdit => "studio.tab.edit",
            GenMode::ImageToVideo => "studio.tab.from_image",
            GenMode::ReferenceToVideo => "studio.tab.references",
            GenMode::Speech => "studio.tab.speech",
            GenMode::Music => "studio.mode.music",
            GenMode::SoundEffects => "studio.tab.sounds",
        },
    )
}

fn prompt_hint(lang: Lang, mode: GenMode) -> &'static str {
    tr(
        lang,
        match mode {
            GenMode::TextToImage => "studio.hint.image",
            GenMode::ImageEdit => "studio.hint.edit",
            GenMode::TextToVideo | GenMode::ImageToVideo | GenMode::ReferenceToVideo => {
                "studio.hint.video"
            }
            GenMode::Speech => "studio.hint.speech",
            GenMode::Music => "studio.hint.music",
            GenMode::SoundEffects => "studio.hint.sounds",
        },
    )
}

#[component]
fn StudioForm(kind: MediaKind, mut draft: StudioDraft) -> Element {
    let mut state = use_context::<AppState>();
    let lang = (state.lang)();
    let mut reloading = use_signal(|| false);
    let mut dropping = use_signal(|| false);
    let mut dragging = use_signal(|| false);
    let mode = draft.mode;
    // И смена черновика, и асинхронная загрузка каталога могут первыми сделать модель доступной.
    use_effect(use_reactive((&kind, &draft), move |(kind, draft)| {
        if let Some(models) = state.models_by_kind.read().get(&kind) {
            draft.choose_initial_model(kind, models);
        }
    }));
    let requested = (draft.model)();
    let prompt = draft.prompt.read();
    let params = (draft.params)();
    let catalogs = state.models_by_kind.read();
    let compatible = models_for(
        catalogs.get(&kind).map(Vec::as_slice).unwrap_or_default(),
        kind,
        mode,
    );
    // Первый доступный вариант удобен для новой формы. При повторе нельзя
    // незаметно подменить исчезнувшую модель: другая модель имеет другую цену.
    let selected = compatible.iter().copied().find(|m| m.id == requested);
    let missing_model = !requested.is_empty() && selected.is_none() && catalogs.contains_key(&kind);
    let video_limits = VideoLimits::for_model(&requested, mode);
    let price = selected
        .filter(|_| video_limits.is_none_or(|limits| limits.supports(&params)))
        .and_then(|m| estimate_generation_price(m, &params, mode, &prompt));
    let audio_second_rate = selected
        .and_then(starting_rate)
        .filter(|(unit, _)| kind == MediaKind::Audio && *unit == PriceUnit::Second);
    let needs_reference = matches!(
        mode,
        GenMode::ImageEdit | GenMode::ImageToVideo | GenMode::ReferenceToVideo
    );
    let attachment = uploads::file_mode(mode);
    let quick = attachment.filter(|(target, _)| *target != mode);
    let mut replace_first = use_signal(|| None::<String>);
    let finish_files = use_callback(move |(urls, replace): (Vec<String>, bool)| {
        let section = state.studio(kind);
        let previous = section.active.peek().mode;
        match uploads::accept_files(state, kind, draft, urls, replace) {
            Ok(conflict) => {
                replace_first.set(conflict);
                if section.active.peek().mode != previous {
                    focus_composer();
                }
            }
            Err(key) => state.set_error(Some(tr(*state.lang.peek(), key).into())),
        }
    });
    let missing_reference = needs_reference && draft.references.read().is_empty();
    let missing_duration = kind == MediaKind::Video
        && !params
            .duration_seconds
            .is_some_and(|d| d.is_finite() && d > 0.0);
    let invalid_video_params = video_limits.is_some_and(|limits| !limits.supports(&params));
    let invalid_video_references =
        video_limits.is_some_and(|limits| !limits.supports_references(&draft.references.read()));
    let missing_folder = state.settings.read().local_folder.is_empty();
    let limit = match (kind, mode) {
        (MediaKind::Audio, _) => Some(4_096),
        (_, GenMode::ImageEdit) => Some(32_000),
        _ => None,
    };
    let count = prompt.chars().count();
    let too_long = limit.is_some_and(|max| count > max);
    let running_mode = state.generating.read().get(&kind).copied();
    let pending = running_mode == Some(mode);
    let can_submit = selected.is_some()
        && !prompt.trim().is_empty()
        && !too_long
        && !missing_reference
        && !missing_duration
        && !invalid_video_params
        && !invalid_video_references
        && !missing_folder
        && running_mode.is_none()
        && !dropping()
        && replace_first.read().is_none()
        && !state.media_locked();
    let prompt_label = if mode == GenMode::Speech {
        tr(lang, "studio.speech_text")
    } else {
        tr(lang, "studio.prompt")
    };
    let button_label = if pending {
        tr(lang, "studio.generating")
    } else {
        tr(lang, "common.generate")
    };
    let shortcut = state.settings.read().workspace.generation_shortcut;
    let submit = use_callback(move |()| {
        if !can_submit
            || state.settings.peek().provider != Provider::CheaperInference
            || state.media_locked()
            || state.generating.peek().contains_key(&kind)
        {
            return;
        }
        state.submit(draft.request(kind));
    });
    rsx! {
        div { class: "studio-form",
            div {class:"generator-editor",class:if quick.is_some(){"quick-attach"}else{""},class:if dragging(){"dragging"}else{""},
                ondragover:move |e|{if attachment.is_some() && is_file_drag(&e){e.prevent_default();dragging.set(true);}},
                ondragleave:move |_|dragging.set(false),
                ondrop:move |e|{
                    let Some((target,file_kind))=attachment else{return;};
                    let files=e.data_transfer().files();
                    if files.is_empty(){return;}
                    e.prevent_default();dragging.set(false);
                    if *dropping.peek(){return;}
                    let Some(files)=files.iter().map(WebFileExt::get_web_file).collect::<Option<Vec<_>>>()
                        else {state.set_error(Some(tr(lang,"controls.upload_read").into()));return;};
                    dropping.set(true);
                    spawn(async move{
                        match read_batch(files,file_kind,target!=GenMode::ImageToVideo).await {
                            Ok(urls)=>finish_files.call((urls,false)),
                            Err(key)=>state.set_error(Some(tr(*state.lang.peek(),key).to_owned())),
                        }
                        dropping.set(false);
                    });
                },
             if needs_reference { StudioUploads { draft,reading:dropping } }
             else if let Some((target,file_kind))=quick {
                div {class:"generator-quick-attach",
                    FilePicker {kind:file_kind,multiple:target!=GenMode::ImageToVideo,reading:dropping,
                        label:tr(lang,if target==GenMode::ImageEdit{"studio.add_reference"}else{"gen.add_first_frame"}).to_owned(),
                        described_by: "studio-quick-attachment-hint",
                        on_load:move |urls|finish_files.call((urls,false))}
                    p { id:"studio-quick-attachment-hint", class:"generator-attachment-hint",
                        {tr(lang, if target==GenMode::ImageEdit {"gen.quick_image_hint"} else {"gen.quick_video_hint"})}
                    }
                }
             }
            textarea {
                class: "studio-prompt", id: "studio-prompt", rows: 2, aria_label: prompt_label,
                value: "{prompt}", placeholder: prompt_hint(lang, mode),
                aria_invalid: "{too_long}",
                oninput: move |event| draft.prompt.set(event.value()),
                onkeydown:move |e|{if shortcut && e.key()==Key::Enter && (e.modifiers().ctrl()||e.modifiers().meta()) && !e.modifiers().shift() && !e.modifiers().alt() && !e.is_composing(){
                    e.prevent_default();if !e.is_auto_repeating(){submit.call(());}
                }},
            }
            }
            if replace_first.read().is_some(){
                div {class:"generator-replace-first", role:"group",aria_label:tr(lang,"gen.replace_frame"),
                    span {{tr(lang,"gen.frame_exists")}}
                    button {r#type:"button",class:"btn",onclick:move |_|{
                        let url=replace_first.write().take();
                        if let Some(url)=url{finish_files.call((vec![url],true));}
                    },{tr(lang,"gen.replace_frame")}}
                    button {r#type:"button",class:"btn",
                        onmounted:move |e|async move{let _=e.set_focus(true).await;},
                        onclick:move |_|{replace_first.set(None);focus_composer();},{tr(lang,"history.cancel")}}
                }
            }
            if let Some(max) = limit {
                p { class: if too_long { "status status-error" } else { "status status-muted" }, "{count} / {max}" }
            }
            div {class:"generator-toolbar",
             div { class: "studio-controls",
                ModelDropdown {
                    kind, mode, selected: requested,
                    on_select: move |id| draft.select_model(id),
                }
                ParamChips { mode, model: selected.cloned(), params: params.clone(), on_change: move |params| draft.params.set(params) }
             }
             div { class: "studio-submit",
                if let Some(amount) = price {
                    span { class: "generator-estimate",id:"generator-price",title:tr(lang,"studio.estimate"), aria_label:format!("{}: {}",tr(lang,"studio.estimate"),format_price(amount)), "≈ {format_price(amount)}" }
                } else if let Some((_, rate))=audio_second_rate {
                    span {class:"generator-estimate",id:"generator-price", {format_price(rate)} " / " {tr(lang,"controls.per_second")} " · " {tr(lang,"studio.output_duration")} }
                }
                button {class:"btn btn-primary generator-submit",r#type:"button",disabled:!can_submit,aria_busy:"{pending}",
                    aria_label:button_label,title:button_label,aria_describedby:(price.is_some()||audio_second_rate.is_some()).then_some("generator-price"),onclick:move |_|submit.call(()),
                    span {{button_label}} span {class:"studio-icon","data-icon":"right",aria_hidden:"true"}
                }
             }
            }
            if missing_model {
                p { class: "status status-error", {tr(lang, "studio.model_gone")} }
            }
            if missing_duration {
                p { class: "status status-error", {tr(lang, "studio.duration_required")} }
            } else if invalid_video_params {
                p { class: "status status-error", {tr(lang, "controls.video_model_limits")} }
            }
            if invalid_video_references {
                p { class: "status status-error", {tr(lang, "controls.veo_references")} }
            }
            if compatible.is_empty() && !(state.busy)() {
                button {
                    r#type: "button", class: "btn", disabled: reloading(),
                    onclick: move |_| {
                        reloading.set(true);
                        state.background_error.set(None);
                        spawn(async move {
                            state.refresh_models(kind).await;
                            reloading.set(false);
                        });
                    },
                    {tr(lang, "studio.reload_models")}
                }
            }
            if missing_folder && !(state.busy)() {
                Link {to:Route::Settings{},class:"generator-folder",{tr(lang,"studio.choose_folder")}}
            }
        }
    }
}

#[component]
fn StudioResults(kind: MediaKind, provider: Provider) -> Element {
    let state = use_context::<AppState>();
    let lang = (state.lang)();
    let mut playback = use_signal(|| (String::new(), 0u32));
    let history = state.history.read();
    let id = state.selected_result(kind);
    let running =
        provider == Provider::CheaperInference && state.generating.read().contains_key(&kind);
    let submission_signal = state.studio(kind).last_submission;
    let submission = submission_signal.read();
    let selection = studio_results::select(
        &history,
        kind,
        provider,
        id.as_deref(),
        submission.as_ref(),
        running,
    );
    let visible_id = selection.record().map(|r| r.id.clone());
    let single = state.settings.read().workspace.single_image;
    let reduced = state.settings.read().workspace.reduce_motion;
    let layout = move |value| {
        let mut settings = state.settings.peek().clone();
        if settings.workspace.single_image != value {
            settings.workspace.single_image = value;
            wasm_bindgen_futures::spawn_local(async move {
                let _ = state.persist_settings(settings).await;
            });
        }
    };
    let selected_play = playback.read();
    rsx! {
        section {class:"generator-results",
            header {class:"generator-feed-heading",h2 {{tr(lang,"gen.results")}}
                if kind==MediaKind::Image {
                    div {class:"generator-view-mode",role:"group",aria_label:tr(lang,"gen.results"),
                        for (value,key,icon) in [(false,"gen.grid","layers"),(true,"gen.one","expand")] {
                            button {key:"{key}",r#type:"button",class:"generator-icon-button", title:tr(lang,key),aria_label:tr(lang,key),
                                aria_pressed:"{single==value}",onclick:move |_|layout(value),
                                span {class:"studio-icon","data-icon":icon,aria_hidden:"true"}
                            }
                        }
                    }
                }
            }
            if let Some(record)=selection.record() {
                ResultCard {key:"{record.id}",id:record.id.clone(),single,
                    play:if selected_play.0==record.id{selected_play.1}else{0},
                    current:matches!(selection,StudioResult::Submitted(_)), running, reduced}
            } else if matches!(selection,StudioResult::Waiting) {
                WaitPanel { running:true,reduced }
            } else if matches!(selection,StudioResult::NoReceipt) {
                if let Some(Submission::Awaiting{model,mode,error,started_at,..})=submission.as_ref(){
                    FailureCard {provider,model:model.clone(),mode:*mode,current:true,created:*started_at,before_record:true,
                        diagnostic:error.clone(),request_id:studio_results::failed_request_id(None,error.as_deref()).map(str::to_owned)}
                }
            }
            RecentStrip {kind,selected:visible_id,
                on_select:move|id|{playback.set((String::new(),0));state.select_result(kind,Some(id));},
                on_play:move|id:String|{let next=playback.peek().1.wrapping_add(1);playback.set((id.clone(),next));state.select_result(kind,Some(id));}
            }
        }
    }
}

#[component]
fn WaitPanel(running: bool, reduced: bool) -> Element {
    let state = use_context::<AppState>();
    let lang = (state.lang)();
    rsx! {div {class:if reduced{"generator-wait quiet"}else{"generator-wait"},role:"status",aria_busy:"true",
        span {class:"generator-wait-mark",aria_hidden:"true",i {} i {} i {} }
        strong {{tr(lang,if running{"gen.wait"}else{"gen.pending"})}}
    }}
}

#[derive(serde::Deserialize, Default)]
struct ReceiptForm<'a> {
    #[serde(default)]
    params: Option<GenParams>,
    #[serde(default, borrow)]
    references: Vec<ReceiptReference<'a>>,
    #[serde(default, borrow)]
    mask_data_url: Option<std::borrow::Cow<'a, str>>,
}
#[derive(serde::Deserialize)]
struct ReceiptReference<'a> {
    #[serde(borrow)]
    data_url: std::borrow::Cow<'a, str>,
}

#[component]
fn ResultCard(
    id: String,
    single: bool,
    play: u32,
    current: bool,
    running: bool,
    reduced: bool,
) -> Element {
    let state = use_context::<AppState>();
    let lang = (state.lang)();
    let history = state.history.read();
    let Some(record) = history.iter().find(|r| r.id == id) else {
        return rsx! {};
    };
    let snapshot: ReceiptForm = serde_json::from_str(&record.params_json).unwrap_or_default();
    let params = snapshot
        .params
        .or_else(|| serde_json::from_str::<GenParams>(&record.params_json).ok())
        .unwrap_or_default();
    let dimensions = params
        .size
        .as_deref()
        .or(params.aspect_ratio.as_deref())
        .or(params.response_format.as_deref());
    let status = record.status;
    let provider = record.provider;
    let editable = provider == Provider::CheaperInference;
    let pending_check = editable && state.media_idle() && status == JobStatus::Pending;
    rsx! {
      article {class:"generator-run","data-run-id":id.clone(),"data-result-origin":if current{"submission"}else{"history"},
        div {class:"generator-output",
            match status {
                JobStatus::Completed=>rsx!{MediaSeries {key:"{id}-{play}", id:id.clone(),kind:record.kind,paths:record.local_paths.clone(),prompt:record.prompt.clone(),
                    single, aspect:image_aspect(&params),reference:editable,play:play>0 } },
                JobStatus::Pending=>rsx!{
                    WaitPanel {running:running&&current,reduced}
                    if let Some(reason)=record.error.as_deref(){p {class:"status-muted", {km_ui_core::studio::pending_reason(lang,reason)} }}
                    if pending_check {button {class:"btn",r#type:"button",onclick:move |_|state.check_pending(), {tr(lang,"studio.check_result")} }}
                },
                JobStatus::Failed=>rsx!{FailureCard {provider,model:record.model.clone(),mode:record.mode,current,
                    created:record.created_at,diagnostic:record.error.clone(),
                    request_id:studio_results::failed_request_id(record.request_id.as_deref(),record.error.as_deref()).map(str::to_owned)}},
            }
        }
        aside {class:"generator-details",
            div {class:"generator-run-label",span {{tab_label(lang,record.mode)}} time {{display_date(record.created_at,lang)}}}
            p {class:"generator-sent-prompt","{record.prompt}"}
            div {class:"generator-meta",
                span {"{record.model}"}
                if let Some(size)=dimensions { span {"{size}"} }
                if let Some(seconds)=params.duration_seconds.filter(|d|d.is_finite()&&*d>0.0) {span {"{seconds} " {tr(lang,"controls.per_second")}}}
                if record.local_paths.len()>1 {span {{tr(lang,"gen.files")} " · {record.local_paths.len()}"}}
                if let Some(cost)=record.cost_usd { span {{km_ui_core::format::format_usd(cost)}} }
            }
            if !snapshot.references.is_empty() || snapshot.mask_data_url.is_some() {
                details {class:"generator-source-details",
                    summary {{tr(lang,"gen.refs")} " · {snapshot.references.len()}"}
                    div {class:"generator-saved-refs",
                        for (i,reference) in snapshot.references.iter().enumerate() {
                            SourceThumbnail {key:"{i}",url:reference.data_url.to_string(),label:format!("{} {}",tr(lang,"studio.reference"),i+1)}
                        }
                        if let Some(mask)=snapshot.mask_data_url.as_deref(){
                            SourceThumbnail {url:mask.to_owned(),label:tr(lang,"gen.mask").to_owned()}
                        }
                    }
                }
            }
            button {class:"btn generator-reuse",r#type:"button",disabled:!editable,
                onclick:move |_|{
                    let records=state.history.peek();
                    if let Some(r)=records.iter().find(|r|r.id==id) {
                        if let Err(error)=state.repeat_record(r){state.set_error(Some(error));}
                        else {focus_composer();}
                    }
                },
                span {class:"studio-icon","data-icon":"reuse",aria_hidden:"true"} {tr(lang,"gen.repeat")}
            }
        }
      }
    }
}

#[component]
fn FailureCard(
    provider: Provider,
    model: String,
    mode: GenMode,
    current: bool,
    created: i64,
    diagnostic: Option<String>,
    request_id: Option<String>,
    #[props(default)] before_record: bool,
) -> Element {
    let state = use_context::<AppState>();
    let lang = (state.lang)();
    rsx! {
        div {class:"generator-result-failure",role:"status","data-error-origin":if current{"submission"}else{"history"},
            strong { {tr(lang,if current{"gen.failure.sent"}else{"gen.failure.stored"})} }
            div {class:"generator-failure-source",
                span { {provider.name()} } span {"{model}"} span {{tab_label(lang,mode)}}
                time {{display_timestamp(created,lang)}}
            }
            p { {tr(lang,if diagnostic.is_none()&&before_record{"gen.no_receipt"}
                else{studio_results::failure_explanation(diagnostic.as_deref())})} }
            if let Some(id)=request_id { p {class:"generator-failure-id","Request ID: " code {"{id}"} } }
            if let Some(message)=diagnostic.filter(|s|!s.is_empty()) {
                details {open:before_record||message.len()<=320,summary {{tr(lang,"gen.failure.diagnostic")}} pre {"{message}"}}
            }
        }
    }
}

fn focus_composer() {
    use wasm_bindgen::{JsCast, closure::Closure};
    let Some(window) = web_sys::window() else {
        return;
    };
    let cb = Closure::once_into_js(|| {
        if let Some(node) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("studio-prompt"))
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = node.focus();
        }
    });
    let _ = window.request_animation_frame(cb.unchecked_ref());
}

#[component]
fn SourceThumbnail(url: String, label: String) -> Element {
    rsx! {div {class:"generator-source-thumbnail", crate::components::upload::ReferencePreviewContent {src:url,label} }}
}

fn image_aspect(params: &GenParams) -> f64 {
    let pair = params
        .size
        .as_deref()
        .and_then(|s| s.split_once('x'))
        .or_else(|| params.aspect_ratio.as_deref()?.split_once(':'));
    pair.and_then(|(w, h)| {
        let width = w.parse::<u32>().ok()?;
        let height = h.parse::<u32>().ok()?;
        (width > 0 && height > 0).then(|| f64::from(width) / f64::from(height))
    })
    .unwrap_or(1.5)
}
