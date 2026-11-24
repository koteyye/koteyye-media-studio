//! Ошибки ядра Koteyye Media Studio.

use std::io;

/// Ошибка ядра: HTTP-ответ CI, сеть, таймаут, разбор JSON, БД или ввод-вывод.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// HTTP-ошибка CI: статус и разобранный конверт `{"error":{code,message}}`.
    #[error("HTTP {status}{}: {message}", format_code(code))]
    Http {
        /// HTTP-статус ответа.
        status: u16,
        /// Код ошибки CI (строка или число, нормализован в строку), если был в теле.
        code: Option<String>,
        /// Сообщение CI либо сырое тело ответа, если конверт не разобрался.
        message: String,
    },
    /// 403 на account-эндпоинтах: у ключа нет scope `account:read`.
    #[error("ключ не имеет доступа к account-эндпоинтам (нужен scope account:read)")]
    ScopeMissing,
    /// API-ключ Cheaper Inference не задан.
    #[error("не задан API-ключ Cheaper Inference")]
    NoKey,
    /// Запрос к CI превысил таймаут.
    #[error("таймаут запроса к Cheaper Inference")]
    Timeout,
    /// Сетевая ошибка: соединение, DNS или TLS.
    #[error("сетевая ошибка: {0}")]
    Network(String),
    /// Ошибка ввода-вывода.
    #[error("ошибка ввода-вывода: {0}")]
    Io(#[from] io::Error),
    /// Ошибка базы данных (текстовое описание).
    #[error("ошибка базы данных: {0}")]
    Db(String),
    /// Ошибка разбора JSON.
    #[error("ошибка разбора JSON: {0}")]
    Serde(String),
    /// Невалидный запрос: проверка не прошла до обращения к CI.
    #[error("ошибка валидации: {0}")]
    Validation(String),
}

/// Форматирует код ошибки CI для Display: ` (code)` либо пустая строка.
fn format_code(code: &Option<String>) -> String {
    code.as_deref()
        .map(|c| format!(" ({c})"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_http_error_includes_status_code_and_message() {
        let err = CoreError::Http {
            status: 401,
            code: Some("invalid_api_key".to_owned()),
            message: "bad key".to_owned(),
        };
        assert_eq!(err.to_string(), "HTTP 401 (invalid_api_key): bad key");
    }

    #[test]
    fn display_http_error_without_code_omits_parentheses() {
        let err = CoreError::Http {
            status: 500,
            code: None,
            message: "boom".to_owned(),
        };
        assert_eq!(err.to_string(), "HTTP 500: boom");
    }

    #[test]
    fn display_scope_missing_mentions_required_scope() {
        assert!(CoreError::ScopeMissing.to_string().contains("account:read"));
    }
}
