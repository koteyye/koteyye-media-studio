//! Интеграционные тесты SQLite-хранилища: история, расход, настройки.
//!
//! Проверяются на реальной SQLite (in-memory и файл через tempfile), без моков.

use km_core::{CoreError, Storage};
use km_shared::{
    GenMode, HistoryFilter, HistoryPatch, HistoryRecord, JobStatus, LangPreference, MediaKind,
    Provider, Settings, Theme,
};
use tempfile::TempDir;

/// Полностью заполненная запись истории: все 13 полей несут значения,
/// чтобы round-trip через SQLite проверял каждое поле.
fn record(
    id: &str,
    created_at: i64,
    kind: MediaKind,
    mode: GenMode,
    status: JobStatus,
    prompt: &str,
) -> HistoryRecord {
    HistoryRecord {
        provider: Provider::CheaperInference,
        id: id.to_owned(),
        created_at,
        updated_at: created_at,
        kind,
        mode,
        model: format!("model-{id}"),
        prompt: prompt.to_owned(),
        status,
        request_id: Some(format!("req-{id}")),
        local_paths: vec![format!("/media/{id}.bin")],
        cost_usd: Some(0.04),
        params_json: format!(r#"{{"id":"{id}"}}"#),
        error: if status == JobStatus::Failed {
            Some(format!("error-{id}"))
        } else {
            None
        },
    }
}

/// Пустой фильтр: вся история без ограничений.
fn all() -> HistoryFilter {
    HistoryFilter::default()
}

#[test]
fn insert_and_history_round_trip_newest_first() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    let older = record(
        "a",
        1000,
        MediaKind::Image,
        GenMode::TextToImage,
        JobStatus::Completed,
        "кот в сапогах",
    );
    let newer = record(
        "b",
        3000,
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Pending,
        "собака",
    );
    let middle = record(
        "c",
        2000,
        MediaKind::Audio,
        GenMode::Speech,
        JobStatus::Failed,
        "попугай",
    );

    // Вставляем в перемешанном порядке — сортировка должна быть по created_at.
    storage.insert_history(&older).expect("insert older");
    storage.insert_history(&newer).expect("insert newer");
    storage.insert_history(&middle).expect("insert middle");

    let got = storage.history(&all()).expect("history должна прочитаться");
    assert_eq!(
        got,
        vec![newer, middle, older],
        "новые первыми и полный round-trip всех полей"
    );
}

#[test]
fn update_history_applies_patch_and_bumps_updated_at() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    let mut rec = record(
        "u",
        1000,
        MediaKind::Image,
        GenMode::TextToImage,
        JobStatus::Pending,
        "промпт",
    );
    rec.cost_usd = None;
    rec.local_paths = vec![];
    rec.request_id = None;
    storage.insert_history(&rec).expect("insert");

    let patch = HistoryPatch {
        status: Some(JobStatus::Completed),
        local_paths: Some(vec!["/saved/u.png".to_owned()]),
        cost_usd: Some(0.04),
        ..HistoryPatch::default()
    };
    storage.update_history("u", &patch).expect("update");

    let got = storage.history(&all()).expect("history");
    assert_eq!(got.len(), 1, "запись одна");
    let got = &got[0];
    assert_eq!(got.status, JobStatus::Completed, "статус обновился");
    assert_eq!(got.local_paths, vec!["/saved/u.png"], "пути обновились");
    assert_eq!(got.cost_usd, Some(0.04), "стоимость обновилась");
    assert!(got.updated_at > rec.updated_at, "updated_at должен вырасти");
    // Незаданные патчем поля не меняются.
    assert_eq!(got.created_at, rec.created_at, "created_at не меняется");
    assert_eq!(got.prompt, rec.prompt, "prompt не меняется");
    assert_eq!(got.model, rec.model, "model не меняется");
    assert_eq!(got.params_json, rec.params_json, "params не меняются");
    assert_eq!(got.request_id, None, "request_id не задан патчем");
}

#[test]
fn empty_patch_only_bumps_updated_at() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    let rec = record(
        "e",
        1000,
        MediaKind::Image,
        GenMode::TextToImage,
        JobStatus::Pending,
        "промпт",
    );
    storage.insert_history(&rec).expect("insert");

    storage
        .update_history("e", &HistoryPatch::default())
        .expect("пустой патч допустим");

    let got = storage.history(&all()).expect("history");
    assert_eq!(got.len(), 1);
    let got = &got[0];
    assert!(got.updated_at > rec.updated_at, "updated_at вырос");
    assert_eq!(got.status, rec.status, "статус не изменился");
    assert_eq!(got.cost_usd, rec.cost_usd, "стоимость не изменилась");
    assert_eq!(got.local_paths, rec.local_paths, "пути не изменились");
}

