//! Ручная проверка команд студии с сохранённым ключом. Только GET каталога.
//! Для запуска: cargo test -p koteyye-media-studio --test live_catalog -- --ignored --nocapture

use km_shared::MediaKind;
use koteyye_media_studio::{
    commands::{cmd_fetch_models, cmd_test_connection},
    keychain::KeychainStore,
    state::AppState,
};
use std::sync::Arc;

#[tokio::test]
#[ignore = "нужны сеть и действующий ключ в системном хранилище студии"]
async fn saved_key_and_live_model_lists() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(
        &dir.path().join("catalog-test.db"),
        Box::new(KeychainStore::new().unwrap()),
        "https://api.cheaperinference.com",
        Arc::new(|_| {}),
    )
    .unwrap();
    assert!(
        cmd_test_connection(&state).await.unwrap(),
        "требуется действующий сохранённый ключ"
    );
    for kind in [MediaKind::Image, MediaKind::Video, MediaKind::Audio] {
        let models = cmd_fetch_models(&state, kind).await.unwrap();
        assert!(models.iter().all(|m| m.matches_kind(kind)));
        println!("{kind:?}: {}", models.len());
    }
}
