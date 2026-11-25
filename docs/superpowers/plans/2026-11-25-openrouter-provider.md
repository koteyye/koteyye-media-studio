# Koteyye Media Studio 2.0 — План: провайдерный слой (OpenRouter основной, CI вторичный)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ввести абстракцию провайдера и полноценный OpenRouter (основной, default), сохранив CI рабочим вторичным; переключатель провайдера + два ключа в Настройках/Онбординге. Полные экраны Студии/Истории — следующим планом.

**Architecture:** Trait `Provider` (async_trait) в km-core с реализациями `OpenRouterProvider` и `CiProvider` (существующий CiClient адаптируется). Нормализованные `MediaModel`/`ProviderBalance`/`VideoOutcome` в km-shared — UI и Jobs работают с ними. Jobs умеет sync (CI) и async (OpenRouter видео submit→poll→download) потоки; recovery по провайдеру. История/настройки получают поле provider; keychain — два ключа.

**Tech Stack:** Rust stable 1.100, Tauri 2, Dioxus 0.7 WASM, reqwest/rustls, rusqlite, async_trait, wiremock (тесты).

**Spec:** `docs/superpowers/specs/2026-11-25-openrouter-provider-design.md`

## Global Constraints

- Rust stable 1.100; комментарии/доки на русском, идентификаторы на английском.
- OpenRouter base `https://openrouter.ai/api/v1`, Bearer; CI base `https://api.cheaperinference.com`, Bearer.
- ProviderId serde lowercase: `openrouter`, `ci`. Default для новых запросов/настроек — OpenRouter; существующие записи истории — Ci (миграция).
- Единицы времени — секунды; локальное сохранение первично; ключи в keychain (не в SQLite/бандле); path-safety; ключ/signed-URL не утекают в ошибки.
- Инварианты надёжности Планов 1–2: оплаченные генерации не теряются при транзиентных сбоях (is_transient: Network|Timeout|Io|Db|Http 5xx/429 → Pending), цена персистится сразу после ответа, recovery сериализован.
- Тесты wiremock + tempdir + реальная SQLite; существующие 277 тестов не регрессировать.
- Аудио OpenRouter = только TTS (синтезация); клонирование голоса — не-цель. Музыка/звуки — CI-модели.
- Ключ OpenRouter для живой проверки — через env `OPENROUTER_API_KEY` (пользователь предоставляет) или keychain; НЕ в чат/репо.

## Review Focus

1. **Транзиентный сбой не сжигает оплату у обоих провайдеров**: OpenRouter async-видео при обрыве poll/download → Pending с job-id; recovery подхватит. → Task 8.
2. **Цена не теряется**: OpenRouter submit/completed цена персистится сразу (по контракту Task 1); если submit без цены — зафиксировать при completed/poll. → Task 6, 8.
3. **Переключатель провайдера не ломает ключи**: два ключа изолированы; test_connection/generate используют ключ выбранного провайдера; отсутствие ключа OpenRouter не уводит в ложный онбординг при наличии CI. → Task 9, 10, 11.
4. **Миграция истории**: существующие записи (без provider) → Ci, не теряются; total_spend считается по всем. → Task 9.
5. **Нормализация каталога**: модели OpenRouter и CI маппятся в MediaModel без потери kind/modes/цен; дропдаун фильтруется по провайдеру. → Task 3, 4.

---

### Task 1: Живая проверка контрактов OpenRouter

**Files:**
- Create: `docs/superpowers/specs/openrouter-contract-notes.md`
- Test: нет (исследовательский шаг; артефакт — заметки)

**Interfaces:**
- Produces: заметки с фактами: кадры i2v (data-URL принимается? иначе ImgBB), формат `input_references` в `/images` (data-URL/URL), цена в submit/completed/poll ответах (поле, формат), каталоги требуют ключа? путь TTS-каталога, идемпотентность submit, формат `/credits`.

- [ ] **Step 1: Проверить каталоги и баланс (без генерации)**

Ключ из env `OPENROUTER_API_KEY`. Curl: `GET /images/models`, `/videos/models`, TTS-каталог (путь уточнить: `/audio/speech/models`), `/credits` — с Bearer и без (требуют ли ключа). Записать формы ответов (data[], pricing, supported_parameters).

- [ ] **Step 2: Проверить дешёвую генерацию (минимальные кредиты)**

