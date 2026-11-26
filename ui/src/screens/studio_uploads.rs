//! Вложения изображений и видео в единой форме студии.

use dioxus::prelude::*;
use km_shared::MAX_REQUEST_WIRE_BYTES;
use km_shared::image::inline_bytes;
use km_shared::video::VideoLimits;
use km_shared::{CatalogModel, GenMode, MediaKind, Provider, Reference};
use km_ui_core::studio::{MAX_OPENROUTER_EDIT_SOURCES, UploadKind, mode_for_input};

use crate::components::upload::{FilePicker, attach_edit_batch};
use crate::state::{AppState, StudioDraft, use_t};

pub(super) fn file_mode(
    provider: Provider,
    kind: MediaKind,
    model: Option<&CatalogModel>,
) -> Option<UploadKind> {
    let model = model.filter(|model| model.matches_kind(kind))?;
    match kind {
        MediaKind::Image if model.supports_mode(GenMode::ImageEdit) => Some(match provider {
            Provider::CheaperInference => UploadKind::EditSource,
            Provider::OpenRouter => UploadKind::OpenRouterEditSource,
        }),
        MediaKind::Video
            if model.supports_mode(GenMode::ImageToVideo)
                || model.supports_mode(GenMode::ReferenceToVideo) =>
        {
            Some(UploadKind::Image)
        }
        _ => None,
    }
}

pub(super) fn allows_multiple(kind: MediaKind, model: &CatalogModel) -> bool {
    match kind {
        MediaKind::Image => true,
        MediaKind::Video => model.supports_mode(GenMode::ReferenceToVideo),
        MediaKind::Audio => false,
    }
}

pub(crate) fn accept_files(
    mut draft: StudioDraft,
    kind: MediaKind,
    model: &CatalogModel,
    urls: Vec<String>,
) -> Result<(), &'static str> {
    if urls.is_empty() || *draft.model.peek() != model.id {
        return Ok(());
    }
    file_mode(draft.provider, kind, Some(model)).ok_or("controls.model_no_references")?;
    let previous_mode = draft.mode_for(kind, Some(model));
    if kind == MediaKind::Image {
        if let Some(options) = &model.openrouter {
            let max = options
                .image_parameters
                .get("input_references")
                .and_then(|parameter| parameter.max.as_deref()?.parse::<usize>().ok())
                .unwrap_or(MAX_OPENROUTER_EDIT_SOURCES);
            if draft.references.peek().len() + urls.len() > max {
                return Err("controls.model_reference_limit");
            }
        }
        attach_edit_batch(&mut draft, urls)?;
    } else if allows_multiple(kind, model) {
        let mut references = draft.references.write();
        let before = references.len();
        let mode = mode_for_input(kind, draft.mode, model, before + urls.len())
            .ok_or("controls.too_many_video_references")?;
        references.extend(urls.into_iter().map(|data_url| Reference { data_url }));
        if draft.provider == Provider::CheaperInference
            && VideoLimits::for_model(&model.id, mode)
                .is_some_and(|limits| !limits.supports_references(&references))
        {
            references.truncate(before);
            return Err("controls.veo_references");
        }
    } else if urls.len() == 1 {
        draft.references.set(
            urls.into_iter()
                .map(|data_url| Reference { data_url })
                .collect(),
        );
    } else {
        return Err("controls.too_many_video_references");
    }
    draft.reference_intent.set(None);
    if kind == MediaKind::Video
        && let Some(mode) = draft.mode_for(kind, Some(model))
        && previous_mode != Some(mode)
    {
        draft.adapt_video_params(mode);
    }
    Ok(())
}

