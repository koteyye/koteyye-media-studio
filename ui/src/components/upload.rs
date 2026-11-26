//! Выбор и перетаскивание одного референса. Родитель владеет загруженными файлами.

use dioxus::prelude::*;
use dioxus::web::{WebEventExt, WebFileExt};
use km_shared::Provider;
use km_ui_core::i18n::tr;
use km_ui_core::studio::{
    MAX_OPENROUTER_EDIT_SOURCES, UploadKind, matches_signature, validate_upload,
};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{File, FileReader, HtmlInputElement};

use crate::state::{AppState, StudioDraft, use_t};

pub(crate) fn attach_edit_batch(
    draft: &mut StudioDraft,
    urls: Vec<String>,
) -> Result<(), &'static str> {
    use km_shared::image::{MAX_EDIT_SOURCES, inline_bytes};
    let mut sources = draft.references.write();
    let count = sources.len() + urls.len();
    match draft.provider {
        Provider::CheaperInference => {
            if count > MAX_EDIT_SOURCES {
                return Err("controls.edit_source_limit");
            }
            let bytes = sources
                .iter()
                .map(|r| inline_bytes(&r.data_url))
                .sum::<usize>()
                + draft.mask.peek().as_deref().map_or(0, inline_bytes)
                + urls.iter().map(|u| inline_bytes(u)).sum::<usize>();
            if bytes > km_shared::MAX_REQUEST_WIRE_BYTES {
                return Err("controls.edit_wire_limit");
            }
        }
        Provider::OpenRouter if count > MAX_OPENROUTER_EDIT_SOURCES => {
            return Err("controls.or_edit_source_limit");
        }
        Provider::OpenRouter => {}
    }
    sources.extend(
        urls.into_iter()
            .map(|data_url| km_shared::Reference { data_url }),
    );
    Ok(())
}

pub(crate) async fn read_batch(
    files: Vec<File>,
    kind: UploadKind,
    multiple: bool,
) -> Result<Vec<String>, &'static str> {
    if files.is_empty() {
        return Err(READ_ERROR);
    }
    if !multiple && files.len() != 1 {
        return Err("controls.upload_single");
    }
    if kind == UploadKind::EditSource && files.len() > km_shared::image::MAX_EDIT_SOURCES {
        return Err("controls.edit_source_limit");
    }
    if kind == UploadKind::OpenRouterEditSource && files.len() > MAX_OPENROUTER_EDIT_SOURCES {
        return Err("controls.or_edit_source_limit");
    }
    let size = files.iter().map(|f| f.size()).sum::<f64>();
    if matches!(kind, UploadKind::EditSource | UploadKind::PngMask)
        && size > km_shared::MAX_REQUEST_WIRE_BYTES as f64
    {
        return Err("controls.edit_wire_limit");
    }
    if size > km_ui_core::studio::MAX_UPLOAD_BYTES as f64 {
        return Err("controls.upload_size");
    }
    let mut urls = Vec::with_capacity(files.len());
    for file in files {
        urls.push(read_file_data_url(&file, kind).await?);
    }
    Ok(urls)
}

const READ_ERROR: &str = "controls.upload_read";

// Переключение модели может убрать пикер раньше завершения FileReader.
// Сбрасываем индикатор формы и при отмене задачи компонента.
struct ReadingGuard(Signal<bool>);

impl Drop for ReadingGuard {
    fn drop(&mut self) {
        if let Ok(mut reading) = self.0.try_write() {
            *reading = false;
        }
    }
}

/// Во время dragover браузер ещё не раскрывает сами файлы. Маркер Files
/// позволяет сохранить обычное перетаскивание текста в поле промпта.
pub(crate) fn is_file_drag(event: &DragEvent) -> bool {
    event
        .as_web_event()
        .data_transfer()
        .is_some_and(|transfer| {
            transfer
                .types()
                .iter()
                .any(|kind| kind.as_string().as_deref() == Some("Files"))
        })
}

