//! Ядро Koteyye Media Studio: HTTP-клиент Cheaper Inference, каталог моделей и ошибки.
//!
//! [`CiClient`] выполняет запросы к API CI и разбирает ошибки в [`CoreError`].
//! Каталог моделей ([`CiClient::public_models`], [`CiClient::models`]) кэшируется
//! в [`CatalogCache`] и фильтруется по виду медиа ([`by_kind`]) и режиму
//! генерации ([`for_mode`]). Методы генерации появятся в последующих задачах.

mod catalog;
mod client;
mod error;

pub use catalog::{CatalogCache, by_kind, for_mode};
pub use client::CiClient;
pub use error::CoreError;