#[component]
pub(super) fn StudioUploads(
    mut draft: StudioDraft,
    kind: MediaKind,
    reading: Signal<bool>,
    selected: Option<CatalogModel>,
) -> Element {
    let first_label = use_t("studio.first_frame");
    let add_reference = use_t("studio.add_reference");
    let add_first = use_t("gen.add_first_frame");
    let replace_first = use_t("gen.replace_frame");
    let reference_alt = use_t("studio.reference");
    let mask_label = use_t("studio.mask");
    let unsupported = use_t("controls.model_no_references");
    let lang = (use_context::<AppState>().lang)();
    let mut error = use_signal(|| None::<&'static str>);
    let mode = draft.mode_for(kind, selected.as_ref());
    let references = draft.references.read();
    let mode_after_remove = selected.as_ref().and_then(|model| {
        mode_for_input(kind, draft.mode, model, references.len().saturating_sub(1))
    });
    let upload_kind = file_mode(draft.provider, kind, selected.as_ref());
    let multiple = selected
        .as_ref()
        .is_some_and(|model| allows_multiple(kind, model));
    let label = match (kind, upload_kind, multiple, references.is_empty()) {
        (MediaKind::Video, Some(_), false, true) => add_first,
        (MediaKind::Video, Some(_), false, false) => replace_first,
        _ => add_reference,
    };
    let show_mask =
        mode == Some(GenMode::ImageEdit) && draft.provider == Provider::CheaperInference;
    let has_model = selected.is_some();
    let picker_key = selected
        .as_ref()
        .map_or(String::new(), |model| model.id.clone());
    let add_source = use_callback(move |urls: Vec<String>| {
        if let Some(model) = &selected {
            error.set(accept_files(draft, kind, model, urls).err());
        }
    });
    rsx! {
        div { class: "studio-uploads",
            div { class: "studio-upload-slot",
                if let Some(upload_kind) = upload_kind {
                    FilePicker {
                        key: "{picker_key}", kind: upload_kind, label: label.to_owned(), multiple,
                        reading, on_load: add_source,
                    }
                    SourceUrl { disabled: reading(), on_add: move |url| add_source.call(vec![url]) }
                } else {
                    button { class: "upload-area", r#type: "button", disabled: true,
                        title: has_model.then_some(unsupported),
                        span { class: "studio-icon", "data-icon": "plus", aria_hidden: "true" }
                        strong { {add_reference} }
                    }
                }
                if !references.is_empty() {
                    div { class: "reference-list",
                        for (index, reference) in references.iter().enumerate() {
                            ReferencePreview {
                                key: "{index}", src: reference.data_url.clone(),
                                label: if mode == Some(GenMode::ImageToVideo) { first_label.to_owned() } else { format!("{reference_alt} {}", index + 1) },
                                on_remove: move |_| {
                                    let empty = {
                                        let mut sources = draft.references.write();
                                        sources.remove(index);
                                        sources.is_empty()
                                    };
                                    if empty { draft.mask.set(None); }
                                    draft.reference_intent.set(None);
                                    if kind == MediaKind::Video
                                        && let Some(next_mode) = mode_after_remove
                                        && mode != Some(next_mode)
                                    {
                                        draft.adapt_video_params(next_mode);
                                    }
                                    error.set(None);
                                },
                            }
                        }
                    }
                }
            }
            if show_mask {
                details { class: "generator-mask",
                    summary { {mask_label} if draft.mask.read().is_some() { " ✓" } }
                    FilePicker {
                        key: "{picker_key}", kind: UploadKind::PngMask, label: mask_label.to_owned(), reading,
                        on_load: move |urls: Vec<String>| {
                            let references = draft.references.peek();
                            if !references.is_empty() && let Some(url) = urls.into_iter().next() {
                                let size = references.iter().map(|r| inline_bytes(&r.data_url)).sum::<usize>();
                                if size + inline_bytes(&url) > MAX_REQUEST_WIRE_BYTES {
                                    error.set(Some("controls.edit_wire_limit"));
                                } else {
                                    error.set(None);
                                    draft.mask.set(Some(url));
                                }
                            }
                        }
                    }
                    if let Some(src) = (draft.mask)() {
                        ReferencePreview { src, label: mask_label.to_owned(), on_remove: move |_| draft.mask.set(None) }
                    }
                }
            }
            if let Some(key) = error() {
                p { class: "status status-error", role: "alert", {km_ui_core::i18n::tr(lang, key)} }
            }
        }
    }
}

#[component]
fn SourceUrl(disabled: bool, on_add: Callback<String>) -> Element {
    let label = use_t("controls.image_url");
    let add = use_t("controls.add_url");
    let invalid = use_t("controls.invalid_url");
    let mut value = use_signal(String::new);
    let mut error = use_signal(|| false);
    rsx! {
        details { class: "extra-params",
            summary { {label} }
            div { class: "field-row",
                input { class: "text-input", r#type: "url", aria_label: label, value: value(), disabled,
                    placeholder: "https://…", oninput: move |event| { error.set(false); value.set(event.value()); } }
                button { r#type: "button", class: "btn", disabled: disabled || value.read().trim().is_empty(),
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
