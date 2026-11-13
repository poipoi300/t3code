use super::execution::activity_status;
use t3_contracts::{orchestration_v2::*, orchestration_v2_id_table::IdOrIndex};

pub fn resolve_active_thread_run(
    p: &OrchestrationV2ThreadProjection,
) -> Option<&OrchestrationV2Run> {
    p.runs.iter().rfind(|r| activity_status(&r.status))
}

pub fn resolve_latest_merge_back_run(
    p: &OrchestrationV2ThreadProjection,
) -> Option<&OrchestrationV2Run> {
    let latest = p
        .runs
        .iter()
        .filter(|r| {
            matches!(
                r.status,
                OrchestrationV2RunStatus::Waiting | OrchestrationV2RunStatus::Completed
            )
        })
        .max_by_key(|r| r.ordinal)?;
    (!p.runs.iter().any(|r| {
        r.ordinal > latest.ordinal
            && matches!(
                r.status,
                OrchestrationV2RunStatus::Preparing
                    | OrchestrationV2RunStatus::Starting
                    | OrchestrationV2RunStatus::Running
            )
    }))
    .then_some(latest)
}

pub fn resolve_thread_provider_session(
    p: &OrchestrationV2ThreadProjection,
) -> Option<&OrchestrationV2ProviderSession> {
    let id = resolve_active_thread_run(p)
        .and_then(|r| r.provider_thread_id.as_ref())
        .or(p.thread.active_provider_thread_id.as_ref());
    let thread = id
        .and_then(|id| p.provider_threads.iter().find(|t| &t.id == id))
        .or_else(|| {
            p.provider_threads.iter().find(|t| {
                t.app_thread_id.as_ref() == Some(&p.thread.id) && t.provider_session_id.is_some()
            })
        });
    if let Some(id) = thread.and_then(|t| t.provider_session_id.as_ref()) {
        return p.provider_sessions.iter().find(|s| &s.id == id);
    }
    p.provider_sessions.iter().rfind(|s| {
        !matches!(
            s.status,
            ProviderSessionStatus::Stopped | ProviderSessionStatus::Error
        )
    })
}

pub fn thread_steers_during_compaction(p: Option<&OrchestrationV2ThreadProjection>) -> bool {
    p.and_then(resolve_thread_provider_session)
        .is_some_and(|s| s.capabilities.turns.active_steering_during_compaction == Some(Some(true)))
}

pub fn thread_supports_provider_handoff(p: Option<&OrchestrationV2ThreadProjection>) -> bool {
    let Some(p) = p else {
        return false;
    };
    if let Some(session) = resolve_thread_provider_session(p) {
        return session
            .capabilities
            .sessions
            .supports_provider_switching_via_handoff;
    }
    if resolve_active_thread_run(p).is_some() {
        return false;
    }
    if p.thread.history_origin == Some(Some(OrchestrationV2ThreadHistoryOrigin::V1Import))
        || p.runs.is_empty()
    {
        return true;
    }
    p.provider_threads.iter().any(|t| {
        Some(&t.id) == p.thread.active_provider_thread_id.as_ref()
            && t.app_thread_id.as_ref() == Some(&p.thread.id)
            && t.provider_instance_id == IdOrIndex::Id(p.thread.model_selection.instance_id.clone())
            && t.native_thread_ref.is_some()
    })
}

pub fn get_user_queued_thread_runs(
    p: &OrchestrationV2ThreadProjection,
) -> Vec<&OrchestrationV2Run> {
    p.runs
        .iter()
        .filter(|r| {
            r.status == OrchestrationV2RunStatus::Queued
                && !p.messages.iter().any(|m| {
                    m.id == r.user_message_id
                        && (m.delegated_completion.as_ref().is_some_and(Option::is_some)
                            || m.notification.as_ref().is_some_and(Option::is_some))
                })
        })
        .collect()
}

