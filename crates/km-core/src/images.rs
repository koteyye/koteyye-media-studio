//! Генерация и редактирование изображений Cheaper Inference.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use km_shared::MAX_REQUEST_WIRE_BYTES;
use km_shared::image::{MAX_EDIT_DECODED_BYTES, MAX_EDIT_SOURCES, inline_bytes, raster_mime};
use km_shared::{Billing, GenMode, GenerationRequest, ImageItem, ImageResponse, MediaKind};
use reqwest::{
    Url,
    header::HeaderMap,
    multipart::{Form, Part},
};
use serde::Serialize;

use crate::client::{CiClient, billed_cost_from_headers, response_request_id};
use crate::error::CoreError;

/// Общие поля двух JSON-эндпоинтов. Только edits принимает референсы и маску.
#[derive(Serialize)]
struct ImageBody<'a> {
    model: &'a str,
    prompt: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quality: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_references: Option<Vec<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mask_reference: Option<&'a str>,
}

impl<'a> ImageBody<'a> {
    fn json(&self) -> Result<Vec<u8>, CoreError> {
        serde_json::to_vec(self).map_err(|e| CoreError::Serde(e.to_string()))
    }

    fn edit_form(&self) -> Form {
        let mut form = Form::new()
            .text("model", self.model.to_owned())
            .text("prompt", self.prompt.to_owned());
        if let Some(n) = self.n {
            form = form.text("n", n.to_string());
        }
        for (name, value) in [
            ("size", self.size),
            ("quality", self.quality),
            ("response_format", self.response_format),
        ] {
            if let Some(value) = value {
                form = form.text(name, value.to_owned());
            }
        }
        form
    }

    fn new(req: &'a GenerationRequest, mode: GenMode) -> Result<Self, CoreError> {
        if req.kind != MediaKind::Image || req.mode != mode {
            return Err(CoreError::Validation(
                "неверный режим генерации изображения".into(),
            ));
        }
        if req.model.trim().is_empty() || req.prompt.trim().is_empty() {
            return Err(CoreError::Validation(
                "модель и промпт должны быть непустыми".into(),
            ));
        }
        if req.params.n.is_some_and(|n| !(1..=10).contains(&n)) {
            return Err(CoreError::Validation(
                "n должно быть в диапазоне 1–10".into(),
            ));
        }
        Ok(Self {
            model: &req.model,
            prompt: &req.prompt,
            n: req.params.n,
            size: req.params.size.as_deref(),
            quality: req.params.quality.as_deref(),
            response_format: req.params.response_format.as_deref(),
            input_references: None,
            mask_reference: None,
        })
    }
}

impl CiClient {
    /// POST /v1/images/generations. Опциональные параметры остаются выбором модели.
    pub async fn generate_image(
        &self,
        req: &GenerationRequest,
    ) -> Result<ImageResponse, CoreError> {
        let body = ImageBody::new(req, GenMode::TextToImage)?;
        let json = body.json()?;
        let (response, headers) = self
            .post_serialized_image("/v1/images/generations", json)
            .await?;
        Ok(image_response(response, &headers))
    }

