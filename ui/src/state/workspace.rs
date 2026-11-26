//! Восстановление несекретных предпочтений и выбор записей в рабочем пространстве.
use std::collections::HashMap;

use dioxus::prelude::*;
use km_shared::{GenMode, HistoryFilter, MediaKind, ModelChoice, Settings};

use super::{AppState, StudioDraft};

impl StudioDraft {
    fn saved_choice(self) -> Option<ModelChoice> {
        let model = self.model.read();
        if model.is_empty() {
            return None;
        }
        let p = self.params.read();
        Some(ModelChoice {
            model: model.clone(),
            size: p.size.clone(),
            aspect_ratio: p.aspect_ratio.clone(),
            resolution: p.resolution.clone(),
            response_format: p.response_format.clone(),
            n: p.n,
        })
    }

    fn apply_choice(mut self, choice: &ModelChoice) {
        if choice.model.trim().is_empty() {
            return;
        }
        self.select_model(choice.model.clone());
        let mut p = self.params.write();
        p.size.clone_from(&choice.size);
        p.aspect_ratio.clone_from(&choice.aspect_ratio);
        p.resolution.clone_from(&choice.resolution);
        p.response_format.clone_from(&choice.response_format);
        p.n = choice.n;
    }
}

impl AppState {
    pub(crate) fn selected_result(self, kind: MediaKind) -> Option<String> {
        let provider = self.settings.read().provider;
        self.selected_records.read().get(&(provider, kind)).cloned()
    }

    pub(crate) fn select_result(mut self, kind: MediaKind, id: Option<String>) {
        let key = (self.settings.peek().provider, kind);
        let mut records = self.selected_records.write();
        if let Some(id) = id {
            records.insert(key, id);
        } else {
            records.remove(&key);
        }
    }

    pub(crate) fn show_history(mut self, kind: MediaKind) {
        self.history_filter.set(HistoryFilter {
            kind: Some(kind),
            search: None,
        });
        self.history_focus.set(false);
    }

    pub(crate) fn search_history(mut self) {
        self.history_filter.set(HistoryFilter::default());
        self.history_focus.set(true);
    }

    pub(crate) fn visit_media(self, kind: MediaKind) {
        if !*self.settings_ready.read() || self.settings.peek().workspace.last_kind == kind {
            return;
        }
        let mut next = self.settings.peek().clone();
        next.workspace.last_kind = kind;
        drop(self.queue_settings(next));
    }

    fn draft_choices(self) -> [HashMap<GenMode, ModelChoice>; 2] {
        self.studio.map(|sections| {
            let mut choices = HashMap::new();
            for section in sections {
                for draft in section.drafts.read().iter().copied() {
                    if let Some(choice) = draft.saved_choice() {
                        choices.insert(draft.mode, choice);
                    }
                }
            }
            choices
        })
    }

    pub(super) fn load_choices(mut self, settings: &Settings) {
        if settings.workspace.remember_models {
            for (sections, choices) in self.studio.into_iter().zip([
                &settings.workspace.model_choices,
                &settings.workspace.openrouter_model_choices,
            ]) {
                for section in sections {
                    for draft in section.drafts.read().iter().copied() {
                        let choice = choices.get(&draft.mode).or_else(|| match draft.mode {
                            GenMode::TextToImage => choices.get(&GenMode::ImageEdit),
                            GenMode::TextToVideo => choices
                                .get(&GenMode::ImageToVideo)
                                .or_else(|| choices.get(&GenMode::ReferenceToVideo)),
                            _ => None,
                        });
                        if let Some(choice) = choice {
                            draft.apply_choice(choice);
                        }
                    }
                }
            }
        }
        self.observed_choices.set(Some(self.draft_choices()));
    }

    /// Сигналы черновика меняются независимо от маршрута и компонента ввода.
    /// Сравниваем последний снимок, чтобы сбой записи не порождал бесконечные ретраи.
    pub(crate) fn watch_choices(mut self) {
        if !*self.settings_ready.read() {
            return;
        }
        if !self.settings.read().workspace.remember_models {
            if self.observed_choices.peek().is_some() {
                self.observed_choices.set(None);
            }
            return;
        }
        let choices = self.draft_choices();
        if self.observed_choices.peek().as_ref() == Some(&choices) {
            return;
        }
        self.observed_choices.set(Some(choices.clone()));
        let mut next = self.settings.peek().clone();
        let [ci, openrouter] = choices;
        if next.workspace.model_choices != ci
            || next.workspace.openrouter_model_choices != openrouter
        {
            next.workspace.model_choices = ci;
            next.workspace.openrouter_model_choices = openrouter;
            drop(self.queue_settings(next));
        }
    }
}
