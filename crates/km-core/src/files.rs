//! Сохранение медиа в локальную папку: имена файлов, расширения, создание папки.
//!
//! Имена строятся как `<вид>-<unix_millis>[-N].<ext>`; суффикс `-N`
//! (номер варианта, `index + 1`) добавляется только при `total > 1`.
//! Папка пересоздаётся при сохранении, если исчезла между запусками.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use km_shared::MediaKind;

use crate::error::CoreError;

type Result<T> = std::result::Result<T, CoreError>;

/// Создаёт папку и родительские директории, если их нет.
///
/// Существующая папка не изменяется. Путь, занятый файлом, — ошибка,
/// а не паника.
///
/// # Errors
///
/// [`CoreError::Io`], если путь не является папкой или создание
/// не удалось (нет прав, недоступный диск).
pub fn ensure_folder(folder: &Path) -> Result<()> {
    if folder.exists() && !folder.is_dir() {
        return Err(CoreError::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("путь занят файлом, а не папкой: {}", folder.display()),
        )));
    }
    fs::create_dir_all(folder)?;
    Ok(())
}

/// Сохраняет байты медиа в папку и возвращает путь к созданному файлу.
///
/// Имя: `<вид>-<unix_millis>[-N].<ext>`, где `-N` — номер варианта
/// (`index + 1`) только при `total > 1`. Видео всегда получает расширение
/// `mp4`, изображение и аудио — по [`ext_for`]. Отсутствующая папка
/// создаётся ([`ensure_folder`]).
///
/// # Errors
///
/// [`CoreError::Io`], если папку не удалось создать или файл не записался.
pub fn save_media(
    folder: &Path,
    kind: MediaKind,
    index: usize,
    total: usize,
    bytes: &[u8],
    content_type: &str,
) -> Result<PathBuf> {
    ensure_folder(folder)?;
    let path = folder.join(file_name(kind, index, total, content_type));
    fs::write(&path, bytes)?;
    Ok(path)
}

/// Расширение файла по content-type; неизвестный тип даёт `bin`.
#[must_use]
pub fn ext_for(content_type: &str) -> &'static str {
    match content_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "video/mp4" => "mp4",
        "audio/mpeg" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        _ => "bin",
    }
}

/// Имя файла `<вид>-<unix_millis>[-N].<ext>`; суффикс только при `total > 1`.
fn file_name(kind: MediaKind, index: usize, total: usize, content_type: &str) -> String {
    let (prefix, ext) = match kind {
        MediaKind::Image => ("image", ext_for(content_type)),
        MediaKind::Video => ("video", "mp4"),
        MediaKind::Audio => ("audio", ext_for(content_type)),
    };
    let millis = unix_millis();
    if total > 1 {
        format!("{prefix}-{millis}-{}.{ext}", index + 1)
    } else {
        format!("{prefix}-{millis}.{ext}")
    }
}

/// Текущее время в миллисекундах от Unix-эпохи; часы до эпохи дают 0.
fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
