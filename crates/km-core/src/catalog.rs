//! Каталог моделей Cheaper Inference: методы клиента, кэш с TTL и фильтры.

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use km_shared::{CatalogModel, GenMode, MediaKind, ModelType};
use serde::Deserialize;

use crate::client::CiClient;
use crate::error::CoreError;

/// Обёртка ответа каталога: CI шлёт `{"models":[...]}` на `/public/models`
/// и `{"data":[...]}` на `/v1/models`; поддержаны оба ключа на обоих путях.
#[derive(Deserialize)]
struct CatalogEnvelope {
    #[serde(default)]
    models: Option<Vec<CatalogModel>>,
    #[serde(default)]
    data: Option<Vec<CatalogModel>>,
}

impl CatalogEnvelope {
    /// Достаёт модели из обёртки; ошибка, если нет ни одного из ключей.
    fn into_models(self) -> Result<Vec<CatalogModel>, CoreError> {
        self.models.or(self.data).ok_or_else(|| {
            CoreError::Serde("ответ каталога не содержит ключей models или data".to_owned())
        })
    }
}

impl CiClient {
    /// Каталог моделей без ключа: `GET /public/models`.
    ///
    /// Заголовок `Authorization` не отправляется, даже если ключ задан в
    /// клиенте. Поддержаны обе обёртки ответа — `models` и `data`.
    ///
    /// # Errors
    ///
    /// [`CoreError`] при сбое запроса или разбора ответа; [`CoreError::Serde`],
    /// если в ответе нет ни `models`, ни `data`.
    pub async fn public_models(&self) -> Result<Vec<CatalogModel>, CoreError> {
        let envelope: CatalogEnvelope = self.get_public_json("/public/models").await?;
        envelope.into_models()
    }

    /// Каталог моделей с ключом: `GET /v1/models` (`Authorization: Bearer`).
    ///
    /// Поддержаны обе обёртки ответа — `data` и `models`.
    ///
    /// # Errors
    ///
    /// [`CoreError`] при сбое запроса или разбора ответа; [`CoreError::Serde`],
    /// если в ответе нет ни `models`, ни `data`.
    pub async fn models(&self) -> Result<Vec<CatalogModel>, CoreError> {
        let envelope: CatalogEnvelope = self.get_json("/v1/models").await?;
        envelope.into_models()
    }
}

/// Состояние кэша: момент загрузки и модели каталога.
type CacheState = Option<(Instant, Vec<CatalogModel>)>;

/// Кэш каталога моделей с временем жизни (TTL).
///
/// Хранит результат [`CiClient::public_models`] и отдаёт его без запроса,
/// пока не истёк TTL. `TTL = 0` отключает кэширование: каждый вызов
/// [`CatalogCache::get_or_fetch`] идёт в сеть. Лок удерживается только на
/// проверку и запись состояния, но не на время HTTP-запроса.
#[derive(Debug)]
pub struct CatalogCache {
    ttl: Duration,
    state: Mutex<CacheState>,
}

impl CatalogCache {
    /// Создаёт кэш с заданным TTL; `Duration::ZERO` означает рефетч каждый вызов.
    #[must_use]
    pub fn new(ttl: Duration) -> CatalogCache {
        CatalogCache {
            ttl,
            state: Mutex::new(None),
        }
    }

    /// Отдаёт каталог из кэша, если он свеж, иначе загружает через
    /// [`CiClient::public_models`] и обновляет кэш.
    ///
    /// Возвращает собственную копию моделей: кэш сохраняет свой экземпляр.
    ///
    /// # Errors
    ///
    /// [`CoreError`] при сбое запроса к CI; состояние кэша при ошибке не меняется.
    pub async fn get_or_fetch(&self, client: &CiClient) -> Result<Vec<CatalogModel>, CoreError> {
        if let Some(models) = self.fresh() {
            return Ok(models);
        }
        // Запрос вне лока: std::sync::Mutex нельзя держать через await.
        let models = client.public_models().await?;
        *self.lock() = Some((Instant::now(), models.clone()));
        Ok(models)
    }

    /// Клон кэша, если он свеж; лок держится только на время проверки.
    fn fresh(&self) -> Option<Vec<CatalogModel>> {
        let guard = self.lock();
        let (fetched_at, models) = guard.as_ref()?;
        if fetched_at.elapsed() < self.ttl {
            Some(models.clone())
        } else {
            None
        }
    }

    /// Захватывает лок состояния; отравление мьютекса фатально.
    fn lock(&self) -> MutexGuard<'_, CacheState> {
        self.state.lock().expect("мьютекс кэша каталога отравлен")
    }
}

// Кэш живёт в Tauri-состоянии и ходит между потоками — фиксируем Send + Sync.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CatalogCache>();
};

/// Отбирает модели по виду медиа: Image → image, Video → video, Audio → speech.
///
/// Порядок моделей сохраняется; возвращаются ссылки на элементы `models`.
#[must_use]
pub fn by_kind(models: &[CatalogModel], kind: MediaKind) -> Vec<&CatalogModel> {
    let model_type = match kind {
        MediaKind::Image => ModelType::Image,
        MediaKind::Video => ModelType::Video,
        MediaKind::Audio => ModelType::Speech,
    };
    models
        .iter()
        .filter(|model| model.model_type == model_type)
        .collect()
}

/// Отбирает модели, совместимые с режимом генерации.
///
/// Правила по типу модели и возможностям:
/// - TextToImage — тип `image`; TextToVideo — тип `video`.
/// - ImageEdit — тип `image` и эндпоинт `/v1/images/edits`.
/// - ImageToVideo — тип `video` и возможность `image-input`.
/// - ReferenceToVideo — тип `video` и (`image-input` или `r2v` в id).
/// - Speech — тип `speech`, id без `music` и `sound-effects`;
///   Music — тип `speech` и `music` в id; SoundEffects — тип `speech`
///   и `sound-effects` в id.
///
/// Проверка типа обязательна: например, `image-input` есть и у image-моделей
/// (для редактирования), но в ImageToVideo они не попадают.
/// Порядок моделей сохраняется; возвращаются ссылки на элементы `models`.
#[must_use]
pub fn for_mode(models: &[CatalogModel], mode: GenMode) -> Vec<&CatalogModel> {
    models
        .iter()
        .filter(|model| match mode {
            GenMode::TextToImage => model.model_type == ModelType::Image,
            GenMode::ImageEdit => {
                model.model_type == ModelType::Image && model.supports("/v1/images/edits")
            }
            GenMode::TextToVideo => model.model_type == ModelType::Video,
            GenMode::ImageToVideo => {
                model.model_type == ModelType::Video && model.has_feature("image-input")
            }
            GenMode::ReferenceToVideo => {
                model.model_type == ModelType::Video
                    && (model.has_feature("image-input") || model.id.contains("r2v"))
            }
            GenMode::Speech => {
                model.model_type == ModelType::Speech
                    && !model.id.contains("music")
                    && !model.id.contains("sound-effects")
            }
            GenMode::Music => model.model_type == ModelType::Speech && model.id.contains("music"),
            GenMode::SoundEffects => {
                model.model_type == ModelType::Speech && model.id.contains("sound-effects")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_future_is_send() {
        fn assert_send<T: Send>(_: T) {}
        let client = CiClient::with_base("k", "http://localhost:1");
        let cache = CatalogCache::new(Duration::from_secs(60));
        assert_send(cache.get_or_fetch(&client));
    }
}
