//! Состояние приложения: хранилище, keychain, кэш каталога и фабрика джобов.
//!
//! [`AppState`] живёт в Tauri-состоянии (`Arc<AppState>` через `.manage`);
//! все поля синхронизированы внутри себя ([`Storage`] — мьютекс соединения,
//! [`CatalogCache`] — мьютекс состояния, [`KeyStore`] — по реализации),
//! поэтому команды работают с `&AppState` без внешнего лока. Исключение —
//! восстановление: проходы сериализуются локом [`AppState::recovery_lock`],
//! чтобы стартовый проход и проход из UI не обработали одни Pending дважды.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use km_core::{CatalogCache, CiClient, Jobs, OpenRouterKeyClient, Storage};
use km_shared::{AppEvent, Provider};
use tauri::async_runtime::RwLock;

use crate::keychain::KeyStore;

/// Время жизни кэша публичного каталога: модели меняются редко,
/// 10 минут достаточно, чтобы не ходить в сеть при каждом открытии студии.
const CATALOG_TTL: Duration = Duration::from_secs(600);

/// Состояние приложения для команд и джобов.
///
/// БД — файл в `app_data_dir` (в тестах — tempdir); ключ — в [`KeyStore`]
/// (системный keychain, не SQLite). Настройки не кэшируются: источник
/// истины — SQLite, чтение одной строки дешёвое.
pub struct AppState {
    /// Хранилище истории, расхода и настроек (соединение команд).
    pub storage: Storage,
    /// Хранилище API-ключа.
    pub key_store: Arc<dyn KeyStore>,
    /// Кэш публичного каталога моделей (для работы без ключа).
    catalog: CatalogCache,
    /// Базовый URL CI: продакшен или мок-сервер в тестах.
    base_url: String,
    pub openrouter: OpenRouterKeyClient,
    /// Путь файла БД — для второго соединения джобов.
    db_path: PathBuf,
    /// Эмиттер событий: в Tauri — события окна, в тестах — коллектор.
    emit: Arc<dyn Fn(AppEvent) + Send + Sync>,
    /// Генерации могут идти одновременно; восстановление ждёт их завершения.
    recovery_lock: RwLock<()>,
    /// Порядок записей SQLite и применения темы к окну должен совпадать.
    pub(crate) settings_write: Mutex<()>,
    /// Устаревший ответ кошелька нельзя принимать даже после перехода CI → OR → CI.
    balance_epochs: [AtomicU64; 2],
}

impl AppState {
    /// Собирает состояние: открывает БД, кэш каталога и эмиттер событий.
    ///
    /// # Errors
    ///
    /// Строка с причиной, если БД не открывается.
    pub fn new(
        db_path: &Path,
        key_store: Box<dyn KeyStore>,
        base_url: &str,
        emit: Arc<dyn Fn(AppEvent) + Send + Sync>,
    ) -> Result<AppState, String> {
        let storage = Storage::open(db_path).map_err(|e| e.to_string())?;
        Ok(AppState {
            storage,
            key_store: key_store.into(),
            catalog: CatalogCache::new(CATALOG_TTL),
            base_url: base_url.to_owned(),
            openrouter: OpenRouterKeyClient::default(),
            db_path: db_path.to_owned(),
            emit,
            recovery_lock: RwLock::new(()),
            settings_write: Mutex::new(()),
            balance_epochs: Default::default(),
        })
    }

    /// Клиент CI с заданным ключом (пустой ключ — для публичных путей).
    #[must_use]
    pub fn client(&self, key: &str) -> CiClient {
        CiClient::with_base(key, &self.base_url)
    }

    /// Фабрика джобов: [`Jobs`] с отдельным соединением той же БД и эмиттером.
    ///
    /// [`Jobs`] владеет [`Storage`] по значению, а соединение не клонируется,
    /// поэтому джобы открывают второе соединение к тому же файлу SQLite.
    ///
    /// # Errors
    ///
    /// Строка с причиной, если соединение БД не открывается.
    pub fn jobs(&self, client: CiClient, folder: PathBuf) -> Result<Jobs, String> {
        let storage = Storage::open(&self.db_path).map_err(|e| e.to_string())?;
        Ok(Jobs::new(client, storage, folder, Arc::clone(&self.emit)))
    }

    /// Кэш публичного каталога моделей.
    #[must_use]
    pub fn catalog(&self) -> &CatalogCache {
        &self.catalog
    }

    fn balance_epoch(&self, provider: Provider) -> &AtomicU64 {
        &self.balance_epochs[match provider {
            Provider::CheaperInference => 0,
            Provider::OpenRouter => 1,
        }]
    }

    pub(crate) fn balance_revision(&self, provider: Provider) -> u64 {
        self.balance_epoch(provider).load(Ordering::Acquire)
    }

    pub(crate) fn invalidate_balance(&self, provider: Provider) {
        self.balance_epoch(provider).fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn provider_changed(&self, previous: Provider, current: Provider) {
        if previous != current {
            self.invalidate_balance(previous);
            self.invalidate_balance(current);
        }
    }

    /// Лок проходов восстановления: захватывается на весь `cmd_recover_pending`.
    ///
    /// Стартовый проход (setup) и проход из UI (load) идут через одну команду;
    /// лок не даёт им прочитать одни `Pending` параллельно и скачать файлы
    /// дважды. Второй проход ждёт первый и видит только ещё-`Pending`.
    #[must_use]
    pub fn recovery_lock(&self) -> &RwLock<()> {
        &self.recovery_lock
    }

    /// Системный диалог Keychain может ждать пользователя; не занимаем поток async-рантайма.
    pub(crate) async fn has_key(&self, provider: Provider) -> Result<bool, String> {
        let store = self.key_store.clone();
        tauri::async_runtime::spawn_blocking(move || store.has(provider))
            .await
            .map_err(|e| e.to_string())?
    }

    pub(crate) async fn read_key(&self, provider: Provider) -> Result<Option<String>, String> {
        let store = self.key_store.clone();
        tauri::async_runtime::spawn_blocking(move || store.get(provider))
            .await
            .map_err(|e| e.to_string())?
    }

    pub(crate) async fn write_key(&self, provider: Provider, key: String) -> Result<(), String> {
        let store = self.key_store.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if key.is_empty() {
                store.delete(provider)
            } else {
                store.set(provider, &key)
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// Шлёт событие ядра через эмиттер (Tauri-события или коллектор тестов).
    pub fn emit(&self, event: AppEvent) {
        (self.emit)(event);
    }
}

// Состояние живёт в Tauri и ходит между потоками — фиксируем Send + Sync.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AppState>();
};
