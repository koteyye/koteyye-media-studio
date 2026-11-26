//! Проверяем формы и расчёт цен на реальной выборке каталога CI.

use km_shared::{CatalogModel, GenMode, GenParams, MediaKind, Reference};
use km_ui_core::studio::*;

fn catalog() -> Vec<CatalogModel> {
    let fixture = include_str!("../../km-shared/tests/fixtures/public-models.json");
    let data: serde_json::Value = serde_json::from_str(fixture).unwrap();
    serde_json::from_value(data["models"].clone()).unwrap()
}

#[test]
fn image_estimates_follow_protocol_size_quality_and_n() {
    let scope = |protocol: &str, variant: &str, rate: &str| {
        serde_json::json!({
            "scope":{"protocol":protocol,"pricingVariant":variant},
            "tiers":[{"rates":{"images":rate}}]
        })
    };
    let mut model: CatalogModel = serde_json::from_value(serde_json::json!({
        "id":"studio-image","model_type":"image","media_unit":"image","media_unit_price":"0.010000",
        "pricing_scopes":[
            scope("images_generations","default","0.010000"),
            scope("images_generations","2048x2048","0.020000"),
            scope("images_generations","2048x2048+low","0.014000"),
            scope("images_edits","default","0.030000"),
            scope("images_edits","low","0.018000")
        ]
    }))
    .unwrap();
    let mut params = GenParams {
        n: Some(2),
        size: Some("2048x2048".into()),
        ..Default::default()
    };
    assert_eq!(
        estimate_generation_price(&model, &params, GenMode::TextToImage, ""),
        Some(0.04)
    );
    assert_eq!(
        estimate_generation_price(&model, &params, GenMode::ImageEdit, ""),
        Some(0.06)
    );
    params.quality = Some("low".into());
    assert_eq!(
        estimate_generation_price(&model, &params, GenMode::TextToImage, ""),
        Some(0.028)
    );
    assert_eq!(
        estimate_generation_price(&model, &params, GenMode::ImageEdit, ""),
        Some(0.036)
    );
    assert_eq!(
        starting_rate_for_mode(&model, GenMode::ImageEdit),
        Some((PriceUnit::Image, 0.018))
    );
    model.pricing_scopes = None;
    assert_eq!(
        estimate_generation_price(&model, &params, GenMode::ImageEdit, ""),
        None
    );
    assert_eq!(starting_rate_for_mode(&model, GenMode::ImageEdit), None);
}

#[test]
fn image_edit_does_not_use_generation_threshold_or_assume_a_rate_across_tiers() {
    let model: CatalogModel = serde_json::from_value(serde_json::json!({
        "id":"seedream-5.0-pro","model_type":"image","media_unit":"image",
        "pricing_scopes":[
            {"scope":{"protocol":"images_generations","pricingVariant":"default"},"tiers":[{"rates":{"images":"0.029250"}}]},
            {"scope":{"protocol":"images_generations","pricingVariant":"over_2610000px"},"tiers":[{"rates":{"images":"0.058500"}}]},
            {"scope":{"protocol":"images_edits","pricingVariant":"default"},"tiers":[{"rates":{"images":"0.029250"}}]},
            {"scope":{"protocol":"images_edits","pricingVariant":"low"},"tiers":[{"rates":{"images":"0.02"}},{"rates":{"images":"0.03"}}]}
        ]
    })).unwrap();
    let mut params = GenParams {
        size: Some("2048x2048".into()),
        ..Default::default()
    };
    assert_eq!(
        estimate_generation_price(&model, &params, GenMode::TextToImage, ""),
        Some(0.0585)
    );
    assert_eq!(
        estimate_generation_price(&model, &params, GenMode::ImageEdit, ""),
        Some(0.02925)
    );
    params.quality = Some("low".into());
    assert_eq!(
        estimate_generation_price(&model, &params, GenMode::ImageEdit, ""),
        None
    );
}

#[test]
fn editing_keeps_references_mask_and_parameters() {
    let reference = Reference {
        data_url: "data:image/png;base64,YQ==".into(),
    };
    let mask = Some("data:image/png;base64,Yg==".into());
    let params = default_params(GenMode::ImageEdit);
    let req = build_request(
        MediaKind::Image,
        GenMode::ImageEdit,
        "flux",
        "  Кот!  ",
        params.clone(),
        vec![reference.clone()],
        mask.clone(),
    );
    assert_eq!(req.prompt, "  Кот!  ");
    assert_eq!(req.references, vec![reference]);
    assert_eq!(req.mask_data_url, mask);
    assert_eq!(req.params, params);
    assert_eq!(serde_json::to_value(&req).unwrap()["mode"], "image_edit");
    assert_eq!(default_params(GenMode::Speech), GenParams::default());
    assert!(default_params(GenMode::Music).voice.is_none());
}

