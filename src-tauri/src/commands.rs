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
use std::path::PathBuf;
use std::sync::Arc;

use km_core::{CoreError, by_kind};
use km_shared::{
    AppEvent, BalanceView, CatalogModel, GenerationRequest, HistoryFilter, HistoryRecord,
    JobStatus, MediaKind, Settings,
};
use tauri::{AppHandle, State};
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
    state
        .storage
        .set_settings(&settings)
        .map_err(|e| e.to_string())
}

/// Сохраняет API-ключ в [`KeyStore`](crate::keychain::KeyStore) (keychain).
///
/// Ключ обрезается по краям; пустой ключ удаляет сохранённый.
///
/// # Errors
///
/// Строка с причиной, если keychain не принял ключ.
pub async fn cmd_set_api_key(state: &AppState, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        state.key_store.delete()
    } else {
        state.key_store.set(key)
    }
}

/// Проверка подключения: лёгкий auth-запрос `GET /v1/models` с Bearer.
///
/// `true` — ключ принят (200), `false` — ключа нет или CI ответил 401.
/// Прочие ошибки (сеть, 5xx) пробрасываются: подключение не проверено.
///
/// # Errors
///
/// Строка с причиной при ошибках, отличных от 401.
pub async fn cmd_test_connection(state: &AppState) -> Result<bool, String> {
    let Some(key) = state.key_store.get() else {
        return Ok(false);
    };
    match state.client(&key).models().await {
        Ok(_) => Ok(true),
        Err(CoreError::Http { status: 401, .. }) => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

/// Каталог моделей по виду медиа.
///
/// С ключом — `GET /v1/models`; без ключа — публичный каталог
/// (`GET /public/models`) через кэш, чтобы UI показывал модели до подключения.
///
/// # Errors
///
/// Строка с причиной при сбое запроса или разбора ответа.
pub async fn cmd_fetch_models(
    state: &AppState,
    kind: MediaKind,
) -> Result<Vec<CatalogModel>, String> {
    let key = state.key_store.get();
    let client = state.client(key.as_deref().unwrap_or_default());
    let models = match key {
        Some(_) => client.models().await,
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
/// не задана папка сохранения; ошибки ядра при генерации (запись истории
/// при этом переведена в `Failed`).
pub async fn cmd_generate(
    state: &AppState,
    req: GenerationRequest,
) -> Result<HistoryRecord, String> {
    let key = state
        .key_store
        .get()
        .ok_or_else(|| CoreError::NoKey.to_string())?;
    let settings = state.storage.get_settings().map_err(|e| e.to_string())?;
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
/// Вызывается и при старте (фоном, из `setup`), и вручную из UI. После
/// восстановления завершённых записей шлёт `balance:updated`. Без ключа
/// CI не опросить: записи остаются `Pending` до следующего запуска с ключом
/// (оплаченная генерация могла завершиться на сервере, файлы живут 7 дней).
///
/// # Errors
///
/// Строка с причиной при сбое хранилища; ошибки CI отдельных записей
/// переводят их в `Failed`, а не прерывают восстановление.
pub async fn cmd_recover_pending(state: &AppState) -> Result<Vec<HistoryRecord>, String> {
    let Some(key) = state.key_store.get() else {
        // Без ключа запрос с пустым Bearer дал бы 401 и ядро сожгло бы
        // восстановимые записи в Failed навсегда — оставляем Pending.
        return Ok(Vec::new());
    };
    let settings = state.storage.get_settings().map_err(|e| e.to_string())?;
    let jobs = state.jobs(state.client(&key), PathBuf::from(settings.local_folder))?;
    let recovered = jobs.recover_pending().await.map_err(|e| e.to_string())?;
    if recovered.iter().any(|r| r.status == JobStatus::Completed) {
        state.emit(AppEvent::BalanceUpdated);
    }
    Ok(recovered)
}

/// Гибридный баланс: реальный из CI (если включён в настройках) + локальный расход.
///
/// # Errors
///
/// Строка с причиной при сбое хранилища; 403 от CI скрывает реальный баланс,
/// оставляя локальный (обрабатывается ядром).
pub async fn cmd_fetch_balance(state: &AppState) -> Result<BalanceView, String> {
    let settings = state.storage.get_settings().map_err(|e| e.to_string())?;
    let key = state.key_store.get().unwrap_or_default();
    let jobs = state.jobs(state.client(&key), PathBuf::from(settings.local_folder))?;
    jobs.balance_view(settings.show_real_balance)
        .await
        .map_err(|e| e.to_string())
}

/// Проверяет, что путь внутри папки сохранения (защита как v1 `resolveSavedFilePath`).
///
/// Оба пути канонизируются (симлинки и `..` раскрываются), поэтому обход
/// через `folder/../secret` или симлинк наружу не проходит. Файл должен
/// существовать. Возвращает канонический путь для opener.
fn resolve_saved_path(state: &AppState, path: &str) -> Result<PathBuf, String> {
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
    state: State<'_, Arc<AppState>>,
    settings: Settings,
) -> Result<(), String> {
    cmd_set_settings(&state, settings).await
}

/// Сохранить API-ключ в системный keychain (пустой ключ удаляет).
#[tauri::command]
pub async fn set_api_key(state: State<'_, Arc<AppState>>, key: String) -> Result<(), String> {
    cmd_set_api_key(&state, &key).await
}

/// Проверить подключение к Cheaper Inference.
#[tauri::command]
pub async fn test_connection(state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    cmd_test_connection(&state).await
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
    state: State<'_, Arc<AppState>>,
    req: GenerationRequest,
) -> Result<HistoryRecord, String> {
    cmd_generate(&state, req).await
}

/// Список истории с фильтром.
#[tauri::command]
pub async fn list_history(
    state: State<'_, Arc<AppState>>,
    filter: HistoryFilter,
) -> Result<Vec<HistoryRecord>, String> {
    cmd_list_history(&state, filter).await
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
pub async fn fetch_balance(state: State<'_, Arc<AppState>>) -> Result<BalanceView, String> {
    cmd_fetch_balance(&state).await
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
