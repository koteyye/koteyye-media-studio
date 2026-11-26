# Koteyye Media Studio 2.0 — План 2: надёжность + UI-фундамент

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Закрыть транзиентные потери оплаченных генераций и построить фундамент Dioxus-UI (шелл, роутер, темы, i18n, Tauri-биндинг, глобальное состояние, Настройки, Онбординг), на котором План 3 построит экраны Студии.

**Architecture:** Чистая UI-логика (i18n, токены тем, форматтеры) — в нативном крейте `km-ui-core` (тестируется `cargo test`); Dioxus-компоненты (`ui`, WASM) тонкие, используют `km-ui-core` и IPC-биндинг `tauri-sys` (вызовы `__TAURI_INTERNALS__`, без npm). Глобальное состояние — сигналы Dioxus + контекст. Темы — CSS-переменные + `data-theme`.

**Tech Stack:** Rust stable 1.100, Tauri 2.12, Dioxus 0.7.10 (web/WASM), tauri-sys (IPC), km-core/km-shared (готовы, План 1).

**Spec:** `docs/superpowers/specs/2026-11-24-tauri-rewrite-design.md`

**Scope:** План 2 = надёжность + UI-фундамент (этот файл). План 3 = Студия (экраны image/video/audio, чипы, дропдаун моделей, плееры, DnD, лента «Недавние», История). План 4 = релиз (updater, CSP, CI-матрикс, подпись, иконка 1024, [lints] BeautifulRust, spawn_blocking keychain). Отдельные решения (не в Плане 2): retry-политика HTTP (осторожно — платные генерации, риск двойной оплаты), судьба `seed` (пробросить в CI или убрать чип), зашифрованный fallback ключа.

## Global Constraints

- Rust stable 1.100; Tauri 2.12; Dioxus 0.7.10; edition 2024; версии пиновать в Cargo.toml.
- Комментарии/доки на русском; идентификаторы на английском.
- UI на Dioxus → WASM; JavaScript/TypeScript нет; допустимы минимальные wasm-bindgen-обёртки. IPC через `tauri-sys` (без npm).
- Темы: тёмная «Графит» (база `#0a0c10`), светлая «Воздух» (база `#f5f6f8`); акцент `#e05616`, hover `#ff7333`. CSS-переменные, атрибут `data-theme` на корне.
- i18n: ru/en, переключение без перезапуска.
- Единицы времени — секунды (unix secs).
- Локальное сохранение первично; оплаченные генерации не теряются при транзиентных сбоях.
- Tauri-команды (имена/args camelCase): `get_settings`, `set_settings{settings}`, `set_api_key{key}`, `test_connection`, `fetch_models{kind}`, `generate{req}`, `list_history{filter}`, `delete_history{id}`, `recover_pending`, `fetch_balance`, `pick_folder`, `open_file{path}`, `show_in_folder{path}`. `kind`/`Settings.lang`/`theme` — lowercase serde.
- События: `generation:started/done/failed`, `balance:updated`; payload `AppEvent` externally-tagged snake_case (`{"generation_started":{"history_id":"..."}}`, `"balance_updated"`).
- `km-core` не зависит от Tauri; UI-логика тестируется нативно в `km-ui-core`.

## Review Focus

1. **Транзиентный сбой не сжигает оплату**: 5xx/429/Db/сеть при recovery и run, video `processing` → запись остаётся Pending с request_id (не Failed), recovery подхватит позже. → Task 1.
2. **Смена темы/языка без перезапуска**: переключение применяется ко всему UI (data-theme + i18n), сохраняется в Settings. → Task 4, 5.
3. **Первый запуск без ключа/папки**: онбординг ведёт ключ→папка→готово, не пустой экран и не паника. → Task 6.
4. **Keychain недоступен / нет соединения**: внятное сообщение пользователю, не молчаливое «нет ключа». → Task 5.
5. **IPC-ошибка**: команда вернула Err → UI показывает ошибку и остаётся отзывчивым, не зависает. → Task 3, 5.

---

### Task 1: Надёжность — транзиентные ошибки не сжигают Paid-Pending

**Files:**
- Modify: `crates/km-core/src/jobs.rs` (is_transient ~:475, recover_one ~:323, run ~:121, video status-guard ~:254)
- Test: `crates/km-core/tests/jobs.rs`

