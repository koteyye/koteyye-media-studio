//! Интеграционные тесты типов каталога CI на фикстуре из живого `/public/models`.

use km_shared::{CatalogModel, ModelType};
use serde::Deserialize;

/// Обёртка ответа каталога CI: `{"models":[...]}`.
#[derive(Deserialize)]
struct CatalogResponse {
    models: Vec<CatalogModel>,
}

/// Парсит фикстуру живого каталога (10 моделей, снято с api.cheaperinference.com).
fn fixture_models() -> Vec<CatalogModel> {
    let raw = include_str!("fixtures/public-models.json");
    let response: CatalogResponse =
        serde_json::from_str(raw).expect("фикстура живого каталога должна десериализоваться");
    response.models
}

/// Находит модель в фикстуре по id; паникует с понятным сообщением, если её нет.
fn find<'a>(models: &'a [CatalogModel], id: &str) -> &'a CatalogModel {
    models
        .iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("модель {id} должна быть в фикстуре"))
}

#[test]
fn fixture_parses_into_ten_models() {
    let models = fixture_models();
    assert_eq!(models.len(), 10, "фикстура содержит ровно 10 моделей");
}

#[test]
fn veo_is_video_with_image_input_and_media_price() {
    let models = fixture_models();
    let veo = find(&models, "veo-3.1-generate-001");

    assert_eq!(veo.model_type, ModelType::Video);
    assert!(
        veo.has_feature("image-input"),
        "features: {:?}",
        veo.features
    );
    assert!(!veo.has_feature("image-edit"));
    assert!(veo.supports("/v1/videos/generations"));
    assert!(!veo.supports("/v1/audio/speech"));
    assert_eq!(veo.price_for("720P+AUDIO"), Some(0.26));
    assert_eq!(veo.price_for("8K"), None, "несуществующий ключ цены");
    assert_eq!(veo.provider_name.as_deref(), Some("Google"));
    assert_eq!(veo.media_unit.as_deref(), Some("second"));
    assert_eq!(veo.discount_percent.as_deref(), Some("35.00"));
    assert!(
        veo.aliases
            .iter()
            .any(|a| a == "google/veo-3.1-generate-001")
    );
}

#[test]
fn music_v2_is_speech_with_speech_endpoint() {
    let models = fixture_models();
    let music = find(&models, "music-v2");

    assert_eq!(music.model_type, ModelType::Speech);
    assert!(music.supports("/v1/audio/speech"));
    assert_eq!(music.logo_url, None, "CI присылает null для logo_url");
}

#[test]
fn flux_2_pro_is_image_with_edit_support() {
    let models = fixture_models();
    let flux = find(&models, "flux-2-pro");

    assert_eq!(flux.model_type, ModelType::Image);
    assert!(flux.supports_image_edit);
    assert!(flux.has_feature("image-edit"));
    assert!(flux.supports("/v1/images/edits"));
    assert_eq!(flux.price_for("1MP"), Some(0.0195));
}

#[test]
fn unknown_model_type_falls_back_and_optional_fields_default() {
    // Неизвестный тип и неизвестные поля не должны ломать десериализацию.
    let json = r#"{"id":"mystery","model_type":"hologram","future_field":{"x":1}}"#;
    let model: CatalogModel =
        serde_json::from_str(json).expect("неизвестный тип должен попадать в Unknown");

    assert_eq!(model.model_type, ModelType::Unknown("hologram".to_owned()));
    assert_eq!(model.provider_name, None);
    assert_eq!(model.logo_url, None);
    assert_eq!(model.media_unit, None);
    assert_eq!(model.discount_percent, None);
    assert!(model.features.is_empty());
    assert!(model.supported_endpoints.is_empty());
    assert!(model.media_prices.is_empty());
    assert!(model.aliases.is_empty());
    assert!(!model.supports_image_edit);
}

#[test]
fn model_type_serializes_back_to_catalog_strings() {
    // Известные типы обязаны попадать в свои варианты, а не в Unknown.
    let cases = [
        ("image", ModelType::Image),
        ("video", ModelType::Video),
        ("speech", ModelType::Speech),
        ("text", ModelType::Text),
        ("embedding", ModelType::Embedding),
        ("transcription", ModelType::Transcription),
        ("rerank", ModelType::Rerank),
        ("decision", ModelType::Decision),
        ("realtime", ModelType::Realtime),
    ];
    for (raw, expected) in cases {
        let json = format!(r#"{{"id":"m","model_type":"{raw}"}}"#);
        let model: CatalogModel =
            serde_json::from_str(&json).expect("тип должен десериализоваться");
        assert!(
            !matches!(model.model_type, ModelType::Unknown(_)),
            "известный тип {raw} не должен попадать в Unknown"
        );
        assert_eq!(model.model_type, expected, "вариант для {raw}");

        // Round-trip: сериализация возвращает те же строки, что присылает каталог.
        let value = serde_json::to_value(&model).expect("тип должен сериализоваться");
        assert_eq!(value["model_type"], raw, "round-trip для {raw}");
        let back: CatalogModel =
            serde_json::from_value(value).expect("обратная десериализация должна работать");
        assert_eq!(back, model, "round-trip для {raw}");
    }

    // Заведомо неизвестный тип — в Unknown с сохранением исходной строки.
    let json = r#"{"id":"m","model_type":"hologram"}"#;
    let model: CatalogModel =
        serde_json::from_str(json).expect("неизвестный тип должен десериализоваться");
    assert!(matches!(model.model_type, ModelType::Unknown(_)));
    assert_eq!(
        model.model_type,
        ModelType::Unknown("hologram".to_owned()),
        "исходная строка каталога должна сохраниться"
    );
    let value = serde_json::to_value(&model).expect("тип должен сериализоваться");
    assert_eq!(value["model_type"], "hologram", "round-trip для hologram");
}

#[test]
fn price_for_non_numeric_value_is_none() {
    let json = r#"{"id":"m","model_type":"image","media_prices":{"1MP":"бесплатно"}}"#;
    let model: CatalogModel =
        serde_json::from_str(json).expect("нечисловая цена не должна ломать парсинг");
    assert_eq!(model.price_for("1MP"), None);
}
