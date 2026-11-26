//! Tauri-команды поверх ядра: чистая логика `cmd_*` и тонкие обёртки.
//!
//! Логика вынесена в `cmd_*(state: &AppState, ..) -> Result<_, String>` без
//! типов Tauri — тестируется с wiremock, in-memory ключом и tempdir-БД
//! (`tests/commands.rs`). Обёртки `#[tauri::command]` только достают State
//! и зовут `cmd_*`; ошибки ядра уже переведены в строки для UI.
//!
//! События: [`event_name`] задаёт имена `generation:started/done/failed`
//! и `balance:updated`; payload — serde [`AppEvent`] (externally tagged,
//! snake_case). `balance:updated` шлётся после успешной генерации или
//! восстановления — локальный расход изменился.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use km_core::{CoreError, by_kind};
use km_shared::{
    AppEvent, BalanceUnavailable, BalanceView, CatalogModel, GenerationRequest, HistoryFilter,
    HistoryRecord, JobStatus, MediaKind, Provider, Settings,
};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::state::AppState;

/// Имя Tauri-события для события ядра; payload — serde [`AppEvent`].
#[must_use]
pub fn event_name(event: &AppEvent) -> &'static str {
    match event {
        AppEvent::GenerationStarted { .. } => "generation:started",
        AppEvent::GenerationDone { .. } => "generation:done",
        AppEvent::GenerationFailed { .. } => "generation:failed",
        AppEvent::BalanceUpdated => "balance:updated",
    }
}

/// Настройки приложения из SQLite.
///
/// # Errors
///
/// Строка с причиной, если хранилище не читается.
pub async fn cmd_get_settings(state: &AppState) -> Result<Settings, String> {
    state.storage.get_settings().map_err(|e| e.to_string())
}

/// Сохраняет настройки в SQLite (ключ API здесь не хранится).
///
/// # Errors
///
/// Строка с причиной, если запись не удалась.
pub async fn cmd_set_settings(state: &AppState, settings: Settings) -> Result<(), String> {
    let _writing = state
        .settings_write
        .lock()
        .map_err(|_| "не удалось заблокировать настройки")?;
    let prior = state
        .storage
        .get_settings()
        .map_err(|e| e.to_string())?
        .provider;
    state
        .storage
        .set_settings(&settings)
        .map_err(|e| e.to_string())?;
    state.provider_changed(prior, settings.provider);
    Ok(())
}

/// Сохраняет API-ключ в [`KeyStore`](crate::keychain::KeyStore) (keychain).
///
/// Ключ обрезается по краям; пустой ключ удаляет сохранённый.
///
/// # Errors
///
/// Строка с причиной, если keychain не принял ключ.
pub async fn cmd_set_api_key(state: &AppState, key: &str) -> Result<(), String> {
    cmd_set_provider_key(state, Provider::CheaperInference, key).await
}

pub async fn cmd_provider_has_key(state: &AppState, provider: Provider) -> Result<bool, String> {
    state.has_key(provider).await
}

pub async fn cmd_set_provider_key(
    state: &AppState,
    provider: Provider,
    key: &str,
) -> Result<(), String> {
    let key = key.trim();
    if let Some(other) = foreign_key(provider, key) {
        return Err(format!(
            "ключ {} не подходит профилю {}",
            other.name(),
            provider.name()
        ));
    }
    state.write_key(provider, key.to_owned()).await?;
    state.invalidate_balance(provider);
    Ok(())
}

/// У шлюзов могут быть и непрозрачные ключи. Отсекаем только узнаваемый ключ другого сервиса.
fn foreign_key(provider: Provider, key: &str) -> Option<Provider> {
    match provider {
        Provider::OpenRouter if key.starts_with("ci_live_") || key.starts_with("ir_live_") => {
            Some(Provider::CheaperInference)
        }
        Provider::CheaperInference if key.starts_with("sk-or-v1-") => Some(Provider::OpenRouter),
        _ => None,
    }
}

