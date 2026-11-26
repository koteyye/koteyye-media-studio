# Koteyye Media Studio 2.0 — План 3: Студия и История

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Построить рабочие экраны генерации (Студия: изображения/видео/аудио) и Истории поверх готового CI-ядра и UI-фундамента, по утверждённым мокапам; приложение становится полноценно рабочим.

**Architecture:** Чистая логика форм (сборка GenerationRequest, оценка цены, фильтр/группировка моделей, параметры режимов) — в нативном km-ui-core (cargo test). Dioxus-компоненты (формы, чипы, дропдаун, плееры, DnD, лента) тонкие, используют km-ui-core + ipc + AppState-сигналы. Генерация: submit → ipc::generate → события ядра → refresh_history → результат из истории. Загрузка файлов — web-sys FileReader → data-URL (без новых Tauri-команд).

**Tech Stack:** Rust stable 1.100, Dioxus 0.7 WASM, web-sys (FileReader/HTMLMediaElement/Web Audio/canvas), km-core/km-shared/km-ui-core, Tauri IPC (готов).

**Spec:** `docs/superpowers/specs/2026-11-24-tauri-rewrite-design.md`; мокапы `.superpowers/brainstorm/94035-1795528124/content/{layout-v2,video,audio,history,model-picker,final-themes}.html`.

## Global Constraints

