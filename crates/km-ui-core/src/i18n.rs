//! Словари интерфейса ru/en и перевод по ключу.
//!
//! Ключи сгруппированы по экранам: `common`, `studio`, `history`,
//! `settings`, `onboarding`. Словари симметричны по построению:
//! каждая запись хранит пару (ru, en), поэтому расхождение языков
//! невозможно.

use km_shared::Lang;
use std::borrow::Cow;

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

/// Translates app-owned errors, including messages persisted by older builds.
/// Provider and OS diagnostics can accompany the localized UI summary separately.
pub fn localize_error(lang: Lang, message: &str) -> Option<Cow<'_, str>> {
    let message = message.trim();
    let translation = entry(message)
        .or_else(|| crate::error_i18n::entry(message))
        .or_else(|| {
            // Older UI callers saved already-translated messages in local state.
            [
                "studio.restore_error",
                "gen.reference_missing",
                "gen.reference_format",
                "controls.edit_wire_limit",
                "controls.edit_source_limit",
                "controls.or_edit_source_limit",
                "controls.model_no_references",
                "controls.model_reference_limit",
                "controls.too_many_video_references",
                "controls.veo_references",
                "controls.upload_read",
                "controls.upload_type",
                "controls.upload_size",
                "controls.upload_invalid",
                "controls.upload_single",
                "ws.balance_wrong_profile",
            ]
            .into_iter()
            .filter_map(entry)
            .find(|(ru, en)| message == *ru || message == *en)
        })?;
    Some(Cow::Borrowed(match lang {
        Lang::Ru => translation.0,
        Lang::En => translation.1,
    }))
}

/// Запись словаря: пара (ru, en) для ключа; `None` для неизвестного.
fn entry(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        // Общие действия и состояния.
        "common.ok" => ("ОК", "OK"),
        "settings.delete_key" => ("Удалить ключ", "Delete key"),
        "settings.confirm_delete_key" => {
            ("Удалить сохранённый API-ключ?", "Delete the saved API key?")
        }
        "common.cancel" => ("Отмена", "Cancel"),
        "common.save" => ("Сохранить", "Save"),
        "common.delete" => ("Удалить", "Delete"),
        "common.error" => ("Ошибка", "Error"),
        "common.action_failed" => (
            "Не удалось выполнить действие.",
            "Could not complete the action.",
        ),
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
        "studio.image" => ("Изображения", "Images"),
        "studio.video" => ("Видео", "Video"),
        "studio.audio" => ("Аудио", "Audio"),
        "studio.prompt" => ("Промпт", "Prompt"),
        "studio.model" => ("Модель", "Model"),
        "studio.size" => ("Размер", "Size"),
        "studio.ratio" => ("Соотношение сторон", "Aspect ratio"),
        "studio.seed" => ("Начальное значение", "Seed"),
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
        "history.to_form" => ("В форму", "Use in form"),
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
        "settings.language_system" => ("Язык системы", "System language"),
        "settings.theme" => ("Тема", "Theme"),
        "settings.dark" => ("Тёмная", "Dark"),
        "settings.light" => ("Светлая", "Light"),
        "settings.system" => ("Системная", "System"),
        "settings.show_real_balance" => ("Показывать реальный баланс", "Show real balance"),
        "settings.key_missing" => ("API-ключ не задан", "API key is not set"),
        "settings.key_removed" => (
            "Ключ удалён. Доступен публичный каталог.",
            "Key removed. The public catalog is available.",
        ),
        "settings.keychain_error" => ("Ошибка доступа к связке ключей", "Keychain access error"),
        "settings.draft_key_ok" => (
            "Ключ принят. Нажмите «Сохранить», чтобы пользоваться им в студии.",
            "Key accepted. Select Save to use it in the studio.",
        ),
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
        "onboarding.intro" => (
            "Выберите сервис для генерации изображений, видео и аудио.",
            "Choose a service to generate images, video and audio.",
        ),
        "onboarding.step_key" => ("Введите API-ключ", "Enter your API key"),
        "onboarding.key_hint" => (
            "Создайте ключ в личном кабинете Cheaper Inference. Для показа реального баланса добавьте scope account:read.",
            "Create a key in your Cheaper Inference account. Add the account:read scope to show the real balance.",
        ),
        "onboarding.openrouter_key_hint" => (
            "Создайте API-ключ в личном кабинете OpenRouter.",
            "Create an API key in your OpenRouter account.",
        ),
        "onboarding.step_folder" => ("Выберите папку для сохранения", "Choose a save folder"),
        "onboarding.folder_hint" => (
            "Сгенерированные файлы будут сохраняться в эту папку.",
            "Generated files will be saved to this folder.",
        ),
        "onboarding.done" => ("Готово", "Done"),
        "onboarding.done_text" => (
            "Настройка завершена. Приятной работы!",
            "Setup is complete. Enjoy!",
        ),
        "onboarding.finish" => ("Начать работу", "Start working"),
        "onboarding.next" => ("Далее", "Next"),
        "onboarding.back" => ("Назад", "Back"),
        "onboarding.skip" => ("Пропустить", "Skip"),
        "onboarding.skip_key_warning" => (
            "Без ключа доступен только каталог моделей. Генерация потребует ключ — его можно задать в Настройках.",
            "Without a key only the model catalog is available. Generation will require a key — you can set it later in Settings.",
        ),
        "onboarding.skip_folder_warning" => (
            "«Пропустить» оставит текущую папку. Без папки сохранения генерация недоступна — файлы некуда записывать; папку можно выбрать в Настройках.",
            "Skip keeps the current folder. Without a save folder generation is unavailable — there is nowhere to write files; you can choose a folder in Settings.",
        ),
        _ => {
            return crate::studio_controls_i18n::entry(key)
                .or_else(|| crate::settings_workspace_i18n::entry(key))
                .or_else(|| crate::studio_i18n::entry(key))
                .or_else(|| crate::media_i18n::lookup(key))
                .or_else(|| crate::generator_i18n::entry(key));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_errors_translate_and_provider_diagnostics_stay_available() {
        assert_eq!(
            localize_error(
                Lang::En,
                "ошибка валидации: папка сохранения не задана в настройках"
            )
            .as_deref(),
            Some("Choose a save folder in Settings.")
        );
        assert_eq!(
            localize_error(
                Lang::Ru,
                "Could not listen on 127.0.0.1:29731: Address already in use"
            )
            .as_deref(),
            Some("Не удалось открыть локальный порт MCP.")
        );
        assert!(localize_error(Lang::Ru, "HTTP 504: Upstream error [request_id: abc]").is_none());
    }
}
