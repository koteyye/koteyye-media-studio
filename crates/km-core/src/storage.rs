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

use km_shared::{HistoryFilter, HistoryPatch, HistoryRecord, JobStatus, Lang, Settings, Theme};
use rusqlite::functions::FunctionFlags;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::CoreError;

/// Результат операций хранилища: ошибка ядра [`CoreError`].
type Result<T> = std::result::Result<T, CoreError>;

/// Схема БД: создаётся при каждом открытии (`IF NOT EXISTS` — миграция не нужна).
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
    error TEXT
);
CREATE TABLE IF NOT EXISTS settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    json TEXT NOT NULL
);
";

/// Выборка всех колонок истории в порядке, ожидаемом [`HistoryRow::from_row`].
const SELECT_HISTORY: &str = "SELECT id, created_at, updated_at, kind, mode, model, prompt, \
     status, request_id, local_paths, cost_usd, params_json, error FROM history";

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
    fn init(conn: Connection) -> Result<Storage> {
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
        let kind = enum_to_text(&rec.kind)?;
        let mode = enum_to_text(&rec.mode)?;
        let status = enum_to_text(&rec.status)?;
        let local_paths = serde_json::to_string(&rec.local_paths).map_err(serde)?;
        let cost = finite_cost(rec.cost_usd);
        let conn = self.lock();
        conn.execute(
            "INSERT INTO history (id, created_at, updated_at, kind, mode, model, prompt, \
             status, request_id, local_paths, cost_usd, params_json, error) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
                rec.error
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

    /// Удаляет запись истории по `id`; отсутствие записи не считается ошибкой.
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] при сбое удаления.
    pub fn delete_history(&self, id: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM history WHERE id = ?", params![id])
            .map_err(db)?;
        Ok(())
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

    /// Сумма расхода по всей истории в USD; `NULL`-стоимости не учитываются.
    ///
    /// Пустая БД даёт `0.0`. Неконечные стоимости отброшены ещё при записи.
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] при сбое запроса.
    pub fn total_spend_usd(&self) -> Result<f64> {
        let conn = self.lock();
        let total: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(cost_usd), 0.0) FROM history",
                [],
                |row| row.get(0),
            )
            .map_err(db)?;
        Ok(total)
    }

    /// Настройки приложения; при пустой БД — значения по умолчанию
    /// (язык Ru, тема System, папка пустая, реальный баланс скрыт).
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] при сбое запроса; [`CoreError::Serde`] при разборе JSON.
    pub fn get_settings(&self) -> Result<Settings> {
        let conn = self.lock();
        let json: Option<String> = conn
            .query_row("SELECT json FROM settings WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(db)?;
        match json {
            Some(text) => serde_json::from_str(&text).map_err(serde),
            None => Ok(default_settings()),
        }
    }

    /// Сохраняет настройки одной JSON-строкой, перезаписывая предыдущие.
    ///
    /// # Errors
    ///
    /// [`CoreError::Db`] при сбое записи; [`CoreError::Serde`] при сериализации.
    pub fn set_settings(&self, settings: &Settings) -> Result<()> {
        let json = serde_json::to_string(settings).map_err(serde)?;
        let conn = self.lock();
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
        let conn = self.lock();
        let mut stmt = conn.prepare(&sql).map_err(db)?;
        let rows = stmt
            .query_map(params_from_iter(values), HistoryRow::from_row)
            .map_err(db)?;
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
        })
    }

    /// Разбирает перечисления и JSON, собирая [`HistoryRecord`].
    fn into_record(self) -> Result<HistoryRecord> {
        Ok(HistoryRecord {
            id: self.id,
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

/// Настройки по умолчанию для пустой БД.
fn default_settings() -> Settings {
    Settings {
        local_folder: String::new(),
        lang: Lang::Ru,
        theme: Theme::System,
        show_real_balance: false,
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
