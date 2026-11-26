//! Глобальное состояние приложения: контекст-сигналы, инициализация, темы.
//!
//! [`AppState`] предоставляется в корне ([`crate::app::App`]) и читается экранами
//! через `use_context`. Сигналы `lang`/`theme` — применённые значения интерфейса;
//! источник правды для хранения — `settings` (SQLite через [`crate::ipc`]).
//!
//! Инициализация ([`AppState::init`]) вызывается РОВНО ОДИН раз из нереактивного
//! эффекта: подписка [`crate::ipc::on_event`] постоянная (`callback.forget`),
//! повторный вызов продублировал бы доставку событий.

use std::collections::HashMap;

use dioxus::prelude::*;
use futures_channel::oneshot;
use km_shared::{
    AppEvent, BalanceView, CatalogModel, GenMode, GenerationRequest, HistoryFilter, HistoryRecord,
    Lang, LangPreference, MediaKind, ModelChoice, Provider, Settings, Theme,
};
use km_ui_core::i18n::tr;
use km_ui_core::studio::build_request;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::ipc;
use km_ui_core::studio_results::Submission;
mod drafts;
pub(crate) use drafts::StudioDraft;
use drafts::StudioSection;
pub(crate) mod updates;
mod workspace;
use updates::UpdatePhase;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BalanceState {
    Loading,
    Ready(BalanceView),
    Failed(String),
}

impl BalanceState {
    pub(crate) fn for_provider(&self, provider: Provider) -> Option<&BalanceView> {
        match self {
            Self::Ready(view) if view.provider == provider => Some(view),
            _ => None,
        }
    }
}

/// Глобальное состояние: сигналы, общие для всех экранов.
///
/// Создаётся один раз в корне приложения; экраны читают и обновляют сигналы,
/// сохранение настроек — через [`crate::ipc`].
#[derive(Clone, Copy, Debug)]
pub struct AppState {
    /// Настройки из SQLite; сохранение — [`AppState::persist_settings`].
    pub settings: Signal<Settings>,
    /// Применённый язык интерфейса (ru/en), меняется без перезапуска.
    pub lang: Signal<Lang>,
    /// Язык ОС, с английским для неподдерживаемых локалей.
    system_lang: Signal<Lang>,
    /// Выбранная тема; [`Theme::System`] резолвится через `system_dark`.
    pub theme: Signal<Theme>,
    /// Системная тема по `matchMedia`: `true` — тёмная.
    pub system_dark: Signal<bool>,
    /// Каталог моделей по виду медиа (загружается при старте).
    pub models_by_kind: Signal<HashMap<MediaKind, Vec<CatalogModel>>>,
    /// История генераций, новые первыми; обновляется по событиям ядра.
    pub history: Signal<Vec<HistoryRecord>>,
    /// Расход и кошелёк выбранного провайдера.
    pub(crate) balance: Signal<BalanceState>,
    /// Глобальная загрузка (инициализация приложения).
    pub busy: Signal<bool>,
    /// Ошибка действия пользователя. Фоновая синхронизация её не сбрасывает.
    pub error: Signal<Option<String>>,
    /// Ошибка фоновой загрузки настроек, каталога, истории или баланса.
    pub background_error: Signal<Option<String>>,
    /// Вызовы generate в полёте. Разные виды медиа работают независимо.
    pub(crate) generating: Signal<HashMap<(Provider, MediaKind), GenMode>>,
    /// Ручная проверка Pending работает отдельно от активных generate.
    pub(crate) recovering: Signal<bool>,
    pub(crate) update: Signal<UpdatePhase>,
    studio: [[StudioSection; 3]; 2],
    selected_records: Signal<HashMap<(Provider, MediaKind), String>>,
    pub(crate) history_filter: Signal<HistoryFilter>,
    pub(crate) history_focus: Signal<bool>,
    pub(crate) settings_ready: Signal<bool>,
    observed_choices: Signal<Option<[HashMap<GenMode, ModelChoice>; 2]>>,
    write_tail: CopyValue<Option<oneshot::Receiver<()>>>,
    provider_changes: Signal<usize>,
    committed_provider: Signal<Provider>,
    /// Поздний ответ list_history не должен вернуть уже удалённую запись.
    history_revision: Signal<u64>,
    credential_revision: Signal<u64>,
    balance_revision: Signal<u64>,
    /// Первый запуск: стартовая проверка не нашла ключа ([`needs_onboarding`]).
    /// Выставляется один раз в [`AppState::init`]/`load`; редирект в онбординг
    /// принимает решение по этому сигналу (см. [`crate::app`]).
    pub onboarding_needed: Signal<bool>,
    /// Онбординг завершён в этой сессии (ключ задан ИЛИ пропущен осознанно).
    /// Пока `true`, редирект не срабатывает даже без ключа; при следующем
    /// запуске без ключа онбординг предлагается снова (сигнал не персистится).
    pub onboarding_done: Signal<bool>,
    /// Монотонный счётчик persist-поколений: захватывается на старте сохранения,
    /// чтобы откат упавшей задачи не затёр более новый оптимистичный apply.
    persist_gen: Signal<u64>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    /// Создаёт состояние с дефолтами до загрузки настроек.
    ///
    /// Дефолты зеркалят `default_settings` ядра (km-core/storage):
    /// пустая папка, язык ОС, светлая тема, реальный баланс выключен.
    #[must_use]
    pub fn new() -> Self {
        let system_lang = browser_system_language();
        Self {
            settings: Signal::new(Settings::default()),
            lang: Signal::new(system_lang),
            system_lang: Signal::new(system_lang),
            theme: Signal::new(Theme::Light),
            selected_records: Signal::new(HashMap::new()),
            history_filter: Signal::new(HistoryFilter::default()),
            history_focus: Signal::new(false),
            settings_ready: Signal::new(false),
            observed_choices: Signal::new(None),
            write_tail: CopyValue::new(None),
            provider_changes: Signal::new(0),
            committed_provider: Signal::new(Provider::default()),
            system_dark: Signal::new(false),
            models_by_kind: Signal::new(HashMap::new()),
            history: Signal::new(Vec::new()),
            balance: Signal::new(BalanceState::Loading),
            busy: Signal::new(true),
            error: Signal::new(None),
            background_error: Signal::new(None),
            generating: Signal::new(HashMap::new()),
            recovering: Signal::new(false),
            update: Signal::new(UpdatePhase::Idle),
            studio: [Provider::CheaperInference, Provider::OpenRouter].map(|provider| {
                [
                    StudioSection::new(provider, GenMode::TextToImage, &[]),
                    StudioSection::new(provider, GenMode::TextToVideo, &[]),
                    StudioSection::new(
                        provider,
                        GenMode::Speech,
                        &[GenMode::Music, GenMode::SoundEffects],
                    ),
                ]
            }),
            history_revision: Signal::new(0),
            credential_revision: Signal::new(0),
            balance_revision: Signal::new(0),
            onboarding_needed: Signal::new(false),
            onboarding_done: Signal::new(false),
            persist_gen: Signal::new(0),
        }
    }

