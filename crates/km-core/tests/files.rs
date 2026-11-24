//! Интеграционные тесты сохранения медиа в локальную папку.
//!
//! Проверяются на реальной ФС через tempfile: имена файлов, расширения,
//! создание отсутствующей папки и ошибка на пути, занятом файлом.

use std::fs;
use std::path::Path;

use km_core::{CoreError, ensure_folder, ext_for, save_media};
use km_shared::MediaKind;
use tempfile::TempDir;

/// Проверяет имя `<prefix>-<unix_millis>[-N].<ext>`: millis — цифры
/// и правдоподобное текущее время в миллисекундах, номер варианта — `number`.
fn assert_file_name(path: &Path, prefix: &str, number: Option<usize>, ext: &str) {
    let name = path
        .file_name()
        .expect("у сохранённого пути есть имя файла")
        .to_string_lossy();
    let stem = name
        .strip_suffix(&format!(".{ext}"))
        .unwrap_or_else(|| panic!("имя {name} должно оканчиваться на .{ext}"));
    let rest = stem
        .strip_prefix(&format!("{prefix}-"))
        .unwrap_or_else(|| panic!("имя {name} должно начинаться с {prefix}-"));
    let millis = match number {
        Some(n) => {
            let (millis, suffix) = rest
                .rsplit_once('-')
                .unwrap_or_else(|| panic!("имя {name} должно содержать суффикс -{n}"));
            assert_eq!(suffix, n.to_string(), "номер варианта в имени {name}");
            millis
        }
        None => rest,
    };
    assert!(
        !millis.is_empty() && millis.bytes().all(|b| b.is_ascii_digit()),
        "unix_millis в имени {name} должен быть цифрами"
    );
    let ts: u128 = millis.parse().expect("unix_millis — число");
    // Нижняя граница — ноябрь 2023: отсекает секунды вместо миллисекунд.
    assert!(
        ts > 1_700_000_000_000,
        "unix_millis в имени {name} должен быть в миллисекундах"
    );
}

#[test]
fn save_image_single_writes_file_with_bytes() {
    let dir = TempDir::new().expect("tempdir создаётся");
    let bytes = b"\x89PNG fake image bytes";
    let path = save_media(dir.path(), MediaKind::Image, 0, 1, bytes, "image/png")
        .expect("изображение сохраняется");
    assert_eq!(
        path.parent(),
        Some(dir.path()),
        "файл лежит в заданной папке"
    );
    assert_file_name(&path, "image", None, "png");
    assert!(path.is_file(), "файл существует: {}", path.display());
    assert_eq!(fs::read(&path).expect("файл читается"), bytes);
}

#[test]
fn save_multi_adds_one_based_suffix() {
    let dir = TempDir::new().expect("tempdir создаётся");
    for index in 0..3 {
        let path = save_media(dir.path(), MediaKind::Image, index, 3, b"data", "image/png")
            .expect("вариант сохраняется");
        assert_file_name(&path, "image", Some(index + 1), "png");
    }
}

#[test]
fn save_video_uses_mp4_extension() {
    let dir = TempDir::new().expect("tempdir создаётся");
    let bytes = b"fake mp4 bytes";
    let path = save_media(dir.path(), MediaKind::Video, 0, 1, bytes, "video/mp4")
        .expect("видео сохраняется");
    assert_file_name(&path, "video", None, "mp4");
    assert_eq!(fs::read(&path).expect("файл читается"), bytes);

    // Видео всегда mp4, даже если content-type неизвестен.
    let path = save_media(
        dir.path(),
        MediaKind::Video,
        0,
        1,
        bytes,
        "application/octet-stream",
    )
    .expect("видео сохраняется");
    assert_file_name(&path, "video", None, "mp4");
}

#[test]
fn save_audio_mpeg_uses_mp3() {
    let dir = TempDir::new().expect("tempdir создаётся");
    let bytes = b"fake mp3 bytes";
    let path = save_media(dir.path(), MediaKind::Audio, 0, 1, bytes, "audio/mpeg")
        .expect("аудио сохраняется");
    assert_file_name(&path, "audio", None, "mp3");
    assert_eq!(fs::read(&path).expect("файл читается"), bytes);
}

