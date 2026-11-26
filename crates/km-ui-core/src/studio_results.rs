//! Выбор результата в рабочем окне. Черновик формы не является записью истории.
use std::collections::HashSet;

use km_shared::{GenMode, GenerationRequest, HistoryRecord, JobStatus, MediaKind, Provider};

#[derive(Clone, Debug, PartialEq)]
pub enum Submission {
    Awaiting {
        provider: Provider,
        kind: MediaKind,
        mode: GenMode,
        model: String,
        prompt: String,
        previous: HashSet<String>,
        error: Option<String>,
        started_at: i64,
    },
    Recorded(String),
}

impl Submission {
    pub fn start(
        provider: Provider,
        req: &GenerationRequest,
        history: &[HistoryRecord],
        started_at: i64,
    ) -> Self {
        let previous = history
            .iter()
            .filter(|r| {
                r.provider == provider
                    && r.kind == req.kind
                    && r.mode == req.mode
                    && r.model == req.model
                    && r.prompt == req.prompt
            })
            .map(|r| r.id.clone())
            .collect();
        Self::Awaiting {
            provider,
            kind: req.kind,
            mode: req.mode,
            model: req.model.clone(),
            prompt: req.prompt.clone(),
            previous,
            error: None,
            started_at,
        }
    }

    /// Идентификатор из нативного события имеет приоритет. При обычном обновлении
    /// фиксируем только одну новую строку, совпадающую с отправленным запросом.
    pub fn inserted<'a>(
        &self,
        history: &'a [HistoryRecord],
        event_id: Option<&str>,
    ) -> Option<&'a HistoryRecord> {
        let Self::Awaiting {
            provider,
            kind,
            mode,
            model,
            prompt,
            previous,
            ..
        } = self
        else {
            return None;
        };
        let mut new = history.iter().filter(|r| {
            r.provider == *provider
                && r.kind == *kind
                && r.mode == *mode
                && r.model == *model
                && r.prompt == *prompt
                && !previous.contains(&r.id)
        });
        if let Some(id) = event_id {
            return new.find(|r| r.id == id);
        }
        let first = new.next()?;
        new.next().is_none().then_some(first)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StudioResult<'a> {
    Historical(&'a HistoryRecord),
    Submitted(&'a HistoryRecord),
    Waiting,
    NoReceipt,
    Empty,
}

impl<'a> StudioResult<'a> {
    pub fn record(self) -> Option<&'a HistoryRecord> {
        match self {
            Self::Historical(r) | Self::Submitted(r) => Some(r),
            _ => None,
        }
    }
}

pub fn select<'a>(
    history: &'a [HistoryRecord],
    kind: MediaKind,
    provider: Provider,
    chosen: Option<&str>,
    submission: Option<&Submission>,
    running: bool,
) -> StudioResult<'a> {
    let scope = history
        .iter()
        .filter(|r| r.kind == kind && r.provider == provider);
    if let Some(selected) = chosen.and_then(|id| scope.clone().find(|r| r.id == id)) {
        return StudioResult::Historical(selected);
    }
    match submission {
        Some(Submission::Recorded(id)) => {
            if let Some(row) = scope.clone().find(|r| &r.id == id) {
                return StudioResult::Submitted(row);
            }
        }
        Some(
            waiting @ Submission::Awaiting {
                provider: p,
                kind: k,
                ..
            },
        ) if *p == provider && *k == kind => {
            if let Some(row) = waiting.inserted(history, None) {
                return StudioResult::Submitted(row);
            }
            return if running {
                StudioResult::Waiting
            } else {
                StudioResult::NoReceipt
            };
        }
        _ => {}
    }
    if running {
        return StudioResult::Waiting;
    }
    scope
        .clone()
        .find(|r| {
            r.status == JobStatus::Pending
                || (r.status == JobStatus::Completed && !r.local_paths.is_empty())
        })
        .or_else(|| scope.into_iter().next())
        .map_or(StudioResult::Empty, StudioResult::Historical)
}

pub fn failed_request_id<'a>(
    stored: Option<&'a str>,
    diagnostic: Option<&'a str>,
) -> Option<&'a str> {
    // При HTTP-ошибке trace id иногда сохранён только в исходном сообщении.
    stored.filter(|id| !id.trim().is_empty()).or_else(|| {
        let message = diagnostic?;
        if !message.starts_with("HTTP ") {
            return None;
        }
        let (_, id) = message.strip_suffix(']')?.rsplit_once(" [request_id: ")?;
        (!id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')))
        .then_some(id)
    })
}

