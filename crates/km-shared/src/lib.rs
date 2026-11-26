//! Общие типы и утилиты Koteyye Media Studio.

/// Максимальное тело запроса на проводе (включая multipart или сжатый JSON).
pub const MAX_REQUEST_WIRE_BYTES: usize = 4_500_000;

mod events;
mod history;
pub mod image;
mod mcp;
mod models;
mod requests;
mod responses;
pub mod video;

pub use events::{
    AppEvent, BalanceUnavailable, BalanceView, Lang, LangPreference, ModelChoice,
    OpenRouterCredits, Provider, Settings, Theme, WorkspacePrefs,
};
pub use history::{HistoryFilter, HistoryPatch, HistoryRecord, JobStatus};
pub use mcp::{MCP_PORT, McpModelDefaults, McpStatus};
pub use models::{
    CatalogModel, CatalogParameter, CatalogPricingScope, CatalogRates, CatalogTier, MediaPrices,
    ModelType, OpenRouterModelOptions,
};
pub use requests::{GenMode, GenParams, GenerationRequest, MediaKind, Reference};
pub use responses::{
    AccountBalance, AudioOutput, Billing, Generation, GenerationFile, ImageItem, ImageResponse,
    VideoItem, VideoResponse,
};

/// Обновления доступны только в сборке с публичным ключом издателя.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    Disabled,
    Current,
    Available(String),
}
