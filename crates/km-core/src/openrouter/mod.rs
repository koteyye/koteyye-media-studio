//! OpenRouter media API adapters.

mod audio;
mod catalog;
mod image;
pub(crate) mod video;

use reqwest::Response;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Usage {
    cost: Option<f64>,
}

impl Usage {
    fn cost_usd(&self) -> Option<f64> {
        self.cost.filter(|cost| cost.is_finite() && *cost >= 0.0)
    }
}

fn generation_id(response: &Response) -> Option<String> {
    response
        .headers()
        .get("x-generation-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| crate::video::safe_request_segment(value))
        .map(str::to_owned)
}
