//! SQLite-хранилище: история генераций, агрегация расхода и настройки.
//!
//! [`Storage`] оборачивает одно соединение `rusqlite` в [`Mutex`] (соединение
//! `Send`, но не `Sync`) и создаёт схему при открытии. История ссылается на
//! локальные пути файлов; `local_paths` и параметры хранятся JSON-текстом.
//! Поиск по промпту регистронезависим и работает с кириллицей: встроенный
//! `LOWER` в SQLite сворачивает только ASCII, поэтому зарегистрирована своя
//! SQL-функция `km_lower` на основе `str::to_lowercase`.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use km_shared::{
    HistoryFilter, HistoryPatch, HistoryRecord, JobStatus, MediaKind, Provider, Settings,
};
use rusqlite::functions::FunctionFlags;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params, params_from_iter};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::CoreError;

/// Результат операций хранилища: ошибка ядра [`CoreError`].
type Result<T> = std::result::Result<T, CoreError>;

/// Схема для новой базы. Старые записи получают источник при миграции в `init`.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS history (
    id TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    kind TEXT NOT NULL,
    mode TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt TEXT NOT NULL,
    status TEXT NOT NULL,
    request_id TEXT,
    local_paths TEXT NOT NULL,
    cost_usd REAL,
    params_json TEXT NOT NULL,
    error TEXT,
    provider TEXT NOT NULL DEFAULT 'ci'
);
CREATE TABLE IF NOT EXISTS settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    json TEXT NOT NULL
);
";

/// Выборка всех колонок истории в порядке, ожидаемом [`HistoryRow::from_row`].
const SELECT_HISTORY: &str = "SELECT id, created_at, updated_at, kind, mode, model, prompt, \
     status, request_id, local_paths, cost_usd, params_json, error, provider FROM history";

/// SQLite-хранилище истории, расхода и настроек на одном соединении.
///
/// Соединение защищено [`Mutex`], поэтому [`Storage`] — `Send + Sync` и может
/// жить в Tauri-состоянии и использоваться из джобов и команд параллельно.
#[derive(Debug)]
pub struct Storage {
    conn: Mutex<Connection>,
}