#[test]
fn pending_returns_only_pending() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    let pending = record(
        "p",
        3000,
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Pending,
        "жду",
    );
    let completed = record(
        "c",
        2000,
        MediaKind::Image,
        GenMode::TextToImage,
        JobStatus::Completed,
        "готово",
    );
    let failed = record(
        "f",
        1000,
        MediaKind::Audio,
        GenMode::Speech,
        JobStatus::Failed,
        "ошибка",
    );
    storage.insert_history(&completed).expect("insert c");
    storage.insert_history(&pending).expect("insert p");
    storage.insert_history(&failed).expect("insert f");

    let got = storage.pending().expect("pending");
    assert_eq!(got.len(), 1, "только одна Pending-запись");
    assert_eq!(got[0].id, "p");
    assert_eq!(got[0].status, JobStatus::Pending);
}

#[test]
fn total_spend_sums_costs_ignoring_none() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    let mut a = record(
        "a",
        1000,
        MediaKind::Image,
        GenMode::TextToImage,
        JobStatus::Completed,
        "a",
    );
    a.cost_usd = Some(0.04);
    let mut b = record(
        "b",
        2000,
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Completed,
        "b",
    );
    b.cost_usd = Some(0.78);
    let mut c = record(
        "c",
        3000,
        MediaKind::Audio,
        GenMode::Speech,
        JobStatus::Completed,
        "c",
    );
    c.cost_usd = None;
    storage.insert_history(&a).expect("insert a");
    storage.insert_history(&b).expect("insert b");
    storage.insert_history(&c).expect("insert c");

    let total = storage
        .total_spend_usd(Provider::CheaperInference)
        .expect("сумма расхода");
    assert!(
        (total - 0.82).abs() < 1e-9,
        "0.04 + 0.78 + None = 0.82, получено {total}"
    );
}

#[test]
fn total_spend_empty_db_is_zero() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    assert_eq!(
        storage
            .total_spend_usd(Provider::CheaperInference)
            .expect("сумма"),
        0.0
    );
}

#[test]
fn non_finite_cost_is_excluded_from_spend() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    for (id, cost) in [
        ("fin", Some(0.04)),
        ("nan", Some(f64::NAN)),
        ("inf", Some(f64::INFINITY)),
        ("neg-inf", Some(f64::NEG_INFINITY)),
        ("none", None),
        ("fin2", Some(0.78)),
    ] {
        let mut rec = record(
            id,
            1000,
            MediaKind::Image,
            GenMode::TextToImage,
            JobStatus::Completed,
            id,
        );
        rec.cost_usd = cost;
        storage.insert_history(&rec).expect("insert");
    }

    let total = storage
        .total_spend_usd(Provider::CheaperInference)
        .expect("сумма расхода");
    assert!(
        (total - 0.82).abs() < 1e-9,
        "non-finite стоимости исключены, ожидалось 0.82, получено {total}"
    );

    // Неконечные стоимости не сохраняются как есть — читаются как None.
    let got = storage.history(&all()).expect("history");
    for rec in got {
        if rec.id == "nan" || rec.id == "inf" || rec.id == "neg-inf" {
            assert_eq!(
                rec.cost_usd, None,
                "non-finite стоимость отброшена: {}",
                rec.id
            );
        }
    }
}

#[test]
fn filter_kind_returns_only_that_kind() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    storage
        .insert_history(&record(
            "i",
            1000,
            MediaKind::Image,
            GenMode::TextToImage,
            JobStatus::Completed,
            "img",
        ))
        .expect("insert image");
    storage
        .insert_history(&record(
            "v",
            2000,
            MediaKind::Video,
            GenMode::TextToVideo,
            JobStatus::Completed,
            "vid",
        ))
        .expect("insert video");
    storage
        .insert_history(&record(
            "a",
            3000,
            MediaKind::Audio,
            GenMode::Speech,
            JobStatus::Completed,
            "aud",
        ))
        .expect("insert audio");

    let filter = HistoryFilter {
        kind: Some(MediaKind::Video),
        ..HistoryFilter::default()
    };
    let got = storage.history(&filter).expect("history с фильтром");
    assert_eq!(got.len(), 1, "только видео");
    assert_eq!(got[0].kind, MediaKind::Video);
    assert_eq!(got[0].id, "v");
}

