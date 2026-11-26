//! Чтение выбранного файла завершённой серии изображений для повторного использования в форме.
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use km_core::Storage;
use km_shared::{MAX_REQUEST_WIRE_BYTES, image::raster_mime};
use tauri::State;

use crate::{commands::recorded_media_path, state::AppState};

fn read_reference(storage: &Storage, id: &str, index: usize) -> Result<String, String> {
    let path = storage
        .completed_image_path(id, index)
        .map_err(|e| e.to_string())?
        .ok_or("gen.reference_missing")?;
    let target = recorded_media_path(Path::new(&path)).ok_or("gen.reference_missing")?;
    let file = File::open(target).map_err(|e| e.to_string())?;
    let limit = MAX_REQUEST_WIRE_BYTES;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("controls.edit_wire_limit".into());
    }
    let mime = raster_mime(&bytes)
        .filter(|mime| matches!(*mime, "image/png" | "image/jpeg" | "image/webp"))
        .ok_or("gen.reference_format")?;
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
}

#[tauri::command]
pub async fn result_reference(
    state: State<'_, Arc<AppState>>,
    id: String,
    index: usize,
) -> Result<String, String> {
    let state = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || read_reference(&state.storage, &id, index))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use km_shared::{GenMode, HistoryRecord, JobStatus, MediaKind, Provider};

    #[test]
    fn only_completed_indexed_images_can_be_attachments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.png");
        let encoded = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
        std::fs::write(&path, STANDARD.decode(encoded).unwrap()).unwrap();
        let storage = Storage::open_in_memory().unwrap();
        let mut row = HistoryRecord {
            id: "ok".into(),
            provider: Provider::CheaperInference,
            created_at: 1,
            updated_at: 1,
            kind: MediaKind::Image,
            mode: GenMode::TextToImage,
            model: "test".into(),
            prompt: "fixture".into(),
            status: JobStatus::Completed,
            request_id: None,
            local_paths: vec![path.to_str().unwrap().to_owned()],
            cost_usd: None,
            params_json: "{}".into(),
            error: None,
        };
        storage.insert_history(&row).unwrap();
        assert_eq!(
            read_reference(&storage, "ok", 0).unwrap(),
            format!("data:image/png;base64,{encoded}")
        );
        assert!(read_reference(&storage, "ok", 1).is_err());
        assert!(read_reference(&storage, path.to_str().unwrap(), 0).is_err());
        row.id = "video".into();
        row.kind = MediaKind::Video;
        storage.insert_history(&row).unwrap();
        assert!(read_reference(&storage, "video", 0).is_err());
        row.id = "pending".into();
        row.kind = MediaKind::Image;
        row.status = JobStatus::Pending;
        storage.insert_history(&row).unwrap();
        assert!(read_reference(&storage, "pending", 0).is_err());
        std::fs::write(&path, b"plain text").unwrap();
        assert!(read_reference(&storage, "ok", 0).is_err());
        File::create(&path)
            .unwrap()
            .set_len(MAX_REQUEST_WIRE_BYTES as u64 + 1)
            .unwrap();
        assert!(read_reference(&storage, "ok", 0).is_err());
    }
}
