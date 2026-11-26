//! Входные изображения Студии: первый кадр, референсы редактирования и PNG-маска.

use dioxus::prelude::*;
use km_shared::MAX_REQUEST_WIRE_BYTES;
use km_shared::image::inline_bytes;
use km_shared::{GenMode, MediaKind, Provider, Reference};
use km_ui_core::i18n::tr;
use km_ui_core::studio::UploadKind;

use crate::components::upload::{FilePicker, attach_edit_batch};
use crate::state::{AppState, StudioDraft, use_t};

pub(super) fn file_mode(mode: GenMode) -> Option<(GenMode, UploadKind)> {
    match mode {
        GenMode::TextToImage | GenMode::ImageEdit => {
            Some((GenMode::ImageEdit, UploadKind::EditSource))
        }
        GenMode::TextToVideo | GenMode::ImageToVideo => {
            Some((GenMode::ImageToVideo, UploadKind::Image))
        }
        GenMode::ReferenceToVideo => Some((mode, UploadKind::Image)),
        _ => None,
    }
}

/// Вызывается только после проверки всего набора. Возвращает файл для подтверждения,
/// если в отдельном черновике видео уже указан другой первый кадр.
pub(super) fn accept_files(
    state: AppState,
    kind: MediaKind,
    source: StudioDraft,
    urls: Vec<String>,
    replace: bool,
) -> Result<Option<String>, &'static str> {
    let mut section = state.studio(kind);
    if state.settings.peek().provider != Provider::CheaperInference
        || section.active.peek().mode != source.mode
    {
        return Ok(None);
    }
    let (target_mode, _) = file_mode(source.mode).ok_or("controls.upload_type")?;
    if target_mode == source.mode {
        add_sources(source, urls)?;
        return Ok(None);
    }
    let mut target = section
        .drafts
        .read()
        .iter()
        .find(|d| d.mode == target_mode)
        .copied()
        .ok_or("controls.upload_type")?;
    if target_mode == GenMode::ImageToVideo && !replace {
        let refs = target.references.peek();
        if let Some(stored) = refs.first()
            && urls.first() != Some(&stored.data_url)
        {
            return Ok(urls.into_iter().next());
        }
    }
    add_sources(target, urls)?;
    if target.prompt.peek().trim().is_empty() {
        let text = source.prompt.peek().clone();
        if !text.is_empty() {
            target.prompt.set(text);
        }
    }
    section.active.set(target);
    Ok(None)
}

fn add_sources(mut draft: StudioDraft, urls: Vec<String>) -> Result<(), &'static str> {
    if draft.mode == GenMode::ImageEdit {
        return attach_edit_batch(&mut draft, urls);
    }
    let added = urls.into_iter().map(|data_url| Reference { data_url });
    if draft.mode == GenMode::ImageToVideo {
        draft.references.set(added.collect());
    } else {
        draft.references.write().extend(added);
    }
    Ok(())
}

