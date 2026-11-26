//! Каталог моделей Cheaper Inference: методы клиента, кэш с TTL и фильтры.

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use km_shared::{CatalogModel, CatalogPricingScope, GenMode, MediaKind, MediaPrices};
use serde::Deserialize;

use crate::client::CiClient;
use crate::error::CoreError;

/// Два каталога имеют разные обёртки. При проверке ключа достаточно массива,
/// поэтому смена полей какой-либо модели не блокирует вход.
#[derive(Deserialize)]
struct ModelList<T> {
    #[serde(alias = "models")]
    data: Vec<T>,
}

/// Публичный каталог содержит плоские поля цен. Авторизованный каталог
/// возвращает цены маршрутов данного ключа во вложенном объекте pricing.
#[derive(Deserialize, Default)]
struct ApiPricing {
    media_unit: Option<String>,
    media_unit_price: Option<String>,
    #[serde(default)]
    resolution_prices: MediaPrices,
    scopes: Option<Vec<CatalogPricingScope>>,
    discount_percent: Option<String>,
}

#[derive(Deserialize, Default)]
struct Capabilities {
    #[serde(default)]
    vision: bool,
    image_edit: Option<bool>,
}

#[derive(Deserialize)]
struct WireModel {
    #[serde(flatten)]
    model: CatalogModel,
    supported_endpoints: Option<Vec<String>>,
    supports_image_edit: Option<bool>,
    endpoint: Option<String>,
    pricing: Option<ApiPricing>,
    capabilities: Option<Capabilities>,
}

impl WireModel {
    fn into_model(self) -> CatalogModel {
        let mut model = self.model;
        model.supported_endpoints = self
            .supported_endpoints
            .unwrap_or_else(|| self.endpoint.into_iter().collect());
        if let Some(pricing) = self.pricing {
            model.media_unit = pricing.media_unit;
            model.media_unit_price = pricing.media_unit_price;
            model.media_prices = pricing.resolution_prices;
            model.pricing_scopes = pricing.scopes;
            model.discount_percent = pricing.discount_percent;
        }
        let edit_endpoint = model.supports("/v1/images/edits");
        let edit_capability = self
            .capabilities
            .as_ref()
            .and_then(|caps| caps.image_edit)
            .or(self.supports_image_edit);
        model.supports_image_edit = edit_endpoint && edit_capability.unwrap_or(true);
        if !model.supports_image_edit {
            model.features.retain(|feature| feature != "image-edit");
        }
        if self.capabilities.is_some_and(|caps| caps.vision) && !model.has_feature("image-input") {
            model.features.push("image-input".to_owned());
        }
        model
    }
}

impl ModelList<WireModel> {
    fn into_models(self) -> Vec<CatalogModel> {
        self.data.into_iter().map(WireModel::into_model).collect()
    }
}

impl CiClient {
    /// Публичный каталог без заголовка Authorization.
    pub async fn public_models(&self) -> Result<Vec<CatalogModel>, CoreError> {
        let list: ModelList<WireModel> = self.get_public_json("/public/models").await?;
        Ok(list.into_models())
    }

    /// Полный каталог, предоставленный для данного ключа.
    pub async fn models(&self) -> Result<Vec<CatalogModel>, CoreError> {
        self.auth_models("/v1/models").await
    }

    /// Фильтр сервера исключает загрузку текстовых и других нерелевантных моделей.
    pub async fn models_for_kind(&self, kind: MediaKind) -> Result<Vec<CatalogModel>, CoreError> {
        let path = match kind {
            MediaKind::Image => "/v1/models?type=image",
            MediaKind::Video => "/v1/models?type=video",
            MediaKind::Audio => "/v1/models?type=speech",
        };
        self.auth_models(path).await
    }

    async fn auth_models(&self, path: &str) -> Result<Vec<CatalogModel>, CoreError> {
        let list: ModelList<WireModel> = self.get_json(path).await?;
        Ok(list.into_models())
    }

    /// Проверяет Bearer на небольшом каталоге. Даже пустой список допустим
    /// для действующего ключа с ограничением по моделям.
    pub async fn verify_key(&self) -> Result<(), CoreError> {
        self.get_json::<ModelList<serde::de::IgnoredAny>>("/v1/models?type=decision")
            .await?;
        Ok(())
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
    models
        .iter()
        .filter(|model| model.matches_kind(kind))
        .collect()
}

/// Отбирает модели, совместимые с режимом генерации.
///
/// Использует те же правила совместимости, что и селектор моделей студии.
#[must_use]
pub fn for_mode(models: &[CatalogModel], mode: GenMode) -> Vec<&CatalogModel> {
    models
        .iter()
        .filter(|model| model.supports_mode(mode))
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