impl Storage {
    /// Открывает (или создаёт) файл БД по пути и создаёт схему.
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`], если файл не открывается или схема не создаётся.
    pub fn open(path: &Path) -> Result<Storage> {
        let conn = Connection::open(path).map_err(db)?;
        Self::init(conn)
    }

    /// Открывает БД в памяти — для тестов и временных сценариев.
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`], если соединение или схема не создаются.
    pub fn open_in_memory() -> Result<Storage> {
        let conn = Connection::open_in_memory().map_err(db)?;
        Self::init(conn)
    }

    /// Регистрирует функцию lowercase и создаёт схему на готовом соединении.
    fn init(mut conn: Connection) -> Result<Storage> {
        // Регистронезависимый поиск с Unicode: встроенный LOWER сворачивает лишь ASCII.
        conn.create_scalar_function(
            "km_lower",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                let text = ctx.get::<String>(0)?;
                Ok(text.to_lowercase())
            },
        )
        .map_err(db)?;
        conn.execute_batch(SCHEMA).map_err(db)?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(db)?;
        if version < 2 {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db)?;
            let has_provider: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('history') WHERE name = 'provider')",
                [], |row| row.get(0),
            ).map_err(db)?;
            if !has_provider {
                tx.execute_batch(
                    "ALTER TABLE history ADD COLUMN provider TEXT NOT NULL DEFAULT 'ci'",
                )
                .map_err(db)?;
            }
            tx.pragma_update(None, "user_version", 2).map_err(db)?;
            tx.commit().map_err(db)?;
        }
        Ok(Storage {
            conn: Mutex::new(conn),
        })
    }

    /// Захватывает соединение; отравление мьютекса фатально.
    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("мьютекс хранилища отравлен")
    }

    /// Добавляет запись истории; `id` должен быть уникальным.
    ///
    /// Неконечная стоимость (NaN/inf) отбрасывается в `NULL`, чтобы не ломать
    /// агрегацию расхода.
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] при сбое вставки (например, дубликат `id`);
    /// [`CoreError::Serde`] при сериализации `local_paths`.
    pub fn insert_history(&self, rec: &HistoryRecord) -> Result<()> {
        Self::insert_into(&self.lock(), rec)
    }

    /// Смена профиля до принятия задачи отклоняет запрос. После записи Pending
    /// задача продолжает работу на своём провайдере без блокировки БД во время HTTP.
    pub(crate) fn begin_generation(&self, rec: &HistoryRecord) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        if Self::read_settings(&tx)?.provider != rec.provider {
            return Err(CoreError::Validation(
                "активный провайдер изменился; проверьте форму перед запуском".into(),
            ));
        }
        Self::insert_into(&tx, rec)?;
        tx.commit().map_err(db)
    }

    fn insert_into(conn: &Connection, rec: &HistoryRecord) -> Result<()> {
        let kind = enum_to_text(&rec.kind)?;
        let mode = enum_to_text(&rec.mode)?;
        let status = enum_to_text(&rec.status)?;
        let provider = enum_to_text(&rec.provider)?;
        let local_paths = serde_json::to_string(&rec.local_paths).map_err(serde)?;
        let cost = finite_cost(rec.cost_usd);
        conn.execute(
            "INSERT INTO history (id, created_at, updated_at, kind, mode, model, prompt, \
             status, request_id, local_paths, cost_usd, params_json, error, provider) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                rec.id,
                rec.created_at,
                rec.updated_at,
                kind,
                mode,
                rec.model,
                rec.prompt,
                status,
                rec.request_id,
                local_paths,
                cost,
                rec.params_json,
                rec.error,
                provider
            ],
        )
        .map_err(db)?;
        Ok(())
    }

    /// Обновляет только заданные патчем поля и поднимает `updated_at` до сейчас.
    ///
    /// `None` в патче означает «не менять». Пустой патч допустим — обновится
    /// только `updated_at`. Отсутствие записи с таким `id` не считается ошибкой.
    /// Момент времени — Unix-время в секундах, как в [`HistoryRecord`].
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] при сбое обновления; [`CoreError::Serde`] при
    /// сериализации `local_paths`.
    pub fn update_history(&self, id: &str, patch: &HistoryPatch) -> Result<()> {
        let mut assignments = vec!["updated_at = ?".to_owned()];
        let mut values: Vec<Value> = vec![Value::Integer(now_unix_secs()?)];
        if let Some(status) = patch.status {
            assignments.push("status = ?".to_owned());
            values.push(Value::Text(enum_to_text(&status)?));
        }
        if let Some(request_id) = &patch.request_id {
            assignments.push("request_id = ?".to_owned());
            values.push(Value::Text(request_id.clone()));
        }
        if let Some(local_paths) = &patch.local_paths {
            assignments.push("local_paths = ?".to_owned());
            values.push(Value::Text(
                serde_json::to_string(local_paths).map_err(serde)?,
            ));
        }
        if let Some(cost) = patch.cost_usd {
            assignments.push("cost_usd = ?".to_owned());
            values.push(if cost.is_finite() {
                Value::Real(cost)
            } else {
                Value::Null
            });
        }
        if let Some(error) = &patch.error {
            assignments.push("error = ?".to_owned());
            values.push(Value::Text(error.clone()));
        } else if patch.status == Some(JobStatus::Completed) {
            assignments.push("error = NULL".to_owned());
        }
        values.push(Value::Text(id.to_owned()));
        let sql = format!("UPDATE history SET {} WHERE id = ?", assignments.join(", "));
        let conn = self.lock();
        conn.execute(&sql, params_from_iter(values)).map_err(db)?;
        Ok(())
    }

    /// Список истории, новые первыми (`created_at DESC`), с фильтром и поиском.
    ///
    /// Фильтр по виду медиа — точное совпадение; поиск — регистронезависимая
    /// подстрока в промпте (символы `%`/`_` в запросе трактуются буквально).
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] при сбое запроса; [`CoreError::Serde`] при разборе
    /// сохранённых JSON-полей.
    pub fn history(&self, filter: &HistoryFilter) -> Result<Vec<HistoryRecord>> {
        let mut clauses: Vec<&str> = Vec::new();
        let mut values: Vec<Value> = Vec::new();
        if let Some(kind) = filter.kind {
            clauses.push("kind = ?");
            values.push(Value::Text(enum_to_text(&kind)?));
        }
        if let Some(search) = &filter.search {
            clauses.push("km_lower(prompt) LIKE ? ESCAPE '\\'");
            let pattern = format!("%{}%", escape_like(&search.to_lowercase()));
            values.push(Value::Text(pattern));
        }
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        self.fetch(&where_sql, values)
    }

    /// Читает запись по локальному идентификатору.
    pub fn history_record(&self, id: &str) -> Result<Option<HistoryRecord>> {
        let sql = format!("{SELECT_HISTORY} WHERE id = ?");
        self.lock()
            .query_row(&sql, [id], HistoryRow::from_row)
            .optional()
            .map_err(db)?
            .map(HistoryRow::into_record)
            .transpose()
    }

    /// Страница истории без сохранённых исходников. Записи содержат до 512 символов промпта.
    pub fn recent_history(
        &self,
        kind: Option<MediaKind>,
        provider: Option<Provider>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<HistoryRecord>> {
        let kind = kind.map(|kind| enum_to_text(&kind)).transpose()?;
        let provider = provider
            .map(|provider| enum_to_text(&provider))
            .transpose()?;
        self.fetch_rows(
            "SELECT id, created_at, updated_at, kind, mode, model, substr(prompt, 1, 512),
                    status, request_id, local_paths, cost_usd, '{}', error, provider
             FROM history WHERE (?1 IS NULL OR kind = ?1) AND (?2 IS NULL OR provider = ?2)
             ORDER BY created_at DESC, rowid DESC LIMIT ?3 OFFSET ?4",
            params![kind, provider, limit, offset],
        )
    }

    /// Удаляет запись истории по `id`; отсутствие записи не считается ошибкой.
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] при сбое удаления.
    pub fn delete_history(&self, id: &str) -> Result<()> {
        let conn = self.lock();
        let removed = conn
            .execute(
                "DELETE FROM history WHERE id = ? AND status <> 'pending'",
                [id],
            )
            .map_err(db)?;
        if removed == 0
            && conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM history WHERE id = ? AND status = 'pending')",
                    [id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(db)?
        {
            return Err(CoreError::Validation(
                "незавершённая генерация ещё нужна для восстановления".into(),
            ));
        }
        Ok(())
    }

    /// Файл прежней папки доступен из истории только по записанному ядром пути.
    pub fn has_result_path(&self, path: &str) -> Result<bool> {
        self.lock()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM history, json_each(history.local_paths) AS file
             WHERE history.status = 'completed' AND file.value = ?)",
                [path],
                |row| row.get(0),
            )
            .map_err(db)
    }

    /// Выбранный файл готовой серии изображений. Запрос не читает промпты и исходные вложения.
    pub fn completed_image_path(&self, id: &str, index: usize) -> Result<Option<String>> {
        let Ok(index) = i64::try_from(index) else {
            return Ok(None);
        };
        self.lock()
            .query_row(
                "SELECT file.value FROM history, json_each(history.local_paths) AS file
                 WHERE history.id = ?1 AND history.kind = 'image' AND history.status = 'completed'
                 AND file.key = ?2 AND file.type = 'text'",
                params![id, index],
                |row| row.get(0),
            )
            .optional()
            .map_err(db)
    }

    /// Записи со статусом `pending`, новые первыми — для восстановления при старте.
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] при сбое запроса; [`CoreError::Serde`] при разборе полей.
    pub fn pending(&self) -> Result<Vec<HistoryRecord>> {
        let status = enum_to_text(&JobStatus::Pending)?;
        self.fetch("WHERE status = ?", vec![Value::Text(status)])
    }

    pub fn has_pending_for(&self, provider: Provider) -> Result<bool> {
        self.lock()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM history WHERE status = 'pending' AND provider = ?1)",
                [enum_to_text(&provider)?],
                |row| row.get(0),
            )
            .map_err(db)
    }

    pub(crate) fn pending_for(&self, provider: Provider) -> Result<Vec<HistoryRecord>> {
        self.fetch(
            "WHERE status = 'pending' AND provider = ?",
            vec![Value::Text(enum_to_text(&provider)?)],
        )
    }

    /// Сумма известных списаний одного провайдера в USD.
    ///
    /// Пустая БД даёт `0.0`. Неконечные стоимости отброшены ещё при записи.
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] при сбое запроса.
    pub fn total_spend_usd(&self, provider: Provider) -> Result<f64> {
        let conn = self.lock();
        let total: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(cost_usd), 0.0) FROM history WHERE provider = ?1",
                [enum_to_text(&provider)?],
                |row| row.get(0),
            )
            .map_err(db)?;
        Ok(total)
    }

    /// Настройки приложения; при пустой БД — значения по умолчанию
    /// согласно `Settings::default`.
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] при сбое запроса; [`CoreError::Serde`] при разборе JSON.
    pub fn get_settings(&self) -> Result<Settings> {
        Self::read_settings(&self.lock())
    }

    fn read_settings(conn: &Connection) -> Result<Settings> {
        let json: Option<String> = conn
            .query_row("SELECT json FROM settings WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(db)?;
        match json {
            Some(text) => serde_json::from_str(&text).map_err(serde),
            None => Ok(Settings::default()),
        }
    }

    /// Сохраняет настройки одной JSON-строкой, перезаписывая предыдущие.
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] при сбое записи; [`CoreError::Serde`] при сериализации.
    pub fn set_settings(&self, settings: &Settings) -> Result<()> {
        Self::write_settings(&self.lock(), settings)
    }

    /// Старые версии UI присылают только папку, язык, тему и видимость баланса.
    /// Сохраняем отсутствующие поля и валидируем типы перед записью.
    pub fn merge_settings(
        &self,
        fields: serde_json::Map<String, serde_json::Value>,
    ) -> Result<Settings> {
        let mut conn = self.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        let mut value = serde_json::to_value(Self::read_settings(&tx)?).map_err(serde)?;
        value
            .as_object_mut()
            .expect("Settings сериализуется объектом")
            .extend(fields);
        let settings: Settings = serde_json::from_value(value).map_err(serde)?;
        Self::write_settings(&tx, &settings)?;
        tx.commit().map_err(db)?;
        Ok(settings)
    }

    fn write_settings(conn: &Connection, settings: &Settings) -> Result<()> {
        let json = serde_json::to_string(settings).map_err(serde)?;
        conn.execute(
            "INSERT OR REPLACE INTO settings (id, json) VALUES (1, ?)",
            params![json],
        )
        .map_err(db)?;
        Ok(())
    }

    /// Читает записи истории с готовым условием `WHERE` и сортировкой по убыванию времени.
    fn fetch(&self, where_sql: &str, values: Vec<Value>) -> Result<Vec<HistoryRecord>> {
        let sql = format!("{SELECT_HISTORY} {where_sql} ORDER BY created_at DESC, rowid DESC");
        self.fetch_rows(&sql, params_from_iter(values))
    }

    fn fetch_rows(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<HistoryRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(sql).map_err(db)?;
        let rows = stmt.query_map(params, HistoryRow::from_row).map_err(db)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row.map_err(db)?.into_record()?);
        }
        Ok(records)
    }
}

