//! Нормализация каталогов медиа OpenRouter для селектора студии.

use std::collections::BTreeMap;

use km_shared::{
    CatalogModel, CatalogParameter, CatalogPricingScope, CatalogRates, CatalogTier, GenMode,
    MediaKind, MediaPrices, ModelType, OpenRouterModelOptions,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Number;

use crate::client::OpenRouterKeyClient;
use crate::error::CoreError;

type Parameters = BTreeMap<String, WireParameter>;

const IMAGES: &str = "/images/models";
const VIDEOS: &str = "/videos/models";
const SPEECH: &str = "/audio/speech/models";
const AUDIO: &str = "/models?output_modalities=speech,audio";
const SEED_AUDIO: &str = "bytedance-seed/seed-audio-1-0";
const CHAT_PATH: &str = "/api/v1/chat/completions";
const SPEECH_PATH: &str = "/api/v1/audio/speech";

#[derive(Deserialize)]
struct ModelList<T> {
    data: Vec<T>,
}

#[derive(Deserialize)]
struct ModelIdentity {
    id: String,
    name: Option<String>,
    canonical_slug: Option<String>,
}

impl ModelIdentity {
    fn catalog(
        self,
        model_type: ModelType,
        endpoint: &str,
        options: OpenRouterModelOptions,
    ) -> CatalogModel {
        let provider_name = self
            .name
            .as_deref()
            .and_then(|name| name.split_once(": ").map(|(author, _)| author))
            .or_else(|| self.id.split_once('/').map(|(author, _)| author))
            .map(str::to_owned);
        let aliases = self
            .canonical_slug
            .filter(|canonical| canonical != &self.id)
            .into_iter()
            .collect();
        CatalogModel {
            id: self.id,
            name: self.name,
            model_type,
            provider_name,
            logo_url: None,
            features: Vec::new(),
            supported_endpoints: vec![endpoint.to_owned()],
            media_unit: None,
            media_prices: BTreeMap::new(),
            media_unit_price: None,
            pricing_scopes: None,
            discount_percent: None,
            supports_image_edit: options.modes.contains(&GenMode::ImageEdit),
            aliases,
            openrouter: Some(options),
        }
    }
}

#[derive(Deserialize, Default)]
struct WireParameter {
    #[serde(default)]
    values: Vec<String>,
    min: Option<Number>,
    max: Option<Number>,
}

impl WireParameter {
    fn into_parameter(self) -> CatalogParameter {
        CatalogParameter {
            values: self.values,
            min: self.min.map(|n| n.to_string()),
            max: self.max.map(|n| n.to_string()),
        }
    }
}

fn parameters_into_catalog(parameters: Parameters) -> BTreeMap<String, CatalogParameter> {
    parameters
        .into_iter()
        .map(|(key, value)| (key, value.into_parameter()))
        .collect()
}

#[derive(Deserialize)]
struct Architecture {
    input_modalities: Vec<String>,
    output_modalities: Vec<String>,
}

#[derive(Deserialize)]
struct ImageModel {
    #[serde(flatten)]
    identity: ModelIdentity,
    architecture: Architecture,
    #[serde(default)]
    supported_parameters: Parameters,
}

impl ImageModel {
    fn into_catalog(mut self) -> Option<CatalogModel> {
        let architecture = self.architecture;
        if !architecture.output_modalities.iter().any(|s| s == "image")
            || !architecture.input_modalities.iter().any(|s| s == "text")
        {
            return None;
        }
        // Локальное сохранение изображений и плеер принимают растровые форматы.
        if let Some(format) = self.supported_parameters.get_mut("output_format")
            && !format.values.is_empty()
        {
            format.values.retain(|format| {
                matches!(format.as_str(), "png" | "jpeg" | "jpg" | "webp" | "gif")
            });
            if format.values.is_empty() {
                return None;
            }
        }
        let references = self.supported_parameters.get("input_references");
        let min_references = references.and_then(|p| p.min.as_ref()?.as_u64());
        let max_references = references.and_then(|p| p.max.as_ref()?.as_u64());
        let mut modes = Vec::new();
        if min_references.unwrap_or(0) == 0 {
            modes.push(GenMode::TextToImage);
        }
        if architecture.input_modalities.iter().any(|s| s == "image")
            && references.is_some()
            && max_references != Some(0)
        {
            modes.push(GenMode::ImageEdit);
        }
        if modes.is_empty() {
            return None;
        }
        let mut model = self.identity.catalog(
            ModelType::Image,
            "/api/v1/images",
            OpenRouterModelOptions {
                modes,
                image_parameters: parameters_into_catalog(self.supported_parameters),
                ..Default::default()
            },
        );
        if model.id.ends_with(":free") {
            model.media_unit = Some("image".into());
            model.media_unit_price = Some("0".into());
        }
        Some(model)
    }
}

#[derive(Deserialize)]
struct VideoModel {
    #[serde(flatten)]
    identity: ModelIdentity,
    supported_durations: Option<Vec<i64>>,
    supported_resolutions: Option<Vec<String>>,
    supported_aspect_ratios: Option<Vec<String>>,
    supported_sizes: Option<Vec<String>>,
    supported_frame_images: Option<Vec<String>>,
    generate_audio: Option<bool>,
    seed: Option<bool>,
    pricing_skus: Option<BTreeMap<String, String>>,
}

impl VideoModel {
    fn into_catalog(self) -> Option<CatalogModel> {
        let id = self.identity.id.as_str();
        // Видеоредактирование, апскейл и аватары требуют исходник,
        // для которого нет поля в текущем запросе студии.
        if matches!(
            id,
            "black-forest-labs/flux-video-edit"
                | "black-forest-labs/flux-video-upscale"
                | "runway/aleph-2"
                | "heygen/avatar-iv"
                | "heygen/heygen-avatar-6"
                | "heygen/heygen-avatar-6-turbo"
        ) {
            return None;
        }
        let mut modes = Vec::new();
        if id != "tbc/mischief-1" {
            modes.push(GenMode::TextToVideo);
        }
        if self
            .supported_frame_images
            .as_ref()
            .is_some_and(|frames| frames.iter().any(|frame| frame == "first_frame"))
        {
            modes.push(GenMode::ImageToVideo);
        }
        // Каталог публикует позиции first/last frame, а отдельный режим
        // input_references пока описывается в карточках моделей.
        if matches!(
            id,
            "alibaba/wan-3.0"
                | "alibaba/happyhorse-1.0"
                | "alibaba/happyhorse-1.1"
                | "bytedance/seedance-2.0"
                | "bytedance/seedance-2.0-fast"
                | "bytedance/seedance-2.0-mini"
                | "bytedance/seedance-2.5"
                | "bytedance/seedance-2.5-flash"
                | "google/gemini-omni-1.1-flash"
                | "google/gemini-omni-1.2-flash"
                | "heygen/heygen-video-1"
                | "minimax/hailuo-3"
                | "x-ai/grok-imagine-video"
        ) {
            modes.push(GenMode::ReferenceToVideo);
        }
        if modes.is_empty() {
            return None;
        }
        let skus = self.pricing_skus.unwrap_or_default();
        let resolutions = self.supported_resolutions.unwrap_or_default();
        let can_toggle_audio = skus.keys().any(|sku| {
            sku.starts_with("duration_seconds_without_audio")
                || sku.starts_with("video_tokens_without_audio")
        });
        let media_prices = normalize_video_prices(&skus, &resolutions, &modes);
        let mut model = self.identity.catalog(
            ModelType::Video,
            "/api/v1/videos",
            OpenRouterModelOptions {
                modes,
                video_resolutions: resolutions,
                video_aspect_ratios: self.supported_aspect_ratios.unwrap_or_default(),
                video_durations: self
                    .supported_durations
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|duration| u32::try_from(duration).ok().filter(|&n| n > 0))
                    .collect(),
                video_sizes: self.supported_sizes.unwrap_or_default(),
                video_generate_audio: self.generate_audio,
                video_can_toggle_audio: can_toggle_audio,
                video_seed: self.seed.unwrap_or(false),
                ..Default::default()
            },
        );
        model.media_unit = Some("second".into());
        model.media_prices = media_prices;
        Some(model)
    }
}

