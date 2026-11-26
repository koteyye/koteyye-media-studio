# Koteyye Media Studio 2.0 — План 1: фундамент и Rust-ядро

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Каркас Tauri 2 + Dioxus и полностью протестированное Rust-ядро (CI-клиент, каталог, SQLite-история, файлы, джобы с восстановлением, команды), на котором План 2 построит UI.

**Architecture:** Cargo workspace из `crates/km-shared` (serde-типы), `crates/km-core` (вся логика, без Tauri — тестируется wiremock), `src-tauri` (тонкие команды + события + keychain) и `ui` (Dioxus WASM, пока заглушка). Ядро не зависит от Tauri, UI вызывает команды через tauri-sys.

**Tech Stack:** Rust stable 1.100, Tauri 2, Dioxus (CSR/WASM), reqwest + tokio, rusqlite, wiremock (тесты), keyring, tauri-plugin-updater (План 2).

**Spec:** `docs/superpowers/specs/2026-11-24-tauri-rewrite-design.md`

## Global Constraints

- Rust stable (установлен 1.100.0); Tauri 2 stable; Dioxus — последний stable, версии пиновать в Cargo.toml при скаффолде.
- CI base URL: `https://api.cheaperinference.com`; аутентификация `Authorization: Bearer ci_live_...`.
- Целевые платформы macOS + Windows; локальная разработка macOS aarch64 (релизные сборки — GitHub Actions, План 2).
- Имя `Koteyye media studio`, версия `2.0.0`, bundle id `ru.koteyye.media-studio`.
- Все результаты сохраняются локально; история в SQLite ссылается на локальные пути; восстановление через `GET /v1/generations/{request_id}`.
- OpenRouter не используется. JavaScript/TypeScript нет; допустимы минимальные wasm-bindgen-обёртки.
- Комментарии и документация — на русском; идентификаторы — на английском.
- `billed_cost_usd` — строка с 6 знаками (микродоллары), не float в JSON.
- Обычный ключ не читает баланс (нужен scope `account:read`): 403 → фолбэк на локальный расход.

## Review Focus

1. **Строковый биллинг**: `billed_cost_usd: "0.000000"` и `null` не должны паниковать или терять точность; сумма расходов сходится. → тест в Task 3.
2. **Таймаут видео и «краш» между ответом и сохранением**: таймаут без ответа → внятный Failed; Pending с request_id после краша → recovery забирает файлы, а не теряет их. → Task 8, 11.
3. **Протухшие ссылки**: signed URL живут 1 час, файлы 7 дней; история и плеер опираются на локальные файлы, истечение remote не ломает удаление/повтор. → Task 11.
4. **Ключ без account:read**: 403 на балансе → локальный расход, а не ошибка UI. → Task 11, 12.
5. **Папка сохранения исчезла**: удалённая/переименованная папка между запусками — пересоздать или внятная ошибка, не паника. → Task 10.

---

### Task 1: Инструментарий, workspace, каркас Tauri + Dioxus

**Files:**
- Create: `Cargo.toml` (workspace), `crates/km-shared/{Cargo.toml,src/lib.rs}`, `crates/km-core/{Cargo.toml,src/lib.rs}`, `src-tauri/{Cargo.toml,tauri.conf.json,capabilities/default.json,src/main.rs,src/lib.rs}`, `ui/{Cargo.toml,Dioxus.toml,src/main.rs,assets/style.css}`, `.gitignore` (дополнить)
- Delete: `src/`, `electron.vite.config.ts`, `tsconfig*.json`, `package.json`, `package-lock.json`, `scripts/prepare-icons.mjs` (старый Electron; иконки `src/assets/icons` перенести в `ui/assets/icons` до удаления)

**Interfaces:**
- Produces: workspace-крейты `km-shared`, `km-core`, `koteyye-media-studio` (bin src-tauri), `km-ui`; команда `cargo tauri dev` открывает окно с заглушкой Dioxus.

- [ ] **Step 1: Установить инструментарий**

