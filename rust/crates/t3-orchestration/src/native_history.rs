//! TypeScript `AgentSessionImporter.importNativeHistory`: a thread bound to a native provider
//! session whose history is still pending gets that session's messages as run-less history,
//! read by opening the thread's provider adapter on the native session.
use crate::{
    api::{CommitGuard, CommitRequest, Decision, EventCommitter, IdKind, RecordFilter, RecordKind},
    composition::OrchestrationRuntime,
    error::{Error, Result},
    projections::read,
    provider::{OpenSession, ProviderError, RuntimePolicy},
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use t3_contracts::{
    base_schemas::{ProviderSessionId, ThreadId},
    orchestration_v2::{
        OrchestrationV2AppThread, OrchestrationV2DomainEvent, OrchestrationV2ProviderThread,
        OrchestrationV2ThreadHistoryOrigin,
    },
};
use tokio_util::sync::CancellationToken;

/// ACP imports read the history while holding the session-mutation lock, and an agent can
/// leave `session/load` unanswered (TypeScript `NATIVE_HISTORY_READ_TIMEOUT`).
const NATIVE_HISTORY_READ_TIMEOUT: Duration = Duration::from_secs(120);
const IMPORT_EVENT_PREFIX: &str = "agent-session-import:v2";

/// TypeScript `isNativeHistoryPending`.
pub fn is_native_history_pending(thread: &OrchestrationV2AppThread, has_run: bool) -> bool {
    thread.history_origin.clone().flatten() != Some(OrchestrationV2ThreadHistoryOrigin::V1Import)
        && !has_run
}

struct Pending {
    thread: OrchestrationV2AppThread,
    providers: Vec<OrchestrationV2ProviderThread>,
}

async fn pending(runtime: &OrchestrationRuntime, id: &ThreadId) -> Result<Option<Pending>> {
    let id = id.clone();
    runtime
        .projections
        .read_snapshot(move |db| {
            let Some(thread) = read::thread(db, &id)? else {
                return Ok(None);
            };
            let records = read::records(
                db,
                &id,
                &[RecordKind::Runs, RecordKind::ProviderThreads],
                &RecordFilter::default(),
            )?;
            let mut has_run = false;
            let mut providers = Vec::new();
            for set in records.records {
                match set {
                    crate::api::ProjectionRecordSet::Runs(runs) => has_run = !runs.is_empty(),
                    crate::api::ProjectionRecordSet::ProviderThreads(threads) => {
                        providers = threads
                    }
                    _ => {}
                }
            }
            Ok(
                is_native_history_pending(&thread, has_run)
                    .then_some(Pending { thread, providers }),
            )
        })
        .await
}

/// One history message: `user`, `assistant` or `reasoning` text with its time.
struct HistoryMessage {
    role: &'static str,
    text: String,
    created_at: Value,
}

/// TypeScript `messageEvents`.
fn message_events(thread: &ThreadId, index: usize, message: &HistoryMessage) -> Vec<Value> {
    let suffix = format!("{index:06}");
    let message_id = format!("{}:{suffix}", thread.0);
    let item_id = format!("{IMPORT_EVENT_PREFIX}:turn-item:{}:{suffix}", thread.0);
    let at = &message.created_at;
    let mut item = json!({"id":item_id,"threadId":thread,"runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":index+1,"status":"completed","title":null,"startedAt":at,"completedAt":at,"updatedAt":at});
    let turn_item = |payload: Value| json!({"id":item_id,"type":"turn-item.updated","threadId":thread,"occurredAt":at,"payload":payload});
    if message.role == "reasoning" {
        item["type"] = json!("reasoning");
        item["text"] = json!(message.text);
        item["streaming"] = json!(false);
        return vec![turn_item(item)];
    }
    let user = message.role == "user";
    if user {
        item["createdBy"] = json!("user");
        item["creationSource"] = json!("server");
        item["type"] = json!("user_message");
        item["inputIntent"] = json!("turn_start");
        item["attachments"] = json!([]);
    } else {
        item["type"] = json!("assistant_message");
        item["streaming"] = json!(false);
    }
    item["messageId"] = json!(message_id);
    item["text"] = json!(message.text);
    vec![
        json!({"id":format!("{IMPORT_EVENT_PREFIX}:message:{}:{suffix}",thread.0),"type":"message.updated","threadId":thread,"occurredAt":at,"payload":{"createdBy":if user {"user"} else {"agent"},"creationSource":"server","id":message_id,"threadId":thread,"runId":null,"nodeId":null,"role":message.role,"text":message.text,"attachments":[],"streaming":false,"createdAt":at,"updatedAt":at}}),
        turn_item(item),
    ]
}

fn read_failed(detail: impl std::fmt::Display) -> Error {
    Error::Provider(ProviderError::Protocol {
        reason: None,
        detail: format!("Could not read the native session history: {detail}"),
        payload: None,
    })
}

/// Imports the native history of `thread` if it is still pending; returns how many messages
/// were written (0 when nothing was pending or the thread has no native session).
pub async fn import_native_history(
    runtime: &OrchestrationRuntime,
    thread: &ThreadId,
    workspace_root: &Path,
) -> Result<usize> {
    let Some(records) = pending(runtime, thread).await? else {
        return Ok(0);
    };
    let Some(provider_thread) = records.providers.into_iter().find(|candidate| {
        candidate
            .app_thread_id
            .as_ref()
            .and_then(|id| crate::projections::read::id_text(id).ok())
            == Some(thread.0.as_str())
            && candidate
                .native_thread_ref
                .as_ref()
                .and_then(|reference| reference.native_id.as_ref())
                .is_some()
    }) else {
        return Ok(0);
    };
    let native = provider_thread
        .native_thread_ref
        .as_ref()
        .and_then(|reference| reference.native_id.as_ref())
        .map(|id| id.0.clone())
        .unwrap_or_default();
    let app = serde_json::to_value(&records.thread)?;
    let identity_version =
        serde_json::to_value(&provider_thread)?["nativeMetadata"]["itemIdentityVersion"]
            .as_u64()
            .and_then(|version| u8::try_from(version).ok());
    // The registry admits this open against sign-in changes, like any other open.
    let adapter = runtime
        .providers()
        .adapter(records.thread.model_selection.instance_id.clone())
        .await?;
    let cwd = app["worktreePath"]
        .as_str()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| workspace_root.to_path_buf());
    let cancellation = CancellationToken::new();
    let read = async {
        let session = adapter
            .open_session(
                OpenSession {
                    thread_id: thread.clone(),
                    provider_session_id: ProviderSessionId(
                        runtime.ids().next(IdKind::ProviderSession),
                    ),
                    model_selection: records.thread.model_selection.clone(),
                    runtime_policy: RuntimePolicy {
                        runtime_mode: records.thread.runtime_mode,
                        interaction_mode: records.thread.interaction_mode,
                        cwd: Some(cwd),
                        approval_policy: None,
                        sandbox_policy: None,
                        max_runtime_mode: None,
                    },
                    prompt_suggestions: None,
                    resume_from_session: None,
                    initial_native_thread_id: Some(native),
                    initial_provider_item_identity_version: identity_version,
                },
                cancellation.clone(),
            )
            .await?;
        let snapshot = session.snapshot(provider_thread.clone()).await;
        let _ = session.close().await;
        snapshot
    };
    let snapshot = tokio::time::timeout(NATIVE_HISTORY_READ_TIMEOUT, read)
        .await
        .map_err(|_| {
            cancellation.cancel();
            read_failed("the provider did not answer in time")
        })?
        .map_err(read_failed)?;
    let reasoning = snapshot.reasoning_message_ids.unwrap_or_default();
    let messages: Vec<HistoryMessage> = snapshot
        .messages
        .into_iter()
        .filter_map(|message| {
            let value = serde_json::to_value(&message).ok()?;
            let role = match value["role"].as_str()? {
                "user" => "user",
                "assistant"
                    if read::id_text(&message.id)
                        .is_ok_and(|id| reasoning.iter().any(|known| known.0 == id)) =>
                {
                    "reasoning"
                }
                "assistant" => "assistant",
                _ => return None,
            };
            (!message.text.trim().is_empty()).then(|| HistoryMessage {
                role,
                text: message.text,
                created_at: value["createdAt"].clone(),
            })
        })
        .collect();
    // Thread commands write whole thread rows, so hold their lane from the re-read through the
    // write to keep both this marker and their changes.
    let _lane = runtime.thread_lanes().acquire([thread.0.clone()]).await;
    let Some(current) = pending(runtime, thread).await? else {
        return Ok(0);
    };
    let mut events: Vec<Value> = messages
        .iter()
        .enumerate()
        .flat_map(|(index, message)| message_events(thread, index, message))
        .collect();
    let mut payload = serde_json::to_value(&current.thread)?;
    payload["historyOrigin"] = json!("v1_import");
    events.push(json!({"id":format!("{IMPORT_EVENT_PREFIX}:thread:{}:native-history",thread.0),"type":"thread.metadata-updated","threadId":thread,"providerInstanceId":payload["providerInstanceId"],"occurredAt":runtime.clock().now(),"payload":payload}));
    let events = events
        .into_iter()
        .map(serde_json::from_value::<OrchestrationV2DomainEvent>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    runtime
        .events
        .commit(CommitRequest {
            receipt: None,
            decision: Decision {
                events,
                ..Default::default()
            },
            guard: CommitGuard::None,
        })
        .await?;
    Ok(messages.len())
}