fn sku_rate(skus: &BTreeMap<String, String>, sku: &str, resolution: &str) -> Option<String> {
    let specific = format!("{sku}_{resolution}");
    let value = skus
        .get(&specific)
        .or_else(|| skus.get(sku))?
        .parse::<f64>()
        .ok()
        .filter(|p| p.is_finite() && *p >= 0.0)?;
    let dollars = if sku.starts_with("cents_") {
        value / 100.0
    } else {
        value
    };
    Some(dollars.to_string())
}

fn normalize_video_prices(
    skus: &BTreeMap<String, String>,
    resolutions: &[String],
    modes: &[GenMode],
) -> MediaPrices {
    // При альтернативных SKUs по длительности ставка зависит от параметров
    // клипа, а токеновые ставки нельзя считать ценой за секунду.
    if skus.keys().any(|sku| sku.starts_with("long_")) {
        return MediaPrices::new();
    }
    let has_image_surcharge = skus.contains_key("cents_per_image_input");
    let has_reference_surcharge = skus.contains_key("reference_images")
        || skus.keys().any(|sku| sku.starts_with("reference_"));
    let priced_by_mode = has_image_surcharge || has_reference_surcharge;
    let mut prices = MediaPrices::new();
    for resolution in resolutions {
        let key = resolution.to_ascii_uppercase();
        let resolution = resolution.to_ascii_lowercase();
        let base = [
            "duration_seconds_without_audio",
            "duration_seconds",
            "cents_per_video_output_second",
            "cents_per_second_output",
        ]
        .into_iter()
        .find_map(|sku| sku_rate(skus, sku, &resolution));
        let audio = sku_rate(skus, "duration_seconds_with_audio", &resolution);
        for &mode in modes {
            let variant = if priced_by_mode {
                match mode {
                    GenMode::TextToVideo => format!("{key}+TEXT_TO_VIDEO"),
                    GenMode::ImageToVideo if !has_image_surcharge => {
                        format!("{key}+IMAGE_TO_VIDEO")
                    }
                    GenMode::ReferenceToVideo
                        if !has_image_surcharge && !has_reference_surcharge =>
                    {
                        format!("{key}+REFERENCE_TO_VIDEO")
                    }
                    _ => continue,
                }
            } else {
                key.clone()
            };
            if let Some(rate) = &base {
                prices.insert(variant.clone(), rate.clone());
            }
            if let Some(rate) = &audio {
                prices.insert(format!("{variant}+AUDIO"), rate.clone());
            }
            if !priced_by_mode {
                break;
            }
        }
    }
    prices
}