- Rust stable 1.100; Dioxus 0.7.10 web; комментарии/доки на русском, идентификаторы на английском.
- Темы Графит #0a0c10 / Воздух #f5f6f8, акцент #e05616, --on-accent #1a0d05; только var(--*), новых цветов не вводить (кроток токена --accent-text для светлой темы, Task 9).
- i18n ru/en парами в km-ui-core; use_t(key:&'static str) литералы; хуки до условных ветвей rsx.
- IPC готов: generate(&GenerationRequest)->HistoryRecord, fetch_models(MediaKind)->Vec<CatalogModel>, list_history(&HistoryFilter)->Vec<HistoryRecord>, delete_history(id), open_file(path), show_in_folder(path), recover_pending, fetch_balance.
- AppState-сигналы: models_by_kind, history, balance, busy, error, settings; методы persist_settings/refresh_balance/refresh_history; use_t.
- События ядра generation:started/done/failed → refresh_history (подписка уже в AppState::init, один call-site).
- GenerationRequest{kind, mode, model, prompt, params: GenParams, references: Vec<Reference{data_url}>, mask_data_url}. GenMode snake_case. CatalogModel{id, name, model_type, provider_name, media_prices, supported_* , features, supported_parameters}.
- Надёжность: UI не блокируется при генерации (busy-состояние), ошибки IPC → баннер, оплаченные генерации не теряются (ядро).
- Тесты: нативная логика km-ui-core (cargo test); компоненты — dx build + smoke (cargo tauri dev). Существующие 277 тестов не регрессировать.

## Review Focus

1. **Генерация не блокирует UI и показывает результат**: submit → busy → событие Done → результат из истории отображается; Failed → внятная ошибка, не зависание. → Task 4/5/6.
2. **Загрузка файлов**: не-изображение/превышение размера → внятная ошибка, не паника; data-URL корректен для references/mask. → Task 2.
3. **Дропдаун моделей**: фильтр по виду/режиму, поиск, цена; пустой каталог → понятное состояние, не паника. → Task 3.
4. **Плееры**: видео/аудио играют локальный файл (local_paths), битый/отсутствующий файл → graceful, не паника. → Task 3 (плееры).
5. **История**: повтор переносит промпт+параметры в студию; удаление убирает запись (файл по решению); фильтр/поиск работают. → Task 7.

---

### Task 1: km-ui-core — логика форм Студии

**Files:**
- Create: `crates/km-ui-core/src/studio.rs`
- Modify: `crates/km-ui-core/src/lib.rs`
- Test: `crates/km-ui-core/tests/studio.rs`

**Interfaces:**
- Consumes: km_shared::{GenerationRequest, GenMode, GenParams, MediaKind, CatalogModel, Reference}.
- Produces:
```rust
pub fn build_request(kind: MediaKind, mode: GenMode, model: &str, prompt: &str, params: GenParams, references: Vec<Reference>, mask: Option<String>) -> GenerationRequest;
pub fn estimate_price(model: &CatalogModel, params: &GenParams) -> Option<f64>;  // из media_prices: разрешение×звук×длительность (видео), за единицу (изображение/аудио)
pub fn models_for(models: &[CatalogModel], kind: MediaKind, mode: GenMode) -> Vec<&CatalogModel>;  // фильтр по виду+режиму (for_mode логика)
pub fn group_by_provider(models: &[&CatalogModel]) -> Vec<(String, Vec<&CatalogModel>)>;  // группировка для дропдауна
pub fn search_models(models: &[&CatalogModel], query: &str) -> Vec<&CatalogModel>;  // поиск по id/name, регистронезависимо
pub fn default_params(mode: GenMode) -> GenParams;  // дефолты параметров режима
```

- [ ] **Step 1: Падающие тесты**

build_request собирает поля (mode snake_case). estimate_price: видео media_prices {"720P+AUDIO":"0.26"} duration 6 audio true → 1.56; изображение за единицу; None если цен нет. models_for фильтрует (i2v не берёт image-модели — логика km-core for_mode). group_by_provider группирует. search_models регистронезависимо. default_params режимов.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-ui-core --test studio` → FAIL.

- [ ] **Step 3: Реализовать логику**

studio.rs: чистые функции. estimate_price парсит media_prices (строки→f64), учитывает resolution/audio/duration. models_for повторяет семантику for_mode (тип+режим).

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-ui-core` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-ui-core && git commit -m "feat: km-ui-core — логика форм Студии (request, цена, фильтр моделей)"
```

---

### Task 2: Компонент загрузки файлов (data-URL)

**Files:**
- Create: `ui/src/components/upload.rs`
- Modify: `ui/src/components/mod.rs`, `crates/km-ui-core/src/studio.rs` (валидация)
- Test: `crates/km-ui-core/tests/studio.rs` (валидация), smoke

**Interfaces:**
- Consumes: web-sys (File, FileReader, HtmlInputElement), km_shared::Reference.
- Produces: `FilePicker{accept, on_load: Callback<String /*data_url*/>, label}` компонент (кнопка выбора + **drag-and-drop зона**: ondrop/ondragover → File → data-URL); `validate_upload(mime: &str, size_bytes: u64, kind: UploadKind) -> Result<(), String>` (нативно: image/audio, лимит размера); `read_file_data_url(file: &web_sys::File) -> Result<String, String>` (WASM, FileReader).

- [ ] **Step 1: Падающие тесты валидации**

validate_upload: image/png 1MB → Ok; image/svg+xml → Err (не растр); аудио audio/mpeg → Ok для audio; превышение лимита (напр. >20MB) → Err; неверный mime → Err.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-ui-core --test studio validate_upload` → FAIL.

- [ ] **Step 3: Реализовать валидацию + FilePicker**

studio.rs validate_upload (нативно). upload.rs: `<input type=file accept=...>` + onchange → FileReader.readAsDataURL → on_load(data_url); **drag-and-drop зона** (ondragover prevent_default, ondrop → files[0] → read_file_data_url); ошибка → баннер. Превью загруженного изображения/аудио.

- [ ] **Step 4: Тесты + сборка + smoke**

Run: `cargo test -p km-ui-core` → PASS; `dx build` успех; smoke: выбор изображения → data-URL превью.

- [ ] **Step 5: Commit**

```bash
git add ui crates/km-ui-core && git commit -m "feat: загрузка файлов — FileReader data-URL, валидация, превью"
```

---

### Task 3: Компоненты — дропдаун моделей, чипы, плееры, лента

**Files:**
- Create: `ui/src/components/{model_dropdown.rs,chips.rs,player.rs,recent.rs}`
- Modify: `ui/src/components/mod.rs`, `ui/assets/style.css`
- Test: нативная логика (Task 1) + smoke

**Interfaces:**
- Consumes: km_ui_core::{models_for, group_by_provider, search_models, estimate_price}, AppState.models_by_kind/history, ipc::{open_file, show_in_folder}, km_shared::{CatalogModel, HistoryRecord, MediaKind}.
- Produces:
```rust
#[component] ModelDropdown{kind: MediaKind, mode: GenMode, selected: String, on_select: Callback<String>}  // поиск+группы+цена
#[component] ParamChips{mode: GenMode, params: GenParams, on_change: Callback<GenParams>}  // чипы параметров режима
#[component] VideoPlayer{path: String, meta: String}  // <video> + контролы + open/download
#[component] AudioPlayer{path: String, meta: String}  // <audio> + waveform canvas + play/seek/time
#[component] RecentStrip{kind: MediaKind}  // последние N результатов из history, клик → просмотр
```

- [ ] **Step 1: Реализовать дропдаун + чипы**

model_dropdown.rs: поиск (search_models), группы (group_by_provider), строка модели (name, price_hint, capabilities), выбор → on_select. chips.rs: чипы по режиму (size/ratio/seed для image; duration/resolution/ratio/audio для video; voice/format/speed для speech, duration для music/sfx), редактирование → on_change.

- [ ] **Step 2: Реализовать плееры + ленту**

player.rs: VideoPlayer `<video src=local_path controls>` + meta + open_file/show_in_folder; AudioPlayer `<audio>` + waveform (Web Audio decode → peaks → canvas bars) + play/seek/time; битый файл → graceful сообщение. recent.rs: последние N history по kind, превью, клик → просмотр/повтор.

- [ ] **Step 3: Сборка + smoke**

Run: `dx build` успех, clippy wasm32; smoke: дропдаун поиск/выбор, чипы меняют params, плееры играют локальный файл, лента показывает недавние.

- [ ] **Step 4: Commit**

```bash
git add ui && git commit -m "feat: компоненты — дропдаун моделей, чипы, видео/аудио плееры, лента недавних"
```

---

### Task 4: Студия изображений (t2i / edit)

**Files:**
- Create: `ui/src/screens/studio_image.rs`
- Modify: `ui/src/screens/studio.rs`, `ui/src/screens/mod.rs`
- Test: нативная логика + smoke

**Interfaces:**
- Consumes: ModelDropdown, ParamChips, FilePicker, RecentStrip, ipc::generate, AppState, km_ui_core::{build_request, estimate_price}.
- Produces: экран ImageStudio (табы «Из текста / По референсу», промпт, чипы, загрузка референса/маски, generate, результат-изображение).

- [ ] **Step 1: Реализовать экран**

Табы t2i/edit (GenMode::TextToImage/ImageEdit). Промпт textarea. ModelDropdown(kind Image, mode). ParamChips. edit: FilePicker референс + маска (references/mask_data_url). Кнопка generate → build_request → ipc::generate (spawn_local), busy-состояние. Результат: последняя Completed запись Image из history → `<img src=local_path>` + meta + open/delete. Ошибка → баннер. Оценка цены estimate_price.

- [ ] **Step 2: Сборка + smoke**

dx build, clippy; smoke: t2i генерация (ключ CI) → результат; edit с референсом; busy не блокирует UI; ошибка → баннер.

- [ ] **Step 3: Commit**

```bash
git add ui && git commit -m "feat: Студия изображений — t2i/edit, референс/маска, результат"
```

---

### Task 5: Студия видео (t2v / i2v / r2v)

**Files:**
- Create: `ui/src/screens/studio_video.rs`
- Modify: `ui/src/screens/studio.rs`, `ui/src/screens/mod.rs`
- Test: нативная логика + smoke

**Interfaces:**
- Consumes: ModelDropdown, ParamChips, FilePicker, VideoPlayer, RecentStrip, ipc::generate, km_ui_core.
- Produces: экран VideoStudio (табы «Из текста / Из изображения / По референсам», кадры first/last, референсы, чипы duration/resolution/ratio/audio, generate, видеоплеер).

- [ ] **Step 1: Реализовать экран**

Табы t2v/i2v/r2v. i2v: FilePicker первый/последний кадр. Первый кадр = references[0] (km-core video.rs уже маппит в image_url). Последний кадр: если мокап требует — расширить `Reference` полем `frame_type: Option<String>` (km-shared) + маппинг video.rs в frame_images first/last (малое расширение ядра, builder). r2v: несколько референсов. ModelDropdown(kind Video, mode) фильтрует i2v/r2v модели. ParamChips (duration/resolution/ratio/audio toggle). generate → build_request → ipc::generate; видео async — результат по событию Done (Pending → busy/прогресс). VideoPlayer результата. Оценка цены (duration×resolution×audio).

- [ ] **Step 2: Сборка + smoke**

dx build, clippy; smoke: t2v генерация → Pending → Done → плеер; i2v с кадром; busy/прогресс не блокирует; ошибка → баннер.

- [ ] **Step 3: Commit**

```bash
git add ui && git commit -m "feat: Студия видео — t2v/i2v/r2v, кадры, видеоплеер"
```

---

### Task 6: Студия аудио (озвучка / музыка / звуки)

**Files:**
- Create: `ui/src/screens/studio_audio.rs`
- Modify: `ui/src/screens/studio.rs`, `ui/src/screens/mod.rs`
- Test: нативная логика + smoke

**Interfaces:**
- Consumes: ModelDropdown, ParamChips, AudioPlayer, RecentStrip, ipc::generate, km_ui_core.
- Produces: экран AudioStudio (табы «Озвучка / Музыка / Звуки», чипы voice/format/speed или duration, generate, аудиоплеер с волной).

- [ ] **Step 1: Реализовать экран**

Табы Speech/Music/SoundEffects. Speech: чипы voice (из model.voices или ввод), format (mp3/wav), speed; Music/SFX: duration. ModelDropdown(kind Audio, mode). generate → build_request → ipc::generate. AudioPlayer результата (waveform). Текст input (промпт). Оценка цены.

- [ ] **Step 2: Сборка + smoke**

dx build, clippy; smoke: озвучка → аудиоплеер играет; музыка/звуки табы; ошибка → баннер.

- [ ] **Step 3: Commit**

```bash
git add ui && git commit -m "feat: Студия аудио — озвучка/музыка/звуки, плеер с волной"
```

---

### Task 7: Экран История

**Files:**
- Create: `ui/src/screens/history_list.rs` (заменить заглушку history.rs)
- Modify: `ui/src/screens/{history.rs,mod.rs}`, `ui/src/router.rs`
- Test: нативная логика фильтра + smoke

**Interfaces:**
- Consumes: AppState.history, ipc::{list_history, delete_history, open_file, show_in_folder}, km_shared::{HistoryFilter, HistoryRecord, MediaKind}, VideoPlayer/AudioPlayer/превью.
- Produces: экран History (список: превью, промпт, модель, дата, цена, статус; фильтры по виду, поиск; действия открыть/повтор/удалить).

- [ ] **Step 1: Реализовать экран**

Список записей (новые первыми): превью (img/video/audio иконка), промпт, модель, format_date, format_usd, статус. Фильтр по виду (Все/🖼/🎬/🎵) + поиск (HistoryFilter). Действия: открыть файл (open_file), показать в папке (show_in_folder), повтор (перенос промпт+params в студию через роутер/сигнал), удалить (delete_history + refresh). Клик → просмотр результата (плеер/изображение). Пустая история → понятное состояние.

- [ ] **Step 2: Сборка + smoke**

dx build, clippy; smoke: список, фильтр/поиск, повтор → студия с промптом, удаление, открытие файла.

- [ ] **Step 3: Commit**

```bash
git add ui && git commit -m "feat: экран История — список, фильтры, поиск, повтор/открыть/удалить"
```

---

### Task 8: Интеграция генерации и состояний

**Files:**
- Modify: `ui/src/state.rs`, `ui/src/screens/studio*.rs`, `ui/src/components/layout.rs`
- Test: нативная логика + smoke

**Interfaces:**
- Consumes: AppState (busy/error/history), ipc::generate, события ядра.
- Produces: общий хук/логика submit-генерации (busy per-kind, ошибка, результат из истории по событию); раздельные сигналы ошибок фона/операций (паркованный нюанс Плана 2).

- [ ] **Step 1: Реализовать интеграцию**

Общий useGeneration/submit: вызов ipc::generate, busy-состояние экрана, по событию Done/Failed — обновление результата/ошибки. Разделить error-сигнал: фоновые ошибки загрузки не стирают ошибку операции (паркованный нюанс). Результат студии = последняя запись своего kind из history (обновляется событием).

- [ ] **Step 2: Сборка + smoke**

dx build, clippy; smoke: генерация → busy → результат; фоновая загрузка не стирает ошибку операции; повторный запуск восстанавливает Pending.

- [ ] **Step 3: Commit**

```bash
git add ui && git commit -m "feat: интеграция генерации — busy/результат/ошибки, раздельные сигналы"
```

---

### Task 9: UI-полировка (паркованное Плана 2)

**Files:**
- Modify: `crates/km-ui-core/src/theme.rs`, `ui/assets/style.css`, `ui/src/screens/settings.rs`, `ui/src/components/{key_status.rs (новый),layout.rs}`, `ui/src/screens/onboarding.rs`
- Test: нативные тесты токенов/DRY + smoke

**Interfaces:**
- Consumes: theme_vars, ConnectionStatus/KeyStatus (settings/onboarding).
- Produces: токен --accent-text (светлая тема WCAG AA ≥4.5:1 на фоне), общий модуль key-status (DRY двух машин), confirm удаления ключа пустым полем, aria счётчика онбординга.

- [ ] **Step 1: Токен --accent-text + контраст**

theme.rs: --accent-text per-theme (светлая напр. #c2410c ≈4.8:1 на #f5f6f8; тёмная — акцент проходит). style.css: error-banner/status-error/nav-active/warning используют var(--accent-text). Тест токенов + контраст.

- [ ] **Step 2: DRY key-status + confirm + aria**

Вынести общий модуль key-status (enum + маппинг + helpers) из settings/onboarding. Settings: confirm при удалении ключа пустым полем (или отдельная кнопка удалить). Онбординг: снять aria-hidden со счётчика шагов.

- [ ] **Step 3: Тесты + сборка + smoke**

cargo test km-ui-core (токены), dx build, clippy; smoke: светлая тема контраст, key-status общий, confirm удаления.

- [ ] **Step 4: Commit**

```bash
git add ui crates/km-ui-core && git commit -m "feat: UI-полировка — accent-text WCAG, DRY key-status, confirm ключа, aria"
```