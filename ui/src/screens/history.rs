//! История генераций, просмотр файлов и повтор с исходными параметрами.

use dioxus::prelude::*;
use km_shared::{HistoryFilter, JobStatus, MediaKind, Provider};
use km_ui_core::format::format_usd;
use wasm_bindgen::JsCast;

use crate::components::player::{DeleteRecord, FileActions, ResultViewer, display_date};
use crate::components::recent::Thumbnail;
use crate::router::Route;
use crate::state::{AppState, use_t};

#[component]
pub fn History() -> Element {
    let mut state = use_context::<AppState>();
    use_effect(move || {
        if *state.history_focus.read() {
            if let Some(field) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id("archive-search"))
                .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
            {
                let _ = field.focus();
            }
            state.history_focus.set(false);
        }
    });
    let mut selected = use_signal(|| None::<String>);
    let title = use_t("history.title");
    let pending = use_t("history.pending");
    let completed = use_t("history.completed");
    let failed = use_t("history.failed");

    let search_label = use_t("history.search_prompt");
    let empty = use_t("history.empty");
    let no_matches = use_t("history.no_matches");
    let loading = use_t("common.loading");
    let filters = [
        (None, use_t("history.all")),
        (Some(MediaKind::Image), use_t("studio.image")),
        (Some(MediaKind::Video), use_t("studio.video")),
        (Some(MediaKind::Audio), use_t("studio.audio")),
    ];

    // Общая история уже загружена и обновляется событиями ядра. Поиск в её снимке
    // не передаёт большие референсы через IPC заново на каждое нажатие клавиши.
    let history = state.history.read();
    let filter = state.history_filter.read().clone();
    let query = filter.search.as_deref().unwrap_or_default();
    let matches = history_matcher(&filter);
    let records: Vec<_> = history
        .iter()
        .filter(|record| {
            matches(
                record.kind,
                &[&record.prompt, &record.model, record.provider.name()],
            )
        })
        .collect();
    let lang = (state.lang)();
    rsx! {
        section {class:"history-page",
            h1 { span {class:"archive-narrow-brand","Koteyye Media Studio v2 /"} {title} }
            div {class:"history-toolbar",
                div {class:"history-filters",role:"group",aria_label:title,
                    for (value,label) in filters {
                        button {key:"{label}",r#type:"button",class:"btn",
                            aria_pressed:(filter.kind==value).to_string(),
                            onclick:move |_| {state.history_filter.write().kind=value;selected.set(None);},{label}}
                    }
                }
                input {id:"archive-search",class:"text-input",r#type:"search",aria_label:search_label,placeholder:search_label,
                    value:"{query}",oninput:move |event| {
                        state.history_filter.write().search=Some(event.value());
                        selected.set(None);
                    }}
            }
            if records.is_empty() {
                p {class:"empty-state muted",role:"status",
                    if history.is_empty() && (state.busy)(){ {loading} }
                    else if history.is_empty(){ {empty} } else {{no_matches}}
                }
            } else {
                ul {class:"archive-list",
                    for record in records {
                        li {key:"{record.id}",class:"archive-entry",
                            div {class:"archive-line",
                                button {r#type:"button",class:"archive-open",title:"{record.prompt}",
                                    aria_expanded:(selected.read().as_ref()==Some(&record.id)).to_string(),
                                    onclick:{
                                        let id=record.id.clone();
                                        move |_| {let open=selected.peek().as_ref()!=Some(&id);selected.set(open.then(||id.clone()));}
                                    },
                                    Thumbnail {kind:record.kind,path:record.local_paths.first().cloned()}
                                    span {class:"archive-copy",
                                        strong {"{record.prompt}"}
                                        small {"{record.provider.name()} · {record.model} · "{display_date(record.created_at,lang)}}
                                    }
                                }
                                span {class:"archive-state",
                                    {match record.status {JobStatus::Pending=>pending,JobStatus::Completed=>completed,JobStatus::Failed=>failed}}
                                    if let Some(cost)=record.cost_usd {span {{format_usd(cost)}}}
                                }
                                EntryActions {id:record.id.clone(),kind:record.kind,provider:record.provider,status:record.status,
                                    path:record.local_paths.first().cloned(),expanded:selected.read().as_ref()==Some(&record.id)}
                            }
                            if selected.read().as_ref()==Some(&record.id) {
                                div {class:"history-detail",ResultViewer {record:record.clone(),in_history:true}}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Буквальный поиск без учёта регистра. Нормализуем запрос один раз для всей истории.
fn history_matcher(filter: &HistoryFilter) -> impl Fn(MediaKind, &[&str]) -> bool {
    let kind = filter.kind;
    let term = filter.search.as_deref().unwrap_or_default().to_lowercase();
    move |candidate, fields| {
        kind.is_none_or(|selected| selected == candidate)
            && (term.is_empty()
                || fields
                    .iter()
                    .any(|field| field.to_lowercase().contains(&term)))
    }
}

#[component]
fn EntryActions(
    id: String,
    kind: MediaKind,
    provider: Provider,
    status: JobStatus,
    path: Option<String>,
    expanded: bool,
) -> Element {
    let state = use_context::<AppState>();
    let nav = use_navigator();
    let repeat = use_t("history.to_form");
    let active = state.settings.read().provider;
    let repeat_label = if active == provider {
        repeat.to_owned()
    } else {
        format!("{repeat} · {}", provider.name())
    };
    let repeat_id = id.clone();
    rsx! {
        div {class:"history-actions",
            if !expanded && let Some(path)=path {FileActions {path,compact:true}}
            button {r#type:"button",class:"btn",disabled:provider!=Provider::CheaperInference,aria_label:"{repeat_label}",title:"{repeat_label}",
                onclick:move |_| {
                    // В истории может быть большой референс. Весь запрос копируем только по явному действию.
                    let record=state.history.peek().iter().find(|r|r.id==repeat_id).cloned();
                    if let Some(record)=record {
                        spawn(async move {
                            if state.settings.peek().provider!=record.provider {
                                let mut next=state.settings.peek().clone();next.provider=record.provider;
                                if state.persist_settings(next).await.is_err() {return;}
                            }
                            match state.repeat_record(&record) {
                                Ok(()) => {state.select_result(kind,Some(record.id));nav.push(Route::studio(kind));}
                                Err(error) => state.set_error(Some(error)),
                            }
                        });
                    }
                },
                "{repeat_label}"
            }
            if status != JobStatus::Pending {
                DeleteRecord { id, compact: true }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_and_unicode_search_match_the_same_entry_and_treat_wildcards_literally() {
        let filter = HistoryFilter {
            kind: Some(MediaKind::Image),
            search: Some("Ёж_%".to_owned()),
        };
        let matches = history_matcher(&filter);
        assert!(matches(MediaKind::Image, &["Сюжет ЁЖ_% и лес"]));
        assert!(matches(MediaKind::Image, &["Сюжет", "Ёж_%"]));
        assert!(!matches(MediaKind::Audio, &["Сюжет ЁЖ_% и лес"]));
        assert!(!matches(MediaKind::Image, &["Сюжет ЁЖик и лес"]));
        assert!(history_matcher(&HistoryFilter::default())(
            MediaKind::Video,
            &["Кадр"]
        ));
    }
}
