//! Параметры формы. Необязательные значения оставляем провайдеру, пока пользователь их не изменил.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use km_shared::video::VideoLimits;
use km_shared::{CatalogModel, GenMode, GenParams};

use crate::state::use_t;

#[derive(Clone, Copy)]
enum Field {
    Size,
    ImageRatio,
    Count,
    Quality,
    Duration,
    Resolution,
    VideoRatio,
    Audio,
    Voice,
    Format,
    Speed,
}

fn dimensions(size: &str) -> Option<(u32, u32)> {
    let (w, h) = size.split_once('x')?;
    let pair = (w.parse().ok()?, h.parse().ok()?);
    (pair.0 > 0 && pair.1 > 0).then_some(pair)
}

fn image_ratio(size: &str) -> Option<String> {
    let (width, height) = dimensions(size)?;
    let (mut a, mut b) = (width, height);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    Some(format!("{}:{}", width / a, height / a))
}

impl Field {
    fn update(self, params: &mut GenParams, value: String) {
        let value = (!value.is_empty()).then_some(value);
        match self {
            Self::Size => {
                params.aspect_ratio = value.as_deref().and_then(image_ratio);
                params.size = value;
            }
            Self::ImageRatio => {
                // /v1/images/{generations,edits} принимает геометрию через size.
                // Для избранного пресета используем распространённые размеры, кратные 64.
                params.size = match value.as_deref() {
                    Some("1:1") => Some("1024x1024".into()),
                    Some("3:2") => Some("1536x1024".into()),
                    Some("2:3") => Some("1024x1536".into()),
                    Some("16:9") => Some("1536x864".into()),
                    Some("9:16") => Some("864x1536".into()),
                    _ => None,
                };
                params.aspect_ratio = value;
            }
            Self::Count => params.n = value.and_then(|v| v.parse().ok()),
            Self::Quality => params.quality = value,
            Self::Duration => {
                params.duration_seconds = value
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|v| v.is_finite() && *v > 0.0);
            }
            Self::Resolution => params.resolution = value,
            Self::VideoRatio => params.aspect_ratio = value,
            Self::Audio => params.audio = value.and_then(|v| v.parse().ok()),
            Self::Voice => params.voice = value,
            Self::Format => params.response_format = value,
            Self::Speed => params.speed = value.and_then(|v| v.parse().ok()),
        }
    }
}

fn choices(values: &[&str]) -> Vec<(String, String)> {
    values.iter().map(|&v| (v.into(), v.into())).collect()
}

fn image_sizes(model: Option<&CatalogModel>, mode: GenMode) -> Vec<(String, String)> {
    let mut sizes = choices(&[
        "1024x1024",
        "1536x1024",
        "1024x1536",
        "1536x864",
        "864x1536",
        "2048x2048",
    ]);
    let Some(model) = model else { return sizes };
    let protocol = if mode == GenMode::ImageEdit {
        "images_edits"
    } else {
        "images_generations"
    };
    let scoped = model.pricing_scopes.iter().flatten().filter_map(|entry| {
        let scope = entry.scope.as_ref()?;
        if scope.protocol.as_deref() == Some(protocol) {
            scope.pricing_variant.as_deref()
        } else {
            None
        }
    });
    let legacy = model
        .media_prices
        .keys()
        .filter(|_| mode == GenMode::TextToImage)
        .map(String::as_str);
    for key in scoped.chain(legacy) {
        let size = match key
            .split('+')
            .next()
            .unwrap_or(key)
            .to_ascii_lowercase()
            .as_str()
        {
            "1k" => "1024x1024".to_owned(),
            "2k" => "2048x2048".to_owned(),
            "4k" => "4096x4096".to_owned(),
            other => other.to_owned(),
        };
        if dimensions(&size).is_some() && !sizes.iter().any(|(value, _)| value == &size) {
            sizes.push((size.clone(), size));
        }
    }
    sizes
}

