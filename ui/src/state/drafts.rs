//! Черновики восьми режимов и выбранная вкладка каждого раздела Студии.

use dioxus::prelude::*;
use km_shared::video::VideoLimits;
use km_shared::{CatalogModel, GenMode, GenParams, GenerationRequest, MediaKind, Reference};
use km_ui_core::studio::{build_request, default_params, models_for};

/// У каждого режима собственная форма на всё время работы приложения.
/// Большие референсы остаются на месте при навигации.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StudioDraft {
    pub mode: GenMode,
    pub model: Signal<String>,
    pub prompt: Signal<String>,
    pub params: Signal<GenParams>,
    pub references: Signal<Vec<Reference>>,
    pub mask: Signal<Option<String>>,
}

impl StudioDraft {
    fn new(mode: GenMode) -> Self {
        Self {
            mode,
            model: Signal::new(String::new()),
            prompt: Signal::new(String::new()),
            params: Signal::new(default_params(mode)),
            references: Signal::new(Vec::new()),
            mask: Signal::new(None),
        }
    }

    /// Первую модель выбираем только для новой формы. Запомненный id переживает
    /// перестановку каталога и временное исчезновение модели.
    pub fn choose_initial_model(self, kind: MediaKind, catalog: &[CatalogModel]) {
        if !self.model.peek().is_empty() {
            return;
        }
        if let Some(first) = models_for(catalog, kind, self.mode).first() {
            self.select_model(first.id.clone());
        }
    }

    /// Настройки модели корректируются один раз при её смене. При переходах
    /// между экранами и повторном выборе того же id значения остаются в форме.
    pub fn select_model(mut self, id: String) {
        if *self.model.peek() == id {
            return;
        }
        let replacing = !self.model.peek().is_empty();
        if replacing {
            match self.mode {
                GenMode::TextToVideo | GenMode::ImageToVideo | GenMode::ReferenceToVideo => {
                    let mut params = self.params.write();
                    params.resolution = None;
                    params.audio = None;
                }
                GenMode::Speech => self.params.set(default_params(self.mode)),
                _ => {}
            }
        }
        if let Some(limits) = VideoLimits::for_model(&id, self.mode) {
            limits.apply_defaults(&mut self.params.write());
        }
        self.model.set(id);
    }

    fn restore(mut self, req: GenerationRequest) {
        self.model.set(req.model);
        self.prompt.set(req.prompt);
        self.params.set(req.params);
        self.references.set(req.references);
        self.mask.set(req.mask_data_url);
    }

    pub fn request(self, kind: MediaKind) -> GenerationRequest {
        build_request(
            kind,
            self.mode,
            &self.model.peek(),
            &self.prompt.peek(),
            self.params.peek().clone(),
            self.references.peek().clone(),
            self.mask.peek().clone(),
        )
    }
}

/// Активная вкладка каждого раздела также живёт в корневом контексте.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StudioSection {
    pub active: Signal<StudioDraft>,
    /// Собственная отправка этого вида в текущей сессии, затем её локальный id.
    pub last_submission: Signal<Option<km_ui_core::studio_results::Submission>>,
    pub drafts: CopyValue<Vec<StudioDraft>>,
}

impl StudioSection {
    pub(super) fn new(first: GenMode, other: &[GenMode]) -> Self {
        let initial = StudioDraft::new(first);
        let drafts = std::iter::once(initial)
            .chain(other.iter().copied().map(StudioDraft::new))
            .collect();
        Self {
            active: Signal::new(initial),
            last_submission: Signal::new(None),
            drafts: CopyValue::new(drafts),
        }
    }

    pub(super) fn restore(mut self, req: GenerationRequest) -> bool {
        let Some(draft) = self
            .drafts
            .read()
            .iter()
            .find(|d| d.mode == req.mode)
            .copied()
        else {
            return false;
        };
        draft.restore(req);
        self.active.set(draft);
        true
    }
}