#[derive(Deserialize)]
struct SpeechModel {
    #[serde(flatten)]
    identity: ModelIdentity,
    output_modalities: Vec<SpeechModality>,
    #[serde(default)]
    endpoints: Vec<SpeechEndpoint>,
}

#[derive(Deserialize)]
struct SpeechEndpoint {
    output_modalities: Vec<SpeechModality>,
}

#[derive(Deserialize)]
struct SpeechModality {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    supported_parameters: Parameters,
}

impl SpeechModel {
    fn into_catalog(self, prices: Option<Pricing>) -> Option<CatalogModel> {
        let mut parameters = self
            .output_modalities
            .into_iter()
            .find(|out| out.kind == "speech")?
            .supported_parameters;
        // У части моделей голоса объявлены в записях маршрутов.
        let endpoint_voices: Vec<String> = self
            .endpoints
            .into_iter()
            .flat_map(|endpoint| endpoint.output_modalities)
            .filter(|out| out.kind == "speech")
            .filter_map(|mut out| out.supported_parameters.remove("voice"))
            .flat_map(|voice| voice.values)
            .collect();
        if !endpoint_voices.is_empty() {
            let voice = parameters.entry("voice".into()).or_default();
            for value in endpoint_voices {
                if !voice.values.contains(&value) {
                    voice.values.push(value);
                }
            }
        }
        let mut modes = vec![GenMode::Speech];
        if self.identity.id == SEED_AUDIO {
            modes.push(GenMode::SoundEffects);
        }
        let mut model = self.identity.catalog(
            ModelType::Speech,
            SPEECH_PATH,
            OpenRouterModelOptions {
                modes,
                tts_parameters: parameters_into_catalog(parameters),
                ..Default::default()
            },
        );
        if let Some(prices) = prices {
            prices.set_speech_rates(&mut model);
        }
        Some(model)
    }
}

#[derive(Deserialize)]
struct AudioModel {
    #[serde(flatten)]
    identity: ModelIdentity,
    #[serde(default)]
    endpoints: Vec<String>,
    pricing: Option<Pricing>,
    supported_voices: Option<Vec<String>>,
}