#[test]
fn search_matches_prompt_case_insensitive_cyrillic() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    storage
        .insert_history(&record(
            "cat",
            1000,
            MediaKind::Image,
            GenMode::TextToImage,
            JobStatus::Completed,
            "Кот в сапогах",
        ))
        .expect("insert cat");
    storage
        .insert_history(&record(
            "dog",
            2000,
            MediaKind::Image,
            GenMode::TextToImage,
            JobStatus::Completed,
            "собака",
        ))
        .expect("insert dog");

    for query in ["кот", "КОТ", "Кот"] {
        let filter = HistoryFilter {
            search: Some(query.to_owned()),
            ..HistoryFilter::default()
        };
        let got = storage.history(&filter).expect("history с поиском");
        assert_eq!(got.len(), 1, "поиск {query:?} должен найти кота");
        assert_eq!(got[0].id, "cat");
    }
}

#[test]
fn search_treats_like_wildcards_literally() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    storage
        .insert_history(&record(
            "sale",
            1000,
            MediaKind::Image,
            GenMode::TextToImage,
            JobStatus::Completed,
            "скидка 100% сегодня",
        ))
        .expect("insert sale");
    storage
        .insert_history(&record(
            "plain",
            2000,
            MediaKind::Image,
            GenMode::TextToImage,
            JobStatus::Completed,
            "обычный текст",
        ))
        .expect("insert plain");

    let filter = HistoryFilter {
        search: Some("100%".to_owned()),
        ..HistoryFilter::default()
    };
    let got = storage.history(&filter).expect("history с поиском");
    assert_eq!(got.len(), 1, "% в запросе — литерал, не маска");
    assert_eq!(got[0].id, "sale");
}

#[test]
fn delete_history_removes_record() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    let keep = record(
        "keep",
        1000,
        MediaKind::Image,
        GenMode::TextToImage,
        JobStatus::Completed,
        "keep",
    );
    let drop = record(
        "drop",
        2000,
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Completed,
        "drop",
    );
    storage.insert_history(&keep).expect("insert keep");
    storage.insert_history(&drop).expect("insert drop");

    storage.delete_history("drop").expect("delete");
    let got = storage.history(&all()).expect("history");
    assert_eq!(got, vec![keep], "удалённая запись исчезла");

    // Повторное удаление несуществующей записи не падает.
    storage.delete_history("drop").expect("повторный delete");
}

#[test]
fn insert_duplicate_id_fails() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    let rec = record(
        "dup",
        1000,
        MediaKind::Image,
        GenMode::TextToImage,
        JobStatus::Pending,
        "dup",
    );
    storage.insert_history(&rec).expect("первый insert");
    let err = storage
        .insert_history(&rec)
        .expect_err("дубликат id должен нарушить PRIMARY KEY");
    assert!(
        matches!(err, CoreError::Db(_)),
        "ошибка БД, получено {err:?}"
    );
}

#[test]
fn update_missing_id_is_noop() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    let patch = HistoryPatch {
        status: Some(JobStatus::Completed),
        ..HistoryPatch::default()
    };
    storage
        .update_history("ghost", &patch)
        .expect("update несуществующей записи не падает");
    assert!(storage.history(&all()).expect("history").is_empty());
}

#[test]
fn settings_round_trip_and_overwrite() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    let first = Settings {
        local_folder: "/media".to_owned(),
        lang: LangPreference::En,
        theme: Theme::Dark,
        show_real_balance: true,
        ..Settings::default()
    };
    storage.set_settings(&first).expect("set settings");
    assert_eq!(storage.get_settings().expect("get settings"), first);

    let second = Settings {
        local_folder: String::new(),
        lang: LangPreference::Ru,
        theme: Theme::Light,
        show_real_balance: false,
        ..Settings::default()
    };
    storage.set_settings(&second).expect("перезапись настроек");
    assert_eq!(
        storage.get_settings().expect("get settings"),
        second,
        "настройки перезаписаны одной строкой"
    );
}

#[test]
fn settings_default_when_empty() {
    let storage = Storage::open_in_memory().expect("in-memory БД должна открыться");
    let got = storage.get_settings().expect("дефолтные настройки");
    assert_eq!(got.lang, LangPreference::System, "язык ОС по умолчанию");
    assert_eq!(
        got.theme,
        Theme::Light,
        "новая база использует светлую тему"
    );
    assert_eq!(got.local_folder, "", "папка по умолчанию пустая");
    assert!(!got.show_real_balance, "реальный баланс по умолчанию скрыт");
}