#[test]
fn save_audio_wav_uses_wav_for_both_content_types() {
    let dir = TempDir::new().expect("tempdir создаётся");
    for content_type in ["audio/wav", "audio/x-wav"] {
        let path = save_media(dir.path(), MediaKind::Audio, 0, 1, b"wav", content_type)
            .expect("аудио сохраняется");
        assert_file_name(&path, "audio", None, "wav");
    }
}

#[test]
fn save_unknown_content_type_falls_back_to_bin() {
    let dir = TempDir::new().expect("tempdir создаётся");
    let path = save_media(
        dir.path(),
        MediaKind::Image,
        0,
        1,
        b"raw",
        "application/octet-stream",
    )
    .expect("неизвестный тип сохраняется как bin");
    assert_file_name(&path, "image", None, "bin");
    assert_eq!(fs::read(&path).expect("файл читается"), b"raw");
}

#[test]
fn save_creates_missing_folder() {
    let dir = TempDir::new().expect("tempdir создаётся");
    let folder = dir.path().join("nested").join("media");
    assert!(!folder.exists(), "папка изначально отсутствует");
    let path = save_media(&folder, MediaKind::Image, 0, 1, b"png", "image/png")
        .expect("папка создаётся при сохранении");
    assert!(folder.is_dir(), "папка создана: {}", folder.display());
    assert_eq!(path.parent(), Some(folder.as_path()));
    assert_file_name(&path, "image", None, "png");
    assert_eq!(fs::read(&path).expect("файл читается"), b"png");
}

#[test]
fn folder_recreated_after_removal_between_saves() {
    let dir = TempDir::new().expect("tempdir создаётся");
    let folder = dir.path().join("media");
    let first = save_media(&folder, MediaKind::Image, 0, 1, b"one", "image/png")
        .expect("первое сохранение");
    fs::remove_dir_all(&folder).expect("папка удаляется");
    assert!(!folder.exists(), "папка исчезла между сохранениями");

    // Другой вид медиа — имена заведомо не совпадут даже в ту же миллисекунду.
    let second = save_media(&folder, MediaKind::Audio, 0, 1, b"two", "audio/mpeg")
        .expect("папка пересоздаётся, файл сохраняется");
    assert!(folder.is_dir(), "папка пересоздана");
    assert!(!first.exists(), "старый файл удалён вместе с папкой");
    assert_eq!(fs::read(&second).expect("файл читается"), b"two");
}

#[test]
fn file_in_place_of_folder_is_io_error_not_panic() {
    let dir = TempDir::new().expect("tempdir создаётся");
    let occupied = dir.path().join("media");
    fs::write(&occupied, b"not a folder").expect("файл-заглушка создаётся");

    let err = ensure_folder(&occupied).expect_err("путь занят файлом — ошибка");
    assert!(
        matches!(err, CoreError::Io(_)),
        "ожидался CoreError::Io, получен {err:?}"
    );
    assert!(
        err.to_string().contains("media"),
        "ошибка должна указывать на проблемный путь: {err}"
    );

    let err = save_media(&occupied, MediaKind::Image, 0, 1, b"data", "image/png")
        .expect_err("сохранение в путь-файл — ошибка");
    assert!(
        matches!(err, CoreError::Io(_)),
        "ожидался CoreError::Io, получен {err:?}"
    );
}

#[test]
fn ensure_folder_keeps_existing_directory() {
    let dir = TempDir::new().expect("tempdir создаётся");
    let folder = dir.path().join("media");
    fs::create_dir(&folder).expect("папка создаётся");
    fs::write(folder.join("keep.txt"), b"keep me").expect("файл создаётся");

    ensure_folder(&folder).expect("существующая папка допустима");
    ensure_folder(&folder).expect("повторный вызов идемпотентен");
    assert_eq!(
        fs::read(folder.join("keep.txt")).expect("файл читается"),
        b"keep me",
        "содержимое папки не изменяется"
    );
}

#[test]
fn ext_for_maps_known_content_types_and_falls_back_to_bin() {
    let cases = [
        ("image/png", "png"),
        ("image/jpeg", "jpg"),
        ("image/webp", "webp"),
        ("image/gif", "gif"),
        ("video/mp4", "mp4"),
        ("audio/mpeg", "mp3"),
        ("audio/wav", "wav"),
        ("audio/x-wav", "wav"),
        ("application/octet-stream", "bin"),
        ("text/plain", "bin"),
        ("", "bin"),
    ];
    for (content_type, expected) in cases {
        assert_eq!(
            ext_for(content_type),
            expected,
            "content_type={content_type}"
        );
    }
}