async fn stored_ci_key(state: &AppState) -> Result<Option<String>, String> {
    let key = state.read_key(Provider::CheaperInference).await?;
    if key
        .as_ref()
        .is_some_and(|k| foreign_key(Provider::CheaperInference, k.trim()).is_some())
    {
        return Err("в профиле Cheaper Inference сохранён ключ OpenRouter".into());
    }
    Ok(key)
}

/// Проверка подключения: лёгкий auth-запрос `GET /v1/models` с Bearer.
///
/// `true` — ключ принят (200), `false` — ключа нет (`Ok(None)` в keychain)
/// или CI ответил 401. Прочие ошибки (сеть, 5xx, сбой чтения keychain)
/// пробрасываются: подключение не проверено, редиректа в онбординг нет.
///
/// # Errors
///
/// Строка с причиной при ошибках, отличных от 401, и при сбое чтения keychain.
pub async fn cmd_test_connection(state: &AppState) -> Result<bool, String> {
    cmd_check_key(state, None).await
}

/// Проверяет введённый ключ без записи; пустое поле означает сохранённый ключ.
pub async fn cmd_check_key(state: &AppState, candidate: Option<&str>) -> Result<bool, String> {
    cmd_check_provider_key(state, Provider::CheaperInference, candidate).await
}

pub async fn cmd_check_provider_key(
    state: &AppState,
    provider: Provider,
    candidate: Option<&str>,
) -> Result<bool, String> {
    let key = match candidate.map(str::trim).filter(|s| !s.is_empty()) {
        Some(value) => value.to_owned(),
        None => match state
            .read_key(provider)
            .await?
            .filter(|key| !key.trim().is_empty())
        {
            Some(value) => value,
            None => return Ok(false),
        },
    };
    if foreign_key(provider, key.trim()).is_some() {
        return Ok(false);
    }
    if provider == Provider::OpenRouter {
        return state
            .openrouter
            .verify(&key)
            .await
            .map_err(|err| match err {
                CoreError::Timeout => "таймаут проверки ключа OpenRouter".into(),
                other => other.to_string(),
            });
    }
    match state.client(&key).verify_key().await {
        Ok(()) => Ok(true),
        Err(CoreError::Http { status: 401, .. }) => Ok(false),
        Err(e) => Err(e.to_string().replace(&key, "[скрыто]")),
    }
}

/// Каталог моделей по виду медиа.
///
/// С ключом — `GET /v1/models`; без ключа — публичный каталог
/// (`GET /public/models`) через кэш, чтобы UI показывал модели до подключения.
///
/// # Errors
///
/// Строка с причиной при сбое запроса, разбора ответа или чтения keychain.
pub async fn cmd_fetch_models(
    state: &AppState,
    kind: MediaKind,
) -> Result<Vec<CatalogModel>, String> {
    let settings = state.storage.get_settings().map_err(|e| e.to_string())?;
    if settings.provider == Provider::OpenRouter {
        return Ok(Vec::new());
    }
    let key = stored_ci_key(state).await?;
    let client = state.client(key.as_deref().unwrap_or_default());
    let models = match key {
        Some(_) => client.models_for_kind(kind).await,
        None => state.catalog().get_or_fetch(&client).await,
    }
    .map_err(|e| e.to_string())?;
    Ok(by_kind(&models, kind).into_iter().cloned().collect())
}

/// Запуск генерации: ключ из keychain, папка из настроек, [`km_core::Jobs`].
///
/// После успеха шлёт `balance:updated` — локальный расход изменился.
///
/// # Errors
///
/// [`CoreError::NoKey`], если ключ не задан; [`CoreError::Validation`], если
/// не задана папка сохранения; ошибки ядра при генерации: definitive
/// переводят запись истории в `Failed`, транзиентные после фиксации
/// `request_id` оставляют `Pending` (оплаченная генерация восстановима);
/// сбой чтения keychain пробрасывается до запуска.
pub async fn cmd_generate(
    state: &AppState,
    req: GenerationRequest,
) -> Result<HistoryRecord, String> {
    cmd_generate_for(state, Provider::CheaperInference, req).await
}