impl AudioModel {
    fn into_speech(self, listed: &[CatalogModel]) -> CatalogModel {
        // Бесплатный вариант может присутствовать только в общем каталоге.
        // Для его параметров используем опубликованные возможности базовой модели.
        let parameters = self
            .identity
            .id
            .strip_suffix(":free")
            .and_then(|base| {
                listed.iter().find(|model| {
                    model.id == base || base.strip_suffix("-free") == Some(model.id.as_str())
                })
            })
            .and_then(|model| model.openrouter.as_ref())
            .map(|options| options.tts_parameters.clone())
            .unwrap_or_default();
        let mut options = OpenRouterModelOptions {
            modes: vec![GenMode::Speech],
            tts_parameters: parameters,
            ..Default::default()
        };
        if let Some(voices) = self.supported_voices.filter(|voices| !voices.is_empty()) {
            options
                .tts_parameters
                .entry("voice".into())
                .or_default()
                .values = voices;
        }
        let mut model = self
            .identity
            .catalog(ModelType::Speech, SPEECH_PATH, options);
        if let Some(prices) = self.pricing {
            prices.set_speech_rates(&mut model);
        }
        model
    }

    fn into_music(self) -> Option<CatalogModel> {
        // Lyria имеет отдельный сценарий генерации музыки. Другие аудио
        // модели из общего каталога отвечают голосом на запросы.
        if !self.identity.id.starts_with("google/lyria-")
            || !self.endpoints.iter().any(|endpoint| endpoint == CHAT_PATH)
        {
            return None;
        }
        Some(self.identity.catalog(
            ModelType::Speech,
            CHAT_PATH,
            OpenRouterModelOptions {
                modes: vec![GenMode::Music],
                ..Default::default()
            },
        ))
    }
}

#[derive(Deserialize)]
struct Pricing {
    prompt: Option<String>,
    completion: Option<String>,
}

impl Pricing {
    fn set_speech_rates(self, model: &mut CatalogModel) {
        let Self { prompt, completion } = self;
        if model.id == SEED_AUDIO {
            model.media_unit = Some("second".into());
            model.media_unit_price = completion;
        } else if model.id.starts_with("google/") {
            model.media_unit = Some("token".into());
            model.pricing_scopes = Some(vec![CatalogPricingScope {
                scope: None,
                tiers: vec![CatalogTier {
                    rates: CatalogRates {
                        input_tokens: prompt,
                        output_tokens: completion,
                        ..Default::default()
                    },
                }],
            }]);
        // Рыночные цены Fish Audio считаются за байты UTF-8, а студия
        // оценивает ставки за символы по количеству символов Unicode.
        } else if (!model.id.starts_with("fish-audio/") || model.id.ends_with(":free"))
            && completion
                .as_deref()
                .and_then(|rate| rate.parse::<f64>().ok())
                == Some(0.0)
        {
            model.media_unit = Some("character".into());
            model.media_unit_price = prompt;
        }
    }
}

impl OpenRouterKeyClient {
    /// Каталоги медиа из документации OpenRouter. С пустым ключом загружается
    /// публичный список, с ключом запросы относятся к профилю пользователя.
    pub async fn models_for_kind(
        &self,
        key: &str,
        kind: MediaKind,
    ) -> Result<Vec<CatalogModel>, CoreError> {
        match kind {
            MediaKind::Image => Ok(self
                .list::<ImageModel>(IMAGES, key)
                .await?
                .into_iter()
                .filter_map(ImageModel::into_catalog)
                .collect()),
            MediaKind::Video => Ok(self
                .list::<VideoModel>(VIDEOS, key)
                .await?
                .into_iter()
                .filter_map(VideoModel::into_catalog)
                .collect()),
            MediaKind::Audio => {
                let speech = self.list::<SpeechModel>(SPEECH, key).await?;
                let mut speech_models = BTreeMap::new();
                let mut music = Vec::new();
                for model in self.list::<AudioModel>(AUDIO, key).await? {
                    if model
                        .endpoints
                        .iter()
                        .any(|endpoint| endpoint == SPEECH_PATH)
                    {
                        speech_models.insert(model.identity.id.clone(), model);
                    } else if let Some(model) = model.into_music() {
                        music.push(model);
                    }
                }
                let mut models: Vec<_> = speech
                    .into_iter()
                    .filter_map(|model| {
                        let price = speech_models
                            .remove(&model.identity.id)
                            .and_then(|details| details.pricing);
                        model.into_catalog(price)
                    })
                    .collect();
                let other_speech: Vec<_> = speech_models
                    .into_values()
                    .map(|model| model.into_speech(&models))
                    .collect();
                models.extend(other_speech);
                models.extend(music);
                Ok(models)
            }
        }
    }

    async fn list<T: DeserializeOwned>(&self, path: &str, key: &str) -> Result<Vec<T>, CoreError> {
        let response = self.get(path, key).await?;
        Ok(self.json::<ModelList<T>>(response, key).await?.data)
    }
}