**Interfaces:**
- Consumes: `CoreError` (Http{status,..}, Network, Timeout, Io, Db), `Jobs::{run, recover_pending}`, `Storage`.
- Produces: поведение — транзиентные ошибки оставляют Pending; definitive (404/410, пустой результат, невалидный id) → Failed.

- [ ] **Step 1: Падающие тесты**

В `tests/jobs.rs`:
- `recovery_http_5xx_keeps_pending`: мок generation 200 → download отдаёт 503 → запись Pending с request_id, не Failed.
- `recovery_http_429_keeps_pending`: 429 → Pending.
- `recovery_db_error_keeps_pending`: имитировать Db-ошибку записи (закрыть/залочить БД) → Pending, не Failed.
- `run_transient_download_keeps_pending`: generate_video 200 (request_id сохранён) → download Network/Timeout → запись Pending с request_id (recovery подхватит).
- `video_processing_keeps_pending`: VideoResponse status="processing" с id → Pending, request_id сохранён (поднять set_request_id выше status-guard).
- Регресс: definitive 404/410/пустой результат → Failed (существующие тесты зелёные).

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test jobs recovery_http_5xx` → FAIL.

- [ ] **Step 3: Реализовать**

`is_transient`: добавить `Http{status: 500..=599 | 429, ..}` и `Db(_)` к `Network|Timeout|Io`. `recover_one`: транзиентная → Ok(rec) Pending. `run`: после `set_request_id`, при транзиентной ошибке скачивания/генерации → оставить Pending (update только request_id, не Failed), вернуть Err для UI но запись Pending. Video: `set_request_id` ДО status-guard; `processing`/`pending`/`in_progress` → Pending (не Failed); `completed|succeeded` + непустые videos → Completed; иначе Failed.

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-core` → PASS (все, включая регресс).

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "fix: транзиентные ошибки (5xx/429/Db/run/processing) не сжигают Paid-Pending"
```

---

### Task 2: km-ui-core — i18n, токены тем, форматтеры (нативный крейт)

**Files:**
- Create: `crates/km-ui-core/{Cargo.toml,src/lib.rs,src/i18n.rs,src/theme.rs,src/format.rs}`
- Modify: `Cargo.toml` (workspace members)
- Test: `crates/km-ui-core/tests/{i18n.rs,theme.rs,format.rs}`

**Interfaces:**
- Consumes: `km_shared::{Lang, Theme}`.
- Produces:
```rust
// i18n.rs
pub fn tr(lang: Lang, key: &str) -> &'static str; // словарь ru/en, fallback en→key
// theme.rs
pub fn theme_vars(theme: Theme) -> &'static str; // CSS-переменные строкой для data-theme (dark/light); System резолвится в UI
pub const ACCENT: &str = "#e05616"; pub const ACCENT_HOVER: &str = "#ff7333";
// format.rs
pub fn format_usd(v: f64) -> String;       // "$0.04", 2 знака; non-finite → "—"
pub fn format_duration(secs: f64) -> String; // "0:12", "1:05"
pub fn format_date(unix_secs: i64, lang: Lang) -> String; // "сегодня"/"вчера"/"20 ноя" (ru), "today"/... (en) — относительно now
```

- [ ] **Step 1: Падающие тесты**

- i18n: `tr(Ru,"settings.title")`=="Настройки", `tr(En,"settings.title")`=="Settings"; неизвестный ключ → fallback (en или key); все ключи экранов покрыты (словарь содержит studio/history/settings/onboarding/common).
- theme: `theme_vars(Dark)` содержит `--bg:#0a0c10` и `--accent:#e05616`; `theme_vars(Light)` содержит `--bg:#f5f6f8`; ACCENT==#e05616.
- format: `format_usd(0.04)`=="$0.04"; `format_usd(f64::NAN)`=="—"; `format_duration(12.0)`=="0:12"; `format_duration(65.0)`=="1:05"; `format_date(now,..)`=="сегодня"/"today".

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-ui-core` → FAIL (крейта нет).

- [ ] **Step 3: Реализовать**

Создать крейт (lib, нативный, без dioxus/web). i18n — статические словари (match key→ru/en), покрыть ключи всех экранов Плана 2/3. theme — CSS-переменные (`--bg,--surface,--border,--text,--text-muted,--accent,--accent-hover`) для dark/light. format — чистые функции. Добавить в workspace members.

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-ui-core` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-ui-core Cargo.toml Cargo.lock && git commit -m "feat: km-ui-core — i18n ru/en, токены тем, форматтеры"
```

---

### Task 3: Tauri-биндинг (km-ui) — invoke + события

**Files:**
- Create: `ui/src/ipc.rs`
- Modify: `ui/Cargo.toml` (tauri-sys, km-shared, km-ui-core, serde, serde_json, wasm-bindgen, wasm-bindgen-futures)
- Test: нативная логика сериализации аргументов (если выделяется) + smoke

**Interfaces:**
- Consumes: Tauri-команды (Global Constraints), `km_shared::{Settings, CatalogModel, GenerationRequest, HistoryRecord, HistoryFilter, BalanceView, MediaKind, AppEvent}`.
- Produces:
```rust
// ipc.rs — обёртки над tauri-sys invoke (args camelCase JSON)
pub async fn get_settings() -> Result<Settings, String>;
pub async fn set_settings(s: &Settings) -> Result<(), String>;
pub async fn set_api_key(key: &str) -> Result<(), String>;
pub async fn test_connection() -> Result<bool, String>;
pub async fn fetch_models(kind: MediaKind) -> Result<Vec<CatalogModel>, String>;
pub async fn generate(req: &GenerationRequest) -> Result<HistoryRecord, String>;
pub async fn list_history(filter: &HistoryFilter) -> Result<Vec<HistoryRecord>, String>;
pub async fn delete_history(id: &str) -> Result<(), String>;
pub async fn recover_pending() -> Result<Vec<HistoryRecord>, String>;
pub async fn fetch_balance() -> Result<BalanceView, String>;
pub async fn pick_folder() -> Result<Option<String>, String>;
pub async fn open_file(path: &str) -> Result<(), String>;
pub async fn show_in_folder(path: &str) -> Result<(), String>;
pub fn on_event<F: Fn(AppEvent) + 'static>(cb: F); // listen generation:* + balance:updated, парсит AppEvent
```

- [ ] **Step 1: Реализовать биндинг**

`tauri-sys` invoke (вызов `__TAURI_INTERNALS__.invoke`, без npm). Если tauri-sys несовместим с Dioxus 0.7 — тонкая wasm-bindgen-обёртка над `window.__TAURI_INTERNALS__.invoke`/`event.listen` (допустимо spec). Аргументы — serde JSON camelCase (`{kind}`, `{req}`, `{filter}`, `{settings}`, `{key}`, `{id}`, `{path}`). Ошибки invoke → String. `on_event` подписывается на 4 события, парсит payload в AppEvent (externally-tagged).

- [ ] **Step 2: Проверка сборки + сериализации**

Run: `dx build --platform web` (или `cargo check -p km-ui --target wasm32-unknown-unknown`) → успех. Нативный тест аргумент-сериализации (если выделен helper): `fetch_models` шлёт `{"kind":"image"}`.

- [ ] **Step 3: Smoke IPC**

`cargo tauri dev`, в консоли webview вызвать `get_settings`/`fetch_models('image')` через биндинг → данные приходят (ручная проверка; ключ не нужен для fetch_models public).

- [ ] **Step 4: Commit**

```bash
git add ui && git commit -m "feat: Tauri IPC-биндинг (tauri-sys) — команды + события"
```

---

### Task 4: App shell — роутер, глобальное состояние, темы, i18n

**Files:**
- Create: `ui/src/{app.rs,router.rs,state.rs,components/layout.rs}`
- Modify: `ui/src/main.rs`, `ui/assets/style.css`
- Test: smoke (dx build + cargo tauri dev)

**Interfaces:**
- Consumes: `ipc::*`, `km_ui_core::{tr, theme_vars}`, `km_shared::{Settings, Theme, Lang, CatalogModel, HistoryRecord, BalanceView}`.
- Produces: `Route` (Routable: Studio/History/Settings/Onboarding), `AppState` контекст (сигналы: settings, models_by_kind, history, balance, lang, theme, busy/error), `Layout` (сайдбар + контент), хук `use_t(key)` для i18n.

- [ ] **Step 1: Реализовать шелл**

`router.rs`: `#[derive(Routable)] enum Route { Studio, History, Settings, Onboarding }`. `state.rs`: глобальный контекст-сигналы (settings, lang, theme, models, history, balance, error); загрузка settings при старте (`get_settings`), применение `data-theme` (System → matchMedia), подписка `on_event` (обновление history/balance по событиям). `layout.rs`: сайдбар (Изображения/Видео/Аудио→Studio, История, Настройки) + outlet. `style.css`: CSS-переменные тем из `theme_vars`, базовый layout. `main.rs`: `dioxus::launch(App)` с Router + контекст-провайдер.

