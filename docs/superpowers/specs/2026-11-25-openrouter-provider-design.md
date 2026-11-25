# Koteyye Media Studio 2.0 — OpenRouter как основной провайдер (CI вторичный)

Дата: 2026-11-25
Статус: на утверждение
Предшественники: `2026-11-24-tauri-rewrite-design.md` (Tauri+Rust, CI), Планы 1–2 (ядро + UI-фундамент, слиты в v2).

## Зачем

Пользователь ещё раз посмотрел Cheaper Inference и решил вернуться к **OpenRouter как целевому (основному) провайдеру** — как в исходном приложении, но на новом стеке Tauri 2 + Rust + Dioxus. Реализацию CI **не удалять**: оставить вторичной, рабочей, «допилю потом». Это разворот: Планы 1–2 строились вокруг CI как единственного провайдера, теперь нужен слой абстракции провайдера и полноценная реализация OpenRouter.

## Цели

1. **OpenRouter — основной провайдер по умолчанию**: изображения (генерация + редактирование по референсам), видео (t2v/i2v/r2v, асинхронные джобы), аудио (озвучка/TTS).
2. **CI — вторичный, сохранён рабочим**: существующая реализация адаптируется за абстракцию, не удаляется; пользователь может переключиться и допилить позже.
3. **Абстракция провайдера** в `km-core`: trait `Provider` с реализациями `OpenRouterProvider` и `CiProvider`. UI и оркестратор не знают деталей провайдера.
4. **Переключатель провайдера в UI** (Настройки, default OpenRouter); два ключа в keychain (`openrouter-api-key`, `ci-api-key`).
5. **Нормализованные каталог и баланс**: единая модель для UI, маппинг из обоих провайдеров.
6. **Оркестратор `Jobs` умеет оба потока**: синхронный (CI) и асинхронный (OpenRouter видео submit→poll→download); recovery по провайдеру.
7. Сохранить инварианты надёжности Планов 1–2: оплаченные генерации не теряются при транзиентных сбоях, локальное сохранение первично, цена персистится.

## Не-цели

- Удаление CI-реализации (явно сохранена).
- Новые виды медиа, STT, realtime-голос, мобильные/облако.
- Полировка CI до паритета с OpenRouter (вторичный, «допилю потом»).
- Релизный пайплайн (План 4).

## Анти-цели

- Потеря сделанного CI-кода или регресс его рабочих путей.
- Нарушение надёжности: оплаченные генерации не должны теряться при транзиентных сбоях (ни у одного провайдера).
- Протекание деталей провайдера в UI (UI работает с нормализованными типами).
- Двойная оплата при retry/recovery.

## Ограничения

### OpenRouter API (base `https://openrouter.ai/api/v1`, Bearer)
- **Изображения**: `POST /images` → `data[].b64_json`; `input_references`, resolution/aspect_ratio/seed. Каталог `GET /images/models` (+ legacy `/models?output_modalities=image`).
- **Видео — асинхронно**: `POST /videos` → `{id, polling_url}`; `GET /videos/{id}` → status (pending/in_progress/completed/failed/cancelled/expired) + `unsigned_urls`/`urls`; `GET /videos/{id}/content` → байты. Поля: model, prompt, duration, resolution, aspect_ratio, size, seed, generate_audio, frame_images (first/last frame, `image_url`), input_references, provider passthrough. Каталог `GET /videos/models` (supported_durations/resolutions/aspect_ratios/frame_images, pricing_skus).
- **Аудио (TTS)**: `POST /audio/speech` → raw bytes (mp3/pcm) или `output_delivery:"url"` → signed URL; voice, response_format, speed, instructions, input_references (voice cloning). Каталог TTS-моделей. **Музыки/SFX как отдельных моделей нет** (в отличие от CI).
- **Баланс**: `GET /credits` → total_credits/total_usage.
- **Кадры i2v**: `frame_images[].image_url.url` — data-URL acceptance проверить живьём; если не принимает — ImgBB-загрузка (как в оригинале, обход для РФ). Решение строителя по факту контракта.