#[test]
fn attachment_count_selects_the_supported_generation_mode() {
    let models: Vec<CatalogModel> = serde_json::from_value(serde_json::json!([
        {"id":"image-text","model_type":"image","supported_endpoints":["/v1/images/generations"]},
        {"id":"image-edit","model_type":"image","supports_image_edit":true,"supported_endpoints":["/v1/images/edits"]},
        {"id":"video-frame","model_type":"video","openrouter":{"modes":["image_to_video"]}},
        {"id":"video-refs","model_type":"video","openrouter":{"modes":["reference_to_video"]}},
        {"id":"video-all","model_type":"video","openrouter":{"modes":["text_to_video","image_to_video","reference_to_video"]}}
    ])).unwrap();
    let image = |model: usize, count| {
        mode_for_input(
            MediaKind::Image,
            GenMode::TextToImage,
            &models[model],
            count,
        )
    };
    assert_eq!(image(0, 0), Some(GenMode::TextToImage));
    assert_eq!(image(0, 1), None);
    assert_eq!(image(1, 0), None);
    assert_eq!(image(1, 1), Some(GenMode::ImageEdit));
    assert_eq!(
        models_for_input(&models, MediaKind::Image, GenMode::TextToImage, 0).len(),
        2
    );

    let video = |model: usize, count| {
        mode_for_input(
            MediaKind::Video,
            GenMode::TextToVideo,
            &models[model],
            count,
        )
    };
    assert_eq!(video(2, 2), None);
    assert_eq!(video(3, 1), Some(GenMode::ReferenceToVideo));
    assert_eq!(video(4, 0), Some(GenMode::TextToVideo));
    assert_eq!(video(4, 1), Some(GenMode::ImageToVideo));
    assert_eq!(video(4, 2), Some(GenMode::ReferenceToVideo));
    assert_eq!(
        models_for_input(&models, MediaKind::Video, GenMode::TextToVideo, 2).len(),
        2
    );
}

#[test]
fn model_picker_filters_kind_and_mode_before_grouping_and_search() {
    let all = catalog();
    assert!(models_for(&all, MediaKind::Image, GenMode::ImageToVideo).is_empty());
    let images = models_for(&all, MediaKind::Image, GenMode::ImageEdit);
    let mut options = search_models(&images, " FLuX ");
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].id, "flux-2-pro");
    options = search_models(&images, "no such model");
    assert!(options.is_empty());
    let videos = models_for(&all, MediaKind::Video, GenMode::ImageToVideo);
    assert!(
        videos
            .iter()
            .all(|m| m.model_type == km_shared::ModelType::Video)
    );
    assert!(videos.iter().any(|m| m.id == "veo-3.1-generate-001"));
    let groups = group_by_provider(&images);
    assert!(
        groups
            .iter()
            .any(|(name, list)| *name == "Black Forest Labs" && list[0].id == "flux-2-pro")
    );
    assert_eq!(
        groups.iter().map(|(_, list)| list.len()).sum::<usize>(),
        images.len()
    );
}

#[test]
fn real_catalog_rates_use_output_parameters_and_billing_units() {
    let all = catalog();
    let model = |id: &str| all.iter().find(|m| m.id == id).unwrap();
    let mut params = GenParams {
        resolution: Some("720p".into()),
        audio: Some(true),
        duration_seconds: Some(6.0),
        ..Default::default()
    };
    assert_eq!(
        estimate_price(model("veo-3.1-generate-001"), &params),
        Some(1.56)
    );
    params.resolution = Some("3P".into());
    assert_eq!(estimate_price(model("veo-3.1-generate-001"), &params), None);
    let params = GenParams {
        size: Some("1024x1024".into()),
        n: Some(2),
        ..Default::default()
    };
    assert_eq!(estimate_price(model("flux-2-pro"), &params), Some(0.039));
    assert_eq!(
        estimate_price(model("seedream-5.0-pro"), &params),
        Some(0.0765)
    );
    assert_eq!(estimate_price(model("gpt-image-2"), &params), None);
    let speech = model("eleven-v4");
    assert_eq!(estimate_price(speech, &GenParams::default()), None);
    assert_eq!(
        estimate_price_for_prompt(speech, &GenParams::default(), "Я🙂"),
        Some(2.0 * 0.000026)
    );
    let music = model("music-v2");
    let params = GenParams {
        duration_seconds: Some(8.0),
        ..Default::default()
    };
    assert_eq!(estimate_price(music, &params), None);
    assert_eq!(starting_rate(music), Some((PriceUnit::Second, 0.002860)));
    assert!(
        build_request(
            MediaKind::Audio,
            GenMode::Music,
            &music.id,
            "music",
            params,
            vec![],
            None
        )
        .params
        .duration_seconds
        .is_none()
    );
}

