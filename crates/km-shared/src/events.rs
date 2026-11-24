//! События ядра для UI, представление баланса и настройки приложения.
//!
//! [`AppEvent`] — payload Tauri-событий `generation:started/done/failed`
//! и `balance:updated`; JSON externally tagged, имена вариантов в snake_case
//! (например `{"generation_started":{"history_id":"..."}}`).

use serde::{Deserialize, Serialize};

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

/// Баланс для UI: реальный из CI плюс локальная сумма трат.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct BalanceView {
    /// Реальный баланс CI; `None`, если ключа нет или не хватает scope.
    #[serde(default)]
    pub real: Option<AccountBalance>,
    /// Сумма трат по локальной истории, USD.
    pub local_spend_usd: f64,
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

/// Настройки приложения; хранятся в SQLite.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Settings {
    /// Папка для сохранения медиа.
    pub local_folder: String,
    /// Язык интерфейса.
    pub lang: Lang,
    /// Тема оформления.
    pub theme: Theme,
    /// Показывать реальный баланс CI (иначе только локальные траты).
    pub show_real_balance: bool,
}

// События и настройки ходят между потоками (jobs, Tauri-команды).
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AppEvent>();
    assert_send_sync::<BalanceView>();
    assert_send_sync::<Settings>();
};
