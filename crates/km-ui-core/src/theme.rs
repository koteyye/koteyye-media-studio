//! CSS-токены тем: тёмная «Графит» и светлая «Воздух».
//!
//! [`theme_vars`] отдаёт набор CSS-переменных строкой — UI подставляет
//! её в `style` корневого элемента вместе с `data-theme`.

use km_shared::Theme;

/// Акцентный цвет интерфейса (кнопки, ссылки, активные состояния).
pub const ACCENT: &str = "#e05616";

/// Акцентный цвет при наведении.
pub const ACCENT_HOVER: &str = "#ff7333";

/// Тёмная тема «Графит»: база `#0a0c10`, приподнятые поверхности и
/// приглушённый текст поверх графитового фона.
const DARK_VARS: &str = "--bg:#0a0c10;--surface:#12151c;--border:#232833;--text:#e8eaee;--text-muted:#8b93a1;--accent:#e05616;--accent-hover:#ff7333";

/// Светлая тема «Воздух»: база `#f5f6f8`, белые поверхности.
const LIGHT_VARS: &str = "--bg:#f5f6f8;--surface:#ffffff;--border:#dfe3e8;--text:#1a1d23;--text-muted:#6b7280;--accent:#e05616;--accent-hover:#ff7333";

/// CSS-переменные темы строкой для `data-theme`.
///
/// Набор: `--bg`, `--surface`, `--border`, `--text`, `--text-muted`,
/// `--accent`, `--accent-hover`.
///
/// [`Theme::System`] резолвится в UI по системной теме; до резолва
/// здесь возвращается светлый набор как безопасный дефолт.
///
/// # Examples
///
/// ```
/// use km_shared::Theme;
/// use km_ui_core::theme::theme_vars;
///
/// assert!(theme_vars(Theme::Dark).contains("--bg:#0a0c10"));
/// assert!(theme_vars(Theme::Light).contains("--bg:#f5f6f8"));
/// ```
#[must_use]
pub fn theme_vars(theme: Theme) -> &'static str {
    match theme {
        Theme::Dark => DARK_VARS,
        Theme::Light | Theme::System => LIGHT_VARS,
    }
}