/// Сырая строка таблицы `history`: перечисления и JSON ещё не разобраны.
struct HistoryRow {
    id: String,
    created_at: i64,
    updated_at: i64,
    kind: String,
    mode: String,
    model: String,
    prompt: String,
    status: String,
    request_id: Option<String>,
    local_paths: String,
    cost_usd: Option<f64>,
    params_json: String,
    error: Option<String>,
    provider: String,
}

impl HistoryRow {
    /// Читает колонки строки в порядке [`SELECT_HISTORY`].
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            created_at: row.get(1)?,
            updated_at: row.get(2)?,
            kind: row.get(3)?,
            mode: row.get(4)?,
            model: row.get(5)?,
            prompt: row.get(6)?,
            status: row.get(7)?,
            request_id: row.get(8)?,
            local_paths: row.get(9)?,
            cost_usd: row.get(10)?,
            params_json: row.get(11)?,
            error: row.get(12)?,
            provider: row.get(13)?,
        })
    }

    /// Разбирает перечисления и JSON, собирая [`HistoryRecord`].
    fn into_record(self) -> Result<HistoryRecord> {
        Ok(HistoryRecord {
            id: self.id,
            provider: text_to_enum(&self.provider)?,
            created_at: self.created_at,
            updated_at: self.updated_at,
            kind: text_to_enum(&self.kind)?,
            mode: text_to_enum(&self.mode)?,
            model: self.model,
            prompt: self.prompt,
            status: text_to_enum(&self.status)?,
            request_id: self.request_id,
            local_paths: serde_json::from_str(&self.local_paths).map_err(serde)?,
            cost_usd: self.cost_usd,
            params_json: self.params_json,
            error: self.error,
        })
    }
}

