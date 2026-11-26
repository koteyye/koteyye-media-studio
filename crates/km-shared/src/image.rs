//! Ограничения шлюза CI и формата входных изображений для /v1/images/edits.

pub const MAX_EDIT_SOURCES: usize = 10;
pub const MAX_EDIT_DECODED_BYTES: usize = 20 * 1024 * 1024;

/// Тип растра по заголовку файла. У b64_json поле content_type необязательно.
pub fn raster_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}

/// Размер вложенных байтов для канонического data-URL FileReader, без выделения памяти.
/// У внешней ссылки нет байтов в теле запроса.
pub fn inline_bytes(reference: &str) -> usize {
    reference
        .strip_prefix("data:")
        .and_then(|url| url.split_once(','))
        .map_or(0, |(_, b64)| {
            let padding = if b64.ends_with("==") {
                2
            } else {
                usize::from(b64.ends_with('='))
            };
            (b64.len() / 4 * 3).saturating_sub(padding)
        })
}
