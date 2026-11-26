//! Media Studio operations exposed through MCP.

use std::sync::Arc;

use km_shared::video::VideoLimits;
use km_shared::{
    CatalogModel, GenMode, GenParams, GenerationRequest, HistoryRecord, MediaKind, Provider,
    Reference,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tauri::Url;
use uuid::Uuid;

use crate::commands::{cmd_fetch_models_for, cmd_start_generation};
use crate::state::AppState;

const MODES: [GenMode; 8] = [
    GenMode::TextToImage,
    GenMode::ImageEdit,
    GenMode::TextToVideo,
    GenMode::ImageToVideo,
    GenMode::ReferenceToVideo,
    GenMode::Speech,
    GenMode::Music,
    GenMode::SoundEffects,
];
const DEFAULT_PAGE_SIZE: u32 = 12;
const MAX_PAGE_SIZE: u32 = 50;

pub(super) fn list_tools() -> Value {
    let kind = json!({"type":"string","enum":["image","video","audio"]});
    let provider = json!({"type":"string","enum":["ci","openrouter"]});
    let limit =
        json!({"type":"integer","minimum":1,"maximum":MAX_PAGE_SIZE,"default":DEFAULT_PAGE_SIZE});
    let offset = json!({"type":"integer","minimum":0,"maximum":u32::MAX,"default":0});
    let request = object_schema(
        json!({
            "kind": kind,
            "mode": {"type":"string","enum":MODES},
            "model": {"type":"string","description":"Omit for a mode with a fixed model in studio_status.model_defaults. In Auto, pass the exact id from list_models."},
            "prompt": {"type":"string"},
            "params": object_schema(json!({
                "size":{"type":"string"},
                "resolution":{"type":"string"},
                "aspect_ratio":{"type":"string"},
                "duration_seconds":{"type":"number","exclusiveMinimum":0,"description":"Required for Cheaper Inference video"},
                "n":{"type":"integer","minimum":1,"maximum":10},
                "seed":{"type":"integer"},
                "voice":{"type":"string"},
                "response_format":{"type":"string"},
                "speed":{"type":"number","exclusiveMinimum":0},
                "audio":{"type":"boolean"},
                "quality":{"type":"string"}
            }), &[]),
            "references": {"type":"array","items":object_schema(json!({
                "data_url":{"type":"string","description":"HTTP(S) URL or base64 image data URL. Encode local files in the ADE before sending."}
            }), &["data_url"])},
            "mask_data_url": {"type":"string","description":"Base64 PNG mask for Cheaper Inference image_edit"}
        }),
        &["kind", "mode", "prompt"],
    );
    json!({"tools":[
        tool("studio_status", "Read the active Studio provider, generation readiness, and fixed models by generation mode. Modes absent from model_defaults use Auto.", object_schema(json!({}), &[]), true, false),
        tool("list_models", "Read the active provider's media catalog with generation modes, parameters and available price information.", object_schema(json!({
            "kind":kind, "mode":{"type":"string","enum":MODES},
            "query":{"type":"string","description":"Case-insensitive model ID, name or model-provider search"},
            "limit":limit, "offset":offset
        }), &["kind"]), true, true),
        tool("start_generation", "Start a potentially paid generation using the active Studio profile. Omit the model when fixed in studio_status; choose an exact catalog model in Auto. Returns the local history ID. Reuse the same UUID and request when checking an uncertain submission.", object_schema(json!({
            "provider":provider,
            "idempotency_key":{"type":"string","format":"uuid","description":"A fresh UUID for each intended generation. Reuse it for retries of the same submission."},
            "request":request
        }), &["provider", "idempotency_key", "request"]), false, true),
        tool("get_generation", "Read status, billed USD when reported, and saved file paths for a generation by local history ID.", object_schema(json!({
            "id":{"type":"string","description":"The local id returned by start_generation or list_history"}
        }), &["id"]), true, false),
        tool("list_history", "Read recent Studio generations without source attachments. Defaults to the active provider.", object_schema(json!({
            "kind":kind, "provider":provider, "limit":limit, "offset":offset
        }), &[]), true, false),
        tool("get_studio_skill", "Read the Studio usage skill and generation workflow.", object_schema(json!({}), &[]), true, false)
    ]})
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object", "properties":properties, "required":required, "additionalProperties":false})
}

fn tool(name: &str, description: &str, schema: Value, read_only: bool, open_world: bool) -> Value {
    json!({
        "name": name, "description":description, "inputSchema":schema,
        "annotations":{
            "readOnlyHint":read_only, "destructiveHint":false,
            "idempotentHint":true, "openWorldHint":open_world
        }
    })
}

pub(super) async fn call_tool(state: &Arc<AppState>, name: &str, args: Value) -> Value {
    match run_tool(state, name, args).await {
        Ok((output, text)) => json!({
            "structuredContent":output,
            "content":[{"type":"text","text":text.unwrap_or_else(|| output.to_string())}]
        }),
        Err(message) => json!({"isError":true,"content":[{"type":"text","text":message}]}),
    }
}

type ToolResult = Result<Value, String>;

async fn run_tool(
    state: &Arc<AppState>,
    name: &str,
    args: Value,
) -> Result<(Value, Option<String>), String> {
    let result = match name {
        "studio_status" => {
            parse::<NoArguments>(args)?;
            studio_status(state).await
        }
        "list_models" => list_models(state, parse(args)?).await,
        "start_generation" => start_generation(state, parse(args)?).await,
        "get_generation" => get_generation(state, parse(args)?),
        "list_history" => list_history(state, parse(args)?),
        "get_studio_skill" => {
            parse::<NoArguments>(args)?;
            return Ok((
                json!({"uri":"koteyye://skills/media-studio/SKILL.md", "text":super::SKILL}),
                Some(super::SKILL.to_owned()),
            ));
        }
        _ => Err(format!("Unknown Studio tool: {name}")),
    };
    result.map(|output| (output, None))
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, String> {
    if !args.is_object() {
        return Err("Tool arguments must be a JSON object.".into());
    }
    serde_json::from_value(args).map_err(|e| format!("Invalid tool arguments: {e}"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArguments {}

fn active_provider(state: &AppState) -> Result<Provider, String> {
    Ok(state
        .storage
        .get_settings()
        .map_err(|e| e.to_string())?
        .provider)
}

async fn studio_status(state: &AppState) -> ToolResult {
    let settings = state.storage.get_settings().map_err(|e| e.to_string())?;
    let provider = settings.provider;
    let key_configured = state.has_key(provider).await?;
    let output_folder_configured = !settings.local_folder.is_empty();
    let model_defaults = state
        .mcp
        .model_defaults()
        .await
        .remove(&provider)
        .unwrap_or_default();
    Ok(json!({
        "provider": provider,
        "provider_name": provider.name(),
        "key_configured": key_configured,
        "output_folder_configured": output_folder_configured,
        "ready_to_generate": key_configured && output_folder_configured,
        "local_spend_usd": state.storage.total_spend_usd(provider).map_err(|e| e.to_string())?,
        "model_defaults": model_defaults,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelArguments {
    kind: MediaKind,
    mode: Option<GenMode>,
    query: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
}

fn page_size(requested: Option<u32>) -> Result<u32, String> {
    let limit = requested.unwrap_or(DEFAULT_PAGE_SIZE);
    (1..=MAX_PAGE_SIZE)
        .contains(&limit)
        .then_some(limit)
        .ok_or_else(|| format!("limit must be between 1 and {MAX_PAGE_SIZE}."))
}

fn model_info(model: &CatalogModel, provider: Provider) -> Value {
    let modes: Vec<_> = MODES
        .into_iter()
        .filter(|mode| model.supports_mode(*mode))
        .collect();
    let video_limits: Vec<_> = modes.iter().filter_map(|&mode| {
        if provider != Provider::CheaperInference || mode.kind() != MediaKind::Video {
            return None;
        }
        let limits = VideoLimits::for_model(&model.id, mode)?;
        Some(json!({"mode":mode, "duration_seconds":limits.durations, "max_outputs":limits.max_outputs,"aspect_ratios":limits.aspect_ratios}))
    }).collect();
    json!({
        "id":model.id, "name":model.name, "model_provider":model.provider_name,
        "modes":modes, "features":model.features, "aliases":model.aliases,
        "pricing":{"unit":model.media_unit,"unit_price_usd":model.media_unit_price,"variants_usd":model.media_prices,"scopes":model.pricing_scopes},
        "openrouter":model.openrouter,
        "ci_video_limits":video_limits
    })
}

async fn list_models(state: &AppState, args: ModelArguments) -> ToolResult {
    let provider = active_provider(state)?;
    let limit = page_size(args.limit)? as usize;
    let offset = args.offset.unwrap_or_default() as usize;
    if args.mode.is_some_and(|mode| mode.kind() != args.kind) {
        return Err("Generation mode does not match the requested media kind.".into());
    }
    let query = args.query.unwrap_or_default().to_lowercase();
    let models = cmd_fetch_models_for(state, provider, args.kind).await?;
    let matches: Vec<_> = models
        .iter()
        .filter(|model| {
            let has_mode = args.mode.map_or_else(
                || MODES.iter().any(|mode| model.supports_mode(*mode)),
                |mode| model.supports_mode(mode),
            );
            let matches = |value: &str| value.to_lowercase().contains(&query);
            has_mode
                && (query.is_empty()
                    || matches(&model.id)
                    || model.name.as_deref().is_some_and(matches)
                    || model.provider_name.as_deref().is_some_and(matches)
                    || model.aliases.iter().any(|alias| matches(alias)))
        })
        .collect();
    let page: Vec<_> = matches
        .iter()
        .skip(offset)
        .take(limit)
        .map(|model| model_info(model, provider))
        .collect();
    let end = offset + page.len();
    Ok(
        json!({"provider":provider,"kind":args.kind,"models":page,"total":matches.len(),
        "next_offset":(end < matches.len()).then_some(end)}),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartArguments {
    provider: Provider,
    idempotency_key: String,
    request: McpGenerationRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpGenerationRequest {
    kind: MediaKind,
    mode: GenMode,
    model: Option<String>,
    prompt: String,
    #[serde(default)]
    params: GenParams,
    #[serde(default)]
    references: Vec<Reference>,
    mask_data_url: Option<String>,
}

impl McpGenerationRequest {
    fn with_model(self, model: String) -> GenerationRequest {
        GenerationRequest {
            kind: self.kind,
            mode: self.mode,
            model,
            prompt: self.prompt,
            params: self.params,
            references: self.references,
            mask_data_url: self.mask_data_url,
        }
    }
}

async fn start_generation(state: &Arc<AppState>, args: StartArguments) -> ToolResult {
    let StartArguments {
        provider,
        idempotency_key,
        request,
    } = args;
    let idempotency_key =
        Uuid::parse_str(&idempotency_key).map_err(|_| "idempotency_key must be a UUID.")?;
    let previous = state
        .storage
        .history_record(&idempotency_key.to_string())
        .map_err(|e| e.to_string())?;
    let model_was_omitted = request.model.is_none();

    let (model, fixed) = if let Some(record) = &previous {
        (
            request
                .model
                .clone()
                .unwrap_or_else(|| record.model.clone()),
            false,
        )
    } else {
        if active_provider(state)? != provider {
            return Err(
                "The active Studio provider changed. Check studio_status and list_models.".into(),
            );
        }
        let defaults = state.mcp.model_defaults().await;
        let configured = defaults
            .get(&provider)
            .and_then(|modes| modes.get(&request.mode));
        match (configured, &request.model) {
            (Some(model), Some(explicit)) if explicit != model => {
                return Err(format!("This mode uses the fixed MCP model '{model}'. Omit request.model or use the configured model."));
            }
            (Some(model), _) => (model.clone(), true),
            (None, Some(model)) => (model.clone(), false),
            (None, None) => return Err("This mode uses Auto. Choose a model with list_models and pass its exact id in request.model.".into()),
        }
    };
    let request = request.with_model(model);
    validate_request(&request, provider)?;
    if previous.is_none() {
        let models = cmd_fetch_models_for(state, provider, request.kind).await?;
        if !models
            .iter()
            .any(|model| model.id == request.model && model.supports_mode(request.mode))
        {
            if fixed {
                return Err("The fixed MCP model is unavailable for this mode. Choose a model or Auto in Settings → MCP.".into());
            }
            return Err("The selected model does not support this mode in the active catalog. Use list_models to choose a compatible model.".into());
        }
    }
    let record =
        cmd_start_generation(state, provider, request, idempotency_key, model_was_omitted).await?;
    Ok(record_info(&record))
}

fn validate_request(request: &GenerationRequest, provider: Provider) -> Result<(), String> {
    if request.mode.kind() != request.kind {
        return Err("Generation mode does not match the requested media kind.".into());
    }
    if request.model.trim().is_empty() || request.prompt.trim().is_empty() {
        return Err("model and prompt are required.".into());
    }
    let count = request.references.len();
    match request.mode {
        GenMode::ImageEdit | GenMode::ReferenceToVideo if count == 0 => {
            return Err("This mode requires at least one reference image.".into());
        }
        GenMode::ImageToVideo if count != 1 => {
            return Err("image_to_video requires exactly one first-frame image.".into());
        }
        GenMode::ImageEdit | GenMode::ImageToVideo | GenMode::ReferenceToVideo => {}
        _ if count != 0 => return Err("This mode does not accept reference images.".into()),
        _ => {}
    }
    if request
        .references
        .iter()
        .any(|reference| !valid_reference(&reference.data_url))
    {
        return Err("References must be HTTP(S) URLs or base64 image data URLs.".into());
    }
    if let Some(mask) = &request.mask_data_url
        && (provider != Provider::CheaperInference
            || request.mode != GenMode::ImageEdit
            || !mask
                .strip_prefix("data:image/png;base64,")
                .is_some_and(|encoded| !encoded.is_empty()))
    {
        return Err("A base64 PNG mask is accepted only for Cheaper Inference image_edit.".into());
    }
    if request.params.n.is_some_and(|n| !(1..=10).contains(&n)) {
        return Err("n must be between 1 and 10.".into());
    }
    if request.kind == MediaKind::Video {
        validate_video(request, provider)?;
    }
    Ok(())
}

fn valid_reference(value: &str) -> bool {
    value
        .strip_prefix("data:")
        .and_then(|data| data.split_once(";base64,"))
        .is_some_and(|(mime, data)| {
            matches!(
                mime,
                "image/png" | "image/jpeg" | "image/webp" | "image/gif"
            ) && !data.is_empty()
        })
        || Url::parse(value)
            .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
}

fn validate_video(request: &GenerationRequest, provider: Provider) -> Result<(), String> {
    if provider == Provider::CheaperInference {
        if !request
            .params
            .duration_seconds
            .is_some_and(|seconds| seconds.is_finite() && seconds > 0.0)
        {
            return Err("params.duration_seconds is required for Cheaper Inference video.".into());
        }
        if let Some(limits) = VideoLimits::for_model(&request.model, request.mode)
            && (!limits.supports(&request.params)
                || !limits.supports_references(&request.references))
        {
            return Err(
                "Video parameters or references do not match the model limits. Check list_models."
                    .into(),
            );
        }
    } else if request.params.n.is_some_and(|n| n != 1) {
        return Err("OpenRouter supports one video per request.".into());
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationArguments {
    id: String,
}

fn get_generation(state: &AppState, args: GenerationArguments) -> ToolResult {
    let record = state
        .storage
        .history_record(&args.id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "Generation not found. Use the local ID from start_generation or list_history."
                .to_owned()
        })?;
    Ok(record_info(&record))
}

fn record_info(record: &HistoryRecord) -> Value {
    json!({
        "id":record.id,"provider":record.provider,"kind":record.kind,"mode":record.mode,
        "model":record.model,"status":record.status,"provider_request_id":record.request_id,
        "local_paths":record.local_paths,"cost_usd":record.cost_usd,"error":record.error,
        "created_at":record.created_at,"updated_at":record.updated_at,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryArguments {
    kind: Option<MediaKind>,
    provider: Option<Provider>,
    limit: Option<u32>,
    offset: Option<u32>,
}

fn list_history(state: &AppState, args: HistoryArguments) -> ToolResult {
    let provider = match args.provider {
        Some(provider) => provider,
        None => active_provider(state)?,
    };
    let limit = page_size(args.limit)?;
    let offset = args.offset.unwrap_or_default();
    let rows = state
        .storage
        .recent_history(args.kind, Some(provider), limit + 1, offset)
        .map_err(|e| e.to_string())?;
    let records: Vec<_> = rows
        .iter()
        .take(limit as usize)
        .map(|record| {
            let mut info = record_info(record);
            info["prompt_preview"] = json!(record.prompt);
            info
        })
        .collect();
    Ok(json!({"provider":provider,"generations":records,
        "next_offset":(rows.len() > limit as usize).then(|| offset.checked_add(limit)).flatten()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use km_shared::{HistoryRecord, JobStatus};
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::keychain::InMemoryKeyStore;

    fn state(directory: &tempfile::TempDir, base_url: &str) -> Arc<AppState> {
        Arc::new(
            AppState::new(
                &directory.path().join("db.sqlite"),
                Box::new(InMemoryKeyStore::default()),
                base_url,
                Arc::new(|_| {}),
            )
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn read_only_tools_report_capabilities_and_allow_retrieving_the_skill() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/public/models")).respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":[
                {"id":"image-standard", "model_type":"image", "supported_endpoints":["/v1/images/generations"]},
                {"id":"image-editor", "model_type":"image", "supported_endpoints":["/v1/images/edits"]}
            ]})),
        ).expect(1).mount(&server).await;
        let directory = tempfile::tempdir().unwrap();
        let state = state(&directory, &server.uri());
        assert_eq!(
            call_tool(&state, "studio_status", json!({})).await["structuredContent"]["ready_to_generate"],
            false
        );
        let result = call_tool(
            &state,
            "list_models",
            json!({"kind":"image","mode":"image_edit"}),
        )
        .await;
        let payload = &result["structuredContent"];
        assert_eq!(payload["provider"], "ci");
        assert_eq!(payload["total"], 1);
        assert_eq!(payload["models"][0]["id"], "image-editor");
        assert_eq!(payload["models"][0]["modes"], json!(["image_edit"]));
        let skill = call_tool(&state, "get_studio_skill", json!({})).await;
        assert_eq!(skill["content"][0]["text"], super::super::SKILL);
    }

    #[tokio::test]
    async fn history_is_paged_without_exposing_input_attachments() {
        let directory = tempfile::tempdir().unwrap();
        let state = state(&directory, "http://127.0.0.1:1");
        let mut record = HistoryRecord {
            id: "generation-one".into(),
            provider: Provider::CheaperInference,
            created_at: 1,
            updated_at: 2,
            kind: MediaKind::Image,
            mode: GenMode::ImageEdit,
            model: "image-standard".into(),
            prompt: "猫".repeat(600),
            status: JobStatus::Completed,
            request_id: Some("trace-1".into()),
            local_paths: vec!["/media/result.png".into()],
            cost_usd: Some(0.5),
            params_json: r#"{"references":[{"data_url":"https://example.test/secret-upload"}]}"#
                .into(),
            error: None,
        };
        state.storage.insert_history(&record).unwrap();
        record.id = "generation-two".into();
        state.storage.insert_history(&record).unwrap();
        record.id = "router-generation".into();
        record.provider = Provider::OpenRouter;
        state.storage.insert_history(&record).unwrap();
        let page = call_tool(&state, "list_history", json!({"limit":1})).await;
        let payload = &page["structuredContent"];
        assert_eq!(payload["provider"], "ci");
        assert_eq!(payload["generations"][0]["id"], "generation-two");
        assert_eq!(
            payload["generations"][0]["prompt_preview"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            512
        );
        assert_eq!(payload["next_offset"], 1);
        let generation =
            call_tool(&state, "get_generation", json!({"id":"router-generation"})).await;
        assert_eq!(
            generation["structuredContent"]["local_paths"],
            json!(["/media/result.png"])
        );
        assert_eq!(generation["structuredContent"]["cost_usd"], 0.5);
        assert!(!generation.to_string().contains("secret-upload"));
    }

    async fn mock_image_generation(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(query_param("type", "image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[
                {"id":"test-image", "type":"image", "supported_endpoints":["/v1/images/generations"]},
                {"id":"other-image", "type":"image", "supported_endpoints":["/v1/images/generations"]}
            ]})))
            .mount(server).await;
        Mock::given(method("POST"))
            .and(path("/v1/images/generations"))
            .and(body_partial_json(json!({"model":"test-image"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "request_id": "mcp-provider-receipt",
                "billing": {"billed_cost_usd":"0.25"},
                "data": [{
                    "b64_json": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII="
                }]
            })))
            .expect(1)
            .mount(server).await;
    }

    fn ready_state(directory: &tempfile::TempDir, server: &MockServer) -> Arc<AppState> {
        let state = state(directory, &server.uri());
        let settings = km_shared::Settings {
            local_folder: directory.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        state.storage.set_settings(&settings).unwrap();
        state
            .key_store
            .set(Provider::CheaperInference, "mock-media-key")
            .unwrap();
        state
    }

    async fn wait_for_generation(state: &Arc<AppState>, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let result = call_tool(state, "get_generation", json!({"id":id})).await;
                let generation = &result["structuredContent"];
                if generation["status"] == "completed" {
                    break generation.clone();
                }
                assert_eq!(generation["status"], "pending", "{result}");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("mock generation completes")
    }

    #[tokio::test]
    async fn concurrent_retries_start_one_generation_and_keep_its_receipt() {
        let server = MockServer::start().await;
        mock_image_generation(&server).await;
        let directory = tempfile::tempdir().unwrap();
        let state = ready_state(&directory, &server);
        let id = Uuid::new_v4().to_string();
        let args = json!({"provider":"ci", "idempotency_key":id, "request":{
            "kind":"image", "mode":"text_to_image", "model":"test-image", "prompt":"Test scene"
        }});
        let (first, retry) = tokio::join!(
            call_tool(&state, "start_generation", args.clone()),
            call_tool(&state, "start_generation", args.clone()),
        );
        for response in [first, retry] {
            assert_eq!(response["structuredContent"]["id"], id, "{response}");
        }
        let mut changed = args.clone();
        changed["request"]["prompt"] = json!("Different scene");
        let conflicting = call_tool(&state, "start_generation", changed).await;
        assert_eq!(conflicting["isError"], true);
        let completed = wait_for_generation(&state, &id).await;
        assert_eq!(completed["provider_request_id"], "mcp-provider-receipt");
        assert_eq!(completed["cost_usd"], 0.25);
        assert!(std::path::Path::new(completed["local_paths"][0].as_str().unwrap()).is_file());
        let retry = call_tool(&state, "start_generation", args).await;
        assert_eq!(retry["structuredContent"], completed);
    }

    #[tokio::test]
    async fn fixed_model_fills_omissions_and_retries_keep_the_original_model() {
        let server = MockServer::start().await;
        mock_image_generation(&server).await;
        let directory = tempfile::tempdir().unwrap();
        let state = ready_state(&directory, &server);
        let provider = Provider::CheaperInference;
        let mode = GenMode::TextToImage;
        let id = Uuid::new_v4().to_string();
        let args = json!({"provider":"ci", "idempotency_key":id, "request":{
            "kind":"image", "mode":"text_to_image", "prompt":"Test scene"
        }});

        state
            .mcp
            .set_model_default(Provider::OpenRouter, mode, Some("router-image".into()))
            .await
            .unwrap();
        let status = call_tool(&state, "studio_status", json!({})).await;
        assert_eq!(status["structuredContent"]["model_defaults"], json!({}));
        let auto = call_tool(&state, "start_generation", args.clone()).await;
        assert_eq!(auto["isError"], true);
        assert!(
            auto["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Auto")
        );

        state
            .mcp
            .set_model_default(provider, mode, Some("test-image".into()))
            .await
            .unwrap();
        let status = call_tool(&state, "studio_status", json!({})).await;
        assert_eq!(
            status["structuredContent"]["model_defaults"],
            json!({"text_to_image":"test-image"})
        );
        let mut explicit = args.clone();
        explicit["request"]["model"] = json!("other-image");
        let conflict = call_tool(&state, "start_generation", explicit.clone()).await;
        assert_eq!(conflict["isError"], true);
        assert!(
            conflict["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("fixed MCP model 'test-image'")
        );

        let started = call_tool(&state, "start_generation", args.clone()).await;
        assert_eq!(
            started["structuredContent"]["model"], "test-image",
            "{started}"
        );
        let completed = wait_for_generation(&state, &id).await;
        state
            .mcp
            .set_model_default(provider, mode, Some("other-image".into()))
            .await
            .unwrap();
        let retry = call_tool(&state, "start_generation", args).await;
        assert_eq!(retry["structuredContent"], completed);
        let conflict = call_tool(&state, "start_generation", explicit).await;
        assert_eq!(conflict["isError"], true);
        assert!(
            conflict["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("different generation")
        );
    }

    #[test]
    fn start_validation_rejects_local_paths_and_incompatible_modes() {
        let mut request: GenerationRequest = serde_json::from_value(json!({
            "kind":"image","mode":"image_edit","model":"test","prompt":"Edit this image",
            "references":[{"data_url":"file:///private/example.png"}]
        }))
        .unwrap();
        assert!(validate_request(&request, Provider::CheaperInference).is_err());
        request.references[0].data_url = "https://example.test/image.png".into();
        assert!(validate_request(&request, Provider::CheaperInference).is_ok());
        request.mode = GenMode::TextToImage;
        assert!(validate_request(&request, Provider::CheaperInference).is_err());
    }
}