#[component]
pub(crate) fn ReferencePreviewContent(src: String, label: String) -> Element {
    let mut enlarged = use_signal(|| false);
    let inspect = use_t("gen.view_ref");
    let close = use_t("common.close");
    let embedded = ["png", "jpeg", "webp", "gif"]
        .iter()
        .any(|mime| src.starts_with(&format!("data:image/{mime};base64,")));
    let host = (!embedded)
        .then(|| web_sys::Url::new(&src).ok().map(|u| u.hostname()))
        .flatten()
        .unwrap_or_default();
    rsx! {
        button {class:"generator-ref-open",r#type:"button",title:inspect,aria_label:"{inspect} · {label}",onclick:move |_|enlarged.set(true),
            if embedded {img {src:src.clone(),alt:label.clone()}} else { span {if host.is_empty() {{label.clone()}} else {"{host}"}}}
        }
        if enlarged(){
            dialog {class:"generator-dialog",aria_label:label.clone(),
                onmounted:move |e|{if let Ok(dialog)=e.as_web_event().dyn_into::<web_sys::HtmlDialogElement>(){let _=dialog.show_modal();}},
                oncancel:move |e|{e.prevent_default();enlarged.set(false);},
                header {span {"{label}"} button {class:"btn",r#type:"button",aria_label:close,onclick:move |_|enlarged.set(false),"×"}}
                if embedded {div {class:"generator-dialog-stage",img {src:src.clone(),alt:label.clone()}}}
                else {p {class:"generator-ref-url","{src}"}}
            }
        }
    }
}

/// Проверяем MIME, длину и первые байты до выделения памяти под полный data-URL.
pub(crate) async fn read_file_data_url(
    file: &File,
    kind: UploadKind,
) -> Result<String, &'static str> {
    let mime = file.type_();
    validate_upload(&mime, file.size() as u64, kind)?;
    let prefix = file
        .slice_with_f64_and_f64(0.0, 16.0)
        .map_err(|_| READ_ERROR)?;
    let buffer = JsFuture::from(prefix.array_buffer())
        .await
        .map_err(|_| READ_ERROR)?;
    if !matches_signature(&mime, &js_sys::Uint8Array::new(&buffer).to_vec()) {
        return Err("controls.upload_invalid");
    }
    let reader = FileReader::new().map_err(|_| READ_ERROR)?;
    let done = js_sys::Promise::new(&mut |resolve, reject| {
        reader.set_onload(Some(&resolve));
        reader.set_onerror(Some(&reject));
        reader.set_onabort(Some(&reject));
    });
    reader.read_as_data_url(file).map_err(|_| READ_ERROR)?;
    JsFuture::from(done).await.map_err(|_| READ_ERROR)?;
    reader
        .result()
        .ok()
        .and_then(|value| value.as_string())
        .ok_or(READ_ERROR)
}

#[component]
pub fn FilePicker(
    kind: UploadKind,
    label: String,
    #[props(default)] multiple: bool,
    mut reading: Signal<bool>,
    on_load: Callback<Vec<String>>,
) -> Element {
    let state = use_context::<AppState>();
    let lang = (state.lang)();
    let hint = use_t("controls.upload_hint");
    let loading = use_t("common.loading");
    let mut error = use_signal(|| None::<&'static str>);
    let mut dragging = use_signal(|| false);
    let mut read = move |files: Vec<dioxus::html::FileData>| {
        if *reading.peek() || files.is_empty() {
            return;
        }
        let Some(files) = files
            .iter()
            .map(WebFileExt::get_web_file)
            .collect::<Option<Vec<_>>>()
        else {
            error.set(Some(READ_ERROR));
            return;
        };
        error.set(None);
        reading.set(true);
        let guard = ReadingGuard(reading);
        spawn(async move {
            let _guard = guard;
            match read_batch(files, kind, multiple).await {
                Ok(urls) => on_load.call(urls),
                Err(message) => error.set(Some(message)),
            }
        });
    };
    rsx! {
        label {
            class: if dragging() { "upload-area dragging" } else { "upload-area" },title:hint,
            ondragover: move |event| {
                if is_file_drag(&event) {
                    event.prevent_default();
                    event.stop_propagation();
                    dragging.set(true);
                }
            },
            ondragleave: move |_| dragging.set(false),
            ondrop: move |event| {
                let files = event.data_transfer().files();
                if files.is_empty() { return; }
                event.prevent_default();
                event.stop_propagation();
                dragging.set(false);
                read(files);
            },
            span { class:"studio-icon","data-icon":"plus",aria_hidden:"true" }
            strong { "{label}" }
            span { class: "upload-hint", if reading() { {loading} } else { {hint} } }
            input {
                r#type: "file", accept: kind.accept(), multiple, disabled: reading(), aria_label: "{label}",
                onchange: move |event| {
                    let files = event.files();
                    if let Some(input) = event.as_web_event().target().and_then(|el| el.dyn_into::<HtmlInputElement>().ok()) {
                        // Повторный выбор того же файла после ошибки/удаления тоже даёт change.
                        input.set_value("");
                    }
                    read(files);
                },
            }
        }
        if let Some(key) = error() {
            p { class: "status status-error", role: "alert", {tr(lang, key)} }
        }
    }
}
