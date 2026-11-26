//! Отдельные записи API-ключей провайдеров в системном keychain.
//!
//! [`KeyStore`] — интерфейс хранилища ключа; [`KeychainStore`] работает через
//! системный keychain (macOS Keychain, Windows Credential Manager),
//! [`InMemoryKeyStore`] — fallback для тестов. Ключ не попадает в БД настроек
//! и истории: [`km_shared::Settings`] не содержит поля ключа.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use keyring::Entry;
use km_shared::Provider;

use crate::APP_IDENTIFIER;

const CI_USER: &str = "ci-api-key";
const OPENROUTER_USER: &str = "openrouter-api-key";

/// Хранилище API-ключа: чтение, запись и удаление.
///
/// Ошибки — строки для UI (команды отдают `Result<T, String>`). Отсутствие
/// ключа — не ошибка, а `Ok(None)`; сбой доступа к хранилищу (заблокированный
/// keychain, отказ prompt) — `Err`: «ключа нет» и «ключ не прочитать» —
/// разные причины, и путать их нельзя (ложный онбординг существующего юзера).
pub trait KeyStore: Send + Sync {
    /// Проверяет наличие записи, не отдавая вызывающему её содержимое.
    fn has(&self, provider: Provider) -> Result<bool, String> {
        Ok(self
            .get(provider)?
            .is_some_and(|key| !key.trim().is_empty()))
    }

    /// Текущий ключ: `Ok(None)` — ключ не задан, `Err` — хранилище не читается.
    ///
    /// # Errors
    ///
    /// Строка с причиной, если доступ к хранилищу не удался.
    fn get(&self, provider: Provider) -> Result<Option<String>, String>;

    /// Сохраняет ключ, перезаписывая предыдущий.
    ///
    /// # Errors
    ///
    /// Строка с причиной, если хранилище не приняло ключ.
    fn set(&self, provider: Provider, key: &str) -> Result<(), String>;

    /// Удаляет ключ; отсутствие записи не считается ошибкой.
    ///
    /// # Errors
    ///
    /// Строка с причиной, если удаление не удалось.
    fn delete(&self, provider: Provider) -> Result<(), String>;
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
    ci: Entry,
    openrouter: Entry,
}

impl KeychainStore {
    /// Готовит идентификаторы записей для обоих сервисов без чтения секретов.
    ///
    /// # Errors
    ///
    /// Строка с причиной, если запись keychain не создаётся.
    pub fn new() -> Result<KeychainStore, String> {
        let entry = |user| {
            Entry::new(APP_IDENTIFIER, user)
                .map_err(|e| format!("не удалось открыть запись keychain: {e}"))
        };
        Ok(Self {
            ci: entry(CI_USER)?,
            openrouter: entry(OPENROUTER_USER)?,
        })
    }

    fn entry(&self, provider: Provider) -> &Entry {
        match provider {
            Provider::CheaperInference => &self.ci,
            Provider::OpenRouter => &self.openrouter,
        }
    }
}

impl KeyStore for KeychainStore {
    fn has(&self, provider: Provider) -> Result<bool, String> {
        match self.entry(provider).get_attributes() {
            Ok(_) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(format!(
                "не удалось проверить наличие ключа в keychain: {e}"
            )),
        }
    }

    fn get(&self, provider: Provider) -> Result<Option<String>, String> {
        match self.entry(provider).get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            // «Ключа нет» и «keychain недоступен» — разные причины: ошибка
            // чтения не должна выглядеть как отсутствие ключа (ложный онбординг).
            Err(e) => Err(format!("не удалось прочитать ключ из keychain: {e}")),
        }
    }

    fn set(&self, provider: Provider, key: &str) -> Result<(), String> {
        self.entry(provider)
            .set_password(key)
            .map_err(|e| format!("не удалось сохранить ключ в keychain: {e}"))
    }

    fn delete(&self, provider: Provider) -> Result<(), String> {
        match self.entry(provider).delete_credential() {
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
    keys: Mutex<HashMap<Provider, String>>,
}

impl KeyStore for InMemoryKeyStore {
    fn get(&self, provider: Provider) -> Result<Option<String>, String> {
        Ok(self
            .keys
            .lock()
            .expect("мьютекс ключей отравлен")
            .get(&provider)
            .cloned())
    }

    fn set(&self, provider: Provider, key: &str) -> Result<(), String> {
        self.keys
            .lock()
            .expect("мьютекс ключей отравлен")
            .insert(provider, key.to_owned());
        Ok(())
    }

    fn delete(&self, provider: Provider) -> Result<(), String> {
        self.keys
            .lock()
            .expect("мьютекс ключей отравлен")
            .remove(&provider);
        Ok(())
    }
}

/// Debug без утечки ключа: только факт наличия.
impl fmt::Debug for InMemoryKeyStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InMemoryKeyStore")
            .field(
                "key_set",
                &!self
                    .keys
                    .lock()
                    .expect("мьютекс ключей отравлен")
                    .is_empty(),
            )
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
        let ci = Provider::CheaperInference;
        let or = Provider::OpenRouter;
        let store = InMemoryKeyStore::default();
        assert_eq!(store.get(ci).expect("get"), None);
        store.set(ci, "ci_live_x").expect("set");
        store.set(or, "router-test").expect("set");
        store.set(ci, "ci_live_y").expect("перезапись");
        assert_eq!(store.get(ci).expect("get").as_deref(), Some("ci_live_y"));
        store.delete(ci).expect("delete");
        assert_eq!(store.get(ci).expect("get"), None);
        store.delete(ci).expect("повторное удаление не ошибка");
        assert_eq!(store.get(or).unwrap().as_deref(), Some("router-test"));
    }

    #[test]
    fn debug_does_not_leak_key() {
        let store = InMemoryKeyStore::default();
        store
            .set(Provider::CheaperInference, "ci_live_secret")
            .expect("set");
        let debug = format!("{store:?}");
        assert!(
            !debug.contains("ci_live_secret"),
            "ключ не печатается: {debug}"
        );
        assert!(debug.contains("key_set: true"));
    }
}
