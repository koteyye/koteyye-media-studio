//! CSS-токены тем: тёмная «Графит» и светлая «Воздух».
//!
//! [`theme_vars`] отдаёт набор CSS-переменных строкой — UI подставляет
//! её в `style` корневого элемента вместе с `data-theme`.

use km_shared::Theme;

/// Акцентный цвет интерфейса (кнопки, ссылки, активные состояния).
pub const ACCENT: &str = "#e05616";

/// Акцентный цвет при наведении.
pub const ACCENT_HOVER: &str = "#ff7333";

/// Текст поверх акцента (primary-кнопки). Тёмный тёплый near-black: белый на
/// `#e05616` даёт лишь 3.8:1 (< WCAG AA 4.5:1), а `#1a0d05` — 4.99:1 на акценте
/// и 7.02:1 на `--accent-hover`. Одинаков в обеих темах: акцент не меняется.
pub const ON_ACCENT: &str = "#1a0d05";

/// Тёмная тема «Графит»: база `#101113`, приподнятые поверхности и
/// приглушённый текст поверх графитового фона.
const DARK_VARS: &str = "--bg:#101113;--sidebar:#17181b;--surface:#1c1d20;--input-bg:#151619;--surface-2:#25262a;--border:#303136;--text:#f0f0f2;--text-muted:#a6a7ae;--quiet:#93959e;--control-border:#70727c;--accent-text:#ff9361;--selected-bg:#36251e;--selected-border:#b46d46;--hover-bg:#292b30;--focus:#ff884b;--matte:#0b0c0e;--disabled-bg:#393a40;--disabled-text:#b0b0b8;--error-text:#ffab95;--success-text:#91cda2;--accent:#e05616;--accent-hover:#ff7333;--on-accent:#1a0d05";

/// Светлая тема «Воздух»: база `#f8f8f7`, белые поверхности.
const LIGHT_VARS: &str = "--bg:#f8f8f7;--sidebar:#eff0f1;--surface:#ffffff;--input-bg:#ffffff;--surface-2:#eceef0;--border:#cdd0d4;--text:#1d2127;--text-muted:#505661;--quiet:#626974;--control-border:#81868e;--accent-text:#a83d0b;--selected-bg:#fde9dc;--selected-border:#b66236;--hover-bg:#e3e6e8;--focus:#a83d0b;--matte:#e0e3e5;--disabled-bg:#e6e8ea;--disabled-text:#5f6570;--error-text:#a43123;--success-text:#276541;--accent:#e05616;--accent-hover:#ff7333;--on-accent:#1a0d05";

/// CSS-переменные темы строкой для `data-theme`.
///
/// Набор: `--bg`, `--surface`, `--border`, `--text`, `--text-muted`,
/// `--accent`, `--accent-hover`, `--on-accent`.
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
/// assert!(theme_vars(Theme::Dark).contains("--bg:#101113"));
/// assert!(theme_vars(Theme::Light).contains("--bg:#f8f8f7"));
/// ```
#[must_use]
pub fn theme_vars(theme: Theme) -> &'static str {
    match theme {
        Theme::Dark => DARK_VARS,
        Theme::Light | Theme::System => LIGHT_VARS,
    }
}
