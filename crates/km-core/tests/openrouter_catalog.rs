use km_core::OpenRouterKeyClient;
use km_shared::{GenMode, MediaKind};
use serde_json::json;
use wiremock::matchers::{header, header_exists, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn image_catalog_distinguishes_creation_and_editing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/images/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                {
                    "id":"example/create:free",
                    "architecture":{"input_modalities":["text"],"output_modalities":["image"]},
                    "supported_parameters":{"n":{"type":"range","min":1,"max":6}}
                },
                {
                    "id":"example/edit",
                    "architecture":{"input_modalities":["text","image"],"output_modalities":["image"]},
                    "supported_parameters":{"input_references":{"type":"range","min":1,"max":8}}
                },
                {
                    "id":"example/both",
                    "architecture":{"input_modalities":["text","image"],"output_modalities":["text","image"]},
                    "supported_parameters":{
                        "input_references":{"type":"range","min":0,"max":1},
                        "output_format":{"type":"enum","values":["png","svg"]}
                    }
                },
                {
                    "id":"recraft/recraft-v4-vector",
                    "architecture":{"input_modalities":["text","image"],"output_modalities":["image"]},
                    "supported_parameters":{"output_format":{"type":"enum","values":["svg"]}}
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenRouterKeyClient::for_localhost(server.address().port());

    let models = client.models_for_kind("", MediaKind::Image).await.unwrap();
    assert_eq!(models.len(), 3);
    assert!(models[0].supports_mode(GenMode::TextToImage));
    assert!(!models[0].supports_mode(GenMode::ImageEdit));
    assert_eq!(models[0].media_unit_price.as_deref(), Some("0"));
    assert!(models[1].supports_mode(GenMode::ImageEdit));
    assert!(!models[1].supports_mode(GenMode::TextToImage));
    assert_eq!(
        models[1].openrouter.as_ref().unwrap().image_parameters["input_references"]
            .max
            .as_deref(),
        Some("8")
    );
    assert!(models[2].supports_mode(GenMode::TextToImage));
    assert!(models[2].supports_mode(GenMode::ImageEdit));
    assert_eq!(
        models[2].openrouter.as_ref().unwrap().image_parameters["output_format"].values,
        ["png"]
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|req| !req.headers.contains_key("authorization"))
    );
}

#[tokio::test]
async fn video_catalog_preserves_modes_and_converts_qualified_prices() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/videos/models"))
        .and(header("Authorization", "Bearer sk-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                {
                    "id":"x-ai/grok-imagine-video",
                    "supported_durations":[-1,1,15],
                    "supported_resolutions":["720p"],
                    "supported_aspect_ratios":["16:9"],
                    "supported_frame_images":["first_frame"],
                    "pricing_skus":{
                        "cents_per_image_input":"0.2",
                        "cents_per_video_output_second_720p":"7"
                    }
                },
                {
                    "id":"google/veo-3.1",
                    "supported_resolutions":["720p", "4K"],
                    "generate_audio":true,
                    "pricing_skus":{
                        "duration_seconds_with_audio":"0.40",
                        "duration_seconds_with_audio_4k":"0.60",
                        "duration_seconds_without_audio":"0.20"
                    }
                },
                {
                    "id":"black-forest-labs/flux-video-upscale"
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenRouterKeyClient::for_localhost(server.address().port());

    let models = client
        .models_for_kind("sk-test", MediaKind::Video)
        .await
        .unwrap();
    assert_eq!(models.len(), 2);
    assert!(models[0].supports_mode(GenMode::TextToVideo));
    assert!(models[0].supports_mode(GenMode::ImageToVideo));
    assert!(models[0].supports_mode(GenMode::ReferenceToVideo));
    assert_eq!(
        models[0].openrouter.as_ref().unwrap().video_durations,
        vec![1, 15]
    );
    assert_eq!(models[0].price_for("720P+TEXT_TO_VIDEO"), Some(0.07));
    assert_eq!(models[0].price_for("720P+IMAGE_TO_VIDEO"), None);
    assert_eq!(models[1].price_for("720P+AUDIO"), Some(0.4));
    assert_eq!(models[1].price_for("4K+AUDIO"), Some(0.6));
    assert_eq!(models[1].price_for("720P"), Some(0.2));
}

#[tokio::test]
async fn audio_catalog_keeps_distinct_modes_and_reads_speech_rates() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/audio/speech/models"))
        .and(header_exists("Authorization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data":[
                {
                    "id":"bytedance-seed/seed-audio-1-0",
                    "output_modalities":[{"type":"speech"}]
                },
                {
                    "id":"google/gemini-2.5-pro-tts",
                    "output_modalities":[{"type":"speech","supported_parameters":{
                        "response_format":{"type":"enum","values":["pcm","wav"]}
                    }}],
                    "endpoints":[{"output_modalities":[{"type":"speech","supported_parameters":{
                        "voice":{"type":"enum","values":["Kore"]}
                    }}]}]
                },
                {
                    "id":"heygen/heygen-voice-1-turbo",
                    "output_modalities":[{"type":"speech","supported_parameters":{
                        "response_format":{"type":"enum","values":["pcm"]}
                    }}]
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/models"))
        .and(query_param("output_modalities", "speech,audio"))
        .and(header_exists("Authorization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data":[
                {
                    "id":"bytedance-seed/seed-audio-1-0",
                    "endpoints":["/api/v1/audio/speech"],
                    "pricing":{"prompt":"0","completion":"0.0025"}
                },
                {
                    "id":"google/gemini-2.5-pro-tts",
                    "endpoints":["/api/v1/audio/speech"],
                    "pricing":{"prompt":"0.000001","completion":"0.00002"}
                },
                {
                    "id":"google/lyria-3-clip-preview",
                    "endpoints":["/api/v1/chat/completions"],
                    "pricing":{"prompt":"0","completion":"0"}
                },
                {
                    "id":"heygen/heygen-voice-1-turbo:free",
                    "endpoints":["/api/v1/audio/speech"],
                    "pricing":{"prompt":"0","completion":"0"},
                    "supported_voices":["nadine"]
                },
                {
                    "id":"openai/gpt-audio",
                    "endpoints":["/api/v1/chat/completions"]
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenRouterKeyClient::for_localhost(server.address().port());

    let models = client
        .models_for_kind("sk-test", MediaKind::Audio)
        .await
        .unwrap();
    assert_eq!(models.len(), 5);
    assert!(models[0].supports_mode(GenMode::Speech));
    assert!(models[0].supports_mode(GenMode::SoundEffects));
    assert_eq!(models[0].media_unit.as_deref(), Some("second"));
    assert_eq!(models[0].media_unit_price.as_deref(), Some("0.0025"));
    assert_eq!(models[1].media_unit.as_deref(), Some("token"));
    let options = models[1].openrouter.as_ref().unwrap();
    assert_eq!(options.tts_parameters["voice"].values, ["Kore"]);
    assert_eq!(
        options.tts_parameters["response_format"].values,
        ["pcm", "wav"]
    );
    let free_voice = &models[3];
    assert_eq!(free_voice.media_unit_price.as_deref(), Some("0"));
    let parameters = &free_voice.openrouter.as_ref().unwrap().tts_parameters;
    assert_eq!(parameters["voice"].values, ["nadine"]);
    assert_eq!(parameters["response_format"].values, ["pcm"]);
    assert!(models[4].supports_mode(GenMode::Music));
    assert!(!models[4].supports_mode(GenMode::Speech));
    assert_eq!(models[4].media_unit_price, None);
}