fn resolution_choices(model: Option<&CatalogModel>) -> Vec<(String, String)> {
    let resolutions: BTreeSet<&str> = model
        .into_iter()
        .flat_map(|m| m.media_prices.keys())
        .map(|key| key.split('+').next().unwrap_or_default())
        .filter(|key| {
            key.strip_suffix(['P', 'K'])
                .is_some_and(|n| n.parse::<u32>().is_ok())
        })
        .collect();
    if resolutions.is_empty() && model.is_none_or(|m| m.media_prices.is_empty()) {
        return choices(&["480P", "720P", "1080P", "4K"]);
    }
    resolutions
        .into_iter()
        .map(|s| (s.into(), s.into()))
        .collect()
}

#[component]
fn SelectChip(
    label: &'static str,
    value: Option<String>,
    mut options: Vec<(String, String)>,
    onchange: Callback<String>,
    #[props(default = true)] optional: bool,
) -> Element {
    let automatic = use_t("controls.auto");
    // Старый результат можно повторить даже после изменения набора подсказок каталога.
    if let Some(value) = &value
        && !options.iter().any(|(v, _)| v == value)
    {
        options.push((value.clone(), value.clone()));
    }
    let value = value.unwrap_or_default();
    rsx! {
        label { class: "param-chip",
            span { {label} }
            select { aria_label: label, value: "{value}", onchange: move |event| onchange.call(event.value()),
                if optional { option { value: "", selected: value.is_empty(), {automatic} } }
                for (v, name) in options {
                    option { key: "{v}", value: "{v}", selected: v == value, "{name}" }
                }
            }
        }
    }
}