- [ ] **Step 2: Проверка сборки**

Run: `dx build --platform web` → успех; `cargo clippy -p km-ui --target wasm32-unknown-unknown -- -D warnings` (или workspace) чисто.

- [ ] **Step 3: Smoke навигации/тем**

`cargo tauri dev`: сайдбар рендерится, переключение экранов работает, смена темы в Settings меняет `data-theme` без перезапуска, смена языка меняет подписи.

- [ ] **Step 4: Commit**

```bash
git add ui && git commit -m "feat: app shell — роутер, состояние, темы, i18n, сайдбар"
```

---

### Task 5: Экран Настройки

**Files:**
- Create: `ui/src/screens/settings.rs`
- Test: smoke + нативная логика (валидация ключа/папки если выделена)

**Interfaces:**
- Consumes: `ipc::{get_settings,set_settings,set_api_key,test_connection,pick_folder,fetch_balance}`, `use_t`, AppState.
- Produces: экран Settings (ключ + проверка подключения, папка + выбор, язык ru/en, тема dark/light/system, show_real_balance, баланс/расход).

- [ ] **Step 1: Реализовать экран**

Поля: API-ключ (password input, `set_api_key` + кнопка «Проверить» → `test_connection`, статус ok/ошибка/нет ключа), папка (`pick_folder` → `set_settings`), язык (select → `set_settings` + сигнал lang), тема (select → `set_settings` + data-theme), toggle show_real_balance, блок баланса (`fetch_balance`: реальный если show_real_balance и есть, иначе локальный расход `format_usd`). Ошибки IPC → внятное сообщение (Review Focus 4/5). Сохранение через `set_settings`.