#[test]
fn token_billed_speech_shows_published_pair_without_guessing_output_length() {
    let model: CatalogModel = serde_json::from_value(serde_json::json!({
        "id": "gemini-3.8-flash-tts", "model_type": "speech", "media_unit": "token",
        "pricing_scopes": [{"tiers": [{"rates": {
            "input_tokens": "0.000000425", "output_tokens": "0.00000765"
        }}]}]
    }))
    .unwrap();
    let rates = speech_token_rates(&model).unwrap();
    assert_eq!(
        (format_price(rates.0), format_price(rates.1)),
        ("$0.425".into(), "$7.65".into())
    );
    assert_eq!(
        estimate_price_for_prompt(&model, &GenParams::default(), "привет"),
        None
    );
}

#[test]
fn upload_checks_raster_types_mask_size_and_signature() {
    assert!(validate_upload("image/gif", 100, UploadKind::Image).is_ok());
    assert!(validate_upload("image/gif", 100, UploadKind::EditSource).is_err());
    assert!(validate_upload("image/webp", 4_000_000, UploadKind::EditSource).is_ok());
    assert_eq!(
        validate_upload("image/png", 4_500_001, UploadKind::PngMask),
        Err("controls.edit_wire_limit")
    );
    assert!(validate_upload("image/png", 1_000_000, UploadKind::Image).is_ok());
    assert!(validate_upload("audio/mpeg", 1000, UploadKind::Audio).is_ok());
    assert!(validate_upload("image/jpeg", 1000, UploadKind::PngMask).is_err());
    assert!(validate_upload("image/svg+xml", 1000, UploadKind::Image).is_err());
    assert!(validate_upload("image/jpeg", MAX_UPLOAD_BYTES + 1, UploadKind::Image).is_err());
    assert!(validate_upload("image/png", 0, UploadKind::Image).is_err());
    assert!(!matches_signature("image/png", b"<svg></svg>"));
    assert!(matches_signature("image/png", b"\x89PNG\r\n\x1a\n"));
}

#[test]
fn default_tariff_and_unknown_audio_variant_are_not_confused() {
    let flat: CatalogModel = serde_json::from_value(serde_json::json!({
        "id": "illustrator", "model_type":"image", "media_unit":"image",
        "media_prices": {"DEFAULT":"0.024883"}
    }))
    .unwrap();
    let params = default_params(GenMode::TextToImage);
    assert_eq!(
        estimate_price(&flat, &params).map(format_price).as_deref(),
        Some("$0.024883")
    );
    let video: CatalogModel = serde_json::from_value(serde_json::json!({
        "id":"video", "model_type":"video", "media_unit":"second",
        "media_prices":{"720P":"0.05","720P+AUDIO":"0.10"}
    }))
    .unwrap();
    let mut params = GenParams {
        resolution: Some("720P".into()),
        duration_seconds: Some(8.0),
        ..Default::default()
    };
    assert_eq!(estimate_price(&video, &params), None);
    params.audio = Some(false);
    assert_eq!(estimate_price(&video, &params), Some(0.4));
}

#[test]
fn video_estimate_uses_the_published_mode_and_resolution() {
    let model: CatalogModel = serde_json::from_value(serde_json::json!({
        "id": "heygen-video-1", "model_type":"video", "media_unit":"second",
        "media_unit_price":"0.045000",
        "media_prices":{
            "480P+TEXT_TO_VIDEO":"0.009000", "480P+IMAGE_TO_VIDEO":"0.009000",
            "768P+TEXT_TO_VIDEO":"0.016200", "768P+IMAGE_TO_VIDEO":"0.016200",
            "1080P+TEXT_TO_VIDEO":"0.045000", "1080P+IMAGE_TO_VIDEO":"0.045000",
            "2K+TEXT_TO_VIDEO":"0.045000", "2K+IMAGE_TO_VIDEO":"0.045000"
        }
    }))
    .unwrap();
    let mut params = GenParams {
        duration_seconds: Some(6.0),
        n: Some(2),
        ..Default::default()
    };
    let estimate = |params: &GenParams, mode| {
        estimate_generation_price(&model, params, mode, "").map(format_price)
    };
    assert_eq!(estimate(&params, GenMode::TextToVideo), None);
    params.resolution = Some("480p".into());
    assert_eq!(
        estimate(&params, GenMode::TextToVideo).as_deref(),
        Some("$0.108")
    );
    assert_eq!(estimate(&params, GenMode::ReferenceToVideo), None);
    params.resolution = Some("768P".into());
    assert_eq!(
        estimate(&params, GenMode::ImageToVideo).as_deref(),
        Some("$0.1944")
    );
    params.audio = Some(true);
    assert_eq!(estimate(&params, GenMode::ImageToVideo), None);
}