pub struct QueuedThreadRun<'a> {
    pub run: &'a OrchestrationV2Run,
    pub text: &'a str,
    pub message: Option<&'a OrchestrationV2ConversationMessage>,
}
pub struct ThreadQueueWorkflowState<'a> {
    pub active_run: Option<&'a OrchestrationV2Run>,
    pub queued_runs: Vec<QueuedThreadRun<'a>>,
    pub is_held: bool,
    pub can_promote_to_steer: bool,
    pub promote_skips_compaction: bool,
    pub can_send_now: bool,
}
pub fn derive_thread_queue_workflow_state(
    p: &OrchestrationV2ThreadProjection,
) -> ThreadQueueWorkflowState<'_> {
    let active = resolve_active_thread_run(p);
    let capabilities = resolve_thread_provider_session(p).map(|s| &s.capabilities.turns);
    let compaction = active
        .and_then(|r| p.messages.iter().find(|m| m.id == r.user_message_id))
        .is_some_and(|m| {
            m.role == ConversationMessageRole::User
                && t3_contracts::wire::trim_js(&m.text).eq_ignore_ascii_case("/compact")
                && m.attachments.is_empty()
        });
    let steerable = active.is_some_and(|r| {
        r.status == OrchestrationV2RunStatus::Running
            && r.active_attempt_id.as_ref().is_some_and(|id| {
                p.provider_turns.iter().any(|t| {
                    t.run_attempt_id.as_ref() == Some(id) && t.status == ProviderTurnStatus::Running
                })
            })
    });
    let mut queued = get_user_queued_thread_runs(p);
    queued.sort_by(|a, b| {
        a.queue_position
            .flatten()
            .unwrap_or(a.ordinal)
            .cmp(&b.queue_position.flatten().unwrap_or(b.ordinal))
            .then(a.ordinal.cmp(&b.ordinal))
    });
    ThreadQueueWorkflowState {
        active_run: active,
        queued_runs: queued
            .into_iter()
            .map(|run| {
                let message = p.messages.iter().find(|m| m.id == run.user_message_id);
                let text = message.map_or("Queued message", |m| {
                    m.context
                        .as_ref()
                        .and_then(Option::as_ref)
                        .and_then(|c| c.guest.as_ref().and_then(Option::as_ref))
                        .map_or(m.text.as_str(), |g| g.text.as_str())
                });
                QueuedThreadRun { run, text, message }
            })
            .collect(),
        is_held: p.runs.iter().any(|r| {
            r.status == OrchestrationV2RunStatus::Queued && r.queue_held == Some(Some(true))
        }),
        can_send_now: active.is_none()
            && p.thread.archived_at.is_none()
            && p.thread.deleted_at.is_none(),
        can_promote_to_steer: compaction
            || (active.is_some_and(|r| {
                matches!(
                    r.status,
                    OrchestrationV2RunStatus::Preparing | OrchestrationV2RunStatus::Starting
                )
            }) && capabilities.is_none_or(|c| {
                c.supports_active_steering || c.supports_steering_by_interrupt_restart
            }))
            || (steerable
                && capabilities.is_some_and(|c| {
                    c.supports_active_steering || c.supports_steering_by_interrupt_restart
                })),
        promote_skips_compaction: compaction,
    }
}

pub fn can_fork_projected_assistant_item(
    row: &OrchestrationV2ProjectedTurnItem,
    capabilities: Option<&OrchestrationV2ProviderCapabilities>,
) -> bool {
    matches!(&row.item, OrchestrationV2TurnItem::AssistantMessage(i) if i.run_id.is_some() && i.status == OrchestrationV2TurnItemStatus::Completed)
        && capabilities.is_none_or(|c| {
            (c.threads.can_fork_thread
                && c.threads.can_fork_from_turn
                && c.identity.native_thread_ids == OrchestrationV2NativeRefStrength::Strong)
                || c.context.supports_full_thread_handoff
        })
}
pub fn can_detach_thread_provider_session(p: &OrchestrationV2ThreadProjection) -> bool {
    resolve_thread_provider_session(p).is_some_and(|s| {
        !matches!(
            s.status,
            ProviderSessionStatus::Stopped | ProviderSessionStatus::Error
        )
    })
}