- [ ] **Step 2: Проверка сборки**

Run: `dx build --platform web` → успех; clippy/fmt чисто.

- [ ] **Step 3: Smoke**

`cargo tauri dev`: ввод ключа + проверка (401→ошибка, валидный→ok), выбор папки, смена языка/темы применяется и сохраняется (перезапуск — значения те же), баланс отображается.

- [ ] **Step 4: Commit**

```bash
git add ui && git commit -m "feat: экран Настройки — ключ, папка, язык, тема, баланс"
```

---

### Task 6: Онбординг (первый запуск)

**Files:**
- Create: `ui/src/screens/onboarding.rs`
- Modify: `ui/src/app.rs` (редирект на Onboarding если ключа нет)
- Test: smoke + нативная логика шагов

**Interfaces:**
- Consumes: `ipc::{set_api_key,test_connection,pick_folder,set_settings,get_settings}`, AppState.
- Produces: экран Onboarding (шаги: приветствие→ключ→папка→готово), логика редиректа.

- [ ] **Step 1: Реализовать**

Шаги: (1) приветствие + выбор языка, (2) ввод ключа + `test_connection` (можно пропустить? нет — ключ нужен; но каталог доступен без ключа — допустим пропуск с предупреждением), (3) выбор папки (`pick_folder`, обязательно для сохранения), (4) готово → `set_settings` + переход в Studio. Редирект: при старте если ключа нет (test_connection false / keychain пуст) → Onboarding. Пропуск ключа допустим (каталог без ключа), но генерация потребует ключ later.

- [ ] **Step 2: Проверка сборки**

Run: `dx build --platform web` → успех; clippy/fmt чисто.

- [ ] **Step 3: Smoke**

`cargo tauri dev` с чистым keychain/БД: онбординг показывается, шаги проходят, после завершения — Studio; повторный запуск с ключом — онбординг не показывается.

- [ ] **Step 4: Commit**

```bash
git add ui && git commit -m "feat: онбординг первого запуска — ключ, папка, язык"
```