    /// Однократная инициализация: подписки и стартовая загрузка данных.
    ///
    /// Вызывается из нереактивного эффекта (без чтения сигналов внутри),
    /// поэтому выполняется один раз за жизнь приложения.
    pub fn init(self) {
        self.subscribe_events();
        self.listen_system_theme();
        spawn_local(async move { self.load().await });
    }

    /// Записывает ошибку IPC в баннер; `None` очищает его.
    ///
    /// Приём `mut self` (AppState — Copy) позволяет вызывать из событийных
    /// обработчиков без мутабельной привязки состояния в компоненте.
    pub fn set_error(mut self, error: Option<String>) {
        self.error.set(error);
    }

    pub fn dismiss_error(mut self) {
        if self.error.peek().is_some() {
            self.error.set(None);
        } else {
            self.background_error.set(None);
        }
    }

    pub(crate) fn studio(self, kind: MediaKind) -> StudioSection {
        self.studio_for(self.settings.peek().provider, kind)
    }

    pub(crate) fn studio_for(self, provider: Provider, kind: MediaKind) -> StudioSection {
        self.studio[provider_index(provider)][match kind {
            MediaKind::Image => 0,
            MediaKind::Video => 1,
            MediaKind::Audio => 2,
        }]
    }

    /// Восстанавливает форму для редактирования. Отправка всегда явная.
    pub(crate) fn repeat_record(self, record: &HistoryRecord) -> Result<(), String> {
        if record.provider == self.settings.peek().provider
            && let Some(request) = request_from_history(record)
            && self.studio(record.kind).restore(request)
        {
            self.select_result(record.kind, Some(record.id.clone()));
            return Ok(());
        }
        Err(tr(*self.lang.peek(), "studio.restore_error").to_owned())
    }

    /// Захватываем вид до spawn: даже два быстрых клика дают один платный вызов.
    /// Задача привязана к жизни приложения и переживает смену экрана.
    pub(crate) fn submit(mut self, req: GenerationRequest) {
        let kind = req.kind;
        let provider = self.settings.peek().provider;
        let key = (provider, kind);
        if self.media_locked() || self.generating.peek().contains_key(&key) {
            return;
        }
        self.generating.write().insert(key, req.mode);
        self.select_result(kind, None);
        let mut submitted = self.studio(kind).last_submission;
        submitted.set(Some(Submission::start(
            provider,
            &req,
            &self.history.peek(),
            (js_sys::Date::now() / 1000.0) as i64,
        )));
        self.error.set(None);
        spawn_local(async move {
            match ipc::generate_for(provider, &req).await {
                Ok(record)
                    if record.kind == kind
                        && record.provider == provider
                        && submitted.peek().is_some() =>
                {
                    submitted.set(Some(Submission::Recorded(record.id)));
                }
                Err(message) => {
                    if let Some(Submission::Awaiting { error, .. }) = submitted.write().as_mut() {
                        *error = Some(message);
                    }
                }
                _ => {}
            }
            self.refresh_history().await;
            self.generating.write().remove(&key);
        });
    }

