//! OpenRouter Images API: text-to-image and reference-based editing.

use km_shared::{Billing, GenMode, GenerationRequest, ImageItem, ImageResponse, MediaKind};
use serde::{Deserialize, Serialize};

use crate::client::OpenRouterKeyClient;
use crate::error::CoreError;

#[derive(Serialize)]
struct ImageBody<'a> {
    model: &'a str,
    prompt: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolution: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    aspect_ratio: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quality: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    input_references: Vec<ImageReference<'a>>,
}

#[derive(Serialize)]
pub(super) struct ImageReference<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    image_url: ImageUrl<'a>,
}

#[derive(Serialize)]
struct ImageUrl<'a> {
    url: &'a str,
}

impl<'a> ImageReference<'a> {
    pub(super) fn new(url: &'a str) -> Result<Self, CoreError> {
        let data_url = url
            .strip_prefix("data:")
            .and_then(|data| data.split_once(";base64,"))
            .is_some_and(|(mime, encoded)| {
                matches!(
                    mime,
                    "image/png" | "image/jpeg" | "image/webp" | "image/gif"
                ) && !encoded.is_empty()
            });
        if !data_url && !valid_http_url(url) {
            return Err(CoreError::Validation(
                "укажите HTTP(S)-ссылку или изображение в формате base64 data URL".into(),
            ));
        }
        Ok(Self {
            kind: "image_url",
            image_url: ImageUrl { url },
        })
    }
}

fn valid_http_url(url: &str) -> bool {
    reqwest::Url::parse(url)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
}

#[derive(Deserialize)]
struct ImageResult {
    created: Option<i64>,
    id: Option<String>,
    data: Vec<ImageResultItem>,
    usage: Option<super::Usage>,
}

#[derive(Deserialize)]
struct ImageResultItem {
    b64_json: Option<String>,
    url: Option<String>,
    media_type: Option<String>,
}

impl OpenRouterKeyClient {
    pub async fn generate_image(
        &self,
        key: &str,
        req: &GenerationRequest,
    ) -> Result<ImageResponse, CoreError> {
        if req.kind != MediaKind::Image
            || !matches!(req.mode, GenMode::TextToImage | GenMode::ImageEdit)
            || req.model.trim().is_empty()
            || req.prompt.trim().is_empty()
        {
            return Err(CoreError::Validation(
                "для генерации изображения нужны модель, промпт и режим изображения".into(),
            ));
        }
        if req.mask_data_url.is_some() {
            return Err(CoreError::Validation(
                "OpenRouter не поддерживает редактирование с PNG-маской".into(),
            ));
        }
        if req.params.n.is_some_and(|n| !(1..=10).contains(&n)) {
            return Err(CoreError::Validation(
                "количество изображений должно быть от 1 до 10".into(),
            ));
        }
        let input_references = match req.mode {
            GenMode::ImageEdit if (1..=16).contains(&req.references.len()) => req
                .references
                .iter()
                .map(|reference| ImageReference::new(&reference.data_url))
                .collect::<Result<_, _>>()?,
            GenMode::ImageEdit => {
                return Err(CoreError::Validation(
                    "OpenRouter принимает от 1 до 16 исходных изображений".into(),
                ));
            }
            _ => Vec::new(),
        };

        let body = ImageBody {
            model: &req.model,
            prompt: &req.prompt,
            n: req.params.n,
            size: req.params.size.as_deref(),
            resolution: req.params.resolution.as_deref(),
            aspect_ratio: req.params.aspect_ratio.as_deref(),
            quality: req.params.quality.as_deref(),
            response_format: req.params.response_format.as_deref(),
            seed: req.params.seed,
            input_references,
        };

        let response = self.post("/images", key, &body).await?;
        let request_id = super::generation_id(&response);
        let output: ImageResult = self.json(response, key).await?;
        let request_id = output.id.or(request_id);
        let billing = output
            .usage
            .and_then(|usage| usage.cost_usd())
            .map(|cost| Billing {
                billed_cost_usd: Some(cost.to_string()),
            });
        let data = output
            .data
            .into_iter()
            .map(|item| ImageItem {
                b64_json: item.b64_json,
                url: item.url.filter(|url| valid_http_url(url)),
                content_type: item.media_type,
            })
            .collect();
        Ok(ImageResponse {
            created: output.created,
            data,
            request_id,
            billing,
        })
    }
}
