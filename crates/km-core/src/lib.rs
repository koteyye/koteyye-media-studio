//! Ядро Koteyye Media Studio: HTTP-клиент Cheaper Inference и ошибки.
//!
//! [`CiClient`] выполняет запросы к API CI и разбирает ошибки в [`CoreError`].
//! Методы каталога и генерации появятся в последующих задачах.

mod client;
mod error;

pub use client::CiClient;
pub use error::CoreError;