    /// Recovery обходит все Pending, в том числе ещё без request_id.
    /// Запускаем его только между генерациями, чтобы не пометить активный
    /// HTTP-запрос как прерванный в предыдущей сессии.
    pub(crate) fn check_pending(mut self) {
        if !self.media_idle() {
            return;
        }
        self.recovering.set(true);
        self.error.set(None);
        spawn_local(async move {
            if let Err(e) = ipc::recover_pending().await {
                self.error.set(Some(e));
            }
            self.refresh_history().await;
            self.recovering.set(false);
        });
    }

    pub(crate) fn media_locked(self) -> bool {
        *self.busy.read()
            || *self.provider_changes.read() != 0
            || *self.recovering.read()
            || self.update.read().requires_restart()
    }

    pub(crate) fn media_idle(self) -> bool {
        !self.media_locked() && self.generating.read().is_empty()
    }

    /// Применяет настройки к сигналам состояния (без записи в SQLite).
    pub fn apply_settings(mut self, settings: Settings) {
        if self.settings.peek().provider != settings.provider {
            self.invalidate_catalog();
        } else if self.settings.peek().show_real_balance != settings.show_real_balance {
            self.invalidate_balance();
        }
        self.lang
            .set(settings.lang.resolve(*self.system_lang.peek()));
        self.theme.set(settings.theme);
        self.settings.set(settings);
    }

    /// Регистрирует новое persist-поколение и возвращает его номер.
    ///
    /// Номер монотонно растёт с каждым стартом сохранения; откат сверяет его
    /// с текущим, чтобы не затереть более новый оптимистичный apply.
    fn next_generation(mut self) -> u64 {
        let next = *self.persist_gen.peek() + 1;
        self.persist_gen.set(next);
        next
    }

    /// Текущий номер persist-поколения (последний стартовавший persist).
    fn current_generation(self) -> u64 {
        *self.persist_gen.peek()
    }

    /// Сохраняет настройки через IPC: оптимистичное применение и откат при ошибке.
    ///
    /// Сначала применяет `settings` к сигналам (UI мгновенно отражает выбор),
    /// затем пишет в SQLite. При отказе
    /// записывает причину в `error` и откатывает сигналы к сохранённым значениям
    /// ([`ipc::get_settings`], при неудаче чтения — снимок до применения).
    ///
    /// Откат защищён generation guard: применяется, только если этот persist —
    /// последний стартовавший ([`rollback_target`]). Если более новый persist уже
    /// в полёте, ранний откат пропускается, чтобы не затереть его оптимистичный
    /// apply (иначе UI разошёлся бы с хранилищем после коммита нового значения).
    ///
    /// Возвращает результат записи для вызывающего кода (например, обновить
    /// баланс только после успешного включения `show_real_balance`); ошибка
    /// также доступна в сигнале `error`.
    pub async fn persist_settings(self, settings: Settings) -> Result<(), String> {
        self.queue_settings(settings)
            .await
            .map_err(|e| e.to_string())?
    }

    // Записи идут в порядке изменений, даже если UI перешёл на другой экран.
    // Снятие подписки вызывающей формы не прерывает запись в SQLite.
    fn queue_settings(mut self, mut settings: Settings) -> oneshot::Receiver<Result<(), String>> {
        if !settings.workspace.remember_models {
            settings.workspace.model_choices.clear();
            settings.workspace.openrouter_model_choices.clear();
        }
        let generation = self.next_generation();
        let previous = self.settings.peek().clone();
        let changed = previous.provider != settings.provider;
        if changed {
            *self.provider_changes.write() += 1;
        }
        self.apply_settings(settings.clone());

        let (done, tail) = oneshot::channel();
        let predecessor = self.write_tail.write().replace(tail);
        let (reply, result) = oneshot::channel();
        spawn_local(async move {
            if let Some(predecessor) = predecessor {
                let _ = predecessor.await;
            }
            let written = ipc::set_settings(&settings).await;
            match &written {
                Ok(()) => {
                    let switched = *self.committed_provider.peek() != settings.provider;
                    self.committed_provider.set(settings.provider);
                    if (changed || switched) && self.settings.peek().provider == settings.provider {
                        self.credential_changed();
                    }
                }
                Err(e) => {
                    self.error.set(Some(e.clone()));
                    if self.current_generation() == generation {
                        let stored = ipc::get_settings().await.ok();
                        if let Some(target) =
                            rollback_target(generation, self.current_generation(), stored, previous)
                        {
                            let restore_catalog =
                                changed || self.settings.peek().provider != target.provider;
                            self.committed_provider.set(target.provider);
                            self.apply_settings(target);
                            if restore_catalog {
                                self.credential_changed();
                            }
                        }
                    }
                }
            }
            if changed {
                *self.provider_changes.write() -= 1;
            }
            let _ = done.send(());
            let _ = reply.send(written);
        });
        result
    }

