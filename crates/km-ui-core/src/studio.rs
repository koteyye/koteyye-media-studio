//! Чистая логика форм Студии и проверка локальных референсов.

use std::collections::BTreeMap;

use km_shared::{
    CatalogModel, GenMode, GenParams, GenerationRequest, MediaKind, ModelType, Reference,
};

pub fn build_request(
    kind: MediaKind,
    mode: GenMode,
    model: &str,
    prompt: &str,
    params: GenParams,
    references: Vec<Reference>,
    mask_data_url: Option<String>,
) -> GenerationRequest {
    GenerationRequest {
        kind,
        mode,
        model: model.to_owned(),
        prompt: prompt.to_owned(),
        params: if matches!(mode, GenMode::Music | GenMode::SoundEffects) {
            // Старые черновики могли содержать duration_seconds, которого
            // шлюз не публикует для /v1/audio/speech.
            GenParams::default()
        } else {
            params
        },
        references,
        mask_data_url,
    }
}

pub fn pending_reason(lang: km_shared::Lang, reason: &str) -> &str {
    match reason {
        "history.video_processing" => crate::i18n::tr(lang, "history.video_processing"),
        "history.recovery_access" => crate::i18n::tr(lang, "history.recovery_access"),
        "history.recovery_not_found" => crate::i18n::tr(lang, "history.recovery_not_found"),
        "history.recovery_response" => crate::i18n::tr(lang, "history.recovery_response"),
        "history.recovery_unavailable" => crate::i18n::tr(lang, "history.recovery_unavailable"),
        _ => reason,
    }
}

pub fn default_params(mode: GenMode) -> GenParams {
    match mode {
        GenMode::TextToImage | GenMode::ImageEdit => GenParams {
            size: Some("1024x1024".into()),
            n: Some(1),
            ..Default::default()
        },
        GenMode::TextToVideo | GenMode::ImageToVideo | GenMode::ReferenceToVideo => GenParams {
            duration_seconds: Some(6.0),
            ..Default::default()
        },
        GenMode::Speech | GenMode::Music | GenMode::SoundEffects => GenParams::default(),
    }
}

pub fn models_for(models: &[CatalogModel], kind: MediaKind, mode: GenMode) -> Vec<&CatalogModel> {
    models
        .iter()
        .filter(|m| m.matches_kind(kind) && m.supports_mode(mode))
        .collect()
}

pub fn mode_for_input(
    kind: MediaKind,
    audio_mode: GenMode,
    model: &CatalogModel,
    reference_count: usize,
) -> Option<GenMode> {
    if !model.matches_kind(kind) {
        return None;
    }
    let mode = match (kind, reference_count) {
        (MediaKind::Image, 0) => GenMode::TextToImage,
        (MediaKind::Image, _) => GenMode::ImageEdit,
        (MediaKind::Video, 0) => GenMode::TextToVideo,
        (MediaKind::Video, 1) if model.supports_mode(GenMode::ImageToVideo) => {
            GenMode::ImageToVideo
        }
        (MediaKind::Video, _) => GenMode::ReferenceToVideo,
        (MediaKind::Audio, _) => audio_mode,
    };
    model.supports_mode(mode).then_some(mode)
}

/// A model that requires attachments is selectable before the first attachment.
pub fn models_for_input(
    models: &[CatalogModel],
    kind: MediaKind,
    mode: GenMode,
    reference_count: usize,
) -> Vec<&CatalogModel> {
    models
        .iter()
        .filter(|model| {
            mode_for_input(kind, mode, model, reference_count).is_some()
                || (reference_count == 0
                    && kind != MediaKind::Audio
                    && mode_for_input(kind, mode, model, 1).is_some())
        })
        .collect()
}

pub fn search_models<'a>(models: &[&'a CatalogModel], query: &str) -> Vec<&'a CatalogModel> {
    let query = query.trim().to_lowercase();
    models
        .iter()
        .copied()
        .filter(|m| {
            let matches = |s: &str| s.to_lowercase().contains(&query);
            matches(&m.id)
                || m.name.as_deref().is_some_and(matches)
                || m.provider_name.as_deref().is_some_and(matches)
                || m.aliases.iter().any(|a| matches(a))
        })
        .collect()
}

/// Пустое имя — отдельная группа, название которой переводится в UI.
pub fn group_by_provider<'a>(models: &[&'a CatalogModel]) -> Vec<(&'a str, Vec<&'a CatalogModel>)> {
    let mut groups = BTreeMap::new();
    for &model in models {
        groups
            .entry(model.provider_name.as_deref().unwrap_or("").trim())
            .or_insert_with(Vec::new)
            .push(model);
    }
    groups.into_iter().collect()
}

