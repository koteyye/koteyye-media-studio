use km_core::{CoreError, OpenRouterKeyClient};
use km_shared::{GenMode, GenParams, GenerationRequest, MediaKind, Reference};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-or-v1-test";
const AUTH: &str = "Bearer sk-or-v1-test";
const IMAGE: &str = "data:image/png;base64,iVBORw0KGgo=";

fn request(kind: MediaKind, mode: GenMode, model: &str) -> GenerationRequest {
    GenerationRequest {
        kind,
        mode,
        model: model.into(),
        prompt: "Sunrise".into(),
        params: GenParams::default(),
        references: Vec::new(),
        mask_data_url: None,
    }
}

fn client(server: &MockServer) -> OpenRouterKeyClient {
    OpenRouterKeyClient::for_localhost(server.address().port())
}

#[tokio::test]
async fn edit_image_sends_references_and_reads_media_type_and_usage() {
    let server = MockServer::start().await;
    let mut req = request(MediaKind::Image, GenMode::ImageEdit, "openai/gpt-image-1");
    req.references.push(Reference {
        data_url: IMAGE.into(),
    });
    req.references.push(Reference {
        data_url: "https://media.example/ref.jpg".into(),
    });
    req.params.n = Some(2);
    req.params.aspect_ratio = Some("16:9".into());
    req.params.quality = Some("high".into());

    Mock::given(method("POST"))
        .and(path("/api/v1/images"))
        .and(header("authorization", AUTH))
        .and(body_json(json!({
            "model": "openai/gpt-image-1",
            "prompt": "Sunrise",
            "n": 2,
            "aspect_ratio": "16:9",
            "quality": "high",
            "input_references": [
                {"type":"image_url","image_url":{"url":IMAGE}},
                {"type":"image_url","image_url":{"url":"https://media.example/ref.jpg"}}
            ]
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-generation-id", "gen-image")
                .set_body_json(json!({
                    "data": [
                        {"b64_json": "eA==", "media_type": "image/webp"},
                        {"url": "https://media.example/result.png", "media_type": "image/png"}
                    ],
                    "usage": { "cost": 0.28 }
                })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let out = client(&server).generate_image(KEY, &req).await.unwrap();
    assert_eq!(out.request_id.as_deref(), Some("gen-image"));
    assert_eq!(out.billing.unwrap().cost_usd(), Some(0.28));
    assert_eq!(out.data[0].b64_json.as_deref(), Some("eA=="));
    assert_eq!(out.data[0].content_type.as_deref(), Some("image/webp"));
    assert_eq!(
        out.data[1].url.as_deref(),
        Some("https://media.example/result.png")
    );
}

#[tokio::test]
async fn video_modes_submit_distinct_inputs_and_download_from_the_job_path() {
    let server = MockServer::start().await;
    let image = json!({"type":"image_url","image_url":{"url":IMAGE}});
    for (mode, input) in [
        (GenMode::TextToVideo, json!({})),
        (
            GenMode::ImageToVideo,
            json!({"frame_images": [{
                "type": "image_url", "image_url": {"url": IMAGE}, "frame_type": "first_frame"
            }]}),
        ),
        (
            GenMode::ReferenceToVideo,
            json!({"input_references": [image]}),
        ),
    ] {
        let mut body = json!({
            "model": "alibaba/wan-3.0", "prompt": "Sunrise",
            "duration": 6, "resolution": "720p", "generate_audio": false
        });
        body.as_object_mut()
            .unwrap()
            .extend(input.as_object().unwrap().clone());
        Mock::given(method("POST"))
            .and(path("/api/v1/videos"))
            .and(header("authorization", AUTH))
            .and(body_json(body))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({
                "id": "job_clip",
                "status": "pending",
                "polling_url": "https://example.invalid/video"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let mut req = request(MediaKind::Video, mode, "alibaba/wan-3.0");
        req.params.duration_seconds = Some(6.0);
        req.params.resolution = Some("720P".into());
        req.params.audio = Some(false);
        if mode != GenMode::TextToVideo {
            req.references.push(Reference {
                data_url: IMAGE.into(),
            });
        }
        let accepted = client(&server).start_video(KEY, &req).await.unwrap();
        assert_eq!(accepted.id, "job_clip");
        assert_eq!(accepted.status, "pending");
    }

    Mock::given(method("GET"))
        .and(path("/api/v1/videos/job_clip"))
        .and(header("authorization", AUTH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "job_clip",
            "status": "completed",
            "unsigned_urls": ["https://example.invalid/video-0", "https://example.invalid/video-1"],
            "usage": { "cost": 1.2 }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/videos/job_clip/content"))
        .and(query_param("index", "1"))
        .and(header("authorization", AUTH))
        .respond_with(ResponseTemplate::new(200).set_body_raw("test MP4", "video/mp4"))
        .expect(1)
        .mount(&server)
        .await;

    let completed = client(&server).video_status(KEY, "job_clip").await.unwrap();
    assert_eq!(completed.cost_usd(), Some(1.2));
    assert_eq!(completed.output_count(), 2);
    assert_eq!(
        client(&server)
            .video_content(KEY, &completed.id, 1)
            .await
            .unwrap(),
        b"test MP4"
    );
    assert!(matches!(
        client(&server).video_status(KEY, "../key").await,
        Err(CoreError::Validation(_))
    ));
}

#[tokio::test]
async fn speech_normalizes_pcm_and_reads_actual_cost() {
    let server = MockServer::start().await;
    let mut req = request(MediaKind::Audio, GenMode::Speech, "heygen/heygen-voice-1");
    req.params.voice = Some("nadine".into());
    req.params.response_format = Some("pcm".into());
    Mock::given(method("POST"))
        .and(path("/api/v1/audio/speech"))
        .and(header("authorization", AUTH))
        .and(body_json(json!({
            "model": "heygen/heygen-voice-1", "input": "Sunrise",
            "voice": "nadine", "response_format": "pcm"
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-generation-id", "gen-speech")
                .set_body_raw([1, 2, 3, 4], "audio/pcm"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/generation"))
        .and(query_param("id", "gen-speech"))
        .and(header("authorization", AUTH))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"total_cost":0.005}})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let out = client(&server).generate_audio(KEY, &req).await.unwrap();
    assert_eq!(out.request_id.as_deref(), Some("gen-speech"));
    assert_eq!(out.content_type, "audio/wav");
    assert_eq!(&out.bytes[24..28], &44_100u32.to_le_bytes());
    assert_eq!(&out.bytes[44..], &[1, 2, 3, 4]);
    assert_eq!(out.cost_usd, Some(0.005));
}

#[tokio::test]
async fn sound_effects_use_seed_audio_prompt_mode_without_voice() {
    let server = MockServer::start().await;
    let mut req = request(
        MediaKind::Audio,
        GenMode::SoundEffects,
        "bytedance-seed/seed-audio-1-0",
    );
    req.params.voice = Some("leftover selection".into());
    Mock::given(method("POST"))
        .and(path("/api/v1/audio/speech"))
        .and(body_json(json!({
            "model":"bytedance-seed/seed-audio-1-0", "input":"Sunrise", "response_format":"mp3"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_raw("test MP3", "audio/mpeg"))
        .expect(1)
        .mount(&server)
        .await;

    let out = client(&server).generate_audio(KEY, &req).await.unwrap();
    assert_eq!(out.content_type, "audio/mpeg");
    assert_eq!(out.bytes, b"test MP3");
}

#[tokio::test]
async fn music_assembles_streamed_audio_and_keeps_usage() {
    let server = MockServer::start().await;
    let req = request(
        MediaKind::Audio,
        GenMode::Music,
        "google/lyria-3-clip-preview",
    );
    let events = concat!(
        ": keep-alive\n\n",
        "data: {\"id\":\"gen-music\",\"choices\":[{\"delta\":{\"audio\":{\"data\":\"AQIDBA==\"}}}]}\n\n",
        "data: {\"id\":\"gen-music\",\"choices\":[{\"delta\":{\"audio\":{\"data\":\"BQYHCA==\"}}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"cost\":0.04}}\n\n",
        "data: [DONE]\n\n"
    );
    Mock::given(method("POST"))
        .and(path("/api/v1/chat/completions"))
        .and(body_json(json!({
            "model": "google/lyria-3-clip-preview",
            "messages": [{"role":"user","content":"Sunrise"}],
            "modalities": ["text", "audio"], "audio": {"format": "pcm16"}, "stream": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_raw(events, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;

    let out = client(&server).generate_audio(KEY, &req).await.unwrap();
    assert_eq!(out.request_id.as_deref(), Some("gen-music"));
    assert_eq!(out.cost_usd, Some(0.04));
    assert_eq!(out.content_type, "audio/wav");
    assert_eq!(&out.bytes[22..28], &[2, 0, 0x80, 0xbb, 0, 0]);
    assert_eq!(&out.bytes[44..], &[1, 2, 3, 4, 5, 6, 7, 8]);
}
