//! Проверка выпуска и установка по подтверждённой пользователем версии.

use dioxus::prelude::*;
use km_shared::UpdateStatus;
use wasm_bindgen_futures::spawn_local;

use super::AppState;
use crate::ipc;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UpdatePhase {
    Idle,
    Checking,
    Checked(UpdateStatus),
    Installing,
    Restarting,
}

impl UpdatePhase {
    pub(crate) fn requires_restart(&self) -> bool {
        matches!(self, Self::Installing | Self::Restarting)
    }

    pub(crate) fn can_check(&self) -> bool {
        matches!(self, Self::Idle | Self::Checked(_))
    }
}

impl AppState {
    pub(crate) fn check_update(mut self) {
        if !self.update.peek().can_check() {
            return;
        }
        self.update.set(UpdatePhase::Checking);
        self.set_error(None);
        spawn_local(async move {
            match ipc::check_update().await {
                Ok(status) => self.update.set(UpdatePhase::Checked(status)),
                Err(message) => {
                    self.update.set(UpdatePhase::Idle);
                    self.set_error(Some(message));
                }
            }
        });
    }

    /// Между проверкой и установкой пользователь может запустить генерацию.
    /// Проверяем состояние в обработчике, блокируем новые задания до рестарта.
    pub(crate) fn install_update(mut self) {
        if !self.media_idle() {
            return;
        }
        let version = match &*self.update.peek() {
            UpdatePhase::Checked(UpdateStatus::Available(version)) => version.clone(),
            _ => return,
        };
        self.update.set(UpdatePhase::Installing);
        self.set_error(None);
        // Продолжаем отслеживать установку при уходе пользователя из настроек.
        spawn_local(async move {
            match ipc::install_update(&version).await {
                Ok(()) => self.update.set(UpdatePhase::Restarting),
                Err(message) => {
                    self.update
                        .set(UpdatePhase::Checked(UpdateStatus::Available(version)));
                    self.set_error(Some(message));
                }
            }
        });
    }
}
