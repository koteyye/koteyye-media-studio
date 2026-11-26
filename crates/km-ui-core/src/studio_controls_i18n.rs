//! Тексты выбора модели, параметров и загрузки файлов.

pub fn entry(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        "history.video_processing" => (
            "Сервер ещё создаёт видео. Готовый файл можно получить кнопкой «Проверить результат».",
            "The server is still generating the video. Use Check result to retrieve the finished file.",
        ),
        "controls.veo_references" => (
            "Veo 3.1 и Fast принимают до трёх референсов JPEG или PNG. У Veo 3.1 Lite режим референсов недоступен.",
            "Veo 3.1 and Fast accept up to three JPEG or PNG reference images. Reference images are unavailable with Veo 3.1 Lite.",
        ),
        "controls.video_model_limits" => (
            "Проверьте длительность, количество видео и формат кадра для этой модели.",
            "Check the duration, video count and aspect ratio for this model.",
        ),
        "history.recovery_access" => (
            "Проверьте доступ и баланс ключа аккаунта, который запустил генерацию.",
            "Check access and balance for the API key of the account that generated this file.",
        ),
        "history.recovery_not_found" => (
            "Файл недоступен по этому ключу. Проверьте аккаунт: файл мог быть удалён или не сохранён при ZDR.",
            "File unavailable for this API key. Check the account. The file may have been deleted or excluded by zero data retention.",
        ),
        "history.recovery_response" => (
            "Сервер принял запрос, но файл не удалось прочитать. Проверьте результат ещё раз.",
            "The server accepted the request, but the file could not be read. Check the result again.",
        ),
        "controls.edit_source_limit" => (
            "Допускается до 10 исходных изображений.",
            "Choose up to 10 source images.",
        ),
        "controls.edit_wire_limit" => (
            "Исходники и маска не помещаются в запрос 4,5 МБ. Уменьшите вложения или добавьте исходники ссылками.",
            "The sources and mask exceed the 4.5 MB request limit. Reduce the attachments or add source URLs.",
        ),
        "controls.image_url" => ("Ссылка на исходник", "Source image URL"),
        "controls.add_url" => ("Добавить ссылку", "Add URL"),
        "controls.invalid_url" => (
            "Нужна полная HTTP(S)-ссылка.",
            "Enter a complete HTTP(S) URL.",
        ),
        "controls.search" => ("Найти модель", "Find a model"),
        "controls.no_models" => ("Подходящих моделей нет", "No matching models"),
        "controls.other" => ("Другие", "Other"),
        "controls.choose" => ("Выбрать модель", "Choose a model"),
        "controls.from" => ("от", "from"),
        "controls.per_image" => ("изобр.", "image"),
        "controls.per_second" => ("с", "s"),
        "controls.per_character" => ("симв.", "char"),
        "controls.auto" => ("Как у модели", "Model default"),
        "controls.count" => ("Количество", "Copies"),
        "controls.quality" => ("Качество", "Quality"),
        "controls.more" => ("Ещё", "More"),
        "controls.on" => ("Вкл.", "On"),
        "controls.off" => ("Выкл.", "Off"),
        "controls.upload_hint" => ("Выберите или перетащите файл", "Choose or drop a file"),
        "controls.upload_type" => (
            "Этот формат файла не поддерживается.",
            "Unsupported file format.",
        ),
        "controls.upload_size" => (
            "Нужен непустой файл размером до 20 МиБ.",
            "Choose a nonempty file up to 20 MiB.",
        ),
        "controls.upload_invalid" => (
            "Содержимое файла не соответствует формату.",
            "The file contents do not match its format.",
        ),
        "controls.upload_read" => ("Не удалось прочитать файл.", "Could not read the file."),
        "controls.upload_single" => ("Выберите один файл за раз.", "Choose one file at a time."),
        _ => return None,
    })
}
