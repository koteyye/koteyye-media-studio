//! Словари интерфейса ru/en и перевод по ключу.
//!
//! Ключи сгруппированы по экранам: `common`, `studio`, `history`,
//! `settings`, `onboarding`. Словари симметричны по построению:
//! каждая запись хранит пару (ru, en), поэтому расхождение языков
//! невозможно.

use km_shared::Lang;

/// Переводит ключ интерфейса на выбранный язык.
///
/// Неизвестный ключ возвращается как есть (fallback en→key):
/// UI не ломается при опечатке, а ключ видно в интерфейсе.
/// Ключ должен быть строковым литералом (`&'static str`) — при промахе
/// словаря возвращается он сам, поэтому не может жить меньше `'static`.
///
/// # Examples
///
/// ```
/// use km_shared::Lang;
/// use km_ui_core::i18n::tr;
///
/// assert_eq!(tr(Lang::Ru, "settings.title"), "Настройки");
/// assert_eq!(tr(Lang::En, "settings.title"), "Settings");
/// assert_eq!(tr(Lang::Ru, "no.such.key"), "no.such.key");
/// ```
#[must_use]
pub fn tr(lang: Lang, key: &'static str) -> &'static str {
    let Some((ru, en)) = entry(key) else {
        return key;
    };
    match lang {
        Lang::Ru => ru,
        Lang::En => en,
    }
}

/// Запись словаря: пара (ru, en) для ключа; `None` для неизвестного.
fn entry(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        // Общие действия и состояния.
        "common.ok" => ("ОК", "OK"),
        "common.cancel" => ("Отмена", "Cancel"),
        "common.save" => ("Сохранить", "Save"),
        "common.delete" => ("Удалить", "Delete"),
        "common.error" => ("Ошибка", "Error"),
        "common.loading" => ("Загрузка…", "Loading…"),
        "common.generate" => ("Сгенерировать", "Generate"),
        "common.retry" => ("Повторить", "Retry"),
        "common.open_folder" => ("Открыть папку", "Open folder"),
        "common.recent" => ("Недавние", "Recent"),
        "common.balance" => ("Баланс", "Balance"),
        "common.spend" => ("Расход", "Spend"),
        "common.refresh" => ("Обновить", "Refresh"),
        "common.close" => ("Закрыть", "Close"),
        // Студия: вкладки медиа и поля формы генерации.
        "studio.title" => ("Студия", "Studio"),
        "studio.image" => ("Изображение", "Image"),
        "studio.video" => ("Видео", "Video"),
        "studio.audio" => ("Аудио", "Audio"),
        "studio.prompt" => ("Промпт", "Prompt"),
        "studio.model" => ("Модель", "Model"),
        "studio.size" => ("Размер", "Size"),
        "studio.ratio" => ("Соотношение сторон", "Aspect ratio"),
        "studio.seed" => ("Seed", "Seed"),
        "studio.duration" => ("Длительность", "Duration"),
        "studio.resolution" => ("Разрешение", "Resolution"),
        "studio.audio_on" => ("Со звуком", "With audio"),
        "studio.voice" => ("Голос", "Voice"),
        "studio.speed" => ("Скорость", "Speed"),
        "studio.format" => ("Формат", "Format"),
        // Режимы генерации (GenMode).
        "studio.mode.t2i" => ("Текст → изображение", "Text → image"),
        "studio.mode.i2i" => ("Изображение → изображение", "Image → image"),
        "studio.mode.t2v" => ("Текст → видео", "Text → video"),
        "studio.mode.i2v" => ("Изображение → видео", "Image → video"),
        "studio.mode.r2v" => ("Референс → видео", "Reference → video"),
        "studio.mode.speech" => ("Речь", "Speech"),
        "studio.mode.music" => ("Музыка", "Music"),
        "studio.mode.sfx" => ("Звуковые эффекты", "Sound effects"),
        // История.
        "history.title" => ("История", "History"),
        "history.search" => ("Поиск", "Search"),
        "history.all" => ("Все", "All"),
        "history.empty" => ("История пуста", "History is empty"),
        "history.repeat" => ("Повторить", "Repeat"),
        // Настройки.
        "settings.title" => ("Настройки", "Settings"),
        "settings.api_key" => ("API-ключ", "API key"),
        "settings.check_connection" => ("Проверить соединение", "Check connection"),
        "settings.folder" => ("Папка сохранения", "Save folder"),
        "settings.language" => ("Язык", "Language"),
        "settings.theme" => ("Тема", "Theme"),
        "settings.dark" => ("Тёмная", "Dark"),
        "settings.light" => ("Светлая", "Light"),
        "settings.system" => ("Системная", "System"),
        "settings.show_real_balance" => ("Показывать реальный баланс", "Show real balance"),
        "settings.key_missing" => ("API-ключ не задан", "API key is not set"),
        "settings.keychain_error" => ("Ошибка доступа к связке ключей", "Keychain access error"),
        "settings.connection_ok" => ("Соединение установлено", "Connection OK"),
        "settings.connection_failed" => ("Не удалось подключиться", "Connection failed"),
        "settings.connection_rejected" => (
            "ключ не задан или отклонён (401)",
            "key is not set or rejected (401)",
        ),
        "settings.connection_unverified" => ("Подключение не проверено", "Connection not verified"),
        "settings.checking" => ("Проверка соединения…", "Checking connection…"),
        "settings.pick_folder" => ("Выбрать папку", "Choose folder"),
        "settings.folder_missing" => ("Папка не выбрана", "No folder selected"),
        "settings.available" => ("Доступно", "Available"),
        // Онбординг.
        "onboarding.welcome" => ("Добро пожаловать", "Welcome"),
        "onboarding.step_key" => ("Введите API-ключ", "Enter your API key"),
        "onboarding.step_folder" => ("Выберите папку для сохранения", "Choose a save folder"),
        "onboarding.done" => ("Готово", "Done"),
        "onboarding.skip" => ("Пропустить", "Skip"),
        _ => return None,
    })
}
