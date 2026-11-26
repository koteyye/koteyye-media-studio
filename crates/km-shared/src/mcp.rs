//! Состояние локального MCP, доступное из настроек приложения.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{GenMode, Provider};

pub const MCP_PORT: u16 = 29731;

/// Пустая запись для режима означает, что модель выбирает агент.
pub type McpModelDefaults = HashMap<Provider, HashMap<GenMode, String>>;

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct McpStatus {
    pub enabled: bool,
    pub running: bool,
    pub endpoint: String,
    pub command: Option<String>,
    pub token: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub model_defaults: McpModelDefaults,
}

impl std::fmt::Debug for McpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpStatus")
            .field("enabled", &self.enabled)
            .field("running", &self.running)
            .field("endpoint", &self.endpoint)
            .field("error", &self.error)
            .field("model_defaults", &self.model_defaults)
            .finish_non_exhaustive()
    }
}