    /// Подписка на события ядра — РОВНО ОДИН вызов [`ipc::on_event`].
    ///
    /// `generation:*` перечитывают историю, `balance:updated` — баланс.
    fn subscribe_events(self) {
        ipc::on_event(move |event| match event {
            AppEvent::GenerationStarted { history_id }
            | AppEvent::GenerationDone { history_id }
            | AppEvent::GenerationFailed { history_id, .. } => {
                spawn_local(async move { self.refresh_history_after(Some(&history_id)).await });
            }
            AppEvent::BalanceUpdated => {
                spawn_local(async move { self.refresh_balance().await });
            }
        });
    }

    /// Читает системную тему (`matchMedia`) и подписывается на её смену.
    ///
    /// Подписка постоянная (closure forget): тема System должна отслеживать
    /// переключение темы ОС, пока приложение открыто.
    fn listen_system_theme(mut self) {
        let Some(mql) = web_sys::window()
            .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
        else {
            return;
        };
        self.system_dark.set(mql.matches());
        let callback = Closure::<dyn FnMut(web_sys::MediaQueryListEvent)>::new(
            move |e: web_sys::MediaQueryListEvent| {
                self.system_dark.set(e.matches());
            },
        );
        let listener = callback.as_ref().unchecked_ref();
        if let Err(e) = mql.add_event_listener_with_callback("change", listener) {
            web_sys::console::error_1(
                &format!("не удалось подписаться на смену темы ОС: {e:?}").into(),
            );
            return;
        }
        callback.forget();
    }

    /// Стартовая загрузка: настройки, проверка ключа, восстановление Pending,
    /// история, баланс, каталоги.
    async fn load(mut self) {
        self.busy.set(true);
        if let Ok(Some(lang)) = ipc::system_language().await {
            self.system_lang.set(lang);
            self.lang.set(self.settings.peek().lang.resolve(lang));
        }
        match ipc::get_settings().await {
            Ok(settings) => {
                self.load_choices(&settings);
                self.committed_provider.set(settings.provider);
                self.apply_settings(settings);
                self.settings_ready.set(true);
            }
            Err(e) => self.background_error.set(Some(e)),
        }
        // Проверка ключа первого запуска: Ok(false) — ключа нет/отклонён →
        // онбординг; Err — соединение не проверено, редирект не нужен.
        {
            let provider = self.settings.peek().provider;
            let revision = *self.credential_revision.peek();
            let check = ipc::check_provider_key(provider, None).await;
            if revision == *self.credential_revision.peek() {
                if let Err(e) = &check {
                    self.background_error.set(Some(e.clone()));
                }
                self.onboarding_needed.set(needs_onboarding(check));
            }
        }
        // Восстанавливаем Pending-записи, пережившие перезапуск, затем читаем историю.
        if let Err(e) = ipc::recover_pending().await {
            self.background_error.set(Some(e));
        }
        self.refresh_history().await;
        self.refresh_balance().await;
        for kind in [MediaKind::Image, MediaKind::Video, MediaKind::Audio] {
            self.refresh_models(kind).await;
        }
        self.busy.set(false);
    }

    /// Смена ключа меняет доступные маршруты, тарифы и кошелёк.
    pub(crate) fn credential_changed(self) {
        self.invalidate_catalog();
        spawn_local(async move { self.refresh_balance().await });
        spawn_local(async move {
            for kind in [MediaKind::Image, MediaKind::Video, MediaKind::Audio] {
                self.refresh_models(kind).await;
            }
        });
    }

    fn invalidate_catalog(mut self) {
        *self.credential_revision.write() += 1;
        self.models_by_kind.write().clear();
        self.invalidate_balance();
        self.background_error.set(None);
    }

    fn invalidate_balance(mut self) {
        *self.balance_revision.write() += 1;
        self.balance.set(BalanceState::Loading);
    }

    pub(crate) async fn refresh_models(mut self, kind: MediaKind) {
        let provider = self.settings.peek().provider;
        if provider != *self.committed_provider.peek() {
            return;
        }

        let revision = *self.credential_revision.peek();
        let models = ipc::fetch_models(provider, kind).await;
        if revision != *self.credential_revision.peek() {
            return;
        }
        match models {
            Ok(models) => {
                self.models_by_kind.write().insert(kind, models);
            }
            Err(e) => self.background_error.set(Some(e)),
        }
    }