/// Текущий момент как Unix-время в секундах — единица [`HistoryRecord`].
fn now_unix_secs() -> Result<i64> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| CoreError::Db(format!("системное время раньше UNIX_EPOCH: {e}")))?
        .as_secs();
    i64::try_from(secs).map_err(|_| CoreError::Db("момент времени вне диапазона i64".to_owned()))
}

/// Оставляет только конечную стоимость: NaN/inf исключаем из денежного поля.
fn finite_cost(cost: Option<f64>) -> Option<f64> {
    cost.filter(|c| c.is_finite())
}

/// Экранирует спецсимволы `LIKE`, чтобы поиск был буквальной подстрокой.
fn escape_like(pattern: &str) -> String {
    pattern
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Сериализует перечисление в каноническую строку через serde (lowercase/snake_case).
fn enum_to_text<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_value(value)
        .map_err(serde)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| CoreError::Serde("перечисление должно сериализоваться в строку".to_owned()))
}

/// Разбирает каноническую строку обратно в перечисление через serde.
fn text_to_enum<T: DeserializeOwned>(text: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).map_err(serde)
}

/// Маппит ошибку rusqlite в строковый вариант [`CoreError::Db`].
fn db(e: rusqlite::Error) -> CoreError {
    CoreError::Db(e.to_string())
}

/// Маппит ошибку serde_json в [`CoreError::Serde`].
fn serde(e: serde_json::Error) -> CoreError {
    CoreError::Serde(e.to_string())
}

// Хранилище живёт в Tauri-состоянии и ходит между потоками — фиксируем Send + Sync.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Storage>();
};