#[component]
pub fn ParamChips(
    mode: GenMode,
    params: GenParams,
    on_change: Callback<GenParams>,
    model: Option<CatalogModel>,
) -> Element {
    let size = use_t("studio.size");
    let ratio = use_t("studio.ratio");
    let duration = use_t("studio.duration");
    let seconds = use_t("controls.per_second");
    let resolution = use_t("studio.resolution");
    let audio = use_t("studio.audio_on");
    let voice = use_t("studio.voice");
    let format = use_t("studio.format");
    let speed = use_t("studio.speed");
    let count = use_t("controls.count");
    let quality = use_t("controls.quality");
    let more = use_t("controls.more");
    let automatic = use_t("controls.auto");
    let on = use_t("controls.on");
    let off = use_t("controls.off");
    let current = params.clone();
    let edit = use_callback(move |(field, value): (Field, String)| {
        let mut next = current.clone();
        field.update(&mut next, value);
        on_change.call(next);
    });
    let is_video = matches!(
        mode,
        GenMode::TextToVideo | GenMode::ImageToVideo | GenMode::ReferenceToVideo
    );
    let speech = mode == GenMode::Speech;
    // Для остальных провайдеров CI публикует свободное поле voice без
    // справочника голосов. Ограниченный список OpenAI подходит только TTS-1.
    let openai_tts = model
        .as_ref()
        .is_some_and(|m| matches!(m.id.rsplit('/').next(), Some("tts-1" | "tts-1-hd")));
    let image = matches!(mode, GenMode::TextToImage | GenMode::ImageEdit);
    let duration_value = params
        .duration_seconds
        .map(|n| n.to_string())
        .unwrap_or_default();
    let video_limits = model
        .as_ref()
        .and_then(|m| VideoLimits::for_model(&m.id, mode));
    let max_videos = video_limits.map_or(10, |limits| limits.max_outputs);
    let aspect_ratios = video_limits.map_or(&["16:9", "9:16", "1:1", "4:3", "3:4"][..], |limits| {
        limits.aspect_ratios
    });
    let resolutions = resolution_choices(model.as_ref());

    rsx! {
        div { class: "param-chips",
            if image {
                SelectChip {
                    label: count, value: Some(params.n.unwrap_or(1).to_string()), optional: false,
                    options: (1..=10).map(|n| (n.to_string(),n.to_string())).collect::<Vec<_>>(),
                    onchange: move |v| edit.call((Field::Count, v)),
                }
                SelectChip {
                    label: ratio, value: params.size.as_deref().and_then(image_ratio), options: choices(&["1:1","3:2","2:3","16:9","9:16"]),
                    onchange: move |v| edit.call((Field::ImageRatio, v)),
                }
                details { class: "extra-params",
                    summary { {more} }
                    div { class: "param-chips",
                        SelectChip {
                            label: size, value: params.size.clone(), options: image_sizes(model.as_ref(), mode),
                            onchange: move |v| edit.call((Field::Size,v)),
                        }
                        SelectChip { label: quality, value: params.quality.clone(), options: choices(&["low","medium","high"]),
                            onchange: move |v| edit.call((Field::Quality, v)),
                        }
                    }
                }
            }
            if is_video {
                SelectChip { label: ratio, value: params.aspect_ratio.clone(), options: choices(aspect_ratios),
                    onchange: move |v| edit.call((Field::VideoRatio, v)),
                }
                details {class:"extra-params",summary {{more}} div {class:"param-chips",
                if let Some(limits) = video_limits {
                    SelectChip { label: duration, value: Some(duration_value.clone()), optional: false,
                        options: limits.durations.iter().map(|d| (d.to_string(),format!("{d} {seconds}"))).collect::<Vec<_>>(),
                        onchange: move |v| edit.call((Field::Duration, v)),
                    }
                } else {
                    label { class: "param-chip",
                        span { "{duration}, {seconds}" }
                        input { r#type: "number", aria_label: "{duration}, {seconds}", min: "0", step: "any", required: is_video,
                            placeholder: automatic, value: "{duration_value}", oninput: move |e| edit.call((Field::Duration,e.value())) }
                    }
                }
                if !resolutions.is_empty() || params.resolution.is_some() {
                    SelectChip { label: resolution, value: params.resolution.clone(), options: resolutions,
                        onchange: move |v| edit.call((Field::Resolution, v)),
                    }
                }
                SelectChip { label: audio, value: params.audio.map(|v| v.to_string()),
                    options: vec![("true".into(), on.into()),("false".into(), off.into())], onchange: move |v| edit.call((Field::Audio, v)),
                }
                SelectChip { label: count, value: Some(params.n.unwrap_or(1).to_string()), optional: false,
                    options: (1..=max_videos).map(|n| (n.to_string(),n.to_string())).collect::<Vec<_>>(),
                    onchange: move |v| edit.call((Field::Count, v)),
                }
                }}
            }
            if speech {
                SelectChip { label: format, value: params.response_format.clone(),
                    options: choices(if openai_tts { &["mp3","wav","opus","aac","flac","pcm"] } else { &["mp3","wav"] }),
                    onchange: move |v| edit.call((Field::Format, v)),
                }
                details {class:"extra-params",summary {{more}} div {class:"param-chips",
                label { class: "param-chip",
                    span { {voice} }
                    input { r#type: "text", aria_label: voice, list: openai_tts.then_some("tts-1-voices"),
                        value: params.voice.as_deref().unwrap_or_default(), placeholder: automatic,
                        oninput: move |e| edit.call((Field::Voice,e.value())) }
                    if openai_tts {
                        datalist { id: "tts-1-voices",
                            for id in ["alloy","ash","coral","echo","fable","onyx","nova","sage","shimmer"] {
                                option { key: "{id}", value: id }
                            }
                        }
                    }
                }
                SelectChip { label: speed, value: params.speed.map(|n| n.to_string()),
                    options: choices(if openai_tts { &["0.25","0.5","0.75","1","1.25","1.5","2","3","4"] } else { &["0.5","0.75","1","1.25","1.5","2"] }),
                    onchange: move |v| edit.call((Field::Speed, v)),
                }
                }}
            }
        }
    }
}