```bash
rustup target add wasm32-unknown-unknown
cargo install tauri-cli --locked   # v2
cargo install dioxus-cli --locked  # dx
```

Проверка: `cargo tauri --version` → 2.x; `dx --version` печатает версию.

- [ ] **Step 2: Перенести иконки, удалить Electron-код**

Скопировать `src/assets/icons/koteyye_media_studio_logo.png` и `.ico` в `ui/assets/icons/`, затем удалить файлы Electron (список выше). Старый код остаётся в git-истории.

- [ ] **Step 3: Создать workspace и крейты**

`Cargo.toml`: `[workspace] members = ["crates/km-shared","crates/km-core","src-tauri","ui"]`, resolver = "2". `km-shared` и `km-core` — пустые lib с заглушечным тестом. Зависимости пиновать по последним stable (`cargo add`).

- [ ] **Step 4: Каркас src-tauri**

`tauri.conf.json`: productName "Koteyye media studio", version "2.0.0", identifier "ru.koteyye.media-studio", build: `devUrl http://localhost:8080`, `frontendDist ../ui/dist`, `beforeDevCommand "dx serve --port 8080"`, `beforeBuildCommand "dx build --release"`. `capabilities/default.json`: core:default + окна. `src/lib.rs`: `tauri::Builder::default().run(...)`; `main.rs` вызывает её.

- [ ] **Step 5: Заглушка Dioxus**

`ui/src/main.rs`: `dioxus::launch(App)`; `App` рендерит `<h1>Koteyye Media Studio 2.0</h1><p>Каркас работает.</p>`. `Dioxus.toml`: web, base_path ".", out_dir "dist". `assets/style.css` подключить.

- [ ] **Step 6: Проверка сборки и окна**

Run: `cargo build --workspace` → успех.
Run: `cargo tauri dev` → открывается окно с заголовком-заглушкой (проверить вручную, скриншот/лог).

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat: каркас Tauri 2 + Dioxus, workspace, снос Electron"
```

---

### Task 2: km-shared — типы каталога моделей

**Files:**
- Create: `crates/km-shared/src/models.rs`, `crates/km-shared/tests/fixtures/public-models.json`
- Modify: `crates/km-shared/src/lib.rs`

**Interfaces:**
- Produces: `CatalogModel`, `ModelType`, `MediaPrices` — используются в Task 5 (каталог) и Планом 2 (дропдаун моделей).

```rust
pub enum ModelType { Image, Video, Speech, Text, Embedding, Transcription, Rerank, Decision, Realtime, Unknown(String) }
pub struct CatalogModel {
    pub id: String,
    pub model_type: ModelType,
    pub provider_name: Option<String>,
    pub logo_url: Option<String>,
    pub features: Vec<String>,
    pub supported_endpoints: Vec<String>,
    pub media_unit: Option<String>,
    pub media_prices: std::collections::BTreeMap<String, String>,
    pub discount_percent: Option<String>,
    pub supports_image_edit: bool,
    pub aliases: Vec<String>,
}
impl CatalogModel {
    pub fn supports(&self, endpoint: &str) -> bool;
    pub fn has_feature(&self, feature: &str) -> bool;
    pub fn price_for(&self, key: &str) -> Option<f64>; // "720P+AUDIO" и т.п.
}
```

- [ ] **Step 1: Фикстура из живого каталога**

```bash
curl -s https://api.cheaperinference.com/public/models -o /tmp/ci-models.json
node -e 'const j=require("/tmp/ci-models.json");const want=["veo-3.1-generate-001","flux-2-pro","music-v2","eleven-v4","kling-v3.0-i2v","gpt-image-2","wan-v2.7-r2v","sound-effects-v2","tts-1","seedream-5.0-pro"];const out={models:j.models.filter(m=>want.includes(m.id))};require("fs").writeFileSync("crates/km-shared/tests/fixtures/public-models.json",JSON.stringify(out,null,2))'
```

- [ ] **Step 2: Падающий тест десериализации**

`crates/km-shared/tests/catalog.rs`: загрузить фикстуру, распарсить в `Vec<CatalogModel>`; ассерты: 10 моделей; `veo-3.1-generate-001` → `ModelType::Video`, `features` содержит "image-input", `price_for("720P+AUDIO") == Some(0.26)`; `music-v2` → `ModelType::Speech`, endpoint `/v1/audio/speech`; `flux-2-pro` → `ModelType::Image`.

- [ ] **Step 3: Запустить тест — упадёт**

Run: `cargo test -p km-shared --test catalog` → FAIL (типы не определены).

- [ ] **Step 4: Реализовать типы**

`models.rs`: структуры выше, `#[derive(Deserialize, Serialize, Clone, Debug)]`, `ModelType` — serde rename lowercase + `Unknown` через `#[serde(other)]`-совместимую схему (кастомный Deserialize или `serde_repr`-подход: строковый enum с fallback). Неизвестные поля игнорировать.

