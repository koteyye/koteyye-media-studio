//! Интеграционные тесты типов запросов/ответов CI, истории, событий и настроек.

use km_shared::{
    AppEvent, Billing, GenMode, GenParams, Generation, GenerationRequest, HistoryRecord,
    ImageResponse, JobStatus, Lang, LangPreference, MediaKind, Settings, Theme, VideoResponse,
};

#[test]
fn billing_cost_usd_parses_string_with_six_decimals() {
    // Строка с 6 знаками парсится в f64; null/отсутствие/мусор дают None, не панику.
    // NaN/inf/отрицательные стоимости тоже None: расход не может быть таким.
    let cases = [
        (r#"{"billed_cost_usd":"0.123456"}"#, Some(0.123456)),
        (r#"{"billed_cost_usd":"0.000000"}"#, Some(0.0)),
        (r#"{}"#, None),
        (r#"{"billed_cost_usd":"abc"}"#, None),
        (r#"{"billed_cost_usd":null}"#, None),
        (r#"{"billed_cost_usd":"NaN"}"#, None),
        (r#"{"billed_cost_usd":"inf"}"#, None),
        (r#"{"billed_cost_usd":"-inf"}"#, None),
        (r#"{"billed_cost_usd":"1e309"}"#, None),
        (r#"{"billed_cost_usd":"-0.040000"}"#, None),
    ];
    for (json, expected) in cases {
        let billing: Billing =
            serde_json::from_str(json).expect("billing должен десериализоваться при любом входе");
        assert_eq!(billing.cost_usd(), expected, "вход: {json}");
    }
}

#[test]
fn image_response_deserializes_b64_and_billing() {
    let json = r#"{"created":1,"data":[{"b64_json":"aGk="}],"request_id":"uuid","billing":{"billed_cost_usd":"0.040000"}}"#;
    let resp: ImageResponse =
        serde_json::from_str(json).expect("ответ images должен десериализоваться");

    assert_eq!(resp.created, Some(1));
    assert_eq!(resp.data.len(), 1);
    assert_eq!(resp.data[0].b64_json.as_deref(), Some("aGk="));
    assert_eq!(resp.data[0].url, None);
    assert_eq!(resp.data[0].content_type, None);
    assert_eq!(resp.request_id.as_deref(), Some("uuid"));
    assert_eq!(
        resp.billing.as_ref().and_then(Billing::cost_usd),
        Some(0.04)
    );
}

#[test]
fn video_response_deserializes_videos() {
    let json = r#"{"id":"v1","status":"completed","videos":[{"url":"https://x/v.mp4","content_type":"video/mp4"}]}"#;
    let resp: VideoResponse =
        serde_json::from_str(json).expect("ответ videos должен десериализоваться");

    assert_eq!(resp.id, "v1");
    assert_eq!(resp.status, "completed");
    assert_eq!(resp.videos.len(), 1);
    assert_eq!(resp.videos[0].url.as_deref(), Some("https://x/v.mp4"));
    assert_eq!(resp.videos[0].content_type.as_deref(), Some("video/mp4"));
    assert_eq!(resp.request_id, None);
    assert_eq!(resp.billing, None);
    assert_eq!(resp.usage, None);
}

#[test]
fn video_response_keeps_usage_and_billing() {
    let json = r#"{"id":"v2","status":"completed","videos":[],"request_id":"r2","billing":{"billed_cost_usd":"0.260000"},"usage":{"seconds":5}}"#;
    let resp: VideoResponse =
        serde_json::from_str(json).expect("usage и billing должны десериализоваться");

    assert_eq!(resp.request_id.as_deref(), Some("r2"));
    assert_eq!(
        resp.billing.as_ref().and_then(Billing::cost_usd),
        Some(0.26)
    );
    assert_eq!(resp.usage, Some(serde_json::json!({"seconds": 5})));
}

#[test]
fn generation_deserializes_files_with_type_rename() {
    let json = r#"{
        "request_id":"gen-1",
        "endpoint":"/v1/videos/generations",
        "created_at":"2026-11-24T10:00:00Z",
        "expires_at":"2026-11-25T10:00:00Z",
        "files":[{"index":0,"type":"video","content_type":"video/mp4","size_bytes":10,"url":"https://x"}]
    }"#;
    let generation: Generation =
        serde_json::from_str(json).expect("generation должен десериализоваться");

    assert_eq!(generation.request_id, "gen-1");
    assert_eq!(generation.endpoint, "/v1/videos/generations");
    assert_eq!(generation.created_at, "2026-11-24T10:00:00Z");
    assert_eq!(generation.expires_at, "2026-11-25T10:00:00Z");
    assert_eq!(generation.files.len(), 1);

    let file = &generation.files[0];
    assert_eq!(file.index, 0);
    assert_eq!(file.kind, "video", "поле JSON type должно попадать в kind");
    assert_eq!(file.content_type, "video/mp4");
    assert_eq!(file.size_bytes, 10);
    assert_eq!(file.url, "https://x");

    // Обратно: kind сериализуется как type, а не как kind.
    let value = serde_json::to_value(&generation).expect("generation должен сериализоваться");
    assert_eq!(value["files"][0]["type"], "video");
    assert!(
        value["files"][0].get("kind").is_none(),
        "поля kind в JSON быть не должно"
    );
}

#[test]
fn history_record_serde_round_trip() {
    let json = r#"{
        "id":"h1","created_at":1700000000,"updated_at":1700000001,
        "kind":"image","mode":"text_to_image","model":"flux-2-pro",
        "prompt":"котейе","status":"completed","request_id":"uuid",
        "local_paths":["/media/km/1.png"],"cost_usd":0.04,
        "params_json":"{\"size\":\"1024x1024\"}","error":null
    }"#;
    let record: HistoryRecord =
        serde_json::from_str(json).expect("запись истории должна десериализоваться");

    assert_eq!(record.id, "h1");
    assert_eq!(record.created_at, 1_700_000_000);
    assert_eq!(record.updated_at, 1_700_000_001);
    assert_eq!(record.kind, MediaKind::Image);
    assert_eq!(record.mode, GenMode::TextToImage);
    assert_eq!(record.model, "flux-2-pro");
    assert_eq!(record.prompt, "котейе");
    assert_eq!(record.status, JobStatus::Completed);
    assert_eq!(record.request_id.as_deref(), Some("uuid"));
    assert_eq!(record.local_paths, ["/media/km/1.png"]);
    assert_eq!(record.cost_usd, Some(0.04));
    assert_eq!(record.params_json, r#"{"size":"1024x1024"}"#);
    assert_eq!(record.error, None);

    // JSON → struct → JSON: ключевые поля равны исходным.
    let original: serde_json::Value = serde_json::from_str(json).expect("исходный JSON валиден");
    let value = serde_json::to_value(&record).expect("запись должна сериализоваться");
    for key in [
        "id",
        "created_at",
        "updated_at",
        "kind",
        "mode",
        "model",
        "prompt",
        "status",
        "request_id",
        "local_paths",
        "cost_usd",
        "params_json",
        "error",
    ] {
        assert_eq!(value[key], original[key], "round-trip поля {key}");
    }
    let back: HistoryRecord =
        serde_json::from_value(value).expect("обратная десериализация должна работать");
    assert_eq!(back, record, "round-trip записи истории");
}

#[test]
fn responses_tolerate_missing_optional_fields() {
    // CI может не прислать опциональные поля — десериализация не должна падать.
    let image: ImageResponse =
        serde_json::from_str("{}").expect("пустой ответ images должен десериализоваться");
    assert!(image.data.is_empty());
    assert_eq!(image.created, None);
    assert_eq!(image.request_id, None);
    assert_eq!(image.billing, None);

    let video: VideoResponse = serde_json::from_str(r#"{"id":"v","status":"pending"}"#)
        .expect("ответ videos без опциональных полей должен десериализоваться");
    assert!(video.videos.is_empty());
    assert_eq!(video.request_id, None);
    assert_eq!(video.billing, None);
    assert_eq!(video.usage, None);
}

#[test]
fn generation_request_tolerates_missing_params_and_references() {
    let json = r#"{"kind":"video","mode":"text_to_video","model":"veo-3.1-generate-001","prompt":"котейе"}"#;
    let req: GenerationRequest =
        serde_json::from_str(json).expect("params/references/mask могут отсутствовать");

    assert_eq!(req.kind, MediaKind::Video);
    assert_eq!(req.mode, GenMode::TextToVideo);
    assert_eq!(req.model, "veo-3.1-generate-001");
    assert_eq!(req.prompt, "котейе");
    assert_eq!(req.params, GenParams::default());
    assert!(req.references.is_empty());
    assert_eq!(req.mask_data_url, None);
}

#[test]
fn app_event_serde_shape_is_snake_case() {
    let event = AppEvent::GenerationFailed {
        history_id: "h1".to_owned(),
        error: "402".to_owned(),
    };
    let value = serde_json::to_value(&event).expect("событие должно сериализоваться");
    assert_eq!(
        value,
        serde_json::json!({"generation_failed": {"history_id": "h1", "error": "402"}})
    );
    let back: AppEvent = serde_json::from_value(value).expect("событие должно десериализоваться");
    assert_eq!(back, event);

    let balance = AppEvent::BalanceUpdated;
    let value = serde_json::to_value(&balance).expect("BalanceUpdated должен сериализоваться");
    let back: AppEvent =
        serde_json::from_value(value).expect("BalanceUpdated должен десериализоваться");
    assert_eq!(back, balance);
}

#[test]
fn settings_round_trip_lowercase_enums() {
    let json =
        r#"{"local_folder":"/media/km","lang":"ru","theme":"system","show_real_balance":true}"#;
    let settings: Settings =
        serde_json::from_str(json).expect("настройки должны десериализоваться");

    assert_eq!(settings.local_folder, "/media/km");
    assert_eq!(settings.lang, LangPreference::Ru);
    assert_eq!(settings.theme, Theme::System);
    assert!(settings.show_real_balance);
    assert_eq!(settings.provider, km_shared::Provider::CheaperInference);
    assert_eq!(settings.workspace, km_shared::WorkspacePrefs::default());

    let value = serde_json::to_value(&settings).expect("настройки должны сериализоваться");
    assert_eq!(value["lang"], "ru");
    assert_eq!(value["theme"], "system");
    let back: Settings =
        serde_json::from_value(value).expect("обратная десериализация должна работать");
    assert_eq!(back, settings, "round-trip настроек");
}

#[test]
fn system_language_follows_the_primary_locale_with_english_fallback() {
    for (locale, expected) in [
        ("ru", Lang::Ru),
        ("ru-RU", Lang::Ru),
        ("RU_BY.UTF-8", Lang::Ru),
        ("en-RU", Lang::En),
        ("en-US", Lang::En),
        ("de-DE", Lang::En),
        ("", Lang::En),
    ] {
        assert_eq!(
            LangPreference::System.resolve(Lang::from_locale(locale)),
            expected
        );
    }
    assert_eq!(LangPreference::En.resolve(Lang::Ru), Lang::En);
    assert_eq!(LangPreference::Ru.resolve(Lang::En), Lang::Ru);
}

#[test]
fn settings_default_to_system_language() {
    let legacy = r#"{"local_folder":"","theme":"light","show_real_balance":false}"#;
    let settings: Settings = serde_json::from_str(legacy).unwrap();
    assert_eq!(settings.lang, LangPreference::System);
    assert_eq!(Settings::default().lang, LangPreference::System);
    assert_eq!(serde_json::to_value(settings).unwrap()["lang"], "system");
}
