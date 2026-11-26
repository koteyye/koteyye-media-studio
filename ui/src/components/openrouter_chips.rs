//! Параметры OpenRouter показываются по возможностям выбранной модели.

use dioxus::prelude::*;
use km_shared::{GenMode, GenParams, OpenRouterModelOptions};

use super::{Field, SelectChip, quality_options};
use crate::state::{AppState, use_t};

fn options(values: &[String]) -> Vec<(String, String)> {
    values.iter().map(|v| (v.clone(), v.clone())).collect()
}

#[component]
pub(crate) fn OpenRouterChips(
    mode: GenMode,
    model: OpenRouterModelOptions,
    params: GenParams,
    on_change: Callback<GenParams>,
) -> Element {
    let lang = (use_context::<AppState>().lang)();
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
    let seed = use_t("studio.seed");
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
    let images = matches!(mode, GenMode::TextToImage | GenMode::ImageEdit);
    let video = matches!(
        mode,
        GenMode::TextToVideo | GenMode::ImageToVideo | GenMode::ReferenceToVideo
    );
    let image_params = &model.image_parameters;
    let image_ratio = image_params
        .get("aspect_ratio")
        .map(|p| options(&p.values))
        .unwrap_or_default();
    let image_resolution = image_params
        .get("resolution")
        .map(|p| options(&p.values))
        .unwrap_or_default();
    let image_quality = image_params
        .get("quality")
        .map(|p| quality_options(lang, p.values.iter().map(String::as_str)))
        .unwrap_or_default();
    let image_count = image_params.get("n").and_then(|p| {
        let min = p
            .min
            .as_deref()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(1)
            .max(1);
        let max = p.max.as_deref()?.parse::<u32>().ok()?.min(10);
        (max > min).then_some(min..=max)
    });
    let supports_size = image_params.contains_key("size");
    let supports_seed = image_params.contains_key("seed") || (video && model.video_seed);
    let tts_speed = model.tts_parameters.get("speed");
    let tts_voice = model.tts_parameters.get("voice");
    let tts_format = model
        .tts_parameters
        .get("response_format")
        .map(|p| options(&p.values))
        .unwrap_or_default();

    rsx! {
        div { class: "param-chips",
            if images {
                if let Some(range) = image_count {
                    SelectChip {
                        label: count, value: Some(params.n.unwrap_or(*range.start()).to_string()), optional: false,
                        options: range.map(|n| (n.to_string(), n.to_string())).collect::<Vec<_>>(),
                        onchange: move |v| edit.call((Field::Count, v)),
                    }
                }
                if !image_ratio.is_empty() {
                    SelectChip { label: ratio, value: params.aspect_ratio.clone(), options: image_ratio,
                        onchange: move |v| edit.call((Field::AspectRatio, v)),
                    }
                }
                if !image_resolution.is_empty() {
                    SelectChip { label: resolution, value: params.resolution.clone(), options: image_resolution,
                        onchange: move |v| edit.call((Field::ResolutionTier, v)),
                    }
                }
                if supports_size || !image_quality.is_empty() || supports_seed {
                    details { class: "extra-params", summary { {more} }
                        div { class: "param-chips",
                            if supports_size {
                                label { class: "param-chip", span { {size} }
                                    input { r#type: "text", aria_label: size, placeholder: "1024x1024",
                                        value: params.size.as_deref().unwrap_or_default(),
                                        oninput: move |e| edit.call((Field::ExactSize, e.value())),
                                    }
                                }
                            }
                            if !image_quality.is_empty() {
                                SelectChip { label: quality, value: params.quality.clone(), options: image_quality,
                                    onchange: move |v| edit.call((Field::Quality, v)),
                                }
                            }
                            if supports_seed {
                                SeedChip { label: seed, value: params.seed, onchange: move |v| edit.call((Field::Seed, v)) }
                            }
                        }
                    }
                }
            }
            if video {
                if !model.video_aspect_ratios.is_empty() {
                    SelectChip { label: ratio, value: params.aspect_ratio.clone(), options: options(&model.video_aspect_ratios),
                        onchange: move |v| edit.call((Field::AspectRatio, v)),
                    }
                }
                if !model.video_durations.is_empty() {
                    SelectChip { label: duration, value: params.duration_seconds.map(|n| n.to_string()),
                        options: model.video_durations.iter().map(|d| (d.to_string(), format!("{d} {seconds}"))).collect::<Vec<_>>(),
                        onchange: move |v| edit.call((Field::Duration, v)),
                    }
                }
                if !model.video_resolutions.is_empty() {
                    SelectChip { label: resolution, value: params.resolution.clone(), options: options(&model.video_resolutions),
                        onchange: move |v| edit.call((Field::ResolutionTier, v)),
                    }
                }
                if model.video_can_toggle_audio {
                    SelectChip { label: audio, value: params.audio.map(|v| v.to_string()),
                        options: vec![("true".into(), on.into()), ("false".into(), off.into())],
                        onchange: move |v| edit.call((Field::Audio, v)),
                    }
                }
                if supports_seed {
                    SeedChip { label: seed, value: params.seed, onchange: move |v| edit.call((Field::Seed, v)) }
                }
            }
            if mode == GenMode::Speech {
                if let Some(voice_options) = tts_voice {
                    if voice_options.values.is_empty() {
                        label { class: "param-chip", span { {voice} }
                            input { r#type: "text", aria_label: voice, placeholder: automatic,
                                value: params.voice.as_deref().unwrap_or_default(),
                                oninput: move |e| edit.call((Field::Voice, e.value())),
                            }
                        }
                    } else {
                        SelectChip { label: voice, value: params.voice.clone(), options: options(&voice_options.values), optional: false,
                            onchange: move |v| edit.call((Field::Voice, v)),
                        }
                    }
                }
                if !tts_format.is_empty() {
                    SelectChip { label: format, value: params.response_format.clone(), options: tts_format, optional: false,
                        onchange: move |v| edit.call((Field::Format, v)),
                    }
                }
                if let Some(speed_options) = tts_speed {
                    label { class: "param-chip", span { {speed} }
                        input { r#type: "number", aria_label: speed, step: "any",
                            min: speed_options.min.clone(), max: speed_options.max.clone(), placeholder: automatic,
                            value: params.speed.map(|v| v.to_string()).unwrap_or_default(),
                            oninput: move |e| edit.call((Field::Speed, e.value())),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SeedChip(label: &'static str, value: Option<i64>, onchange: Callback<String>) -> Element {
    let automatic = use_t("controls.auto");
    rsx! {
        label { class: "param-chip", span { {label} }
            input { r#type: "number", aria_label: label, step: "1", placeholder: automatic,
                value: value.map(|seed| seed.to_string()).unwrap_or_default(),
                oninput: move |e| onchange.call(e.value()),
            }
        }
    }
}
