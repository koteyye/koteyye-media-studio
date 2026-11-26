//! Display translations for app errors. Provider payloads are kept as diagnostics.

type Text = (&'static str, &'static str);

pub(super) fn entry(raw: &str) -> Option<Text> {
    if let Some(reason) = raw.strip_prefix("ошибка валидации: ") {
        return Some(exact(reason).or_else(|| category(reason)).unwrap_or((
            "Проверьте параметры запроса.",
            "Check the request parameters.",
        )));
    }
    exact(raw).or_else(|| category(raw))
}

fn exact(message: &str) -> Option<Text> {
    Some(match message {
        "не задан API-ключ Cheaper Inference" => (
            "Не задан API-ключ Cheaper Inference.",
            "The Cheaper Inference API key is missing.",
        ),
        "не задан API-ключ OpenRouter" => (
            "Не задан API-ключ OpenRouter.",
            "The OpenRouter API key is missing.",
        ),
        "таймаут запроса к Cheaper Inference" => (
            "Превышено время ожидания ответа Cheaper Inference.",
            "The request to Cheaper Inference timed out.",
        ),
        "таймаут запроса к OpenRouter" => (
            "Превышено время ожидания ответа OpenRouter.",
            "The request to OpenRouter timed out.",
        ),
        "таймаут проверки ключа OpenRouter" => (
            "Истекло время проверки ключа OpenRouter.",
            "OpenRouter key verification timed out.",
        ),
        "ключ не имеет доступа к account-эндпоинтам (нужен scope account:read)" => {
            (
                "Для просмотра баланса ключу нужно право account:read.",
                "The API key needs the account:read scope to read the balance.",
            )
        }
        "папка сохранения не задана в настройках" => (
            "Выберите папку сохранения в настройках.",
            "Choose a save folder in Settings.",
        ),
        "Профиль баланса изменился во время запроса" => (
            "Профиль баланса изменился во время запроса.",
            "The balance profile changed during the request.",
        ),
        "не удалось заблокировать настройки" => (
            "Не удалось получить доступ к настройкам.",
            "Could not access the settings.",
        ),
        "Дождитесь завершения текущей генерации или восстановления" => {
            (
                "Дождитесь завершения генерации или восстановления.",
                "Wait for the generation or recovery to finish.",
            )
        }
        "Проверьте наличие обновления ещё раз" => (
            "Проверьте наличие обновления ещё раз.",
            "Check for updates again.",
        ),
        "промпт пустой" | "input пустой" => {
            ("Введите текст запроса.", "Enter the request text.")
        }
        "модель не выбрана" => ("Выберите модель.", "Choose a model."),
        "модель и промпт должны быть непустыми"
        | "для генерации изображения нужны модель, промпт и режим изображения"
        | "для генерации видео нужны модель и промпт"
        | "для генерации аудио нужны модель и промпт" => (
            "Выберите модель и введите текст запроса.",
            "Choose a model and enter the request text.",
        ),
        "неверный режим генерации изображения" => (
            "Выберите допустимый режим изображения.",
            "Choose a supported image mode.",
        ),
        "требуется положительная длительность видео" => (
            "Задайте положительную длительность видео.",
            "Set a positive video duration.",
        ),
        "длительность видео должна быть целым числом секунд" => {
            (
                "Укажите длительность видео в целых секундах.",
                "Enter the video duration in whole seconds.",
            )
        }
        "n должно быть в диапазоне 1–10" | "количество изображений должно быть от 1 до 10" => {
            (
                "Выберите от 1 до 10 изображений.",
                "Choose between 1 and 10 images.",
            )
        }
        "количество видео должно быть от 1 до 10" => (
            "Выберите от 1 до 10 видео.",
            "Choose between 1 and 10 videos.",
        ),
        "OpenRouter принимает одно видео на запрос" => (
            "OpenRouter принимает одно видео на запрос.",
            "OpenRouter accepts one video per request.",
        ),
        "промпт редактирования длиннее 32000 символов" => (
            "Сократите текст запроса до 32 000 символов.",
            "Shorten the request to 32,000 characters.",
        ),
        "нужно от 1 до 10 исходных изображений" => (
            "Прикрепите от 1 до 10 исходных изображений.",
            "Attach between 1 and 10 source images.",
        ),
        "OpenRouter принимает от 1 до 16 исходных изображений" => (
            "OpenRouter принимает от 1 до 16 исходных изображений.",
            "OpenRouter accepts between 1 and 16 source images.",
        ),
        "исходники и маска в сумме превышают 20 МиБ" => (
            "Общий размер исходников и маски превышает 20 МиБ.",
            "The source images and mask exceed 20 MiB in total.",
        ),
        "маска должна быть встроенным PNG" => {
            ("Используйте PNG для маски.", "Use a PNG for the mask.")
        }
        "OpenRouter не поддерживает редактирование с PNG-маской" => {
            (
                "PNG-маска доступна для Cheaper Inference.",
                "PNG masks are available with Cheaper Inference.",
            )
        }
        "требуется HTTP(S)-ссылка или встроенный PNG, JPEG или WebP"
        | "укажите HTTP(S)-ссылку или встроенное изображение"
        | "укажите HTTP(S)-ссылку или изображение в формате base64 data URL" => {
            (
                "Прикрепите изображение или добавьте HTTP(S)-ссылку.",
                "Attach an image or add an HTTP(S) URL.",
            )
        }
        "исходник должен быть PNG, JPEG или WebP в формате base64 data URL" => {
            (
                "Используйте PNG, JPEG или WebP для исходника.",
                "Use PNG, JPEG or WebP for the source image.",
            )
        }
        "для видео из изображения нужен один первый кадр, для видео по референсам нужен хотя бы один референс" => {
            (
                "Прикрепите один первый кадр или хотя бы один референс в зависимости от режима видео.",
                "Attach one first frame or at least one reference, depending on the video mode.",
            )
        }
        "скорость озвучки должна быть положительной" => (
            "Задайте положительную скорость озвучки.",
            "Set a positive speech speed.",
        ),
        "выбранная модель не поддерживает этот режим аудио OpenRouter" => {
            (
                "Выберите модель OpenRouter, которая поддерживает этот режим аудио.",
                "Choose an OpenRouter model that supports this audio mode.",
            )
        }
        "OpenRouter поддерживает форматы озвучки MP3, PCM и WAV" => (
            "Для озвучки в OpenRouter доступны MP3, PCM и WAV.",
            "OpenRouter speech is available in MP3, PCM and WAV.",
        ),
        "подписанная ссылка на файл недоступна" => (
            "Ссылка на файл недоступна.",
            "The file link is unavailable.",
        ),
        "размер загруженного файла не совпадает с ответом CI" => {
            (
                "Размер файла отличается от указанного Cheaper Inference.",
                "The file size differs from the size reported by Cheaper Inference.",
            )
        }
        "MCP server stopped" => ("Сервер MCP остановлен.", "The MCP server has stopped."),
        "MCP token is missing" => ("Токен MCP отсутствует.", "The MCP token is missing."),
        "Start the MCP server in this instance before rotating its token" => (
            "Запустите сервер MCP в этом экземпляре студии, чтобы заменить токен.",
            "Start the MCP server in this Studio instance to rotate its token.",
        ),
        "Invalid model ID" => ("Некорректный идентификатор модели.", "Invalid model ID."),
        "The selected model does not support this generation mode in the provider catalog" => (
            "Выбранная модель не поддерживает этот режим генерации.",
            "The selected model does not support this generation mode.",
        ),
        "IPC is unavailable" => (
            "Соединение с приложением недоступно.",
            "The connection to the app is unavailable.",
        ),
        _ => return None,
    })
}

