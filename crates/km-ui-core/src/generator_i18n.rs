//! Дополнительные подписи рабочего экрана генерации.

pub(crate) fn entry(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        "gen.find" => ("Найти запрос", "Find a prompt"),
        "gen.results" => ("Результаты", "Results"),
        "gen.history" => ("Вся история", "All history"),
        "gen.empty" => (
            "Здесь появятся результаты этого вида и провайдера.",
            "Results for this media type and provider will appear here.",
        ),
        "gen.open_image" => ("Открыть кадр", "View image"),
        "gen.one" => ("Кадры по одному", "One image at a time"),
        "gen.grid" => ("Варианты рядом", "Side by side"),
        "gen.view_mode" => ("Вид результатов", "Result layout"),
        "gen.request_id" => ("Идентификатор запроса", "Request ID"),
        "gen.use" => ("Как референс", "Use as reference"),
        "gen.prev" => ("Предыдущий кадр", "Previous image"),
        "gen.next" => ("Следующий кадр", "Next image"),
        "gen.repeat" => ("Вернуть запрос", "Restore request"),
        "gen.play" => ("Воспроизвести", "Play"),
        "gen.select" => ("Выбрать результат", "Select result"),
        "gen.wait" => ("Запрос выполняется", "Request in progress"),
        "gen.pending" => ("Ожидает получения результата", "Awaiting result"),
        "gen.open_settings" => ("Настройки провайдера", "Provider settings"),
        "gen.refs" => ("Референсы", "References"),
        "gen.view_ref" => ("Посмотреть референс", "View reference"),
        "gen.add_first_frame" => ("Добавить первый кадр", "Add first frame"),
        "gen.quick_image_hint" => (
            "Выберите или перетащите PNG, JPEG или WebP. Откроется режим «По референсу».",
            "Choose or drop a PNG, JPEG or WebP image to switch to Edit image.",
        ),
        "gen.quick_video_hint" => (
            "Выберите или перетащите одно изображение PNG, JPEG, WebP или GIF. Откроется режим «Из изображения».",
            "Choose or drop one PNG, JPEG, WebP or GIF image to switch to From image.",
        ),
        "gen.mask" => ("Маска PNG", "PNG mask"),
        "gen.files" => ("Файлы", "Files"),
        "gen.failure.sent" => ("Ошибка последней отправки", "Latest submission failed"),
        "gen.failure.stored" => ("Ошибка запроса из истории", "Failed request from history"),
        "gen.failure.diagnostic" => ("Исходная диагностика", "Original diagnostic"),
        "gen.failure.server" => (
            "Сервис вернул серверную ошибку при обработке этого запроса.",
            "The service returned a server error while processing this request.",
        ),
        "gen.failure.client_timeout" => (
            "Студия не дождалась ответа. Статус запроса можно посмотреть в кабинете провайдера.",
            "The studio stopped waiting for a response. Check the request status in the provider dashboard.",
        ),
        "gen.failure.server_timeout" => (
            "Сервис завершил запрос по таймауту.",
            "The service timed out while processing this request.",
        ),
        "gen.failure.access" => (
            "Сервис отклонил доступ к этому запросу.",
            "The service denied access to this request.",
        ),
        "gen.failure.rate" => (
            "Сервис ограничил частоту запросов.",
            "The service rate-limited this request.",
        ),
        "gen.failure.other" => (
            "Получить готовый результат этого запроса не удалось.",
            "The result of this request could not be obtained.",
        ),
        "gen.frame_exists" => (
            "В черновике видео уже есть первый кадр.",
            "The video draft already has a first frame.",
        ),
        "gen.replace_frame" => ("Заменить первый кадр", "Replace first frame"),
        "gen.reference_missing" => (
            "Файл выбранного результата недоступен",
            "The selected output file is unavailable",
        ),
        "gen.reference_format" => (
            "Референсом может быть PNG, JPEG или WebP",
            "Use a PNG, JPEG or WebP image as a reference",
        ),
        "gen.no_receipt" => (
            "Результат последней отправки недоступен.",
            "The result of the latest submission is unavailable.",
        ),
        "gen.result_error" => (
            "Последняя отправка завершилась ошибкой",
            "Latest submission failed",
        ),
        _ => return None,
    })
}
