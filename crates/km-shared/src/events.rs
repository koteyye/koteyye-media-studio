//! События ядра для UI, представление баланса и настройки приложения.
//!
//! [`AppEvent`] — payload Tauri-событий `generation:started/done/failed`
//! и `balance:updated`; JSON externally tagged, имена вариантов в snake_case
//! (например `{"generation_started":{"history_id":"..."}}`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::requests::{GenMode, MediaKind};

use crate::responses::AccountBalance;

/// Событие ядра для UI.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppEvent {
    /// Генерация запущена, запись истории создана.
    GenerationStarted {
        /// Идентификатор записи истории.
        history_id: String,
    },
    /// Генерация успешно завершена.
    GenerationDone {
        /// Идентификатор записи истории.
        history_id: String,
    },
    /// Генерация завершилась ошибкой.
    GenerationFailed {
        /// Идентификатор записи истории.
        history_id: String,
        /// Текст ошибки.
        error: String,
    },
    /// Баланс обновился — UI должен перечитать его.
    BalanceUpdated,
}

/// Почему баланс аккаунта недоступен. Интерфейс выбирает перевод с учётом провайдера.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BalanceUnavailable {
    MissingKey,
    PermissionDenied,
    RejectedKey,
    AccessRestricted,
    KeyStore,
    Network,
    Other,
}

/// Итоги аккаунта из OpenRouter `GET /api/v1/credits`, в USD.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct OpenRouterCredits {
    pub total_credits: f64,
    pub total_usage: f64,
}

impl OpenRouterCredits {
    pub fn remaining_usd(&self) -> f64 {
        self.total_credits - self.total_usage
    }
}

/// Локальный расход и удалённый кошелёк относятся к одному профилю.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct BalanceView {
    /// Старые версии отправляли отчёт исключительно по CI.
    #[serde(default)]
    pub provider: Provider,
    /// Кошелёк CI, включая доступный и зарезервированный остаток.
    #[serde(default)]
    pub real: Option<AccountBalance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openrouter_credits: Option<OpenRouterCredits>,
    /// Сумма известных списаний в локальной истории этого провайдера, USD.
    pub local_spend_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<BalanceUnavailable>,
    /// Причина недоступности кошелька. Локальный расход остаётся доступен.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub real_error: Option<String>,
}

impl BalanceView {
    pub fn local(provider: Provider, local_spend_usd: f64) -> Self {
        Self {
            provider,
            local_spend_usd,
            ..Self::default()
        }
    }
}

/// Язык интерфейса.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    /// Русский.
    Ru,
    /// Английский.
    En,
}

impl Lang {
    /// Студия поддерживает ru и en. Другие локали используют английский.
    pub fn from_locale(locale: &str) -> Self {
        let primary = locale.trim().split(['-', '_', '.', '@']).next();
        if primary.is_some_and(|language| language.eq_ignore_ascii_case("ru")) {
            Self::Ru
        } else {
            Self::En
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Self::Ru => "ru",
            Self::En => "en",
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn detect_system() -> Option<Self> {
        sys_locale::get_locale().as_deref().map(Self::from_locale)
    }
}

/// Сохранённый выбор языка. Системный язык вычисляется при запуске.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LangPreference {
    #[default]
    System,
    Ru,
    En,
}

impl LangPreference {
    pub const fn code(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Ru => Lang::Ru.code(),
            Self::En => Lang::En.code(),
        }
    }

    pub const fn resolve(self, system: Lang) -> Lang {
        match self {
            Self::System => system,
            Self::Ru => Lang::Ru,
            Self::En => Lang::En,
        }
    }
}

/// Тема оформления.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Тёмная.
    Dark,
    /// Светлая.
    Light,
    /// Как в системе.
    System,
}

/// Профиль подключения. Старые настройки и записи студии принадлежат CI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum Provider {
    #[default]
    #[serde(rename = "ci")]
    CheaperInference,
    #[serde(rename = "openrouter")]
    OpenRouter,
}

impl Provider {
    pub const fn name(self) -> &'static str {
        match self {
            Self::CheaperInference => "Cheaper Inference",
            Self::OpenRouter => "OpenRouter",
        }
    }

    pub const fn api_keys_url(self) -> &'static str {
        match self {
            Self::CheaperInference => "https://www.cheaperinference.com/dashboard/keys",
            Self::OpenRouter => "https://openrouter.ai/settings/keys",
        }
    }
}

/// Небольшие параметры, которые переносятся между запусками для одного режима.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ModelChoice {
    pub model: String,
    pub size: Option<String>,
    pub aspect_ratio: Option<String>,
    pub resolution: Option<String>,
    pub response_format: Option<String>,
    pub n: Option<u32>,
}

/// Представление студии и предпочтения её владельца.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct WorkspacePrefs {
    pub compact: bool,
    pub single_image: bool,
    pub reduce_motion: bool,
    pub generation_shortcut: bool,
    pub remember_models: bool,
    pub recent_count: u8,
    /// None открывает последний посещённый вид медиа.
    pub startup: Option<MediaKind>,
    pub last_kind: MediaKind,
    pub model_choices: HashMap<GenMode, ModelChoice>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub openrouter_model_choices: HashMap<GenMode, ModelChoice>,
}

impl Default for WorkspacePrefs {
    fn default() -> Self {
        Self {
            compact: false,
            single_image: false,
            reduce_motion: false,
            generation_shortcut: true,
            remember_models: true,
            recent_count: 5,
            startup: None,
            last_kind: MediaKind::Image,
            model_choices: HashMap::new(),
            openrouter_model_choices: HashMap::new(),
        }
    }
}

/// Настройки приложения; хранятся в SQLite.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Settings {
    /// Папка для сохранения медиа.
    pub local_folder: String,
    /// Язык интерфейса.
    #[serde(default)]
    pub lang: LangPreference,
    /// Тема оформления.
    pub theme: Theme,
    /// Показывать реальный баланс CI (иначе только локальные траты).
    pub show_real_balance: bool,
    #[serde(default)]
    pub provider: Provider,
    #[serde(default)]
    pub workspace: WorkspacePrefs,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            local_folder: String::new(),
            lang: LangPreference::System,
            theme: Theme::Light,
            show_real_balance: false,
            provider: Provider::default(),
            workspace: WorkspacePrefs::default(),
        }
    }
}

// События и настройки ходят между потоками (jobs, Tauri-команды).
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AppEvent>();
    assert_send_sync::<BalanceView>();
    assert_send_sync::<Settings>();
};