pub fn failure_explanation(diagnostic: Option<&str>) -> &'static str {
    let status = diagnostic
        .and_then(|e| e.strip_prefix("HTTP "))
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse::<u16>().ok());
    match status {
        Some(500..=599) => "gen.failure.server",
        Some(401 | 403) => "gen.failure.access",
        Some(429) => "gen.failure.rate",
        _ => "gen.failure.other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use km_shared::GenParams;

    fn form(mode: GenMode, model: &str, prompt: &str) -> GenerationRequest {
        GenerationRequest {
            kind: MediaKind::Image,
            mode,
            model: model.into(),
            prompt: prompt.into(),
            params: GenParams::default(),
            references: vec![],
            mask_data_url: None,
        }
    }
    fn row(req: &GenerationRequest, id: &str, status: JobStatus) -> HistoryRecord {
        HistoryRecord {
            id: id.into(),
            provider: Provider::CheaperInference,
            created_at: 1,
            updated_at: 1,
            mode: req.mode,
            kind: req.kind,
            model: req.model.clone(),
            prompt: req.prompt.clone(),
            status,
            request_id: None,
            local_paths: if status == JobStatus::Completed {
                vec!["image.png".into()]
            } else {
                vec![]
            },
            cost_usd: None,
            params_json: "{}".into(),
            error: None,
        }
    }
    fn view<'a>(
        rows: &'a [HistoryRecord],
        selected: Option<&str>,
        attempt: Option<&Submission>,
        running: bool,
    ) -> StudioResult<'a> {
        select(
            rows,
            MediaKind::Image,
            Provider::CheaperInference,
            selected,
            attempt,
            running,
        )
    }
    #[test]
    fn startup_prefers_finished_output_and_keeps_archived_failure_selectable() {
        let failed = row(
            &form(GenMode::ImageEdit, "minimax-h3-image", "old edit"),
            "old",
            JobStatus::Failed,
        );
        let success = row(
            &form(GenMode::TextToImage, "flux-2-flex", "old generation"),
            "ok",
            JobStatus::Completed,
        );
        let mut foreign = success.clone();
        foreign.id = "other".into();
        foreign.provider = Provider::OpenRouter;
        let rows = [foreign, failed, success];
        assert_eq!(
            view(&rows, None, None, false),
            StudioResult::Historical(&rows[2])
        );
        assert_eq!(
            view(&rows, Some("old"), None, false),
            StudioResult::Historical(&rows[1])
        );
        assert_eq!(
            view(&rows[..2], None, None, false),
            StudioResult::Historical(&rows[1])
        );
        assert_eq!(view(&rows[..1], None, None, false), StudioResult::Empty);
        let pending = row(
            &form(GenMode::ImageEdit, "minimax-h3-image", "awaiting recovery"),
            "pending",
            JobStatus::Pending,
        );
        let mut reconnect = vec![rows[1].clone(), pending, rows[2].clone()];
        assert_eq!(
            view(&reconnect, None, None, false),
            StudioResult::Historical(&reconnect[1])
        );
        reconnect.swap(1, 2);
        assert_eq!(
            view(&reconnect, None, None, false),
            StudioResult::Historical(&reconnect[1])
        );
    }

    #[test]
    fn a_new_failure_or_pending_result_stays_attached_to_the_submitted_id() {
        let req = form(GenMode::ImageEdit, "minimax-h3-image", "same prompt");
        let old = row(&req, "old", JobStatus::Completed);
        let submission = Submission::start(
            Provider::CheaperInference,
            &req,
            std::slice::from_ref(&old),
            1,
        );
        let unrelated = row(
            &form(GenMode::TextToImage, "flux-2-flex", "other"),
            "unrelated",
            JobStatus::Completed,
        );
        let mut rows = vec![unrelated, old];
        assert_eq!(
            view(&rows, None, Some(&submission), true),
            StudioResult::Waiting
        );
        assert_eq!(
            view(&rows, None, Some(&submission), false),
            StudioResult::NoReceipt
        );
        rows.insert(1, row(&req, "new", JobStatus::Pending));
        assert_eq!(
            view(&rows, None, Some(&submission), true),
            StudioResult::Submitted(&rows[1])
        );
        let bound =
            Submission::Recorded(submission.inserted(&rows, Some("new")).unwrap().id.clone());
        rows[1].status = JobStatus::Failed;
        assert_eq!(
            view(&rows, None, Some(&bound), false),
            StudioResult::Submitted(&rows[1])
        );
        assert_eq!(
            view(&rows, Some("old"), Some(&bound), false),
            StudioResult::Historical(&rows[2])
        );
        assert_eq!(
            view(&rows, None, None, false),
            StudioResult::Historical(&rows[0])
        );
    }

    #[test]
    fn ambiguous_new_rows_require_the_id_from_the_native_event() {
        let req = form(GenMode::ImageEdit, "model", "identical");
        let submission = Submission::start(Provider::CheaperInference, &req, &[], 0);
        let mut other = row(&req, "foreign", JobStatus::Failed);
        other.provider = Provider::OpenRouter;
        let mut video = row(&req, "video", JobStatus::Pending);
        video.kind = MediaKind::Video;
        let rows = [
            other,
            video,
            row(&req, "one", JobStatus::Pending),
            row(&req, "two", JobStatus::Pending),
        ];
        assert!(submission.inserted(&rows, None).is_none());
        assert!(submission.inserted(&rows, Some("foreign")).is_none());
        assert!(submission.inserted(&rows, Some("video")).is_none());
        assert_eq!(submission.inserted(&rows, Some("two")), Some(&rows[3]));
    }
    #[test]
    fn failed_http_response_keeps_its_original_diagnostic_and_trace() {
        let diagnostic = "HTTP 500 (server_error): Unexpected proxy error [request_id: ci_42]";
        assert_eq!(failure_explanation(Some(diagnostic)), "gen.failure.server");
        assert_eq!(
            failed_request_id(Some("persisted"), Some(diagnostic)),
            Some("persisted")
        );
        assert_eq!(failed_request_id(None, Some(diagnostic)), Some("ci_42"));
        assert_eq!(
            failed_request_id(
                None,
                Some("HTTP 500 (server_error): Unexpected proxy error")
            ),
            None
        );
    }
}
