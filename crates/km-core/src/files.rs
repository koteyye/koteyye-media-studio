//! Сохранение медиа в локальную папку: имена файлов, расширения, создание папки.
//!
//! Имена строятся как `<вид>-<unix_millis>[-N].<ext>`; суффикс `-N`
//! (номер варианта, `index + 1`) добавляется только при `total > 1`.
//! Папка пересоздаётся при сохранении, если исчезла между запусками.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use km_shared::MediaKind;
use tempfile::NamedTempFile;

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
    save_at(
        folder,
        kind,
        index,
        total,
        bytes,
        content_type,
        unix_millis(),
    )
}

/// При коллизии выбираем следующий свободный номер миллисекунды; сохраняется
/// существующий формат имени. persist_noclobber атомарно резервирует конечный
/// путь между процессами. При сбое записи временный файл удаляется автоматически.
fn save_at(
    folder: &Path,
    kind: MediaKind,
    index: usize,
    total: usize,
    bytes: &[u8],
    content_type: &str,
    mut millis: u128,
) -> Result<PathBuf> {
    ensure_folder(folder)?;
    let path_for = |millis| folder.join(file_name(kind, index, total, content_type, millis));
    let initial_path = path_for(millis);
    let mut temp = NamedTempFile::new_in(folder).map_err(|err| save_error(&initial_path, err))?;
    temp.write_all(bytes)
        .map_err(|err| save_error(&initial_path, err))?;
    loop {
        let path = path_for(millis);
        match temp.persist_noclobber(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                temp = error.file;
                millis += 1;
            }
            Err(error) => return Err(save_error(&path, error.error)),
        }
    }
}

fn save_error(path: &Path, error: io::Error) -> CoreError {
    CoreError::Io(io::Error::new(
        error.kind(),
        format!("не удалось сохранить файл {}: {error}", path.display()),
    ))
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
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" | "audio/wave" | "audio/vnd.wave" => "wav",
        "audio/ogg" | "application/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/aac" | "audio/x-aac" => "aac",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/pcm" | "audio/l16" => "pcm",
        _ => "bin",
    }
}

/// Имя файла `<вид>-<unix_millis>[-N].<ext>`; суффикс только при `total > 1`.
fn file_name(
    kind: MediaKind,
    index: usize,
    total: usize,
    content_type: &str,
    millis: u128,
) -> String {
    let (prefix, ext) = match kind {
        MediaKind::Image => ("image", ext_for(content_type)),
        MediaKind::Video => ("video", "mp4"),
        MediaKind::Audio => ("audio", ext_for(content_type)),
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colliding_saves_keep_each_payload_and_variant_name() {
        let dir = tempfile::tempdir().unwrap();
        let save =
            |bytes| save_at(dir.path(), MediaKind::Audio, 1, 2, bytes, "audio/mpeg", 123).unwrap();
        let first = save(b"first");
        let second = save(b"second");
        assert_eq!(first, dir.path().join("audio-123-2.mp3"));
        assert_eq!(second, dir.path().join("audio-124-2.mp3"));
        assert_eq!(fs::read(first).unwrap(), b"first");
        assert_eq!(fs::read(second).unwrap(), b"second");
    }
}