pub async fn cmd_generate_for(
    state: &AppState,
    provider: Provider,
    req: GenerationRequest,
) -> Result<HistoryRecord, String> {
    // Восстановление без request_id помечает запись как прерванную. Пока
    // идёт запрос, Pending без request_id — нормальное состояние живой задачи.
    let _running = state.recovery_lock().read().await;
    let settings = state.storage.get_settings().map_err(|e| e.to_string())?;
    if provider == Provider::OpenRouter || settings.provider == Provider::OpenRouter {
        return Err("Генерация через OpenRouter пока недоступна".into());
    }
    let key = stored_ci_key(state)
        .await?
        .ok_or_else(|| CoreError::NoKey.to_string())?;
    if settings.local_folder.is_empty() {
        return Err(
            CoreError::Validation("папка сохранения не задана в настройках".to_owned()).to_string(),
        );
    }
    let jobs = state.jobs(state.client(&key), PathBuf::from(settings.local_folder))?;
    let record = jobs.run(req).await.map_err(|e| e.to_string())?;
    state.emit(AppEvent::BalanceUpdated);
    Ok(record)
}

/// Список истории с фильтром, новые первыми.
///
/// # Errors
///
/// Строка с причиной, если хранилище не читается.
pub async fn cmd_list_history(
    state: &AppState,
    filter: HistoryFilter,
) -> Result<Vec<HistoryRecord>, String> {
    state.storage.history(&filter).map_err(|e| e.to_string())
}

/// Удаляет запись истории; файлы и remote не трогает.
///
/// # Errors
///
/// Строка с причиной, если удаление не удалось.
pub async fn cmd_delete_history(state: &AppState, id: &str) -> Result<(), String> {
    state.storage.delete_history(id).map_err(|e| e.to_string())
}

/// Восстанавливает `Pending`-записи, пережившие перезапуск.
///
/// Вызывается и при старте (фоном, из `setup`), и вручную из UI. Проходы
/// сериализованы локом [`AppState::recovery_lock`]: параллельные вызовы не
/// обрабатывают одни записи дважды (без дублей файлов и событий). После
/// восстановления завершённых записей шлёт `balance:updated`. Без ключа
/// CI не опросить, без папки сохранения некуда писать файлы: записи остаются
/// `Pending` до следующего запуска с ключом и папкой (оплаченная генерация
/// могла завершиться на сервере, файлы живут 7 дней).
///
/// # Errors
///
/// Строка с причиной при сбое хранилища или чтении ключа из keychain;
/// definitive-ошибки CI отдельных записей переводят их в `Failed`,
/// транзиентные (сеть, таймаут, ввод-вывод, БД, HTTP 5xx/429) оставляют
/// `Pending` — и те и другие не прерывают восстановление остальных.
pub async fn cmd_recover_pending(state: &AppState) -> Result<Vec<HistoryRecord>, String> {
    // Сериализация проходов: setup при старте и load() UI зовут эту команду;
    // без лока оба прочитали бы одни Pending и скачали бы файлы дважды.
    let _guard = state.recovery_lock().write().await;
    let settings = state.storage.get_settings().map_err(|e| e.to_string())?;
    if settings.local_folder.is_empty()
        || !state
            .storage
            .has_pending_for(Provider::CheaperInference)
            .map_err(|e| e.to_string())?
    {
        return Ok(Vec::new());
    }
    // Активный профиль здесь роли не играет: восстановление привязано к источнику записи.
    let Some(key) = stored_ci_key(state).await? else {
        // Без ключа запрос с пустым Bearer дал бы 401 и ядро сожгло бы
        // восстановимые записи в Failed навсегда — оставляем Pending.
        return Ok(Vec::new());
    };
    let jobs = state.jobs(state.client(&key), PathBuf::from(settings.local_folder))?;
    let recovered = jobs.recover_pending().await.map_err(|e| e.to_string())?;
    if recovered.iter().any(|r| r.status == JobStatus::Completed) {
        state.emit(AppEvent::BalanceUpdated);
    }
    Ok(recovered)
}

