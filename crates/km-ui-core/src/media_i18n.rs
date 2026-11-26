//! Подписи плееров, истории и действий с результатами.

pub(crate) fn lookup(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        "history.search_prompt" => ("Поиск по промпту", "Search prompts"),
        "history.no_matches" => (
            "По этому запросу ничего не найдено",
            "No matching generations",
        ),
        "history.date" => ("Дата", "Date"),
        "history.cost" => ("Стоимость", "Cost"),
        "history.status" => ("Статус", "Status"),
        "history.actions" => ("Действия", "Actions"),
        "history.pending" => ("В работе", "In progress"),
        "history.completed" => ("Готово", "Completed"),
        "history.failed" => ("Ошибка", "Failed"),
        "history.remove" => ("Удалить из истории", "Remove from history"),
        "history.remove_confirm" => (
            "Удалить запись из истории? Файлы останутся на диске.",
            "Remove this entry from history? Your files will stay on disk.",
        ),
        "history.cancel" => ("Отмена", "Cancel"),
        "media.open_file" => ("Открыть файл", "Open file"),
        "media.reveal" => ("Показать в папке", "Show in folder"),
        "media.no_file" => ("Локальный файл отсутствует", "No local file"),
        "media.preview_failed" => (
            "Не удалось открыть файл для предпросмотра",
            "This file could not be previewed",
        ),
        _ => return None,
    })
}