fn amount(value: &str) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
}

fn base_rate(model: &CatalogModel) -> Option<f64> {
    model
        .media_unit_price
        .as_deref()
        .and_then(amount)
        .or_else(|| {
            model
                .media_prices
                .iter()
                .find(|(variant, _)| variant.eq_ignore_ascii_case("default"))
                .and_then(|(_, price)| amount(price))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriceUnit {
    Image,
    Second,
    Character,
}

/// Конкретная ставка аудио принимается только при совпадении доступных тарифных уровней.
fn audio_rate(model: &CatalogModel) -> Option<(PriceUnit, f64)> {
    if model.media_unit.as_deref() == Some("token") {
        return None;
    }
    let unit = match model.media_unit.as_deref() {
        Some("second") => Some(PriceUnit::Second),
        Some("character") => Some(PriceUnit::Character),
        _ => None,
    };
    if let (Some(unit), Some(rate)) = (unit, base_rate(model)) {
        return Some((unit, rate));
    }
    let mut rates = model
        .pricing_scopes
        .as_ref()?
        .iter()
        .flat_map(|scope| &scope.tiers)
        .map(|tier| match (&tier.rates.characters, &tier.rates.seconds) {
            (Some(price), None) => Some((PriceUnit::Character, amount(price)?)),
            (None, Some(price)) => Some((PriceUnit::Second, amount(price)?)),
            _ => None,
        });
    let first = rates.next()??;
    rates.all(|rate| rate == Some(first)).then_some(first)
}

/// Для токеновых TTS-моделей цена за миллион входных и выходных токенов.
/// Количество токенов готового аудио до генерации неизвестно.
pub fn speech_token_rates(model: &CatalogModel) -> Option<(f64, f64)> {
    if model.model_type != ModelType::Speech || model.media_unit.as_deref() != Some("token") {
        return None;
    }
    let mut rates = model
        .pricing_scopes
        .as_ref()?
        .iter()
        .flat_map(|scope| &scope.tiers)
        .map(|tier| {
            let input = amount(tier.rates.input_tokens.as_deref()?)? * 1_000_000.0;
            let output = amount(tier.rates.output_tokens.as_deref()?)? * 1_000_000.0;
            (input.is_finite() && output.is_finite()).then_some((input, output))
        });
    let pair = rates.next()??;
    rates.all(|rate| rate == Some(pair)).then_some(pair)
}

/// Минимальная ставка для строки выбора модели; единица указывается рядом с суммой.
pub fn starting_rate(model: &CatalogModel) -> Option<(PriceUnit, f64)> {
    let unit = match model.model_type {
        ModelType::Image if model.media_unit.as_deref() == Some("image") => PriceUnit::Image,
        ModelType::Video if model.media_unit.as_deref() == Some("second") => PriceUnit::Second,
        ModelType::Speech => return audio_rate(model),
        _ => return None,
    };
    let rate = model
        .media_prices
        .values()
        .filter_map(|v| amount(v))
        .chain(model.media_unit_price.as_deref().and_then(amount))
        .reduce(f64::min)?;
    Some((unit, rate))
}

pub fn starting_rate_for_mode(model: &CatalogModel, mode: GenMode) -> Option<(PriceUnit, f64)> {
    if model.model_type != ModelType::Image {
        return starting_rate(model);
    }
    let mut rates = image_rates(model, Some(mode))?.into_values();
    let first = rates.next()??;
    let minimum = rates.try_fold(first, |minimum, rate| Some(minimum.min(rate?)))?;
    Some((PriceUnit::Image, minimum))
}

/// Небольшие цены (например, за один символ речи) сохраняют значащие знаки.
pub fn format_price(value: f64) -> String {
    format!("${value:.8}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

/// В публичном каталоге базовое поле media_unit_price описывает генерацию.
/// Один и тот же размер у edits может стоить другую сумму.
fn image_rates(
    model: &CatalogModel,
    mode: Option<GenMode>,
) -> Option<BTreeMap<String, Option<f64>>> {
    if model
        .media_unit
        .as_deref()
        .is_some_and(|unit| unit != "image")
    {
        return None;
    }
    let mut rates = BTreeMap::new();
    if let Some(mode) = mode {
        let protocol = match mode {
            GenMode::ImageEdit => "images_edits",
            GenMode::TextToImage => "images_generations",
            _ => return None,
        };
        for entry in model.pricing_scopes.iter().flatten() {
            let Some(scope) = entry
                .scope
                .as_ref()
                .filter(|s| s.protocol.as_deref() == Some(protocol))
            else {
                continue;
            };
            let variant = scope.pricing_variant.as_deref()?.to_ascii_uppercase();
            let mut tiers = entry
                .tiers
                .iter()
                .map(|t| t.rates.images.as_deref().and_then(amount));
            let rate = tiers
                .next()
                .flatten()
                .filter(|&first| tiers.all(|r| r == Some(first)));
            rates
                .entry(variant)
                .and_modify(|old| {
                    if *old != rate {
                        *old = None;
                    }
                })
                .or_insert(rate);
        }
        if !rates.is_empty() {
            return Some(rates);
        }
        if mode == GenMode::ImageEdit && model.openrouter.is_none() {
            return None;
        }
    }
    rates.extend(
        model
            .media_prices
            .iter()
            .map(|(key, price)| (key.to_ascii_uppercase(), amount(price))),
    );
    if let Some(base) = base_rate(model) {
        rates.insert("DEFAULT".into(), Some(base));
    }
    Some(rates)
}

fn image_rate(model: &CatalogModel, params: &GenParams, mode: Option<GenMode>) -> Option<f64> {
    let rates = image_rates(model, mode)?;
    let base = rates.get("DEFAULT").copied().flatten();
    let quality = params.quality.as_deref().map(str::to_ascii_uppercase);
    if let (Some(size), Some(quality)) = (&params.size, &quality)
        && let Some(rate) = rates.get(&format!("{}+{quality}", size.to_ascii_uppercase()))
    {
        return *rate;
    }
    let quality_rate = quality.as_ref().and_then(|q| rates.get(q)).copied();
    let Some(size) = params.size.as_deref() else {
        return quality_rate.unwrap_or(base);
    };
    if let Some(exact) = rates.get(&size.to_ascii_uppercase()) {
        return *exact;
    }
    if let Some(rate) = quality_rate {
        return rate;
    }
    if rates.keys().all(|key| {
        matches!(
            key.as_str(),
            "DEFAULT" | "LOW" | "MEDIUM" | "HIGH" | "VECTOR_ILLUSTRATION"
        )
    }) {
        return base;
    }
    let (width, height) = size.split_once('x')?;
    let width = width.parse::<u64>().ok()?;
    let height = height.parse::<u64>().ok()?;
    let pixels = width.checked_mul(height)?;
    if pixels == 0 {
        return None;
    }
    if width == height
        && matches!(width, 1024 | 2048 | 4096)
        && let Some(price) = rates.get(&format!("{}K", width / 1024))
    {
        return *price;
    }
    let mp = pixels.div_ceil(1024 * 1024);
    if let Some(price) = rates.get(&format!("{mp}MP")) {
        return *price;
    }
    if rates
        .keys()
        .any(|k| k.ends_with("MP") || matches!(k.as_str(), "1K" | "2K" | "4K"))
    {
        return None;
    }
    let thresholds = rates.iter().filter_map(|(key, value)| {
        let cutoff = key
            .strip_prefix("OVER_")?
            .strip_suffix("PX")?
            .parse::<u64>()
            .ok()?;
        Some((cutoff, *value))
    });
    match thresholds
        .filter(|(cutoff, _)| pixels > *cutoff)
        .max_by_key(|(cutoff, _)| *cutoff)
    {
        Some((_, price)) => price,
        None => base,
    }
}

fn video_rate(model: &CatalogModel, params: &GenParams, mode: Option<GenMode>) -> Option<f64> {
    let base = base_rate(model);
    if model
        .media_prices
        .keys()
        .all(|key| key.eq_ignore_ascii_case("default"))
    {
        return base;
    }
    let priced_by_mode = model.media_prices.keys().any(|key| {
        key.split('+').any(|part| {
            matches!(
                part,
                "TEXT_TO_VIDEO" | "IMAGE_TO_VIDEO" | "REFERENCE_TO_VIDEO"
            )
        })
    });
    // При разных тарифах выбор разрешения и звука по умолчанию зависит от
    // поставщика. media_unit_price из карточки не определяет итоговый вариант.
    let resolution = params.resolution.as_deref()?;
    let mut variant = resolution.to_ascii_uppercase();
    if priced_by_mode {
        variant.push_str(match mode? {
            GenMode::TextToVideo => "+TEXT_TO_VIDEO",
            GenMode::ImageToVideo => "+IMAGE_TO_VIDEO",
            GenMode::ReferenceToVideo => "+REFERENCE_TO_VIDEO",
            _ => return None,
        });
    }
    let audio_variant = format!("{variant}+AUDIO");
    let priced_with_audio = model.media_prices.contains_key(&audio_variant);
    let audio = if priced_with_audio {
        params
            .audio
            .or_else(|| model.openrouter.as_ref()?.video_generate_audio)
    } else {
        params.audio
    };
    if audio.is_none() && priced_with_audio {
        // У модели два тарифа для этого разрешения, а её настройка звука по умолчанию неизвестна.
        return None;
    }
    if audio == Some(true) && (model.openrouter.is_none() || priced_with_audio) {
        variant = audio_variant;
    }
    model.media_prices.get(&variant).and_then(|s| amount(s))
}

pub fn estimate_price(model: &CatalogModel, params: &GenParams) -> Option<f64> {
    estimate(model, params, None, None)
}

pub fn estimate_price_for_prompt(
    model: &CatalogModel,
    params: &GenParams,
    prompt: &str,
) -> Option<f64> {
    estimate(model, params, None, Some(prompt))
}

pub fn estimate_generation_price(
    model: &CatalogModel,
    params: &GenParams,
    mode: GenMode,
    prompt: &str,
) -> Option<f64> {
    estimate(model, params, Some(mode), Some(prompt))
}

fn estimate(
    model: &CatalogModel,
    params: &GenParams,
    mode: Option<GenMode>,
    prompt: Option<&str>,
) -> Option<f64> {
    let copies = f64::from(params.n.unwrap_or(1));
    let (rate, units) = match model.model_type {
        ModelType::Image if model.media_unit.as_deref().is_none_or(|u| u == "image") => {
            (image_rate(model, params, mode)?, copies)
        }
        ModelType::Video if model.media_unit.as_deref().is_none_or(|u| u == "second") => (
            video_rate(model, params, mode)?,
            params.duration_seconds? * copies,
        ),
        ModelType::Speech => {
            let (unit, rate) = audio_rate(model)?;
            let count = match unit {
                PriceUnit::Character => prompt?.chars().count() as f64,
                // Для музыки и эффектов каталог даёт тариф, но длительность
                // результата становится известна только после генерации.
                PriceUnit::Second | PriceUnit::Image => return None,
            };
            (rate, count)
        }
        _ => return None,
    };
    let total = rate * units;
    (units > 0.0 && total.is_finite()).then_some(total)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadKind {
    Image,
    EditSource,
    OpenRouterEditSource,
    PngMask,
    Audio,
}

impl UploadKind {
    pub fn accept(self) -> &'static str {
        match self {
            Self::Image => "image/png,image/jpeg,image/webp,image/gif",
            Self::EditSource | Self::OpenRouterEditSource => "image/png,image/jpeg,image/webp",
            Self::PngMask => "image/png",
            Self::Audio => "audio/mpeg,audio/wav,audio/x-wav,audio/ogg",
        }
    }
}

pub const MAX_UPLOAD_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_OPENROUTER_EDIT_SOURCES: usize = 16;

/// Ключ ошибки переводится в тот язык, который выбран сейчас в интерфейсе.
pub fn validate_upload(mime: &str, size_bytes: u64, kind: UploadKind) -> Result<(), &'static str> {
    if matches!(kind, UploadKind::EditSource | UploadKind::PngMask)
        && size_bytes > km_shared::MAX_REQUEST_WIRE_BYTES as u64
    {
        return Err("controls.edit_wire_limit");
    }
    if !(1..=MAX_UPLOAD_BYTES).contains(&size_bytes) {
        return Err("controls.upload_size");
    }
    if !kind.accept().split(',').any(|allowed| allowed == mime) {
        return Err("controls.upload_type");
    }
    Ok(())
}

/// MIME браузера привязан к расширению. До показа/отправки сверяем заголовок файла.
pub fn matches_signature(mime: &str, bytes: &[u8]) -> bool {
    match mime {
        mime if mime.starts_with("image/") => km_shared::image::raster_mime(bytes) == Some(mime),
        "audio/wav" | "audio/x-wav" => {
            bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE")
        }
        "audio/ogg" => bytes.starts_with(b"OggS"),
        "audio/mpeg" => {
            bytes.starts_with(b"ID3") || matches!(bytes, [0xff, head, ..] if head & 0xe0 == 0xe0)
        }
        _ => false,
    }
}