    /// Перечитывает историю без фильтра; последнее чтение имеет приоритет.
    pub(crate) async fn refresh_history(self) {
        self.refresh_history_after(None).await;
    }

    async fn refresh_history_after(mut self, event_id: Option<&str>) {
        let revision = *self.history_revision.peek() + 1;
        self.history_revision.set(revision);
        let result = ipc::list_history(&HistoryFilter::default()).await;
        if revision == *self.history_revision.peek() {
            match result {
                Ok(records) => {
                    for section in self.studio.into_iter().flatten() {
                        let mut submitted = section.last_submission;
                        let id = submitted
                            .peek()
                            .as_ref()
                            .and_then(|s| s.inserted(&records, event_id))
                            .map(|r| r.id.clone());
                        if let Some(id) = id {
                            submitted.set(Some(Submission::Recorded(id)));
                        }
                    }
                    self.history.set(records);
                }
                Err(e) => self.background_error.set(Some(e)),
            }
        }
    }

    /// Перечитывает баланс.
    ///
    /// Доступен экранам (кнопка «Обновить» в настройках); вызывается также
    /// по событию `balance:updated`.
    pub(crate) async fn refresh_balance(mut self) {
        let provider = self.settings.peek().provider;
        if provider != *self.committed_provider.peek() {
            return;
        }
        self.invalidate_balance();
        let revision = *self.balance_revision.peek();
        let result = ipc::fetch_balance(provider).await;
        if revision != *self.balance_revision.peek() || provider != self.settings.peek().provider {
            return;
        }
        self.balance.set(match result {
            Ok(view) if view.provider == provider => BalanceState::Ready(view),
            Ok(_) => BalanceState::Failed(tr(*self.lang.peek(), "ws.balance_wrong_profile").into()),
            Err(e) => BalanceState::Failed(e),
        });
    }

    /// Применяет тему и язык к документу: `data-theme` и `lang` на `<html>`.
    ///
    /// Вызывается из реактивного эффекта; чтение сигналов внутри подписывает
    /// эффект на их изменение. Сбой записи атрибута логируется, не паникует.
    pub(crate) fn apply_document_theme(self, resolved: Theme) {
        let lang = (self.lang)();
        let prefs = self.settings.read();
        let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        else {
            return;
        };
        for (name, value) in [
            ("data-theme", theme_name(resolved)),
            ("lang", lang.code()),
            (
                "data-density",
                if prefs.workspace.compact {
                    "compact"
                } else {
                    "comfortable"
                },
            ),
            (
                "data-motion",
                if prefs.workspace.reduce_motion {
                    "reduced"
                } else {
                    "system"
                },
            ),
        ] {
            if let Err(e) = root.set_attribute(name, value) {
                web_sys::console::error_1(
                    &format!("не удалось задать {name}={value}: {e:?}").into(),
                );
            }
        }
    }
}

const fn provider_index(provider: Provider) -> usize {
    match provider {
        Provider::CheaperInference => 0,
        Provider::OpenRouter => 1,
    }
}

/// Ядро сохраняет целый GenerationRequest, включая исходные изображения.
/// Старая запись, где сохранены только GenParams, опирается на поля строки.
fn request_from_history(record: &HistoryRecord) -> Option<GenerationRequest> {
    if let Ok(req) = serde_json::from_str::<GenerationRequest>(&record.params_json) {
        return (req.kind == record.kind && req.mode == record.mode).then_some(req);
    }
    let saved = serde_json::from_str::<serde_json::Value>(&record.params_json).ok()?;
    let fields = saved.as_object()?;
    if [
        "kind",
        "mode",
        "model",
        "prompt",
        "references",
        "mask_data_url",
        "params",
    ]
    .iter()
    .any(|key| fields.contains_key(*key))
    {
        // Повреждённый полный запрос нельзя молча повторить без вложений.
        return None;
    }
    Some(build_request(
        record.kind,
        record.mode,
        &record.model,
        &record.prompt,
        serde_json::from_value(saved).ok()?,
        Vec::new(),
        None,
    ))
}

/// Решает, нужен ли онбординг первого запуска по результату стартовой проверки.
///
/// `Ok(false)` — ключа нет в keychain или CI ответил 401: показываем онбординг.
/// `Ok(true)` — ключ принят. `Err` — соединение не проверено (сеть/5xx, сбой
/// чтения keychain), ключ при этом может быть задан: существующего
/// пользователя в онбординг не уводим.
#[must_use]
pub fn needs_onboarding(check: Result<bool, String>) -> bool {
    matches!(check, Ok(false))
}

/// Решает, нужен ли откат при ошибке persist и какие настройки применить.
///
/// `None` — откат не применяется: более новый persist уже стартовал
/// (`current_generation > generation`), его оптимистичное применение отражает
/// текущее намерение пользователя, и ранний откат затёр бы его (гонка отката).
/// `Some(settings)` — применить: авторитетное значение из хранилища `stored`,
/// либо снимок `previous` до применения, если хранилище не прочиталось.
#[must_use]
fn rollback_target(
    generation: u64,
    current_generation: u64,
    stored: Option<Settings>,
    previous: Settings,
) -> Option<Settings> {
    if generation != current_generation {
        return None;
    }
    Some(stored.unwrap_or(previous))
}

