//! Экраны приложения.
//!
//! Сейчас это заглушки каркаса (План 2): заголовки через [`crate::state::use_t`]
//! и данные из [`crate::state::AppState`]. Реальные экраны Studio/History — План 3,
//! Settings — Task 5, Onboarding — Task 6.

mod history;
mod onboarding;
mod settings;
mod studio;

pub use history::History;
pub use onboarding::Onboarding;
pub use settings::Settings;
pub use studio::Studio;
