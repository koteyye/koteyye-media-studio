//! Интеграционные тесты словарей i18n: pinned-значения, fallback, покрытие экранов.

use km_shared::Lang::{En, Ru};
use km_ui_core::i18n::tr;

/// Ключи всех экранов Плана 2/3 — словарь обязан покрыть каждый.
const SCREEN_KEYS: &[&str] = &[
    // common
    "common.ok",
    "common.cancel",
    "common.save",
    "common.delete",
    "common.error",
    "common.loading",
    "common.generate",
    "common.retry",
    "common.open_folder",
    "common.recent",
    "common.balance",
    "common.spend",
    "common.refresh",
    "common.close",
    // studio
    "studio.title",
    "studio.image",
    "studio.video",
    "studio.audio",
    "studio.prompt",
    "studio.model",
    "studio.size",
    "studio.ratio",
    "studio.seed",
    "studio.duration",
    "studio.resolution",
    "studio.audio_on",
    "studio.voice",
    "studio.speed",
    "studio.format",
    "studio.mode.t2i",
    "studio.mode.i2i",
    "studio.mode.t2v",
    "studio.mode.i2v",
    "studio.mode.r2v",
    "studio.mode.speech",
    "studio.mode.music",
    "studio.mode.sfx",
    // history
    "history.title",
    "history.search",
    "history.all",
    "history.empty",
    "history.repeat",
    // settings
    "settings.title",
    "settings.api_key",
    "settings.check_connection",
    "settings.folder",
    "settings.language",
    "settings.theme",
    "settings.dark",
    "settings.light",
    "settings.system",
    "settings.show_real_balance",
    "settings.key_missing",
    "settings.keychain_error",
    "settings.connection_ok",
    "settings.connection_failed",
    "settings.connection_rejected",
    "settings.connection_unverified",
    "settings.checking",
    "settings.pick_folder",
    "settings.folder_missing",
    "settings.available",
    // onboarding
    "onboarding.welcome",
    "onboarding.step_key",
    "onboarding.step_folder",
    "onboarding.done",
    "onboarding.skip",
];

#[test]
fn settings_title_pinned() {
    assert_eq!(tr(Ru, "settings.title"), "Настройки");
    assert_eq!(tr(En, "settings.title"), "Settings");
}

#[test]
fn spot_values_pinned() {
    assert_eq!(tr(Ru, "common.ok"), "ОК");
    assert_eq!(tr(En, "common.ok"), "OK");
    assert_eq!(tr(Ru, "common.cancel"), "Отмена");
    assert_eq!(tr(En, "common.cancel"), "Cancel");
    assert_eq!(tr(Ru, "studio.title"), "Студия");
    assert_eq!(tr(En, "studio.title"), "Studio");
    assert_eq!(tr(Ru, "history.title"), "История");
    assert_eq!(tr(En, "history.title"), "History");
    assert_eq!(tr(Ru, "onboarding.welcome"), "Добро пожаловать");
    assert_eq!(tr(En, "onboarding.welcome"), "Welcome");
}

#[test]
fn unknown_key_falls_back_to_key() {
    assert_eq!(tr(En, "no.such.key"), "no.such.key");
    assert_eq!(tr(Ru, "no.such.key"), "no.such.key");
    assert_eq!(tr(Ru, ""), "");
}

#[test]
fn all_screen_keys_covered_in_both_langs() {
    for &key in SCREEN_KEYS {
        assert_ne!(tr(Ru, key), key, "нет ru-перевода для {key}");
        assert_ne!(tr(En, key), key, "нет en-перевода для {key}");
    }
}

#[test]
fn ru_and_en_differ_for_translated_keys() {
    // Переводы должны быть реальными переводами, а не копиями en.
    // Исключения (seed) в список не входят.
    for &key in SCREEN_KEYS {
        if key == "studio.seed" {
            continue;
        }
        assert_ne!(tr(Ru, key), tr(En, key), "ru/en совпадают для {key}");
    }
}