Самая дешёвая image-модель: `POST /images` (prompt, n=1) → форма data[] (b64_json/url), наличие цены/usage. Кадры: `POST /videos` с frame_images data-URL (дешёвая модель) → принимает ли data-URL или нужен URL; submit-ответ {id, polling_url}; `GET /videos/{id}` → status/цена/unsigned_urls. TTS: `POST /audio/speech` (дешёвая модель, короткий input) → bytes/content-type/цена. Записать ВСЁ в заметки.

- [ ] **Step 3: Зафиксировать решения**

В заметках: data-URL vs ImgBB для кадров и image-референсов; где брать цену (submit/poll/completed); путь TTS-каталога; каталоги до ключа (да/нет); идемпотентность submit (retry-политика). Это ветвит Task 4–8.

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/openrouter-contract-notes.md && git commit -m "docs: живая проверка контрактов OpenRouter"
```

---

### Task 2: km-shared — ProviderId, нормализованные типы, provider-поля

**Files:**
- Create: `crates/km-shared/src/provider.rs`
- Modify: `crates/km-shared/src/{requests.rs,history.rs,events.rs,lib.rs}`
- Test: `crates/km-shared/tests/provider.rs`

**Interfaces:**
- Produces:
```rust
pub enum ProviderId { OpenRouter, Ci }  // lowercase serde; Default=OpenRouter; Copy/Eq/Hash
pub struct MediaModel { pub id: String, pub name: String, pub provider: ProviderId, pub kind: MediaKind, pub modes: Vec<GenMode>, pub image_input: bool, pub image_edit: bool, pub audio_output: bool, pub price_hint: Option<String>, pub resolutions: Vec<String>, pub aspect_ratios: Vec<String>, pub durations: Vec<f64>, pub voices: Vec<String> }
pub struct ProviderBalance { pub real_available_usd: Option<f64>, pub total_credits_usd: Option<f64>, pub total_usage_usd: Option<f64> }
pub enum VideoOutcome { Ready { files: Vec<MediaFile>, cost_usd: Option<f64> }, Job(String), Processing(String) }
pub struct MediaFile { pub url: Option<String>, pub bytes: Option<Vec<u8>>, pub content_type: String }
```
- GenerationRequest.provider: ProviderId (serde default OpenRouter); HistoryRecord.provider: ProviderId (serde default **Ci** — миграция); Settings.provider: ProviderId (default OpenRouter).

- [ ] **Step 1: Падающие тесты**

ProviderId serde "openrouter"/"ci"; Default OpenRouter. GenerationRequest без provider → OpenRouter; HistoryRecord без provider → **Ci**; Settings без provider → OpenRouter. MediaModel round-trip; VideoOutcome serde.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-shared --test provider` → FAIL.

- [ ] **Step 3: Реализовать типы + поля**

provider.rs с типами; добавить provider-поля с #[serde(default = ...)] (HistoryRecord default Ci, остальные OpenRouter). Реэкспорт в lib.rs.

- [ ] **Step 4: Тесты проходят + регресс**

Run: `cargo test -p km-shared` → PASS (существующие тестыHistoryRecord round-trip адаптировать под provider).

- [ ] **Step 5: Commit**

```bash
git add crates/km-shared && git commit -m "feat: ProviderId, MediaModel/ProviderBalance/VideoOutcome, provider-поля"
```

---

### Task 3: Provider trait + CiProvider адаптер

**Files:**
- Create: `crates/km-core/src/provider.rs`, `crates/km-core/src/ci_provider.rs`
- Modify: `crates/km-core/src/lib.rs`, `crates/km-core/Cargo.toml` (async_trait)
- Test: `crates/km-core/tests/ci_provider.rs`

**Interfaces:**
- Consumes: CiClient (все методы), CatalogModel, km-shared типы Task 2.
- Produces:
```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    async fn models(&self, kind: MediaKind) -> Result<Vec<MediaModel>, CoreError>;
    async fn generate_image(&self, req: &GenerationRequest) -> Result<Vec<MediaFile>, CoreError>;
    async fn generate_video(&self, req: &GenerationRequest) -> Result<VideoOutcome, CoreError>;
    async fn poll_video(&self, job_id: &str) -> Result<VideoOutcome, CoreError>;
    async fn speech(&self, req: &GenerationRequest) -> Result<AudioOutput, CoreError>;
    async fn balance(&self) -> Result<ProviderBalance, CoreError>;
    async fn download(&self, url: &str) -> Result<Vec<u8>, CoreError>;
}
pub struct CiProvider { client: CiClient }  // new(client)
```
- CiProvider: models → CatalogModel→MediaModel маппинг; generate_video → VideoOutcome::Ready{files, cost_usd из billing} (sync); poll_video → generation(request_id)→Ready/Processing; balance → AccountBalance→ProviderBalance.