#[component]
pub(super) fn StudioUploads(mut draft: StudioDraft, reading: Signal<bool>) -> Element {
    let mode = draft.mode;
    let first_label = use_t("studio.first_frame");
    let reference_label = use_t("studio.add_reference");
    let reference_alt = use_t("studio.reference");
    let mask_label = use_t("studio.mask");
    let state = use_context::<AppState>();
    let mut error = use_signal(|| None::<&'static str>);
    let add_source = use_callback(move |urls: Vec<String>| {
        error.set(add_sources(draft, urls).err());
    });
    let references = draft.references.read();
    rsx! {
        div { class: "studio-uploads",
            if mode == GenMode::ImageToVideo {
                SingleUpload {
                    label: first_label, kind: UploadKind::Image,reading,
                    value: references.first().map(|r| r.data_url.clone()),
                    on_set: move |url: Option<String>| {
                        draft.references.set(url.into_iter().map(|data_url| Reference { data_url }).collect());
                    },
                }
            } else {
                div { class: "studio-upload-slot",
                    FilePicker {
                        kind: if mode == GenMode::ImageEdit { UploadKind::EditSource } else { UploadKind::Image },
                        label: reference_label.to_owned(), multiple:true,reading,on_load: add_source,
                    }
                    SourceUrl { on_add: move |url|add_source.call(vec![url]) }
                    if !references.is_empty() {
                        div { class: "reference-list",
                            for (index, reference) in references.iter().enumerate() {
                                ReferencePreview {
                                    key: "{index}", src: reference.data_url.clone(),
                                    label: format!("{reference_alt} {}", index + 1),
                                    on_remove: move |_| { draft.references.write().remove(index); error.set(None); },
                                }
                            }
                        }
                    }
                }
                if mode == GenMode::ImageEdit {
                    details { class:"generator-mask",
                      summary { {mask_label} if draft.mask.read().is_some() {" ✓"} }
                      SingleUpload {
                        label: mask_label, kind: UploadKind::PngMask, reading, value: (draft.mask)(),
                        on_set: move |url: Option<String>| {
                            let size = draft.references.peek().iter().map(|r| inline_bytes(&r.data_url)).sum::<usize>();
                            if url.as_deref().is_some_and(|mask| size + inline_bytes(mask) > MAX_REQUEST_WIRE_BYTES) {
                                error.set(Some("controls.edit_wire_limit"));
                            } else {
                                error.set(None);
                                draft.mask.set(url);
                            }
                        },
                      }
                    }
                }
            }
            if let Some(key) = error() {
                p { class: "status status-error", role: "alert", {tr((state.lang)(), key)} }
            }
        }
    }
}

#[component]
fn SourceUrl(on_add: Callback<String>) -> Element {
    let label = use_t("controls.image_url");
    let add = use_t("controls.add_url");
    let invalid = use_t("controls.invalid_url");
    let mut value = use_signal(String::new);
    let mut error = use_signal(|| false);
    rsx! {
        details { class: "extra-params",
            summary { {label} }
            div { class: "field-row",
                input { class: "text-input", r#type: "url", aria_label: label, value: value(),
                    placeholder: "https://…", oninput: move |event| { error.set(false); value.set(event.value()); } }
                button { r#type: "button", class: "btn", disabled: value.read().trim().is_empty(),
                    onclick: move |_| {
                        let raw = value.peek().trim().to_owned();
                        if web_sys::Url::new(&raw).is_ok_and(|url| matches!(url.protocol().as_str(), "http:" | "https:") && !url.hostname().is_empty()) {
                            on_add.call(raw);
                            error.set(false);
                            value.set(String::new());
                        } else { error.set(true); }
                    }, {add} }
            }
            if error() { p { class: "status-error", role: "alert", {invalid} } }
        }
    }
}

#[component]
fn SingleUpload(
    label: &'static str,
    kind: UploadKind,
    reading: Signal<bool>,
    value: Option<String>,
    on_set: Callback<Option<String>>,
) -> Element {
    rsx! {
        div { class: "studio-upload-slot",
            FilePicker { kind, reading,label: label.to_owned(), on_load: move |urls:Vec<String>| on_set.call(urls.into_iter().next()) }
            if kind == UploadKind::Image {
                SourceUrl { on_add: move |url| on_set.call(Some(url)) }
            }
            if let Some(src) = value {
                ReferencePreview { src, label: label.to_owned(), on_remove: move |_| on_set.call(None) }
            }
        }
    }
}

#[component]
fn ReferencePreview(src: String, label: String, on_remove: Callback<()>) -> Element {
    let remove = use_t("studio.remove_attachment");
    // Добавленные через форму данные уже проверены FilePicker. История также
    // может содержать старые адреса: показываем только встроенные растры.
    rsx! {
        div { class: "reference-preview",
            crate::components::upload::ReferencePreviewContent { src, label:label.clone() }
            button {
                r#type: "button", class: "btn", aria_label: "{remove} · {label}", title: remove,
                onclick: move |_| on_remove.call(()), "×"
            }
        }
    }
}
