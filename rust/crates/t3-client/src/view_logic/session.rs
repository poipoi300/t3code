//! Session and timeline presentation. Rendering and host URLs belong to the caller.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, rc::Rc};
use t3_contracts::{
    base_schemas::*, chat_attachment::ChatAttachment, orchestration_v2::*,
    orchestration_v2_id_table::IdOrIndex,
};
#[path = "session/timeline.rs"]
mod timeline;
pub use timeline::*;
#[path = "session/work.rs"]
pub(crate) mod work;
pub use work::*;
#[path = "session/resources.rs"]
mod resources;
pub use resources::*;
#[path = "session/floating_status.rs"]
pub mod floating_status;
#[path = "session/image_gallery.rs"]
pub mod image_gallery;
#[path = "session/links.rs"]
pub mod links;
#[path = "session/markdown_link_presentation.rs"]
pub mod markdown_link_presentation;
#[path = "session/markdown.rs"]
pub mod markdown;
#[path = "session/markdown_references.rs"]
pub mod markdown_references;
#[path = "session/questions.rs"]
pub mod questions;
#[path = "session/review.rs"]
pub mod review;
#[path = "session/terminal.rs"]
pub mod terminal;
#[path = "session/terminal_drawer.rs"]
pub mod terminal_drawer;
#[path = "session/text.rs"]
pub mod text;
#[path = "session/thread_links.rs"]
pub mod thread_links;
#[path = "session/timestamps.rs"]
pub mod timestamps;
#[path = "session/version.rs"]
pub mod version;
#[path = "session/workspace_lookup.rs"]
pub mod workspace_lookup;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageAttachment {
    #[serde(flatten)]
    pub attachment: ChatAttachment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: IdOrIndex<MessageId>,
    pub role: String,
    pub text: String,
    pub run_id: Option<IdOrIndex<RunId>>,
    pub streaming: bool,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Rc<Vec<MessageAttachment>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_intent: Option<OrchestrationV2UserMessageInputIntent>,
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub run_id: IdOrIndex<RunId>,
    pub status: OrchestrationV2RunStatus,
    pub started_at: Option<String>,
    pub requested_at: Option<String>,
    pub completed_at: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeView {
    pub status: Option<OrchestrationV2RunStatus>,
    pub active_run_id: Option<IdOrIndex<RunId>>,
    /// None is absent; Some(None) is an explicit cleared activity start.
    #[serde(default, with = "t3_contracts::wire::double_option")]
    pub activity_started_at: Option<Option<String>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionPhase {
    Disconnected,
    Connecting,
    Running,
    Ready,
}
pub fn derive_phase(runtime: Option<&RuntimeView>) -> SessionPhase {
    match runtime {
        None => SessionPhase::Disconnected,
        Some(RuntimeView {
            status:
                Some(
                    OrchestrationV2RunStatus::Preparing
                    | OrchestrationV2RunStatus::Starting
                    | OrchestrationV2RunStatus::Queued,
                ),
            ..
        }) => SessionPhase::Connecting,
        Some(RuntimeView {
            status: Some(OrchestrationV2RunStatus::Running | OrchestrationV2RunStatus::Waiting),
            ..
        }) => SessionPhase::Running,
        _ => SessionPhase::Ready,
    }
}
pub fn derive_can_interrupt_running_thread(active: bool, runtime: Option<&RuntimeView>) -> bool {
    active
        && (derive_phase(runtime) == SessionPhase::Running
            || runtime.is_some_and(|r| {
                r.active_run_id.is_some()
                    && matches!(
                        r.status,
                        Some(
                            OrchestrationV2RunStatus::Preparing
                                | OrchestrationV2RunStatus::Starting
                                | OrchestrationV2RunStatus::Queued
                        )
                    )
            }))
}
pub fn is_latest_run_settled(run: Option<&RunView>, runtime: Option<&RuntimeView>) -> bool {
    run.is_some_and(|run| {
        !matches!(
            run.status,
            OrchestrationV2RunStatus::Preparing
                | OrchestrationV2RunStatus::Queued
                | OrchestrationV2RunStatus::Starting
                | OrchestrationV2RunStatus::Running
                | OrchestrationV2RunStatus::Waiting
        ) && runtime.and_then(|r| r.active_run_id.as_ref()) != Some(&run.run_id)
    })
}
pub fn derive_active_work_started_at<'a>(
    run: Option<&'a RunView>,
    runtime: Option<&'a RuntimeView>,
    send: Option<&'a str>,
    valid_date: impl Fn(&str) -> bool,
) -> Option<&'a str> {
    let start = if let Some(activity) = runtime.and_then(|r| r.activity_started_at.as_ref()) {
        activity.as_deref().filter(|s| valid_date(s))
    } else {
        run.filter(|r| {
            r.completed_at.is_none()
                && runtime.and_then(|r| r.active_run_id.as_ref()) == Some(&r.run_id)
        })
        .and_then(|r| {
            r.started_at
                .as_deref()
                .filter(|s| valid_date(s))
                .or_else(|| r.requested_at.as_deref().filter(|s| valid_date(s)))
        })
    };
    start.or_else(|| {
        if runtime.and_then(|r| r.active_run_id.as_ref()).is_none() {
            send
        } else {
            None
        }
    })
}
pub fn sort_pending_requests<T>(requests: &[T], created_at: impl Fn(&T) -> &str) -> Vec<&T> {
    let mut result: Vec<_> = requests.iter().collect();
    result.sort_by(|a, b| created_at(a).cmp(created_at(b)));
    result
}
pub fn is_streaming_message_text_update(previous: &ChatMessage, next: &ChatMessage) -> bool {
    if previous.role != "assistant"
        || next.role != "assistant"
        || !previous.streaming
        || !next.streaming
    {
        return false;
    }
    let mut before = previous.clone();
    before.text.clone_from(&next.text);
    before.updated_at.clone_from(&next.updated_at);
    before == *next
}

#[cfg(test)]
#[path = "session/acceptance.rs"]
mod acceptance;
#[cfg(test)]
#[path = "session/behavior_tests.rs"]
mod behavior_tests;
#[cfg(test)]
#[path = "session/helper_acceptance.rs"]
mod helper_acceptance;
#[cfg(test)]
#[path = "session/tests.rs"]
mod tests;

#[path = "session/plan_presentation.rs"]
pub mod plan_presentation;