fn category(message: &str) -> Option<Text> {
    let starts = |prefix| message.starts_with(prefix);
    Some(if starts("сетевая ошибка: ") {
        (
            "Не удалось подключиться к сервису.",
            "Could not connect to the service.",
        )
    } else if starts("ошибка разбора JSON: ") || starts("Invalid IPC response for command ")
    {
        (
            "Не удалось прочитать ответ сервиса.",
            "Could not read the service response.",
        )
    } else if starts("ошибка базы данных: ") || starts("Не удалось заблокировать историю студии: ")
    {
        (
            "Не удалось получить доступ к истории студии.",
            "Could not access the Studio history.",
        )
    } else if starts("ошибка ввода-вывода: ") {
        (
            "Не удалось прочитать или сохранить файл.",
            "Could not read or save the file.",
        )
    } else if starts("ответ на запрос ") {
        (
            "Не удалось обработать ответ. Проверьте результат ещё раз.",
            "Could not process the response. Check the result again.",
        )
    } else if starts("папка сохранения недоступна: ") {
        (
            "Папка сохранения недоступна.",
            "The save folder is unavailable.",
        )
    } else if starts("файл недоступен: ") {
        ("Файл недоступен.", "The file is unavailable.")
    } else if starts("путь вне папки сохранения: ") {
        (
            "Файл находится вне папки сохранения.",
            "The file is outside the save folder.",
        )
    } else if starts("диалог выбора папки не завершился: ") {
        (
            "Не удалось открыть диалог выбора папки.",
            "Could not open the folder picker.",
        )
    } else if starts("Could not open provider key page: ") {
        (
            "Не удалось открыть страницу ключей провайдера.",
            "Could not open the provider's API keys page.",
        )
    } else if starts("не удалось") && message.contains("keychain:") {
        (
            "Не удалось получить доступ к сохранённому ключу.",
            "Could not access the saved API key.",
        )
    } else if starts("Изменился активный провайдер") {
        (
            "Активный провайдер изменился. Проверьте настройки.",
            "The active provider changed. Check Settings.",
        )
    } else if starts("в профиле ") || starts("ключ ") && message.contains("не подходит профилю")
    {
        (
            "Ключ принадлежит другому провайдеру.",
            "The API key belongs to a different provider.",
        )
    } else if message.contains("4,5 МБ") && message.contains("запрос") {
        (
            "Запрос превышает 4,5 МБ. Уменьшите вложения или добавьте ссылки.",
            "The request exceeds 4.5 MB. Reduce the attachments or add URLs.",
        )
    } else if starts("JSON превышает 20 МБ") {
        (
            "Запрос превышает 20 МБ. Уменьшите вложения или добавьте ссылки.",
            "The request exceeds 20 MB. Reduce the attachments or add URLs.",
        )
    } else if starts("Could not listen on ") {
        (
            "Не удалось открыть локальный порт MCP.",
            "Could not bind the local MCP port.",
        )
    } else if starts("Failed to read MCP settings: ") {
        (
            "Не удалось прочитать настройки MCP.",
            "Could not read the MCP settings.",
        )
    } else if starts("MCP server failed: ") || starts("MCP server task failed: ") {
        (
            "Сервер MCP завершился с ошибкой.",
            "The MCP server stopped with an error.",
        )
    } else if starts("восстановление невозможно: ") {
        (
            "Запрос нельзя восстановить без идентификатора.",
            "The request cannot be recovered without an ID.",
        )
    } else if starts("input длиннее ") {
        (
            "Текст озвучки превышает ограничение длины.",
            "The speech text exceeds the length limit.",
        )
    } else if starts("режим ") && message.contains("хотя бы один референс") {
        (
            "Этот режим требует хотя бы одного референса.",
            "This mode needs at least one reference.",
        )
    } else if starts("Veo 3.1 и Fast ") {
        return crate::studio_controls_i18n::entry("controls.veo_references");
    } else if starts("для ") && message.contains("длительность в секундах")
    {
        return crate::studio_controls_i18n::entry("controls.video_model_limits");
    } else {
        return None;
    })
}
