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
use km_shared::{
    AppEvent, BalanceView, CatalogModel, HistoryFilter, HistoryRecord, Lang, MediaKind, Settings,
    Theme,
};
use km_ui_core::i18n::tr;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::ipc;

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
    /// Выбранная тема; [`Theme::System`] резолвится через `system_dark`.
    pub theme: Signal<Theme>,
    /// Системная тема по `matchMedia`: `true` — тёмная.
    pub system_dark: Signal<bool>,
    /// Каталог моделей по виду медиа (загружается при старте).
    pub models_by_kind: Signal<HashMap<MediaKind, Vec<CatalogModel>>>,
    /// История генераций, новые первыми; обновляется по событиям ядра.
    pub history: Signal<Vec<HistoryRecord>>,
    /// Баланс; `None`, пока не загружен.
    pub balance: Signal<Option<BalanceView>>,
    /// Глобальная загрузка (инициализация приложения).
    pub busy: Signal<bool>,
    /// Последняя ошибка IPC для баннера в интерфейсе.
    ///
    /// Семантика «последняя операция»: успешная IPC-операция очищает сигнал
    /// (`None`), отказ — записывает причину. Баннер также закрывается вручную
    /// (кнопка в [`crate::components::layout::Layout`]).
    pub error: Signal<Option<String>>,
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
    /// пустая папка, ru, System, реальный баланс выключен.
    #[must_use]
    pub fn new() -> Self {
        Self {
            settings: Signal::new(Settings {
                local_folder: String::new(),
                lang: Lang::Ru,
                theme: Theme::System,
                show_real_balance: false,
            }),
            lang: Signal::new(Lang::Ru),
            theme: Signal::new(Theme::System),
            system_dark: Signal::new(false),
            models_by_kind: Signal::new(HashMap::new()),
            history: Signal::new(Vec::new()),
            balance: Signal::new(None),
            busy: Signal::new(true),
            error: Signal::new(None),
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

    /// Применяет настройки к сигналам состояния (без записи в SQLite).
    pub fn apply_settings(mut self, settings: Settings) {
        self.lang.set(settings.lang);
        self.theme.set(settings.theme);
        self.settings.set(settings);
    }

    /// Регистрирует новое persist-поколение и возвращает его номер.
    ///
    /// Номер монотонно растёт с каждым стартом сохранения; откат сверяет его
    /// с текущим, чтобы не затереть более новый оптимистичный apply.
    fn next_generation(mut self) -> u64 {
        let next = (self.persist_gen)() + 1;
        self.persist_gen.set(next);
        next
    }

    /// Текущий номер persist-поколения (последний стартовавший persist).
    fn current_generation(self) -> u64 {
        (self.persist_gen)()
    }

    /// Сохраняет настройки через IPC: оптимистичное применение и откат при ошибке.
    ///
    /// Сначала применяет `settings` к сигналам (UI мгновенно отражает выбор),
    /// затем пишет в SQLite. При успехе очищает баннер ошибки. При отказе
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
    pub async fn persist_settings(mut self, settings: Settings) -> Result<(), String> {
        let generation = self.next_generation();
        let previous = (self.settings)();
        // Оптимистичное применение: Settings мал (строка + три enum), клон осознанный.
        self.apply_settings(settings.clone());
        match ipc::set_settings(&settings).await {
            Ok(()) => {
                self.error.set(None);
                Ok(())
            }
            Err(e) => {
                self.error.set(Some(e.clone()));
                let stored = ipc::get_settings().await.ok();
                if let Some(target) =
                    rollback_target(generation, self.current_generation(), stored, previous)
                {
                    self.apply_settings(target);
                }
                Err(e)
            }
        }
    }

    /// Подписка на события ядра — РОВНО ОДИН вызов [`ipc::on_event`].
    ///
    /// `generation:*` перечитывают историю, `balance:updated` — баланс.
    fn subscribe_events(self) {
        ipc::on_event(move |event| match event {
            AppEvent::GenerationStarted { .. }
            | AppEvent::GenerationDone { .. }
            | AppEvent::GenerationFailed { .. } => {
                spawn_local(async move { self.refresh_history().await });
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

    /// Стартовая загрузка: настройки, восстановление Pending, история, баланс, каталоги.
    ///
    /// Каждый успех очищает баннер ошибки, отказ — записывает причину
    /// (семантика «последняя операция», см. поле `error`).
    async fn load(mut self) {
        self.busy.set(true);
        self.error.set(None);
        match ipc::get_settings().await {
            Ok(settings) => self.apply_settings(settings),
            Err(e) => self.error.set(Some(e)),
        }
        // Восстанавливаем Pending-записи, пережившие перезапуск, затем читаем историю.
        match ipc::recover_pending().await {
            Ok(_) => self.error.set(None),
            Err(e) => self.error.set(Some(e)),
        }
        self.refresh_history().await;
        self.refresh_balance().await;
        for kind in [MediaKind::Image, MediaKind::Video, MediaKind::Audio] {
            match ipc::fetch_models(kind).await {
                Ok(models) => {
                    self.error.set(None);
                    self.models_by_kind.write().insert(kind, models);
                }
                Err(e) => self.error.set(Some(e)),
            }
        }
        self.busy.set(false);
    }

    /// Перечитывает историю без фильтра (новые первыми); успех очищает баннер ошибки.
    async fn refresh_history(mut self) {
        match ipc::list_history(&HistoryFilter::default()).await {
            Ok(records) => {
                self.error.set(None);
                self.history.set(records);
            }
            Err(e) => self.error.set(Some(e)),
        }
    }

    /// Перечитывает баланс; успех очищает баннер ошибки.
    ///
    /// Доступен экранам (кнопка «Обновить» в настройках); вызывается также
    /// по событию `balance:updated`.
    pub(crate) async fn refresh_balance(mut self) {
        match ipc::fetch_balance().await {
            Ok(view) => {
                self.error.set(None);
                self.balance.set(Some(view));
            }
            Err(e) => self.error.set(Some(e)),
        }
    }

    /// Применяет тему и язык к документу: `data-theme` и `lang` на `<html>`.
    ///
    /// Вызывается из реактивного эффекта; чтение сигналов внутри подписывает
    /// эффект на их изменение. Сбой записи атрибута логируется, не паникует.
    pub(crate) fn apply_document_theme(self, resolved: Theme) {
        let lang = (self.lang)();
        let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        else {
            return;
        };
        for (name, value) in [
            ("data-theme", theme_name(resolved)),
            ("lang", lang_name(lang)),
        ] {
            if let Err(e) = root.set_attribute(name, value) {
                web_sys::console::error_1(
                    &format!("не удалось задать {name}={value}: {e:?}").into(),
                );
            }
        }
    }
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

/// Код языка для атрибута `lang` и select: `ru`/`en` (как serde).
#[must_use]
pub fn lang_name(lang: Lang) -> &'static str {
    match lang {
        Lang::Ru => "ru",
        Lang::En => "en",
    }
}

/// Разбирает код языка из select; неизвестное значение — [`Lang::Ru`].
#[must_use]
pub fn parse_lang(raw: &str) -> Lang {
    match raw {
        "en" => Lang::En,
        _ => Lang::Ru,
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
    fn lang_names_roundtrip() {
        for lang in [Lang::Ru, Lang::En] {
            assert_eq!(parse_lang(lang_name(lang)), lang, "lang {lang:?}");
        }
    }

    #[test]
    fn parse_lang_unknown_falls_back_to_ru() {
        assert_eq!(parse_lang(""), Lang::Ru);
        assert_eq!(parse_lang("de"), Lang::Ru);
    }

    fn settings_with_theme(theme: Theme) -> Settings {
        Settings {
            local_folder: String::new(),
            lang: Lang::Ru,
            theme,
            show_real_balance: false,
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