    /// POST /v1/images/edits. Локальные файлы отправляем стандартными полями
    /// multipart. Наличие внешней ссылки выбирает JSON-расширение шлюза.
    pub async fn edit_image(&self, req: &GenerationRequest) -> Result<ImageResponse, CoreError> {
        const PATH: &str = "/v1/images/edits";
        let mut body = ImageBody::new(req, GenMode::ImageEdit)?;
        if req.prompt.chars().count() > 32_000 {
            return Err(CoreError::Validation(
                "промпт редактирования длиннее 32000 символов".into(),
            ));
        }
        if !(1..=MAX_EDIT_SOURCES).contains(&req.references.len()) {
            return Err(CoreError::Validation(
                "нужно от 1 до 10 исходных изображений".into(),
            ));
        }
        let refs: Vec<_> = req.references.iter().map(|r| r.data_url.as_str()).collect();
        let mask = req.mask_data_url.as_deref();
        let total = refs
            .iter()
            .copied()
            .chain(mask)
            .map(inline_bytes)
            .sum::<usize>();
        if total > MAX_EDIT_DECODED_BYTES {
            return Err(CoreError::Validation(
                "исходники и маска в сумме превышают 20 МиБ".into(),
            ));
        }
        if total > MAX_REQUEST_WIRE_BYTES {
            return Err(wire_limit());
        }

        let all_local = refs.iter().all(|reference| reference.starts_with("data:"));
        let mut form = all_local.then(|| body.edit_form());
        for (index, reference) in refs.iter().enumerate() {
            if reference.starts_with("data:") {
                let part = image_part(reference, false, &format!("source-{}", index + 1))?;
                form = form.map(|f| f.part("image", part));
            } else if !Url::parse(reference)
                .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.has_host())
            {
                return Err(CoreError::Validation(
                    "требуется HTTP(S)-ссылка или встроенный PNG, JPEG или WebP".into(),
                ));
            }
        }
        if let Some(mask) = mask {
            let part = image_part(mask, true, "mask")?;
            form = form.map(|f| f.part("mask", part));
        }
        let (response, headers) = if let Some(form) = form {
            self.post_multipart_json_with_headers(PATH, form).await?
        } else {
            body.input_references = Some(refs);
            body.mask_reference = mask;
            let json = body.json()?;
            if json.len() > MAX_REQUEST_WIRE_BYTES {
                return Err(wire_limit());
            }
            self.post_serialized_image(PATH, json).await?
        };
        Ok(image_response(response, &headers))
    }
}

fn wire_limit() -> CoreError {
    CoreError::Validation("Тело запроса превышает 4,5 МБ. Уменьшите вложения или передайте исходники HTTP(S)-ссылками.".into())
}

fn image_part(data_url: &str, mask: bool, name: &str) -> Result<Part, CoreError> {
    let invalid = || {
        CoreError::Validation(if mask {
            "маска должна быть встроенным PNG".into()
        } else {
            "исходник должен быть PNG, JPEG или WebP в формате base64 data URL".into()
        })
    };
    let (mime, b64) = data_url
        .strip_prefix("data:")
        .and_then(|s| s.split_once(";base64,"))
        .ok_or_else(invalid)?;
    if !matches!(mime, "image/png" | "image/jpeg" | "image/webp") || (mask && mime != "image/png") {
        return Err(invalid());
    }
    let bytes = STANDARD.decode(b64).map_err(|_| invalid())?;
    if raster_mime(&bytes) != Some(mime) {
        return Err(invalid());
    }
    Part::bytes(bytes)
        .file_name(format!("{name}.{}", crate::files::ext_for(mime)))
        .mime_str(mime)
        .map_err(|_| invalid())
}

fn image_response(mut response: ImageResponse, headers: &HeaderMap) -> ImageResponse {
    response.request_id = response_request_id(response.request_id.as_deref(), headers);
    if response.billing.is_none()
        && let Some(cost) = billed_cost_from_headers(headers)
    {
        response.billing = Some(Billing {
            billed_cost_usd: Some(format!("{cost:.6}")),
        });
    }
    response
}

/// Встроенные байты не требуют сети. Если провайдер прислал оба варианта,
/// работоспособная ссылка также позволяет восстановиться при пустом или битом base64.
pub async fn image_bytes(item: &ImageItem, client: &CiClient) -> Result<Vec<u8>, CoreError> {
    if let Some(b64) = item.b64_json.as_ref().filter(|s| !s.is_empty()) {
        match STANDARD.decode(b64) {
            Ok(bytes) if !bytes.is_empty() => return Ok(bytes),
            Err(e) if item.url.as_deref().is_none_or(str::is_empty) => {
                return Err(CoreError::Serde(format!(
                    "невалидный base64 изображения: {e}"
                )));
            }
            _ => {}
        }
    }
    if let Some(url) = item.url.as_ref().filter(|s| !s.is_empty()) {
        let bytes = client.download(url).await?;
        if !bytes.is_empty() {
            return Ok(bytes);
        }
    }
    Err(CoreError::Serde("CI вернул пустое изображение".into()))
}
