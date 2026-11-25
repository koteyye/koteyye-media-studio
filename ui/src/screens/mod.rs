//! Экраны приложения.
//!
//! Settings — экран настроек (Task 5): ключ, папка, язык, тема, баланс.
//! Onboarding — онбординг первого запуска (Task 6): приветствие → ключ → папка
//! → готово, вне сайдбара. Studio/History — заглушки каркаса: заголовки через
//! [`crate::state::use_t`] и данные из [`crate::state::AppState`].
//! Реальные экраны Studio/History — План 3.

mod history;
mod onboarding;
mod settings;
mod studio;

pub use history::History;
pub use onboarding::Onboarding;
pub use settings::Settings;
pub use studio::Studio;
