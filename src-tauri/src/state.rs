//! Состояние приложения: хранилище, keychain, кэш каталога и фабрика джобов.
//!
//! [`AppState`] живёт в Tauri-состоянии (`Arc<AppState>` через `.manage`);
//! все поля синхронизированы внутри себя ([`Storage`] — мьютекс соединения,
//! [`CatalogCache`] — мьютекс состояния, [`KeyStore`] — по реализации),
//! поэтому команды работают с `&AppState` без внешнего лока. Исключение —
//! восстановление: проходы сериализуются локом [`AppState::recovery_lock`],
//! чтобы стартовый проход и проход из UI не обработали одни Pending дважды.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use km_core::{CatalogCache, CiClient, Jobs, Storage};
use km_shared::AppEvent;
use tauri::async_runtime::Mutex;

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
    pub key_store: Box<dyn KeyStore>,
    /// Кэш публичного каталога моделей (для работы без ключа).
    catalog: CatalogCache,
    /// Базовый URL CI: продакшен или мок-сервер в тестах.
    base_url: String,
    /// Путь файла БД — для второго соединения джобов.
    db_path: PathBuf,
    /// Эмиттер событий: в Tauri — события окна, в тестах — коллектор.
    emit: Arc<dyn Fn(AppEvent) + Send + Sync>,
    /// Сериализует проходы восстановления (setup при старте и load из UI).
    recovery_lock: Mutex<()>,
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
            key_store,
            catalog: CatalogCache::new(CATALOG_TTL),
            base_url: base_url.to_owned(),
            db_path: db_path.to_owned(),
            emit,
            recovery_lock: Mutex::new(()),
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

    /// Лок проходов восстановления: захватывается на весь `cmd_recover_pending`.
    ///
    /// Стартовый проход (setup) и проход из UI (load) идут через одну команду;
    /// лок не даёт им прочитать одни `Pending` параллельно и скачать файлы
    /// дважды. Второй проход ждёт первый и видит только ещё-`Pending`.
    #[must_use]
    pub fn recovery_lock(&self) -> &Mutex<()> {
        &self.recovery_lock
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
