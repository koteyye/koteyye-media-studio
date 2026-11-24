//! Экраны приложения.
//!
//! Settings — реальный экран настроек (Task 5): ключ, папка, язык, тема, баланс.
//! Studio/History/Onboarding — заглушки каркаса: заголовки через
//! [`crate::state::use_t`] и данные из [`crate::state::AppState`].
//! Реальные экраны Studio/History — План 3, Onboarding — Task 6.

mod history;
mod onboarding;
mod settings;
mod studio;

pub use history::History;
pub use onboarding::Onboarding;
pub use settings::Settings;
pub use studio::Studio;