/// Реальный кошелёк, если включён в настройках, и локальный расход активного профиля.
///
/// # Errors
///
/// Ошибка БД либо смена профиля во время запроса. Ошибки удалённого
/// кошелька оставляют доступным подтверждённый локальный расход.
pub async fn cmd_fetch_balance(state: &AppState) -> Result<BalanceView, String> {
    let provider = state
        .storage
        .get_settings()
        .map_err(|e| e.to_string())?
        .provider;
    cmd_fetch_balance_for(state, provider).await
}

pub async fn cmd_fetch_balance_for(
    state: &AppState,
    provider: Provider,
) -> Result<BalanceView, String> {
    let revision = state.balance_revision(provider);
    let wallet_enabled = || -> Result<bool, String> {
        let prefs = state.storage.get_settings().map_err(|e| e.to_string())?;
        if prefs.provider != provider || state.balance_revision(provider) != revision {
            Err("Профиль баланса изменился во время запроса".into())
        } else {
            Ok(prefs.show_real_balance)
        }
    };
    let enabled = wallet_enabled()?;
    let local_spend = state
        .storage
        .total_spend_usd(provider)
        .map_err(|e| e.to_string())?;
    let local = BalanceView::local(provider, local_spend);
    if !enabled {
        return Ok(local);
    }
    let credential = state.read_key(provider).await;
    // Диалог системного keychain может завершиться спустя несколько секунд.
    if !wallet_enabled()? {
        return Ok(local);
    }
    let key = match credential {
        Ok(Some(key)) if !key.trim().is_empty() => key,
        Ok(_) => {
            return Ok(BalanceView {
                unavailable: Some(BalanceUnavailable::MissingKey),
                ..local
            });
        }
        Err(_) => {
            return Ok(BalanceView {
                unavailable: Some(BalanceUnavailable::KeyStore),
                ..local
            });
        }
    };
    let key = key.trim();
    if foreign_key(provider, key).is_some() {
        return Ok(BalanceView {
            unavailable: Some(BalanceUnavailable::RejectedKey),
            ..local
        });
    }
    let result = match provider {
        Provider::CheaperInference => state.client(key).balance_view(local_spend).await,
        Provider::OpenRouter => state.openrouter.balance_view(key, local_spend).await,
    };
    Ok(if wallet_enabled()? { result } else { local })
}

/// Проверяет, что путь внутри папки сохранения (защита как v1 `resolveSavedFilePath`).
///
/// Оба пути канонизируются (симлинки и `..` раскрываются), поэтому обход
/// через `folder/../secret` или симлинк наружу не проходит. Файл должен
/// существовать. Возвращает канонический путь для opener.
fn resolve_saved_path(state: &AppState, path: &str) -> Result<PathBuf, String> {
    if let Some(target) = recorded_media_path(Path::new(path))
        && state
            .storage
            .has_result_path(path)
            .map_err(|e| e.to_string())?
    {
        return Ok(target);
    }
    let settings = state.storage.get_settings().map_err(|e| e.to_string())?;
    if settings.local_folder.is_empty() {
        return Err("папка сохранения не задана в настройках".to_owned());
    }
    let folder = fs::canonicalize(&settings.local_folder)
        .map_err(|e| format!("папка сохранения недоступна: {e}"))?;
    let target = fs::canonicalize(path).map_err(|e| format!("файл недоступен: {e}"))?;
    if !target.starts_with(&folder) {
        return Err(format!("путь вне папки сохранения: {}", target.display()));
    }
    Ok(target)
}

