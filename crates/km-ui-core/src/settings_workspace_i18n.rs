//! Настройки рабочего пространства и профилей подключения.

pub(crate) fn entry(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        "ws.connection" => ("Подключение", "Connection"),
        "ws.interface" => ("Интерфейс", "Interface"),
        "ws.provider" => ("Провайдер", "Provider"),
        "ws.ci_service" => (
            "Генерация изображений, видео и аудио",
            "Image, video and audio generation",
        ),
        "ws.or_service" => (
            "Проверка ключа доступна. Генерация запланирована.",
            "Key checks are available. Generation is planned.",
        ),
        "ws.key_hint" => (
            "Введите новый ключ или оставьте поле пустым для проверки сохранённого. Ключи хранятся в системном хранилище учётных данных.",
            "Enter a new key or leave the field empty to check the saved one. Keys are kept in the system credential store.",
        ),
        "ws.key_placeholder" => ("Ключ выбранного провайдера", "Selected provider’s key"),
        "ws.show" => ("Показать", "Show"),
        "ws.hide" => ("Скрыть", "Hide"),
        "ws.show_key" => ("Показать введённый ключ", "Show entered key"),
        "ws.hide_key" => ("Скрыть введённый ключ", "Hide entered key"),
        "ws.save_key" => ("Сохранить ключ", "Save key"),
        "ws.save_unchecked" => ("Сохранить без проверки", "Save without checking"),
        "ws.saved" => ("Ключ сохранён.", "Key saved."),
        "ws.removed" => ("Сохранённый ключ удалён.", "Saved key removed."),
        "ws.balance_scope" => (
            "Ключу требуется разрешение account:read.",
            "The key needs the account:read scope.",
        ),
        "ws.verified_saved" => ("Сохранённый ключ проверен.", "Saved key verified."),
        "ws.verified_draft" => (
            "Введённый ключ проверен. Можно сохранить.",
            "Entered key verified. Ready to save.",
        ),
        "ws.draft" => (
            "Значение в поле готово к проверке.",
            "The entered key is ready to check.",
        ),
        "ws.rejected" => (
            "Провайдер отклонил ключ. Проверьте значение и права доступа.",
            "The provider rejected this key. Check the value and its permissions.",
        ),
        "ws.unreachable" => (
            "Подтвердить ключ не удалось. Проверьте соединение и попробуйте снова.",
            "Could not verify the key. Check your connection and try again.",
        ),
        "ws.key_read_failed" => (
            "Не удалось прочитать системное хранилище ключей.",
            "Could not read the system credential store.",
        ),
        "ws.key_write_failed" => (
            "Не удалось изменить ключ в системном хранилище.",
            "Could not update the key in the system credential store.",
        ),
        "ws.writing" => ("Сохраняем изменения…", "Saving changes…"),
        "ws.appearance" => ("Оформление", "Appearance"),
        "ws.light" => ("Светлая", "Light"),
        "ws.dark" => ("Тёмная", "Dark"),
        "ws.system" => ("Системная", "System"),
        "ws.density" => ("Плотность интерфейса", "Interface density"),
        "ws.comfortable" => ("Обычная", "Comfortable"),
        "ws.compact" => ("Компактная", "Compact"),
        "ws.motion" => ("Анимация ожидания", "Generation animation"),
        "ws.results" => ("Результаты и история", "Results and history"),
        "ws.layout" => ("Вид изображений", "Image layout"),
        "ws.grid" => ("Варианты рядом", "Side by side"),
        "ws.single" => ("По одному", "One at a time"),
        "ws.recent" => ("Недавние запросы", "Recent requests"),
        "ws.recent_hint" => (
            "Под результатом каждого раздела",
            "Shown below each section’s result",
        ),
        "ws.count3" => ("3 записи", "3 entries"),
        "ws.count5" => ("5 записей", "5 entries"),
        "ws.count8" => ("8 записей", "8 entries"),
        "ws.work" => ("Работа в студии", "Workspace"),
        "ws.startup" => ("Стартовый раздел", "Startup section"),
        "ws.last" => ("Последний открытый", "Last opened"),
        "ws.remember" => ("Запоминать модель и формат", "Remember model and format"),
        "ws.remember_hint" => ("Отдельно для каждого режима", "Separately for each mode"),
        "ws.shortcut" => ("Запуск с клавиатуры", "Keyboard generation shortcut"),
        "ws.storage" => ("Локальные файлы", "Local files"),
        "ws.reset" => ("Сбросить настройки интерфейса", "Reset interface settings"),
        "ws.reset_ask" => (
            "Восстановить исходное оформление и параметры рабочего пространства?",
            "Restore the default appearance and workspace preferences?",
        ),
        "ws.reset_yes" => ("Сбросить", "Reset"),
        "ws.balance" => ("Расход и баланс", "Spend and balance"),
        "ws.local_spend" => ("Расход в студии", "Spend in this studio"),
        "ws.real_balance" => ("Показывать баланс аккаунта", "Show account balance"),
        "ws.account_available" => ("Доступно на аккаунте", "Available on account"),
        "ws.wallet_management_key" => (
            "OpenRouter открывает баланс аккаунта только ключам управления.",
            "OpenRouter only shares the account balance with management keys.",
        ),
        "ws.wallet_missing_key" => (
            "У этого провайдера ещё нет сохранённого ключа.",
            "No key has been saved for this provider yet.",
        ),
        "ws.wallet_rejected_key" => (
            "Провайдер отклонил сохранённый ключ.",
            "The provider rejected the saved key.",
        ),
        "ws.wallet_access_restricted" => (
            "Провайдер ограничил доступ к балансу.",
            "The provider restricted access to the account balance.",
        ),
        "ws.wallet_network" => (
            "Не удалось соединиться с провайдером.",
            "Could not reach the provider.",
        ),
        "ws.wallet_unavailable" => (
            "Провайдер не вернул баланс.",
            "The provider did not return a balance.",
        ),
        "ws.spend_unavailable" => ("Не удалось загрузить расход.", "Could not load spending."),
        "ws.balance_wrong_profile" => (
            "Данные относятся к другому профилю.",
            "The balance belongs to a different profile.",
        ),
        _ => return None,
    })
}
