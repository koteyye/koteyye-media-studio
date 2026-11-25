//! Хранение API-ключа Cheaper Inference: системный keychain, не SQLite.
//!
//! [`KeyStore`] — интерфейс хранилища ключа; [`KeychainStore`] работает через
//! системный keychain (macOS Keychain, Windows Credential Manager),
//! [`InMemoryKeyStore`] — fallback для тестов. Ключ не попадает в БД настроек
//! и истории: [`km_shared::Settings`] не содержит поля ключа.

use std::fmt;
use std::sync::{Mutex, MutexGuard};

use keyring::Entry;

/// Имя сервиса записи keychain (идентификатор приложения).
const SERVICE: &str = "ru.koteyye.media-studio";

/// Имя пользователя записи keychain — единственная запись ключа CI.
const USER: &str = "ci-api-key";

/// Хранилище API-ключа: чтение, запись и удаление.
///
/// Ошибки — строки для UI (команды отдают `Result<T, String>`). Отсутствие
/// ключа — не ошибка, а `Ok(None)`; сбой доступа к хранилищу (заблокированный
/// keychain, отказ prompt) — `Err`: «ключа нет» и «ключ не прочитать» —
/// разные причины, и путать их нельзя (ложный онбординг существующего юзера).
pub trait KeyStore: Send + Sync {
    /// Текущий ключ: `Ok(None)` — ключ не задан, `Err` — хранилище не читается.
    ///
    /// # Errors
    ///
    /// Строка с причиной, если доступ к хранилищу не удался.
    fn get(&self) -> Result<Option<String>, String>;

    /// Сохраняет ключ, перезаписывая предыдущий.
    ///
    /// # Errors
    ///
    /// Строка с причиной, если хранилище не приняло ключ.
    fn set(&self, key: &str) -> Result<(), String>;

    /// Удаляет ключ; отсутствие записи не считается ошибкой.
    ///
    /// # Errors
    ///
    /// Строка с причиной, если удаление не удалось.
    fn delete(&self) -> Result<(), String>;
}

/// Системный keychain через крейт `keyring`.
///
/// Запись `ru.koteyye.media-studio` / `ci-api-key`. На macOS доступ к Keychain
/// может потребовать подтверждения пользователя; ошибка чтения (заблокированный
/// keychain, отказ prompt — типично для неподписанных dev-сборок) возвращается
/// как `Err`, а не как отсутствие ключа: иначе существующий пользователь с
/// валидным ключом попал бы в онбординг.
#[derive(Debug)]
pub struct KeychainStore {
    entry: Entry,
}

impl KeychainStore {
    /// Открывает запись keychain для ключа CI.
    ///
    /// # Errors
    ///
    /// Строка с причиной, если запись keychain не создаётся.
    pub fn new() -> Result<KeychainStore, String> {
        let entry = Entry::new(SERVICE, USER)
            .map_err(|e| format!("не удалось открыть запись keychain: {e}"))?;
        Ok(KeychainStore { entry })
    }
}

impl KeyStore for KeychainStore {
    fn get(&self) -> Result<Option<String>, String> {
        match self.entry.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            // «Ключа нет» и «keychain недоступен» — разные причины: ошибка
            // чтения не должна выглядеть как отсутствие ключа (ложный онбординг).
            Err(e) => Err(format!("не удалось прочитать ключ из keychain: {e}")),
        }
    }

    fn set(&self, key: &str) -> Result<(), String> {
        self.entry
            .set_password(key)
            .map_err(|e| format!("не удалось сохранить ключ в keychain: {e}"))
    }

    fn delete(&self) -> Result<(), String> {
        match self.entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("не удалось удалить ключ из keychain: {e}")),
        }
    }
}

/// Ключ в памяти процесса — fallback для тестов и headless-сборок.
///
/// Не персистентен: после перезапуска ключа нет. В продакшене используется
/// [`KeychainStore`].
#[derive(Default)]
pub struct InMemoryKeyStore {
    key: Mutex<Option<String>>,
}

impl InMemoryKeyStore {
    /// Захватывает ключ; отравление мьютекса фатально.
    fn lock(&self) -> MutexGuard<'_, Option<String>> {
        self.key.lock().expect("мьютекс ключа отравлен")
    }
}

impl KeyStore for InMemoryKeyStore {
    fn get(&self) -> Result<Option<String>, String> {
        Ok(self.lock().clone())
    }

    fn set(&self, key: &str) -> Result<(), String> {
        *self.lock() = Some(key.to_owned());
        Ok(())
    }

    fn delete(&self) -> Result<(), String> {
        *self.lock() = None;
        Ok(())
    }
}

/// Debug без утечки ключа: только факт наличия.
impl fmt::Debug for InMemoryKeyStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InMemoryKeyStore")
            .field("key_set", &self.lock().is_some())
            .finish()
    }
}

// Хранилища ключа живут в Tauri-состоянии и ходят между потоками.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<KeychainStore>();
    assert_send_sync::<InMemoryKeyStore>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_store_roundtrip() {
        let store = InMemoryKeyStore::default();
        assert_eq!(store.get().expect("get"), None, "изначально ключа нет");
        store.set("ci_live_x").expect("set");
        assert_eq!(store.get().expect("get").as_deref(), Some("ci_live_x"));
        store.set("ci_live_y").expect("перезапись");
        assert_eq!(store.get().expect("get").as_deref(), Some("ci_live_y"));
        store.delete().expect("delete");
        assert_eq!(store.get().expect("get"), None);
        store.delete().expect("повторное удаление не ошибка");
    }

    #[test]
    fn debug_does_not_leak_key() {
        let store = InMemoryKeyStore::default();
        store.set("ci_live_secret").expect("set");
        let debug = format!("{store:?}");
        assert!(
            !debug.contains("ci_live_secret"),
            "ключ не печатается: {debug}"
        );
        assert!(debug.contains("key_set: true"));
    }
}
