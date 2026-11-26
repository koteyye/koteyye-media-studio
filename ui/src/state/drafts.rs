//! Общие черновики изображений и видео и отдельные вкладки аудио.

use dioxus::prelude::*;
use km_shared::video::VideoLimits;
use km_shared::{
    CatalogModel, GenMode, GenParams, GenerationRequest, MediaKind, Provider, Reference,
};
use km_ui_core::studio::{build_request, default_params, mode_for_input, models_for_input};

/// Поля формы и вложения сохраняются при навигации.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StudioDraft {
    pub provider: Provider,
    pub mode: GenMode,
    pub model: Signal<String>,
    pub prompt: Signal<String>,
    pub params: Signal<GenParams>,
    pub references: Signal<Vec<Reference>>,
    pub mask: Signal<Option<String>>,
    /// Сохраняет тип единственного вложения при повторе запроса из истории.
    pub reference_intent: Signal<Option<GenMode>>,
}

impl StudioDraft {
    fn new(provider: Provider, mode: GenMode) -> Self {
        Self {
            provider,
            mode,
            model: Signal::new(String::new()),
            prompt: Signal::new(String::new()),
            params: Signal::new(match provider {
                Provider::CheaperInference => default_params(mode),
                Provider::OpenRouter => GenParams::default(),
            }),
            references: Signal::new(Vec::new()),
            mask: Signal::new(None),
            reference_intent: Signal::new(None),
        }
    }

    /// Первую модель выбираем только для новой формы. Запомненный id переживает
    /// перестановку каталога и временное исчезновение модели.
    pub fn choose_initial_model(self, kind: MediaKind, catalog: &[CatalogModel]) {
        if !self.model.peek().is_empty() {
            return;
        }
        let count = self.references.peek().len();
        let available = models_for_input(catalog, kind, self.mode, count);
        if let Some(first) = available
            .iter()
            .find(|model| mode_for_input(kind, self.mode, model, count).is_some())
            .copied()
            .or_else(|| available.first().copied())
        {
            self.select_model_for_input(kind, first);
        }
    }

    pub fn mode_for(self, kind: MediaKind, model: Option<&CatalogModel>) -> Option<GenMode> {
        let model = model?;
        let count = self.references.read().len();
        if kind == MediaKind::Video
            && count == 1
            && *self.reference_intent.read() == Some(GenMode::ReferenceToVideo)
            && model.supports_mode(GenMode::ReferenceToVideo)
        {
            return Some(GenMode::ReferenceToVideo);
        }
        mode_for_input(kind, self.mode, model, count)
    }

    pub fn select_model_for_input(self, kind: MediaKind, model: &CatalogModel) {
        let changed = *self.model.peek() != model.id;
        self.select_model(model.id.clone());
        if changed
            && let Some(mode) = mode_for_input(kind, self.mode, model, self.references.peek().len())
        {
            self.adapt_video_params(mode);
        }
    }

    /// Настройки модели корректируются один раз при её смене. При переходах
    /// между экранами и повторном выборе того же id значения остаются в форме.
    pub fn select_model(mut self, id: String) {
        if *self.model.peek() == id {
            return;
        }
        let replacing = !self.model.peek().is_empty();
        if replacing && self.provider == Provider::OpenRouter {
            self.params.set(GenParams::default());
        } else if replacing {
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
        if self.provider == Provider::CheaperInference
            && let Some(limits) = VideoLimits::for_model(&id, self.mode)
        {
            limits.apply_defaults(&mut self.params.write());
        }
        self.reference_intent.set(None);
        self.model.set(id);
    }

    pub fn adapt_video_params(mut self, mode: GenMode) {
        if self.provider == Provider::CheaperInference
            && matches!(
                mode,
                GenMode::TextToVideo | GenMode::ImageToVideo | GenMode::ReferenceToVideo
            )
            && let Some(limits) = VideoLimits::for_model(&self.model.peek(), mode)
        {
            limits.apply_defaults(&mut self.params.write());
        }
    }

    fn restore(mut self, req: GenerationRequest) {
        self.model.set(req.model);
        self.prompt.set(req.prompt);
        self.params.set(req.params);
        self.reference_intent
            .set((req.mode == GenMode::ReferenceToVideo).then_some(req.mode));
        self.references.set(req.references);
        self.mask.set(req.mask_data_url);
    }

    pub fn request(self, kind: MediaKind, mode: GenMode) -> GenerationRequest {
        build_request(
            kind,
            mode,
            &self.model.peek(),
            &self.prompt.peek(),
            self.params.peek().clone(),
            self.references.peek().clone(),
            if mode == GenMode::ImageEdit {
                self.mask.peek().clone()
            } else {
                None
            },
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
    pub(super) fn new(provider: Provider, first: GenMode, other: &[GenMode]) -> Self {
        let initial = StudioDraft::new(provider, first);
        let drafts = std::iter::once(initial)
            .chain(
                other
                    .iter()
                    .copied()
                    .map(|mode| StudioDraft::new(provider, mode)),
            )
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
            .find(|d| {
                d.mode == req.mode
                    || (d.mode == GenMode::TextToImage && req.mode == GenMode::ImageEdit)
                    || (d.mode == GenMode::TextToVideo
                        && matches!(req.mode, GenMode::ImageToVideo | GenMode::ReferenceToVideo))
            })
            .copied()
        else {
            return false;
        };
        draft.restore(req);
        self.active.set(draft);
        true
    }
}
