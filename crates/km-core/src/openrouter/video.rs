//! OpenRouter video job submission, status and authenticated output download.

use std::time::Duration;

use km_shared::{GenMode, GenerationRequest, MediaKind};
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};

use crate::client::{OpenRouterKeyClient, map_transport_error};
use crate::error::CoreError;
use crate::video::safe_request_segment;

use super::image::ImageReference;

#[derive(Serialize)]
struct VideoBody<'a> {
    model: &'a str,
    prompt: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolution: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    aspect_ratio: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generate_audio: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    frame_images: Vec<FrameImage<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    input_references: Vec<ImageReference<'a>>,
}

#[derive(Serialize)]
struct FrameImage<'a> {
    #[serde(flatten)]
    reference: ImageReference<'a>,
    frame_type: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct OpenRouterVideoJob {
    pub id: String,
    pub status: String,
    pub error: Option<String>,
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub unsigned_urls: Vec<String>,
    usage: Option<super::Usage>,
}

impl OpenRouterVideoJob {
    pub fn cost_usd(&self) -> Option<f64> {
        self.usage.as_ref()?.cost_usd()
    }

    pub fn output_count(&self) -> usize {
        self.unsigned_urls.len().max(self.urls.len()).max(1)
    }
}

impl OpenRouterKeyClient {
    pub async fn start_video(
        &self,
        key: &str,
        req: &GenerationRequest,
    ) -> Result<OpenRouterVideoJob, CoreError> {
        if req.kind != MediaKind::Video
            || req.model.trim().is_empty()
            || req.prompt.trim().is_empty()
        {
            return Err(CoreError::Validation(
                "для генерации видео нужны модель и промпт".into(),
            ));
        }
        if req.params.n.is_some_and(|n| n != 1) {
            return Err(CoreError::Validation(
                "OpenRouter принимает одно видео на запрос".into(),
            ));
        }
        let duration = req
            .params
            .duration_seconds
            .map(|seconds| {
                let max = f64::from(u32::MAX);
                if seconds.is_finite() && (1.0..=max).contains(&seconds) && seconds.fract() == 0.0 {
                    Ok(seconds as u32)
                } else {
                    Err(CoreError::Validation(
                        "длительность видео должна быть целым числом секунд".into(),
                    ))
                }
            })
            .transpose()?;

        let mut frame_images = Vec::new();
        let mut input_references = Vec::new();
        match req.mode {
            GenMode::TextToVideo => {}
            GenMode::ImageToVideo if req.references.len() == 1 => {
                frame_images.push(FrameImage {
                    reference: ImageReference::new(&req.references[0].data_url)?,
                    frame_type: "first_frame",
                });
            }
            GenMode::ReferenceToVideo if !req.references.is_empty() => {
                input_references = req
                    .references
                    .iter()
                    .map(|reference| ImageReference::new(&reference.data_url))
                    .collect::<Result<_, _>>()?;
            }
            _ => {
                return Err(CoreError::Validation(
                    "для видео из изображения нужен один первый кадр, для видео по референсам нужен хотя бы один референс".into(),
                ));
            }
        }
        let resolution = req.params.resolution.as_deref().map(|value| {
            if let Some(pixels) = value.strip_suffix('P') {
                format!("{pixels}p")
            } else {
                value.to_owned()
            }
        });
        let body = VideoBody {
            model: &req.model,
            prompt: &req.prompt,
            duration,
            resolution: resolution.as_deref(),
            aspect_ratio: req.params.aspect_ratio.as_deref(),
            generate_audio: req.params.audio,
            seed: req.params.seed,
            frame_images,
            input_references,
        };
        let response = self.post("/videos", key, &body).await?;
        let job: OpenRouterVideoJob = self.json(response, key).await?;
        video_path(&job.id)?;
        Ok(job)
    }

    pub async fn video_status(
        &self,
        key: &str,
        job_id: &str,
    ) -> Result<OpenRouterVideoJob, CoreError> {
        let response = self.get(&video_path(job_id)?, key).await?;
        let mut job: OpenRouterVideoJob = self.json(response, key).await?;
        if job.id != job_id {
            return Err(CoreError::Serde(
                "OpenRouter вернул статус другого задания".into(),
            ));
        }
        if let Some(error) = job.error.as_mut()
            && !key.is_empty()
        {
            *error = error.replace(key, "[API key]");
        }
        Ok(job)
    }

    pub async fn video_content(
        &self,
        key: &str,
        job_id: &str,
        index: usize,
    ) -> Result<Vec<u8>, CoreError> {
        // Construct the documented API path locally so server-provided URLs cannot redirect the key.
        let path = format!("{}/content?index={index}", video_path(job_id)?);
        let response = self
            .get_with_timeout(&path, key, Duration::from_secs(600))
            .await?;
        let response = self.check_status(response, key).await?;
        if response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|mime| !matches!(mime, "video/mp4" | "application/octet-stream"))
        {
            return Err(CoreError::Serde("OpenRouter вернул ответ без MP4".into()));
        }
        let bytes = response.bytes().await.map_err(map_transport_error)?;
        if bytes.is_empty() {
            return Err(CoreError::Serde("OpenRouter вернул пустое видео".into()));
        }
        Ok(bytes.to_vec())
    }
}

fn video_path(job_id: &str) -> Result<String, CoreError> {
    if !safe_request_segment(job_id) {
        return Err(CoreError::Validation(
            "некорректный идентификатор видео OpenRouter".into(),
        ));
    }
    Ok(format!("/videos/{job_id}"))
}
