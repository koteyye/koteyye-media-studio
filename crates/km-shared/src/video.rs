//! Ограничения видеомоделей, общие для формы и проверки перед платным POST.

use crate::{GenMode, GenParams, Reference};

/// Только явно перечисленные версии Google Veo. Остальным моделям шлюз
/// задаёт собственные ограничения поверх общего контракта CI.
#[derive(Clone, Copy)]
pub struct VideoLimits {
    pub durations: &'static [u8],
    pub max_outputs: u32,
    pub aspect_ratios: &'static [&'static str],
    max_references: Option<usize>,
}

impl VideoLimits {
    /// https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/veo/3-1-generate
    pub fn for_model(model: &str, mode: GenMode) -> Option<Self> {
        let name = model.strip_prefix("google/").unwrap_or(model);
        let durations: &[u8] = match name {
            "veo-3.1-generate-001" if mode == GenMode::ReferenceToVideo => &[8],
            "veo-3.1-generate-001" | "veo-3.1-fast-generate-001" | "veo-3.1-lite-generate-001" => {
                &[4, 6, 8]
            }
            _ => return None,
        };
        Some(Self {
            durations,
            max_outputs: 4,
            aspect_ratios: &["16:9", "9:16"],
            max_references: (mode == GenMode::ReferenceToVideo).then_some(
                if name == "veo-3.1-lite-generate-001" {
                    0
                } else {
                    3
                },
            ),
        })
    }

    /// Google принимает до трёх subject images. У HTTP(S)-источника MIME
    /// устанавливает сервер при получении файла.
    pub fn supports_references(self, references: &[Reference]) -> bool {
        self.max_references.is_none_or(|max| {
            max > 0
                && references.len() <= max
                && references.iter().all(|r| {
                    r.data_url.strip_prefix("data:").is_none_or(|url| {
                        url.starts_with("image/jpeg;") || url.starts_with("image/png;")
                    })
                })
        })
    }

    fn supports_duration(self, seconds: Option<f64>) -> bool {
        self.durations
            .iter()
            .any(|&d| seconds == Some(f64::from(d)))
    }

    pub fn supports(self, params: &GenParams) -> bool {
        self.supports_duration(params.duration_seconds)
            && params.n.is_none_or(|n| (1..=self.max_outputs).contains(&n))
            && params
                .aspect_ratio
                .as_deref()
                .is_none_or(|r| self.aspect_ratios.contains(&r))
    }

    /// В новой форме и при ручной смене модели переносим только допустимые параметры.
    pub fn apply_defaults(self, params: &mut GenParams) {
        if !self.supports_duration(params.duration_seconds) {
            params.duration_seconds = self.durations.first().copied().map(f64::from);
        }
        if params
            .n
            .is_some_and(|n| !(1..=self.max_outputs).contains(&n))
        {
            params.n = None;
        }
        if params
            .aspect_ratio
            .as_deref()
            .is_some_and(|r| !self.aspect_ratios.contains(&r))
        {
            params.aspect_ratio = None;
        }
    }
}