/// Резолвит тему для применения: [`Theme::System`] — по системному предпочтению.
#[must_use]
pub fn resolve_theme(theme: Theme, system_dark: bool) -> Theme {
    match theme {
        Theme::System if system_dark => Theme::Dark,
        Theme::System => Theme::Light,
        explicit => explicit,
    }
}

/// Имя темы для `data-theme` и select: `dark`/`light`/`system` (как serde).
#[must_use]
pub fn theme_name(theme: Theme) -> &'static str {
    match theme {
        Theme::Dark => "dark",
        Theme::Light => "light",
        Theme::System => "system",
    }
}

/// Разбирает имя темы из select; неизвестное значение — [`Theme::System`].
#[must_use]
pub fn parse_theme(raw: &str) -> Theme {
    match raw {
        "dark" => Theme::Dark,
        "light" => Theme::Light,
        _ => Theme::System,
    }
}

#[cfg(target_arch = "wasm32")]
fn browser_system_language() -> Lang {
    web_sys::window()
        .and_then(|window| window.navigator().language())
        .as_deref()
        .map_or(Lang::En, Lang::from_locale)
}

#[cfg(not(target_arch = "wasm32"))]
fn browser_system_language() -> Lang {
    Lang::En
}

/// Разбирает сохранённый режим языка из select.
#[must_use]
pub fn parse_lang_preference(raw: &str) -> LangPreference {
    match raw {
        "ru" => LangPreference::Ru,
        "en" => LangPreference::En,
        _ => LangPreference::System,
    }
}