- [ ] **Step 5: Тест проходит**

Run: `cargo test -p km-shared --test catalog` → PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/km-shared && git commit -m "feat: типы каталога CI + фикстура из живого /public/models"
```

---

### Task 3: km-shared — запросы, ответы, история, события

**Files:**
- Create: `crates/km-shared/src/{requests.rs,responses.rs,history.rs,events.rs}`
- Modify: `crates/km-shared/src/lib.rs`
- Test: `crates/km-shared/tests/types.rs`

**Interfaces:**
- Produces: типы ниже — используются во всех задачах ядра и командах Task 12.

```rust
// responses.rs
pub struct Billing { pub billed_cost_usd: Option<String> }
impl Billing { pub fn cost_usd(&self) -> Option<f64>; } // парсит строку, None при null/мусоре
pub struct ImageItem { pub b64_json: Option<String>, pub url: Option<String>, pub content_type: Option<String> }
pub struct ImageResponse { pub created: Option<i64>, pub data: Vec<ImageItem>, pub request_id: Option<String>, pub billing: Option<Billing> }
pub struct VideoItem { pub url: Option<String>, pub content_type: Option<String> }
pub struct VideoResponse { pub id: String, pub status: String, pub videos: Vec<VideoItem>, pub request_id: Option<String>, pub billing: Option<Billing>, pub usage: Option<serde_json::Value> }
pub struct GenerationFile { pub index: u32, #[serde(rename="type")] pub kind: String, pub content_type: String, pub size_bytes: u64, pub url: String }
pub struct Generation { pub request_id: String, pub endpoint: String, pub created_at: String, pub expires_at: String, pub files: Vec<GenerationFile> }
pub struct AccountBalance { pub currency: String, pub balance_usd: f64, pub available_usd: f64, pub reserved_usd: f64 }
pub struct AudioOutput { pub bytes: Vec<u8>, pub content_type: String, pub cost_usd: Option<f64> }

// requests.rs
pub enum MediaKind { Image, Video, Audio }
pub enum GenMode { TextToImage, ImageEdit, TextToVideo, ImageToVideo, ReferenceToVideo, Speech, Music, SoundEffects }
pub struct GenParams { pub size: Option<String>, pub resolution: Option<String>, pub aspect_ratio: Option<String>, pub duration_seconds: Option<f64>, pub n: Option<u32>, pub seed: Option<i64>, pub voice: Option<String>, pub response_format: Option<String>, pub speed: Option<f64>, pub audio: Option<bool>, pub quality: Option<String> }
pub struct Reference { pub data_url: String }           // изображение как data-URL
pub struct GenerationRequest { pub kind: MediaKind, pub mode: GenMode, pub model: String, pub prompt: String, pub params: GenParams, pub references: Vec<Reference>, pub mask_data_url: Option<String> }

// history.rs
pub enum JobStatus { Pending, Completed, Failed }
pub struct HistoryRecord { pub id: String, pub created_at: i64, pub updated_at: i64, pub kind: MediaKind, pub mode: GenMode, pub model: String, pub prompt: String, pub status: JobStatus, pub request_id: Option<String>, pub local_paths: Vec<String>, pub cost_usd: Option<f64>, pub params_json: String, pub error: Option<String> }
pub struct HistoryPatch { pub status: Option<JobStatus>, pub request_id: Option<String>, pub local_paths: Option<Vec<String>>, pub cost_usd: Option<f64>, pub error: Option<String> }
pub struct HistoryFilter { pub kind: Option<MediaKind>, pub search: Option<String> }

// events.rs
pub enum AppEvent { GenerationStarted { history_id: String }, GenerationDone { history_id: String }, GenerationFailed { history_id: String, error: String }, BalanceUpdated }
pub struct BalanceView { pub real: Option<AccountBalance>, pub local_spend_usd: f64 }
pub enum Lang { Ru, En }
pub enum Theme { Dark, Light, System }
pub struct Settings { pub local_folder: String, pub lang: Lang, pub theme: Theme, pub show_real_balance: bool }
```

- [ ] **Step 1: Падающие тесты**

`tests/types.rs`:
- `Billing::cost_usd`: `"0.123456"` → `Some(0.123456)`; `"0.000000"` → `Some(0.0)`; `None` → `None`; мусор `"abc"` → `None` (не паника).
- `ImageResponse` десериализация: `{"created":1,"data":[{"b64_json":"aGk="}],"request_id":"uuid","billing":{"billed_cost_usd":"0.040000"}}` → data[0].b64_json == "aGk=", cost 0.04.
- `VideoResponse`: `{"id":"v1","status":"completed","videos":[{"url":"https://x/v.mp4","content_type":"video/mp4"}]}` → videos[0].url.
- `Generation`/`GenerationFile`: фикстура с files[0] {index:0,type:"video",content_type:"video/mp4",size_bytes:10,url:"https://x"} → поля совпадают.
- `HistoryRecord` serde round-trip (JSON → struct → JSON, ключевые поля равны).

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-shared --test types` → FAIL.

- [ ] **Step 3: Реализовать типы**

Файлы выше; все `#[derive(Serialize, Deserialize, Clone, Debug)]`; enum'ы — lowercase rename; опциональные поля `#[serde(default)]` где CI может не прислать. `cost_usd` — `str::parse::<f64>().ok()`.

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-shared` → PASS (catalog + types).

- [ ] **Step 5: Commit**

```bash
git add crates/km-shared && git commit -m "feat: типы запросов/ответов CI, история, события, настройки"
```

---

### Task 4: km-core — HTTP-клиент и ошибки

**Files:**
- Create: `crates/km-core/src/{client.rs,error.rs}`
- Modify: `crates/km-core/src/lib.rs`, `crates/km-core/Cargo.toml` (reqwest, tokio, wiremock dev)
- Test: `crates/km-core/tests/client_errors.rs`

**Interfaces:**
- Consumes: `km-shared::responses::*`
- Produces: `CiClient::new(key: &str) -> CiClient`, `CiClient::with_base(key: &str, base: &str)` (для тестов), `CoreError`, метод `get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, CoreError>`, `post_json<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T, CoreError>`.

```rust
pub enum CoreError {
    Http { status: u16, code: Option<String>, message: String },
    ScopeMissing,                 // 403 на account-эндпоинтах
    NoKey,
    Timeout,
    Network(String),
    Io(std::io::Error),
    Db(String),
    Serde(String),
}
```

- [ ] **Step 1: Падающие тесты (wiremock)**

`tests/client_errors.rs`: мок-сервер, `CiClient::with_base("ci_live_test", &server.uri())`:
- 401 `{"error":{"code":"invalid_api_key","message":"bad key"}}` → `CoreError::Http{status:401, code:Some("invalid_api_key")}`.
- 402 insufficient_balance → status 402.
- 403 на `/v1/account/balance` → `CoreError::ScopeMissing`.
- Невалидный JSON 200 → `CoreError::Serde`.
- Запрос несёт заголовок `Authorization: Bearer ci_live_test` (ассерт через mock expectations).

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test client_errors` → FAIL.

- [ ] **Step 3: Реализовать клиент**

`client.rs`: reqwest::Client с таймаутом по умолчанию 60с (видео переопределит), base URL, заголовок Authorization. Разбор ошибок: тело `{"error":{code,message}}` (OpenAI-конверт CI); 403 на account-путях → ScopeMissing. `error.rs`: CoreError + Display на русском.

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-core --test client_errors` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: CI HTTP-клиент, разбор ошибок, ScopeMissing"
```

---

### Task 5: km-core — каталог моделей и кэш

**Files:**
- Create: `crates/km-core/src/catalog.rs`
- Test: `crates/km-core/tests/catalog.rs`

**Interfaces:**
- Consumes: `CiClient::{get_json}`, `km-shared::CatalogModel`
- Produces: `CiClient::public_models() -> Result<Vec<CatalogModel>>` (без ключа, путь `/public/models`), `CiClient::models() -> Result<Vec<CatalogModel>>` (`/v1/models`), `CatalogCache::new(ttl: Duration)`, `CatalogCache::get_or_fetch(&self, client: &CiClient) -> Result<Vec<CatalogModel>>`, фильтры `by_kind(models, MediaKind) -> Vec<&CatalogModel>` (Image→model_type image; Video→video; Audio→speech), `for_mode(models, GenMode) -> Vec<&CatalogModel>` (ImageEdit→supports("/v1/images/edits"); ImageToVideo→feature "image-input"; ReferenceToVideo→endpoint videos + feature "image-input" или r2v в id).

- [ ] **Step 1: Падающие тесты**

- `public_models` с моком (фикстура Task 2) → 10 моделей, запрос БЕЗ Authorization.
- `models` → с Authorization.
- Кэш: два вызова `get_or_fetch` → один HTTP-запрос (mock expect 1); после истечения TTL (ttl=0) → два.
- `by_kind(Audio)` включает music-v2 и sound-effects-v2 (оба speech).
- `for_mode(ImageEdit)` включает только модели с `/v1/images/edits`.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test catalog` → FAIL.

- [ ] **Step 3: Реализовать**

`catalog.rs`: обёртка `{"models":[...]}` для public и `{"data":[...]}` для /v1/models (проверить живьём; поддержать оба ключа). Кэш — `Mutex<Option<(Instant, Vec<CatalogModel>)>>`. Фильтры — чистые функции.

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-core --test catalog` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: каталог CI, кэш с TTL, фильтры по типу и режиму"
```

---

### Task 6: km-core — изображения (генерация и редактирование)

**Files:**
- Create: `crates/km-core/src/images.rs`
- Test: `crates/km-core/tests/images.rs`

**Interfaces:**
- Consumes: `CiClient::post_json`, `km-shared::{ImageResponse, GenerationRequest}`
- Produces: `CiClient::generate_image(&self, req: &GenerationRequest) -> Result<ImageResponse>`, `CiClient::edit_image(&self, req: &GenerationRequest) -> Result<ImageResponse>`, `image_bytes(item: &ImageItem, client: &CiClient) -> Result<Vec<u8>>` (b64 decode или download url).

- [ ] **Step 1: Падающие тесты**

- generate: мок `/v1/images/generations`, ассерт тела: model, prompt, n=2, size; ответ 2×b64 → `data.len()==2`.
- edit: мок `/v1/images/edits`, тело содержит `input_references: ["data:image/png;base64,..."]` и `mask_reference`; prompt ≤ 32000.
- `image_bytes`: b64 "aGk=" → b"hi"; item с url → скачивает через мок.
- Пустой prompt → ошибка до HTTP-запроса (валидация).

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test images` → FAIL.

- [ ] **Step 3: Реализовать**

`images.rs`: маппинг GenerationRequest → тело CI (n, size, quality, response_format; для edit — input_references строками data-URL, mask_reference). Проброс дополнительных параметров не нужен (YAGNI).

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-core --test images` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: генерация и редактирование изображений CI"
```

---

### Task 7: km-core — аудио (озвучка / музыка / звуки)

**Files:**
- Create: `crates/km-core/src/audio.rs`
- Test: `crates/km-core/tests/audio.rs`

**Interfaces:**
- Consumes: `CiClient` (нужен `post_bytes(&self, path, body) -> Result<(Vec<u8>, Headers)>` — добавить в client.rs), `km-shared::{AudioOutput, GenerationRequest}`
- Produces: `CiClient::speech(&self, req: &GenerationRequest) -> Result<AudioOutput>`.

- [ ] **Step 1: Падающие тесты**

- Мок `/v1/audio/speech` отдаёт бинарный mp3 + заголовок `x-ci-billed-cost-usd: 0.020000`; тело запроса: model "eleven-v4", input, voice, response_format "mp3", speed.
- `AudioOutput.bytes` == тело, `content_type` из Content-Type, `cost_usd == Some(0.02)`.
- Музыка: model "music-v2", без voice → тело без voice; режим Music/SoundEffects маппится на тот же endpoint.
- input > 4096 символов → ошибка валидации до запроса.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test audio` → FAIL.

- [ ] **Step 3: Реализовать**

`audio.rs`: один endpoint на все три режима; voice только для Speech; response_format по умолчанию "mp3"; cost из заголовка. `post_bytes` в client.rs — возвращает bytes + headers.

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-core --test audio` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: аудио CI — озвучка, музыка, звуки через /v1/audio/speech"
```

---

### Task 8: km-core — видео и восстановление

**Files:**
- Create: `crates/km-core/src/video.rs`
- Test: `crates/km-core/tests/video.rs`

**Interfaces:**
- Consumes: `CiClient::{post_json, get_json}`, `km-shared::{VideoResponse, Generation, GenerationRequest}`
- Produces: `CiClient::generate_video(&self, req: &GenerationRequest) -> Result<VideoResponse>` (таймаут запроса 10 мин), `CiClient::generation(&self, request_id: &str) -> Result<Generation>`, `CiClient::download(&self, url: &str) -> Result<Vec<u8>>` (signed URL, БЕЗ Authorization).

- [ ] **Step 1: Падающие тесты**

- generate t2v: тело model/prompt/duration_seconds/resolution/aspect_ratio/audio; ответ VideoResponse completed.
- i2v: `references[0]` → тело `image_url: "<data_url>"`; r2v: несколько → `input_references: [...]`.
- `generation("uuid")`: мок `/v1/generations/uuid` → Generation с files[0].url; запрос с Authorization.
- `download`: мок signed URL отдаёт байты БЕЗ Authorization (ассерт отсутствие заголовка).
- Таймаут: мок задерживает ответ > клиентского таймаута (в тесте таймаут 100мс через with_base-конфиг) → `CoreError::Timeout`.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test video` → FAIL.

- [ ] **Step 3: Реализовать**

`video.rs`: маппинг режимов (TextToVideo→без картинок; ImageToVideo→image_url первый референс; ReferenceToVideo→input_references). Отдельный reqwest-клиент с таймаутом 600с для видео. `generation` и `download` — как в интерфейсе.

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-core --test video` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: видео CI (t2v/i2v/r2v), recovery-эндпоинт, скачивание"
```

---

### Task 9: km-core — SQLite-хранилище

**Files:**
- Create: `crates/km-core/src/storage.rs`
- Test: `crates/km-core/tests/storage.rs`

**Interfaces:**
- Consumes: `km-shared::{HistoryRecord, HistoryPatch, HistoryFilter, JobStatus, Settings}`
- Produces: `Storage::open(path: &Path) -> Result<Storage>` (создаёт схему), `Storage::open_in_memory() -> Result<Storage>` (тесты), `insert_history(&self, rec: &HistoryRecord) -> Result<()>`, `update_history(&self, id: &str, patch: &HistoryPatch) -> Result<()>`, `history(&self, filter: &HistoryFilter) -> Result<Vec<HistoryRecord>>` (новые первыми), `delete_history(&self, id: &str) -> Result<()>`, `pending(&self) -> Result<Vec<HistoryRecord>>`, `total_spend_usd(&self) -> Result<f64>`, `get_settings(&self) -> Result<Settings>`, `set_settings(&self, s: &Settings) -> Result<()>`.

- [ ] **Step 1: Падающие тесты**

- insert + history → запись найдена, порядок по created_at desc.
- update_history(status Completed, local_paths, cost 0.04) → поля обновились, updated_at вырос.
- pending → только Pending.
- total_spend_usd: записи 0.04 + 0.78 + None → 0.82 (без паники на None).
- filter kind=Video → только видео; search "кот" → по prompt (регистронезависимо).
- delete → запись исчезла.
- settings round-trip; дефолт при пустой БД (lang Ru, theme System, folder "").

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test storage` → FAIL.

- [ ] **Step 3: Реализовать**

`storage.rs`: rusqlite, таблица `history` (local_paths и params — JSON-текст), `settings` (одна строка JSON). Миграция схемы при open (CREATE TABLE IF NOT EXISTS). Сумма расхода — SQL `SUM(cost_usd)` с COALESCE.

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-core --test storage` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: SQLite — история, расход, настройки"
```

---

### Task 10: km-core — файлы

**Files:**
- Create: `crates/km-core/src/files.rs`
- Test: `crates/km-core/tests/files.rs`

**Interfaces:**
- Consumes: `km-shared::MediaKind`
- Produces: `ensure_folder(folder: &Path) -> Result<()>` (создаёт при отсутствии), `save_media(folder: &Path, kind: MediaKind, index: usize, total: usize, bytes: &[u8], content_type: &str) -> Result<PathBuf>`, `ext_for(content_type: &str) -> &'static str`.

- [ ] **Step 1: Падающие тесты (tempdir)**

- save_media image/png, total=1 → файл `image-<ts>.png` существует, байты совпадают.
- total=3, index=1 → суффикс `-2` в имени.
- video/mp4 → `.mp4`; audio/mpeg → `.mp3`; audio/wav → `.wav`; неизвестный content_type → `.bin`.
- Папка не существует → создаётся и файл сохраняется (Review Focus 5).
- Путь папки — файл (не директория) → внятная `CoreError::Io`, не паника.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test files` → FAIL.

- [ ] **Step 3: Реализовать**

`files.rs`: имена `image-<unix_millis>[-N].<ext>`, `video-<unix_millis>[-N].mp4`, `audio-<unix_millis>[-N].<ext>`; ensure_folder — create_dir_all; проверка is_dir.

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-core --test files` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: сохранение медиа в локальную папку, имена, расширения"
```

---

### Task 11: km-core — оркестратор джобов, recovery, баланс

**Files:**
- Create: `crates/km-core/src/jobs.rs`
- Test: `crates/km-core/tests/jobs.rs`

**Interfaces:**
- Consumes: все методы `CiClient`, `Storage`, `files::*`, `km-shared::{GenerationRequest, HistoryRecord, AppEvent, BalanceView}`
- Produces: `Jobs::new(client: CiClient, storage: Storage, folder: PathBuf, emit: Arc<dyn Fn(AppEvent) + Send + Sync>) -> Jobs`, `Jobs::run(&self, req: GenerationRequest) -> Result<HistoryRecord>`, `Jobs::recover_pending(&self) -> Result<Vec<HistoryRecord>>`, `Jobs::balance_view(&self, show_real: bool) -> Result<BalanceView>`.

- [ ] **Step 1: Падающие тесты (wiremock + tempdir + in-memory DB)**

- **run image**: мок генерации (b64) → история Completed, файл на диске, cost записан, события Started→Done (порядок).
- **run video таймаут**: мок задерживается > таймаута → джоб Failed с ошибкой «таймаут», без request_id recovering невозможен (тело не получено).
- **recover_pending после «краша»**: запись Pending С request_id (ответ был получен, но приложение упало до сохранения) + мок generation → Completed, файл сохранён; запись Pending без request_id → Failed.
- **recover_pending**: запись Pending с request_id + мок generation → Completed, файл сохранён; запись Pending без request_id → Failed.
- **balance_view**: show_real=true, мок 200 → real Some; мок 403 → real None, local_spend из БД (Review Focus 4); show_real=false → только local.
- **Протухшая ссылка** (Review Focus 3): история Completed с local_paths → повторное скачивание не требуется; delete_history не трогает remote.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test jobs` → FAIL.

- [ ] **Step 3: Живая проверка + реализация**

Сначала проверить живым запросом (ключ пользователя, дешёвая модель) точные поля ответов — зафиксировать расхождения в комментарии. Реализовать `jobs.rs`: run = insert Pending → emit Started → вызов клиента по kind → bytes (b64/url/бинарь) → save_media → update Completed + cost → emit Done; при ошибке/таймауте — Failed + emit Failed. recover_pending = для каждой Pending с request_id → generation → download → save → Completed; без id → Failed. balance_view = try balance (ScopeMissing → None) + total_spend.

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-core --test jobs` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: оркестратор джобов, recovery, гибридный баланс"
```

---

### Task 12: src-tauri — команды, события, keychain

**Files:**
- Create: `src-tauri/src/{commands.rs,state.rs,keychain.rs}`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml` (keyring, tauri-plugin-dialog)
- Test: `src-tauri/tests/commands.rs` (тестируем функции команд напрямую с мок-ядром)

**Interfaces:**
- Consumes: `km-core::{CiClient, Storage, Jobs}`, `km-shared::*`
- Produces (Tauri-команды, имена пиновать): `get_settings() -> Settings`, `set_settings(Settings)`, `set_api_key(String)`, `test_connection() -> bool`, `fetch_models(MediaKind) -> Vec<CatalogModel>`, `generate(GenerationRequest) -> HistoryRecord`, `list_history(HistoryFilter) -> Vec<HistoryRecord>`, `delete_history(String)`, `recover_pending() -> Vec<HistoryRecord>`, `fetch_balance() -> BalanceView`, `pick_folder() -> Option<String>`, `open_file(String)`, `show_in_folder(String)`. События: `generation:started/done/failed`, `balance:updated` (payload — AppEvent serde).

- [ ] **Step 1: Падающие тесты**

`tests/commands.rs`: функции команд вынесены в чистые `async fn cmd_generate(state: &AppState, req) -> Result<HistoryRecord>` и т.п. Тесты с wiremock-ядром: generate → запись в истории; test_connection с 401 → false; fetch_balance 403 → BalanceView с local. Keychain в тестах — in-memory fallback (trait `KeyStore`). `open_file`/`show_in_folder`: путь вне папки сохранения → ошибка (защита как в v1, resolveSavedFilePath).

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p koteyye-media-studio` → FAIL.

- [ ] **Step 3: Реализовать**

`state.rs`: AppState { storage, jobs_factory, key_store, settings } под Mutex; БД в `app.path().app_data_dir()`. `keychain.rs`: trait KeyStore + keyring-реализация + in-memory для тестов; ключ не в SQLite. `commands.rs`: тонкие обёртки, эмиттер событий через `tauri::Emitter`; `open_file`/`show_in_folder` проверяют, что путь внутри папки сохранения. `lib.rs`: регистрация команд и плагинов (dialog), в setup — spawn `recover_pending()` при старте. Capabilities: разрешить dialog, core events.

- [ ] **Step 4: Тесты проходят + смоук**

Run: `cargo test --workspace` → PASS.
Run: `cargo tauri dev`, в devtools консоли: `await window.__TAURI__.core.invoke('fetch_models', {kind:'Image'})` с тестовым ключом → список моделей (ручная проверка, нужен ключ пользователя).

- [ ] **Step 5: Commit**

```bash
git add src-tauri && git commit -m "feat: Tauri-команды ядра, события, keychain для ключа"
```