/// Канонический путь к файлу внутри папки сохранения — для открытия в ОС.
///
/// # Errors
///
/// Строка с причиной, если папка не задана, файл не существует или путь
/// вне папки сохранения.
pub async fn cmd_open_file(state: &AppState, path: &str) -> Result<PathBuf, String> {
    resolve_saved_path(state, path)
}

/// Канонический путь к файлу внутри папки сохранения — для показа в папке.
///
/// # Errors
///
/// Те же случаи, что у [`cmd_open_file`].
pub async fn cmd_show_in_folder(state: &AppState, path: &str) -> Result<PathBuf, String> {
    resolve_saved_path(state, path)
}

// --- Тонкие обёртки #[tauri::command]: State -> cmd_* -> Result для UI ---

/// Настройки приложения.
#[tauri::command]
pub async fn get_settings(state: State<'_, Arc<AppState>>) -> Result<Settings, String> {
    cmd_get_settings(&state).await
}

/// Сохранить настройки.
#[tauri::command]
pub async fn set_settings(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    settings: serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let _writing = state
        .settings_write
        .lock()
        .map_err(|_| "не удалось заблокировать настройки")?;
    let prior = state
        .storage
        .get_settings()
        .map_err(|e| e.to_string())?
        .provider;
    let saved = state
        .storage
        .merge_settings(settings)
        .map_err(|e| e.to_string())?;
    state.provider_changed(prior, saved.provider);
    crate::apply_native_theme(&app, saved.theme);
    Ok(())
}

/// Сохранить API-ключ в системный keychain (пустой ключ удаляет).
#[tauri::command]
pub async fn set_api_key(state: State<'_, Arc<AppState>>, key: String) -> Result<(), String> {
    cmd_set_api_key(&state, &key).await
}

#[tauri::command]
pub async fn provider_has_key(
    state: State<'_, Arc<AppState>>,
    provider: Provider,
) -> Result<bool, String> {
    cmd_provider_has_key(&state, provider).await
}

#[tauri::command]
pub async fn set_provider_key(
    state: State<'_, Arc<AppState>>,
    provider: Provider,
    key: String,
) -> Result<(), String> {
    cmd_set_provider_key(&state, provider, &key).await
}

#[tauri::command]
pub async fn check_provider_key(
    state: State<'_, Arc<AppState>>,
    provider: Provider,
    key: Option<String>,
) -> Result<bool, String> {
    cmd_check_provider_key(&state, provider, key.as_deref()).await
}

/// Проверить подключение к Cheaper Inference.
#[tauri::command]
pub async fn test_connection(
    state: State<'_, Arc<AppState>>,
    key: Option<String>,
) -> Result<bool, String> {
    cmd_check_key(&state, key.as_deref()).await
}

/// Каталог моделей по виду медиа (`image`/`video`/`audio`).
#[tauri::command]
pub async fn fetch_models(
    state: State<'_, Arc<AppState>>,
    kind: MediaKind,
) -> Result<Vec<CatalogModel>, String> {
    cmd_fetch_models(&state, kind).await
}

/// Запустить генерацию.
#[tauri::command]
pub async fn generate(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    req: GenerationRequest,
    provider: Option<Provider>,
) -> Result<HistoryRecord, String> {
    let record = cmd_generate_for(&state, provider.unwrap_or_default(), req).await?;
    allow_result_previews(&app, std::slice::from_ref(&record));
    Ok(record)
}

/// Список истории с фильтром.
#[tauri::command]
pub async fn list_history(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    filter: HistoryFilter,
) -> Result<Vec<HistoryRecord>, String> {
    let records = cmd_list_history(&state, filter).await?;
    allow_result_previews(&app, &records);
    Ok(records)
}

/// Разрешаем webview читать только конкретные медиафайлы, сохранённые ядром.
/// Удалённый файл или заменившая его ссылка не должны ломать остальную историю.
fn allow_result_previews(app: &AppHandle, records: &[HistoryRecord]) {
    let scope = app.asset_protocol_scope();
    for path in records.iter().flat_map(|r| &r.local_paths) {
        if let Some(target) = recorded_media_path(Path::new(path))
            && let Err(e) = scope.allow_file(target)
        {
            eprintln!("не удалось разрешить предпросмотр: {e}");
        }
    }
}