/// Переводит ключ интерфейса на текущий язык из контекста [`AppState`].
///
/// Ключ — строковый литерал (`&'static str`); чтение сигнала `lang` внутри
/// подписывает компонент на смену языка. Неизвестный ключ возвращается как есть.
#[must_use]
pub fn use_t(key: &'static str) -> &'static str {
    let state = use_context::<AppState>();
    tr((state.lang)(), key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use km_shared::{GenParams, JobStatus, Reference};

    fn edit_record(params_json: String) -> HistoryRecord {
        HistoryRecord {
            id: "edit".into(),
            provider: Provider::CheaperInference,
            created_at: 1,
            updated_at: 1,
            kind: MediaKind::Image,
            mode: GenMode::ImageEdit,
            model: "test-edit".into(),
            prompt: "Кадр".into(),
            status: JobStatus::Completed,
            request_id: None,
            local_paths: vec![],
            cost_usd: None,
            params_json,
            error: None,
        }
    }

    fn with_studio(test: impl FnOnce(AppState)) {
        let dom = VirtualDom::new(VNode::empty);
        dom.runtime()
            .in_scope(ScopeId::ROOT, || test(AppState::new()));
    }

    #[test]
    fn image_attachments_share_the_prompt_model_and_parameters() {
        with_studio(|state| {
            let mut draft = *state.studio(MediaKind::Image).active.peek();
            let model: CatalogModel = serde_json::from_value(serde_json::json!({
                "id": "image", "model_type": "image", "supports_image_edit": true,
                "supported_endpoints": ["/v1/images/generations", "/v1/images/edits"]
            }))
            .unwrap();
            draft.select_model_for_input(MediaKind::Image, &model);
            draft.prompt.set("Пейзаж".into());
            draft.params.write().size = Some("1536x1024".into());
            let mode = move || draft.mode_for(MediaKind::Image, Some(&model)).unwrap();
            assert_eq!(mode(), GenMode::TextToImage);
            draft.references.write().push(Reference {
                data_url: "https://example.com/input.png".into(),
            });
            let edit = draft.request(MediaKind::Image, mode());
            assert_eq!(edit.mode, GenMode::ImageEdit);
            assert_eq!(edit.model, "image");
            assert_eq!(edit.prompt, "Пейзаж");
            assert_eq!(edit.params.size.as_deref(), Some("1536x1024"));
            draft.references.write().clear();
            assert_eq!(mode(), GenMode::TextToImage);
            state
                .repeat_record(&edit_record(serde_json::to_string(&edit).unwrap()))
                .unwrap();
            assert_eq!(draft.request(MediaKind::Image, mode()), edit);
        });
    }

    #[test]
    fn remembered_choices_restore_from_legacy_reference_modes() {
        with_studio(|state| {
            let mut settings = Settings::default();
            settings.workspace.model_choices.insert(
                GenMode::ImageEdit,
                ModelChoice {
                    model: "saved-edit".into(),
                    size: Some("1536x1024".into()),
                    n: Some(2),
                    ..Default::default()
                },
            );
            settings.workspace.openrouter_model_choices.insert(
                GenMode::ReferenceToVideo,
                ModelChoice {
                    model: "openrouter/saved-video".into(),
                    resolution: Some("1080p".into()),
                    ..Default::default()
                },
            );
            state.load_choices(&settings);
            let draft = *state.studio(MediaKind::Image).active.peek();
            let req = draft.request(MediaKind::Image, GenMode::TextToImage);
            assert_eq!(req.model, "saved-edit");
            assert_eq!(req.params.n, Some(2));
            assert_eq!(req.params.size.as_deref(), Some("1536x1024"));
            assert!(
                req.prompt.is_empty() && req.references.is_empty() && req.mask_data_url.is_none()
            );
            let openrouter = state.studio_for(Provider::OpenRouter, MediaKind::Video);
            let req = openrouter
                .active
                .peek()
                .request(MediaKind::Video, GenMode::TextToVideo);
            assert_eq!(req.model, "openrouter/saved-video");
            assert_eq!(req.params.resolution.as_deref(), Some("1080p"));
            assert!(req.params.size.is_none() && req.params.n.is_none());
        });
    }

    #[test]
    fn selected_results_belong_to_provider_and_kind() {
        with_studio(|mut state| {
            state.select_result(MediaKind::Image, Some("picture".into()));
            assert_eq!(
                state.selected_result(MediaKind::Image).as_deref(),
                Some("picture")
            );
            assert!(state.selected_result(MediaKind::Video).is_none());
            state.settings.write().provider = Provider::OpenRouter;
            assert!(state.selected_result(MediaKind::Image).is_none());
            state.settings.write().provider = Provider::CheaperInference;
            assert_eq!(
                state.selected_result(MediaKind::Image).as_deref(),
                Some("picture")
            );
        });
    }

    #[test]
    fn first_model_is_sticky_and_veo_defaults_apply_only_when_choosing_another_model() {
        with_studio(|state| {
            let models: Vec<CatalogModel> = serde_json::from_value(serde_json::json!([
                {"id":"veo-3.1-generate-001","model_type":"video","features":["image-input"],"supported_endpoints":["/v1/videos/generations"]},
                {"id":"another-r2v","model_type":"video","supported_endpoints":["/v1/videos/generations"]}
            ])).unwrap();
            let section = state.studio(MediaKind::Video);
            let mut draft = *section.active.peek();
            draft.choose_initial_model(MediaKind::Video, &[]);
            assert!(draft.model.peek().is_empty());
            draft.choose_initial_model(MediaKind::Video, &models);
            assert_eq!(draft.model.peek().as_str(), models[0].id);
            assert_eq!(draft.params.peek().duration_seconds, Some(6.0));

            draft.params.write().duration_seconds = None;
            draft.choose_initial_model(MediaKind::Video, &[models[1].clone(), models[0].clone()]);
            draft.choose_initial_model(MediaKind::Video, &models[1..]);
            assert_eq!(
                draft.model.peek().as_str(),
                models[0].id,
                "исчезнувшую модель не подменяем"
            );
            draft.select_model(models[0].id.clone());
            assert_eq!(
                draft.params.peek().duration_seconds,
                None,
                "выбор той же модели не перезаписывает ввод"
            );
            draft.select_model(models[1].id.clone());
            draft.select_model(models[0].id.clone());
            assert_eq!(draft.params.peek().duration_seconds, Some(4.0));

            let mut historical = draft.request(MediaKind::Video, GenMode::ReferenceToVideo);
            historical.params.duration_seconds = Some(8.0);
            historical.references = vec![Reference {
                data_url: "https://example.com/subject.png".into(),
            }];
            assert!(section.restore(historical.clone()));
            assert_eq!(
                draft.mode_for(MediaKind::Video, Some(&models[0])),
                Some(GenMode::ReferenceToVideo)
            );
            draft.choose_initial_model(MediaKind::Video, &models);
            assert_eq!(
                draft.request(MediaKind::Video, GenMode::ReferenceToVideo),
                historical,
                "при повторе сохраняем параметры истории даже если нужна коррекция"
            );
        });
    }

    #[test]
    fn repeat_preserves_original_edit_references_mask_and_parameters() {
        let req = build_request(
            MediaKind::Image,
            GenMode::ImageEdit,
            "test-edit",
            "Кадр",
            GenParams {
                n: Some(2),
                ..Default::default()
            },
            vec![Reference {
                data_url: "data:image/png;base64,AQ==".into(),
            }],
            Some("data:image/png;base64,Ag==".into()),
        );
        let record = edit_record(serde_json::to_string(&req).unwrap());
        assert_eq!(request_from_history(&record), Some(req));
    }

    #[test]
    fn repeat_reads_params_only_records_and_rejects_broken_envelopes() {
        let record = edit_record(r#"{"n":2,"size":"1024x1024"}"#.into());
        let req = request_from_history(&record).unwrap();
        assert_eq!(req.prompt, record.prompt);
        assert_eq!(req.params.n, Some(2));
        let broken = edit_record(r#"{"kind":"image","references":"broken"}"#.into());
        assert_eq!(request_from_history(&broken), None);
    }

    #[test]
    fn resolve_theme_system_follows_preference() {
        assert_eq!(resolve_theme(Theme::System, true), Theme::Dark);
        assert_eq!(resolve_theme(Theme::System, false), Theme::Light);
    }

    #[test]
    fn resolve_theme_explicit_ignores_preference() {
        assert_eq!(resolve_theme(Theme::Dark, false), Theme::Dark);
        assert_eq!(resolve_theme(Theme::Light, true), Theme::Light);
    }

    #[test]
    fn theme_names_roundtrip() {
        for theme in [Theme::Dark, Theme::Light, Theme::System] {
            assert_eq!(parse_theme(theme_name(theme)), theme, "theme {theme:?}");
        }
    }

    #[test]
    fn parse_theme_unknown_falls_back_to_system() {
        assert_eq!(parse_theme(""), Theme::System);
        assert_eq!(parse_theme("neon"), Theme::System);
    }

    #[test]
    fn language_preferences_roundtrip() {
        for lang in [
            LangPreference::System,
            LangPreference::Ru,
            LangPreference::En,
        ] {
            assert_eq!(parse_lang_preference(lang.code()), lang, "lang {lang:?}");
        }
    }

    #[test]
    fn unknown_language_preference_follows_system() {
        assert_eq!(parse_lang_preference(""), LangPreference::System);
        assert_eq!(parse_lang_preference("de"), LangPreference::System);
    }

    #[test]
    fn applies_explicit_and_system_languages_to_the_ui() {
        with_studio(|mut state| {
            state.system_lang.set(Lang::Ru);
            state.apply_settings(Settings::default());
            assert_eq!(*state.lang.peek(), Lang::Ru);
            state.apply_settings(Settings {
                lang: LangPreference::En,
                ..Settings::default()
            });
            assert_eq!(*state.lang.peek(), Lang::En);
            state.apply_settings(Settings::default());
            assert_eq!(*state.lang.peek(), Lang::Ru);
            assert_eq!(state.settings.peek().lang, LangPreference::System);
        });
    }

    #[test]
    fn onboarding_needed_when_key_absent_or_rejected() {
        // Ok(false) — ключа нет или 401: первый запуск, нужен онбординг.
        assert!(needs_onboarding(Ok(false)));
    }

    #[test]
    fn onboarding_not_needed_when_key_accepted() {
        assert!(!needs_onboarding(Ok(true)));
    }

    #[test]
    fn onboarding_not_needed_when_connection_unverified() {
        // Сеть/5xx — ключ может быть задан; существующего пользователя не трогаем.
        assert!(!needs_onboarding(Err("сеть недоступна".to_owned())));
    }

    #[test]
    fn onboarding_not_needed_when_keychain_unreadable() {
        // Сбой чтения keychain (заблокирован, отказ prompt): ключ может быть
        // задан — test_connection отдаёт Err, редиректа в онбординг нет.
        assert!(!needs_onboarding(Err(
            "не удалось прочитать ключ из keychain: доступ запрещён".to_owned()
        )));
    }

    fn settings_with_theme(theme: Theme) -> Settings {
        Settings {
            local_folder: String::new(),
            lang: LangPreference::Ru,
            theme,
            show_real_balance: false,
            ..Settings::default()
        }
    }

    #[test]
    fn early_rollback_does_not_clobber_later_optimistic_apply() {
        // Persist A (gen=1) падает, но persist B (gen=2) уже стартовал и оптимистично
        // применил своё значение: откат A не должен затереть его (гонка отката).
        let stored = settings_with_theme(Theme::Dark);
        let previous = settings_with_theme(Theme::Light);
        assert_eq!(
            rollback_target(1, 2, Some(stored), previous),
            None,
            "ранний откат обязан пропуститься, пока более новый persist в полёте"
        );
    }

    #[test]
    fn rollback_applies_stored_for_latest_persist() {
        // Одиночный/последний persist: откат применяет авторитетное значение из БД.
        let stored = settings_with_theme(Theme::Dark);
        let previous = settings_with_theme(Theme::Light);
        assert_eq!(
            rollback_target(2, 2, Some(stored.clone()), previous),
            Some(stored)
        );
    }

    #[test]
    fn rollback_falls_back_to_previous_when_storage_unreadable() {
        // Хранилище не прочиталось — откат к снимку до оптимистичного применения.
        let previous = settings_with_theme(Theme::Light);
        assert_eq!(
            rollback_target(1, 1, None, previous.clone()),
            Some(previous)
        );
    }
}