- [ ] **Step 1: Падающие тесты (wiremock)**

CiProvider::models(Image) → MediaModel с kind Image, modes из for_mode; generate_video → Ready(files); balance → ProviderBalance; id()==Ci. Маппинг CatalogModel→MediaModel сохраняет price_hint/resolutions/durations.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test ci_provider` → FAIL.

- [ ] **Step 3: Реализовать trait + CiProvider**

provider.rs (trait, async_trait); ci_provider.rs оборачивает CiClient, маппит типы. CI-логика не меняется (адаптер).

- [ ] **Step 4: Тесты проходят + регресс CI**

Run: `cargo test -p km-core` → PASS (существующие CI-тесты зелёные).

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: Provider trait + CiProvider адаптер (CI сохранён)"
```

---

### Task 4: OpenRouter клиент — фундамент, каталог, баланс

**Files:**
- Create: `crates/km-core/src/openrouter/{mod.rs,client.rs,catalog.rs}`
- Test: `crates/km-core/tests/openrouter_catalog.rs`

**Interfaces:**
- Consumes: контракт-заметки Task 1, km-shared MediaModel/ProviderBalance/ProviderId.
- Produces: `OpenRouterClient::new(key)/with_base(key,base)/with_timeout`, `get_json/post_json/post_bytes/download` (как CiClient), `OpenRouterProvider{client}` impl Provider: models(kind) → /images/models|/videos/models|TTS-каталог → MediaModel; balance → /credits → ProviderBalance.

- [ ] **Step 1: Падающие тесты (wiremock по формам из Task 1)**

models(Image) → /images/models data[] → MediaModel (kind Image, modes TextToImage/ImageEdit по supported_parameters, price_hint из pricing); models(Video) → /videos/models (durations/resolutions/aspect_ratios, modes по supported_frame_images); balance → /credits total_credits/total_usage → ProviderBalance. Bearer заголовок; каталоги до ключа — по заметкам Task 1.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test openrouter_catalog` → FAIL.

- [ ] **Step 3: Реализовать клиент + каталог + баланс**

openrouter/client.rs (reqwest, base openrouter.ai/api/v1, ошибки OpenAI-конверт); catalog.rs маппинг в MediaModel; OpenRouterProvider impl models/balance/download. generate_image/video/speech/poll — заглушки todo!() до Task 5–7 (или сразу trait с unimplemented).

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-core --test openrouter_catalog` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: OpenRouter клиент — каталог, баланс, нормализация MediaModel"
```

---

### Task 5: OpenRouter изображения

**Files:**
- Modify: `crates/km-core/src/openrouter/{mod.rs,images.rs}`
- Test: `crates/km-core/tests/openrouter_images.rs`

**Interfaces:**
- Consumes: OpenRouterClient, GenerationRequest, MediaFile, контракт Task 1 (input_references формат).
- Produces: `OpenRouterProvider::generate_image` → POST /images (model, prompt, input_references по контракту, resolution/aspect_ratio/seed, n) → data[].b64_json → MediaFile{bytes}. ImageEdit → input_references/mask по контракту.

- [ ] **Step 1: Падающие тесты**

generate_image: тело model/prompt/n/input_references (формат из Task 1); ответ 2×b64 → 2 MediaFile bytes. Валидация prompt до запроса. edit: references/mask по контракту.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test openrouter_images` → FAIL.

- [ ] **Step 3: Реализовать generate_image/edit**

images.rs: маппинг GenerationRequest→тело /images, b64 decode→MediaFile. Кадры/референсы: data-URL или ImgBB по заметкам Task 1 (если ImgBB — upload-сервис, отдельный ключ опционально).

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-core --test openrouter_images` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: OpenRouter изображения (генерация + редактирование)"
```

---

### Task 6: OpenRouter видео (async submit/poll/download)

**Files:**
- Create: `crates/km-core/src/openrouter/video.rs`
- Test: `crates/km-core/tests/openrouter_video.rs`

