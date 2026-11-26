//! Подписи форм Студии и проверки обновлений.

pub(crate) fn entry(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        "updates.title" => ("Обновления", "Updates"),
        "updates.check" => ("Проверить обновления", "Check for updates"),
        "updates.checking" => ("Проверяем версию…", "Checking for updates…"),
        "updates.disabled" => (
            "В этой сборке обновления отключены.",
            "Updates are disabled in this build.",
        ),
        "updates.current" => (
            "Установлена последняя версия.",
            "You have the latest version.",
        ),
        "updates.available" => ("Доступна версия", "Available version"),
        "updates.install" => ("Установить и перезапустить", "Install and restart"),
        "updates.installing" => ("Устанавливаем обновление…", "Installing the update…"),
        "updates.restarting" => ("Перезапуск студии…", "Restarting the studio…"),
        "updates.wait" => (
            "Установка станет доступна после завершения активных задач.",
            "Installation is available when active jobs finish.",
        ),
        "studio.tab.from_text" => ("Из текста", "From text"),
        "studio.tab.edit" => ("По референсу", "Edit image"),
        "studio.tab.from_image" => ("Из изображения", "From image"),
        "studio.tab.references" => ("По референсам", "From references"),
        "studio.tab.speech" => ("Озвучка", "Voiceover"),
        "studio.tab.sounds" => ("Звуки", "Sound effects"),
        "studio.speech_text" => ("Текст для озвучки", "Voiceover text"),
        "studio.hint.image" => ("Опишите изображение…", "Describe the image…"),
        "studio.hint.edit" => (
            "Что изменить в изображении?",
            "What would you like to change?",
        ),
        "studio.hint.video" => (
            "Опишите сцену и движение…",
            "Describe the scene and movement…",
        ),
        "studio.hint.speech" => (
            "Введите текст, который должен прозвучать…",
            "Enter the text to read aloud…",
        ),
        "studio.hint.music" => (
            "Опишите жанр, темп и инструменты…",
            "Describe the genre, tempo and instruments…",
        ),
        "studio.hint.sounds" => ("Опишите звук…", "Describe the sound…"),
        "studio.add_reference" => ("Добавить референс", "Add reference"),
        "studio.reference" => ("Референс", "Reference"),
        "studio.first_frame" => ("Первый кадр", "First frame"),
        "studio.mask" => ("PNG-маска (необязательно)", "PNG mask (optional)"),
        "studio.remove_attachment" => ("Убрать", "Remove"),
        "studio.generating" => ("Генерация…", "Generating…"),
        "studio.check_result" => ("Проверить результат", "Check result"),
        "studio.duration_required" => (
            "Укажите длительность видео в секундах.",
            "Enter the video duration in seconds.",
        ),
        "studio.waiting" => (
            "Ожидаем результат генерации…",
            "Waiting for the generation result…",
        ),
        "studio.estimate" => ("Оценка стоимости", "Estimated cost"),
        "studio.output_duration" => ("по длительности результата", "billed on output duration"),
        "studio.token_rates" => (
            "вход / выход за 1 млн токенов",
            "input / output per 1M tokens",
        ),
        "studio.selected_recent" => ("Из недавних работ", "From recent work"),
        "studio.result" => ("Результат", "Result"),
        "studio.reload_models" => ("Обновить каталог", "Refresh catalog"),
        "studio.model_gone" => (
            "Выбранная модель недоступна. Выберите другую модель.",
            "This model is unavailable. Choose another model.",
        ),
        "studio.choose_folder" => ("Выбрать папку сохранения", "Choose a save folder"),
        "studio.restore_error" => (
            "Не удалось прочитать параметры этой генерации.",
            "Could not read the settings for this generation.",
        ),
        _ => return None,
    })
}
