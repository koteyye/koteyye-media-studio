//! Ядро Koteyye Media Studio: HTTP-клиент Cheaper Inference, каталог моделей и ошибки.
//!
//! [`CiClient`] выполняет запросы к API CI и разбирает ошибки в [`CoreError`].
//! Каталог моделей ([`CiClient::public_models`], [`CiClient::models`]) кэшируется
//! в [`CatalogCache`] и фильтруется по виду медиа ([`by_kind`]) и режиму
//! генерации ([`for_mode`]). Изображения генерируются и редактируются
//! ([`CiClient::generate_image`], [`CiClient::edit_image`]), байты результата
//! даёт [`image_bytes`]. Аудио (озвучка, музыка, эффекты) генерируется
//! через [`CiClient::speech`]. Видео генерируется синхронно
//! ([`CiClient::generate_video`]), восстановление завершённых генераций —
//! [`CiClient::generation`]. История, расход и настройки хранятся в SQLite
//! ([`Storage`]). Результаты генераций сохраняются в локальную папку
//! ([`save_media`], [`ensure_folder`]), расширения подбираются по
//! content-type ([`ext_for`]). Оркестратор [`Jobs`] связывает клиент, хранилище
//! и файлы в единый поток генерации: запуск ([`Jobs::run`]), восстановление
//! незавершённых записей ([`Jobs::recover_pending`]) и гибридный баланс
//! ([`Jobs::balance_view`], [`CiClient::balance`]).

mod audio;
mod catalog;
mod client;
mod error;
mod files;
mod images;
mod jobs;
mod storage;
mod video;

pub use catalog::{CatalogCache, by_kind, for_mode};
pub use client::CiClient;
pub use error::CoreError;
pub use files::{ensure_folder, ext_for, save_media};
pub use images::image_bytes;
pub use jobs::Jobs;
pub use storage::Storage;