**Interfaces:**
- Consumes: OpenRouterClient, GenerationRequest, VideoOutcome, контракт Task 1 (цена, кадры).
- Produces: `generate_video` → POST /videos → VideoOutcome::Job(id) (request_id=id); `poll_video(id)` → GET /videos/{id} → Ready(files из unsigned_urls/content)/Processing/Failed; download /content. Цена из completed/poll ответа (по Task 1) — возвращается с VideoOutcome::Ready (MediaFile + cost) либо отдельно.

- [ ] **Step 1: Падающие тесты**

submit: тело model/prompt/duration/resolution/aspect_ratio/generate_audio/frame_images (data-URL/ImgBB по Task 1) → Job(id). poll completed → Ready + цена; poll in_progress → Processing(id); poll failed → Err. download /content без Authorization (signed) или с Bearer по контракту.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test openrouter_video` → FAIL.

- [ ] **Step 3: Реализовать submit/poll/download**

video.rs: submit→Job, poll→Ready{files, cost_usd из completed/poll ответа}/Processing. Цена в VideoOutcome::Ready (тип из Task 2).

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-core --test openrouter_video` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: OpenRouter видео — async submit/poll/download"
```

---

### Task 7: OpenRouter аудио (TTS)

**Files:**
- Create: `crates/km-core/src/openrouter/audio.rs`
- Test: `crates/km-core/tests/openrouter_audio.rs`

**Interfaces:**
- Consumes: OpenRouterClient, GenerationRequest, AudioOutput, контракт Task 1.
- Produces: `speech` → POST /audio/speech (model, input, voice, response_format mp3, speed) → AudioOutput{bytes, content_type, cost_usd}. Только Speech режим (Music/SoundEffects → Validation «не поддерживается OpenRouter»).

- [ ] **Step 1: Падающие тесты**

speech: тело model/input/voice/response_format; бинарный ответ → AudioOutput bytes + cost (заголовок/usage по Task 1). Music/SoundEffects → Validation. input > лимита → валидация до запроса.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test openrouter_audio` → FAIL.

- [ ] **Step 3: Реализовать speech**

audio.rs: маппинг, бинарный ответ, cost. Guard режимов (только Speech).

- [ ] **Step 4: Тесты проходят**

Run: `cargo test -p km-core --test openrouter_audio` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: OpenRouter аудио — TTS синтезация"
```

---

### Task 8: Jobs refactor — dispatch по провайдеру, async-видео, recovery

**Files:**
- Modify: `crates/km-core/src/jobs.rs`
- Test: `crates/km-core/tests/jobs.rs`

**Interfaces:**
- Consumes: Provider trait, CiProvider, OpenRouterProvider, Storage, VideoOutcome.
- Produces: `Jobs::new(providers: HashMap<ProviderId, Arc<dyn Provider>> или factory, storage, folder, emit)`; run(req) выбирает provider по req.provider; async-видео OpenRouter: submit→Job→persist request_id+cost→Pending→poll loop→Ready→save→Completed; recovery по provider (OpenRouter poll_video, CI generation).

- [ ] **Step 1: Падающие тесты**

CI sync path регресс (существующие тесты адаптировать под provider). OpenRouter async: submit Job → Pending с request_id → poll Ready → Completed + cost + файл. OpenRouter транзиентный poll/download → Pending (recovery подхватит). recovery OpenRouter Pending → poll → Completed; CI Pending → generation → Completed. Цена персистится сразу после submit/completed.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test jobs` → FAIL.

- [ ] **Step 3: Реализовать refactor**

Jobs хранит providers (или factory по ProviderId+key). run: insert Pending(provider) → dispatch → sync Ready→save→Completed; async Job→persist id+cost→poll loop (интервал 10–30с, таймаут) → Ready→save→Completed; транзиент→Pending. recover_pending: по rec.provider → poll_video/generation. is_transient/exhaustive сохранён.

- [ ] **Step 4: Тесты проходят + регресс**