pub(crate) fn recorded_media_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute()
        || !matches!(
            path.extension()?.to_str()?,
            "jpg"
                | "jpeg"
                | "png"
                | "webp"
                | "gif"
                | "mp4"
                | "mp3"
                | "wav"
                | "ogg"
                | "opus"
                | "aac"
                | "flac"
                | "m4a"
                | "pcm"
        )
    {
        return None;
    }
    let canonical = dunce::canonicalize(path).ok()?;
    // Не следуем по подменённым симлинкам ни файла, ни родительских папок.
    (canonical == dunce::simplified(path) && canonical.is_file()).then_some(canonical)
}

/// Удалить запись истории.
#[tauri::command]
pub async fn delete_history(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    cmd_delete_history(&state, &id).await
}

/// Восстановить незавершённые генерации.
#[tauri::command]
pub async fn recover_pending(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<HistoryRecord>, String> {
    cmd_recover_pending(&state).await
}

/// Гибридный баланс: реальный из CI плюс локальный расход.
#[tauri::command]
pub async fn fetch_balance(
    state: State<'_, Arc<AppState>>,
    provider: Option<Provider>,
) -> Result<BalanceView, String> {
    match provider {
        Some(provider) => cmd_fetch_balance_for(&state, provider).await,
        None => cmd_fetch_balance(&state).await,
    }
}

/// Выбрать папку сохранения в нативном диалоге; `None` — пользователь отменил.
///
/// Диалог исполняется на главном потоке (внутри плагина), ожидание —
/// в блокирующем пуле, чтобы не занимать IPC-поток.
#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    let folder =
        tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
            .await
            .map_err(|e| format!("диалог выбора папки не завершился: {e}"))?;
    Ok(folder.map(|path| path.to_string()))
}

/// Открыть файл в приложении ОС по умолчанию; путь только внутри папки сохранения.
#[tauri::command]
pub async fn open_file(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<(), String> {
    let target = cmd_open_file(&state, &path).await?;
    // Windows: canonicalize даёт verbatim-путь (\\?\), а open_path ведёт в
    // ShellExecuteExW, который такие пути не принимает; dunce снимает префикс,
    // когда это безопасно. На macOS/Linux преобразование тождественно.
    let target = dunce::simplified(&target);
    app.opener()
        .open_path(target.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Показать файл в папке (Finder/Explorer); путь только внутри папки сохранения.
#[tauri::command]
pub async fn show_in_folder(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<(), String> {
    let target = cmd_show_in_folder(&state, &path).await?;
    // reveal_item_in_dir на Windows сам упрощает путь (dunce + GetFullPathNameW),
    // поэтому verbatim-префикс canonicalize здесь не мешает.
    app.opener()
        .reveal_item_in_dir(target)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    /// Контракт упрощения пути для shell, на котором держится `open_file`:
    /// Windows — verbatim-префикс `\\?\` снимается, остальные платформы —
    /// тождественное преобразование. Исполнить Windows-ветку здесь (macOS)
    /// нельзя; она проверяется на Windows-сборке.
    #[test]
    fn shell_path_simplifies_verbatim_prefix_on_windows() {
        #[cfg(windows)]
        {
            use std::path::Path;
            let verbatim = PathBuf::from(r"\\?\C:\media\image.png");
            assert_eq!(
                dunce::simplified(&verbatim),
                Path::new(r"C:\media\image.png"),
                "verbatim-префикс снят для ShellExecuteExW"
            );
        }
        #[cfg(not(windows))]
        {
            let path = PathBuf::from("/media/image.png");
            assert_eq!(
                dunce::simplified(&path),
                path,
                "на macOS/Linux путь не меняется"
            );
        }
    }
}