#[test]
fn open_persists_history_and_settings_across_reopen() {
    let dir = TempDir::new().expect("временная директория");
    let path = dir.path().join("history.db");
    let rec = record(
        "persist",
        1000,
        MediaKind::Video,
        GenMode::TextToVideo,
        JobStatus::Completed,
        "сохранено",
    );
    let settings = Settings {
        local_folder: "/saved".to_owned(),
        lang: LangPreference::En,
        theme: Theme::Dark,
        show_real_balance: true,
        ..Settings::default()
    };
    {
        let storage = Storage::open(&path).expect("open файла БД");
        storage.insert_history(&rec).expect("insert");
        storage.set_settings(&settings).expect("set settings");
    }

    let storage = Storage::open(&path).expect("повторное open того же файла");
    assert_eq!(
        storage.history(&all()).expect("history"),
        vec![rec],
        "история пережила переоткрытие"
    );
    assert_eq!(
        storage.get_settings().expect("settings"),
        settings,
        "настройки пережили переоткрытие"
    );
}

#[test]
fn upgrades_an_existing_database_without_changing_jobs_or_preferences() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("legacy.db");
    let legacy = rusqlite::Connection::open(&path).unwrap();
    legacy
        .execute_batch(
            "
        CREATE TABLE history (
            id TEXT PRIMARY KEY, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
            kind TEXT NOT NULL, mode TEXT NOT NULL, model TEXT NOT NULL, prompt TEXT NOT NULL,
            status TEXT NOT NULL, request_id TEXT, local_paths TEXT NOT NULL, cost_usd REAL,
            params_json TEXT NOT NULL, error TEXT);
        CREATE TABLE settings(id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL);
        INSERT INTO history VALUES ('old',100,101,'video','text_to_video','veo','old prompt',
            'pending','receipt-7','[]',0.36,'{}',NULL);
    ",
        )
        .unwrap();
    let old_settings = serde_json::json!({"local_folder":"C:/legacy","lang":"en","theme":"dark","show_real_balance":true});
    legacy
        .execute(
            "INSERT INTO settings VALUES(1, ?1)",
            [old_settings.to_string()],
        )
        .unwrap();
    drop(legacy);
    for _ in 0..2 {
        let s = Storage::open(&path).unwrap();
        let old = s.pending().unwrap().remove(0);
        assert_eq!(old.provider, Provider::CheaperInference);
        assert_eq!(old.id, "old");
        assert_eq!(old.request_id.as_deref(), Some("receipt-7"));
        assert_eq!(old.cost_usd, Some(0.36));
        assert_eq!(s.total_spend_usd(Provider::CheaperInference).unwrap(), 0.36);
        assert_eq!(s.total_spend_usd(Provider::OpenRouter).unwrap(), 0.0);
        assert_eq!(old.params_json, "{}");
        assert_eq!(old.updated_at, 101);
        let mut old_ipc = serde_json::to_value(&old).unwrap();
        old_ipc.as_object_mut().unwrap().remove("provider");
        assert_eq!(
            serde_json::from_value::<HistoryRecord>(old_ipc).unwrap(),
            old
        );
        let got = s.get_settings().unwrap();
        assert_eq!(got.local_folder, "C:/legacy");
        assert_eq!(got.theme, Theme::Dark);
        assert_eq!(got.lang, LangPreference::En);
        assert!(got.show_real_balance);
        assert_eq!(got.provider, Provider::CheaperInference);
    }
}

#[test]
fn old_settings_ipc_preserves_the_selected_provider_and_workspace() {
    let s = Storage::open_in_memory().unwrap();
    let mut prefs = Settings {
        provider: Provider::OpenRouter,
        ..Settings::default()
    };
    prefs.workspace.compact = true;
    prefs.workspace.recent_count = 8;
    s.set_settings(&prefs).unwrap();
    let payload = serde_json::json!({"local_folder":"D:/renders","theme":"system","lang":"en","show_real_balance":true});
    s.merge_settings(payload.as_object().unwrap().clone())
        .unwrap();
    let got = s.get_settings().unwrap();
    assert_eq!(got.local_folder, "D:/renders");
    assert_eq!(got.theme, Theme::System);
    assert_eq!(got.provider, Provider::OpenRouter);
    assert_eq!(got.workspace, prefs.workspace);
    let bad = serde_json::json!({"provider":"unknown","local_folder":"wrong"});
    assert!(s.merge_settings(bad.as_object().unwrap().clone()).is_err());
    assert_eq!(s.get_settings().unwrap(), got);
}