Run: `cargo test -p km-core` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core && git commit -m "feat: Jobs — dispatch по провайдеру, async-видео OpenRouter, recovery"
```

---

### Task 9: Storage миграция + keychain два ключа

**Files:**
- Modify: `crates/km-core/src/storage.rs`, `src-tauri/src/keychain.rs`
- Test: `crates/km-core/tests/storage.rs`, `src-tauri/tests/commands.rs`

**Interfaces:**
- Consumes: ProviderId, Settings.provider, HistoryRecord.provider.
- Produces: history.provider колонка (ALTER TABLE ADD COLUMN default 'ci' — миграция существующих); settings.provider; KeyStore per-provider: `get(provider)/set(provider,key)/delete(provider)` (keychain USER `openrouter-api-key`/`ci-api-key`).

- [ ] **Step 1: Падающие тесты**

Миграция: БД со старой схемой (без provider) → open → записи provider=Ci, не потеряны; total_spend по всем. settings.provider round-trip (default OpenRouter). KeyStore: два ключа изолированы (set OpenRouter не трогает Ci), get(provider), delete(provider).

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p km-core --test storage` → FAIL.

- [ ] **Step 3: Реализовать миграцию + два ключа**

storage.rs: ALTER TABLE history ADD COLUMN provider TEXT DEFAULT 'ci' (идемпотентно); settings provider. keychain.rs: KeyStore методы с provider-параметром (или ProviderKeyStore), USER по provider. Обновить вызовы.

- [ ] **Step 4: Тесты проходят + регресс**

Run: `cargo test --workspace` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/km-core src-tauri && git commit -m "feat: миграция history.provider, два ключа keychain"
```

---

### Task 10: Команды + IPC provider-aware

**Files:**
- Modify: `src-tauri/src/{commands.rs,state.rs}`, `ui/src/ipc.rs`
- Test: `src-tauri/tests/commands.rs`

**Interfaces:**
- Consumes: Provider trait, AppState, KeyStore(provider), Settings.provider.
- Produces: команды используют provider из req/settings: fetch_models{kind, provider?}, generate{req(provider)}, test_connection{provider}, set_api_key{provider, key}, fetch_balance{provider}. AppState строит provider по id+key. IPC-обёртки обновлены.

- [ ] **Step 1: Падающие тесты**

generate OpenRouter → OpenRouterProvider; generate Ci → CiProvider. test_connection{provider} использует ключ того провайдера. set_api_key{provider,key} изолирован. fetch_models фильтруется по provider. AppState.jobs/client по provider.

- [ ] **Step 2: Запустить — упадёт**

Run: `cargo test -p koteyye-media-studio` → FAIL.

- [ ] **Step 3: Реализовать provider-aware команды**

commands.rs: provider из аргумента/settings, построение OpenRouterProvider/CiProvider по ключу. state.rs: фабрика providers. ipc.rs: аргументы provider. test_connection per-provider (keychain-ошибка → Err без ложного онбординга).

- [ ] **Step 4: Тесты проходят + регресс**

Run: `cargo test --workspace` → PASS; clippy/fmt host+wasm32.

- [ ] **Step 5: Commit**

```bash
git add src-tauri ui && git commit -m "feat: provider-aware команды + IPC (два провайдера, два ключа)"
```

---

### Task 11: UI Настройки/Онбординг — переключатель провайдера

**Files:**
- Modify: `ui/src/screens/{settings.rs,onboarding.rs}`, `ui/src/state.rs`, `crates/km-ui-core/src/i18n.rs`
- Test: нативные тесты логики + smoke

**Interfaces:**
- Consumes: ipc (provider-aware), Settings.provider, ProviderId.
- Produces: Settings — select провайдера (default OpenRouter), два поля ключа (по провайдеру), проверка per-provider; Онбординг — ключ основного провайдера (OpenRouter). i18n ключи provider.*.

- [ ] **Step 1: Реализовать переключатель**

Settings: select provider → set_settings + сигнал; поля ключа OpenRouter/CI (set_api_key{provider,key}), test_connection{provider}. Онбординг: ключ OpenRouter (default). needs_onboarding учитывает ключ выбранного провайдера. i18n provider.openrouter/ci/ключи.

- [ ] **Step 2: Проверка сборки + нативные тесты**

dx build, clippy wasm32; нативные тесты логики переключателя/ключа per-provider.

- [ ] **Step 3: Smoke**

cargo tauri dev: переключение провайдера, два ключа изолированы, проверка подключения per-provider, default OpenRouter, онбординг с OpenRouter-ключом.

- [ ] **Step 4: Commit**

```bash
git add ui crates/km-ui-core && git commit -m "feat: переключатель провайдера в Настройках/Онбординге (default OpenRouter)"
```