### CI API (существующий, сохранён)
- Синхронное видео `/v1/videos/generations`, recovery `/v1/generations/{request_id}`, изображения `/v1/images/generations`+`/edits`, аудио `/v1/audio/speech` (озвучка+музыка+звуки), каталог `/v1/models`+`/public/models`, баланс `/v1/account/balance`.

### Стек и инварианты
- Rust stable, Tauri 2, Dioxus 0.7 WASM; комментарии/доки на русском, идентификаторы на английском.
- Единицы времени — секунды; локальное сохранение первично; ключи в keychain (не в SQLite/бандле); path-safety; signed-URL/ключ не утекают в ошибки.
- Существующие тесты (277) и надёжность Планов 1–2 не регрессировать.

## Подходы

### A · Полноценная абстракция с переключателем — выбран
Trait `Provider`, две реализации (OpenRouter основной, CI вторичный рабочий), переключатель в UI, два ключа, нормализованные каталог/баланс, Jobs sync+async. Сохраняет всё сделанное, CI usable и «допилю потом».

### B · OpenRouter единственный активный, CI заморожен — отклонён
CI-код сохранён компилируемым, но без переключателя/второго ключа. Меньше работы, но CI фактически не usable — противоречит «не убирай, оставь рабочим, допилю». Пользователь выбрал A.

## Как строим (решения)

### Абстракция провайдера (km-core)
- Trait `Provider` (async): каталог по виду медиа, generate_image/edit_image, generate_video (возвращает либо готовый результат, либо job-id для async), speech, balance, recovery-операции. Конкретные сигнатуры — строитель.
- `OpenRouterProvider`: новый клиент (reqwest), async-видео (submit+poll+download), images b64, TTS, /credits, раздельные каталоги.
- `CiProvider`: существующий `CiClient` адаптируется за трейт (sync-видео, /v1/generations recovery, unified каталог, /account/balance).
- Нормализованная модель `MediaModel` (id, name, kind, capabilities/prices) — маппинг из обоих провайдеров; UI использует её. `BalanceView` нормализуется.

### Оркестратор Jobs
- `GenerationRequest`/история получают `provider` (OpenRouter|Ci). Jobs выбирает реализацию по provider.
- Async-видео OpenRouter: submit → poll с интервалом (10–30с) → download; request_id/job-id персистится для recovery; транзиентные сбои → Pending (инвариант).
- Recovery диспетчеризуется по провайдеру: OpenRouter — poll `GET /videos/{id}`; CI — `/v1/generations/{request_id}`.
- Цена персистится сразу после ответа (инвариант План 2).

### Настройки/ключи/UI
- Settings: переключатель провайдера (default OpenRouter), два поля ключа (по провайдеру), проверка подключения per-provider.
- Keychain: `openrouter-api-key`, `ci-api-key`.
- Студия: текущий провайдер виден; дропдаун моделей фильтруется по провайдеру; аудио-табы адаптируются (OpenRouter → «Озвучка»; CI → «Озвучка/Музыка/Звуки»).
- Онбординг: ключ основного провайдера (OpenRouter) + папка.

### Кадры i2v
- data-URL предпочтительно; строитель проверяет живьём OpenRouter `frame_images`. Если не принимает — ImgBB-загрузка (трейт upload-сервиса, ImgBB-ключ опционально). Решение по факту контракта.

## Что остаётся строителю

- Точные сигнатуры trait `Provider` и нормализованных типов; маппинг каталогов/баланса.
- Реализация OpenRouter-клиента (images/video/audio/credits/models), poll-loop тайминги, retry-политика (осторожно: двойная оплата).
- Адаптация существующего CI-кода за трейт без регресса тестов.
- Миграция истории/настроек (поле provider; существующие CI-записи → provider=Ci).
- ImgBB vs data-URL по факту живого контракта OpenRouter.
- Тесты: wiremock для OpenRouter (async-видео, images, TTS, credits), нормализация, recovery per-provider, переключатель, два ключа.
- Порядок задач и декомпозиция (план).