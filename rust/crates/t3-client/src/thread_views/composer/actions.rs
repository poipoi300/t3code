//! Current composer actions, derived from the destination and the authoritative draft.
use crate::{composition::ClientRuntime, platform::Platform, types::ThreadKey};
use futures::{StreamExt, stream::LocalBoxStream};
use std::rc::Rc;
use t3_contracts::{orchestration_v2::*, orchestration_v2_id_table::IdOrIndex, rpc::WsOrchestrationV2DispatchCommandRpc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FollowUpBehavior { Queue, Steer }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrimaryActionState {
    pub can_operate_thread: bool,
    pub is_running: bool,
    pub can_interrupt: bool,
    pub run_id: Option<String>,
    pub follow_up_behavior: Option<FollowUpBehavior>,
    pub can_steer: bool,
    pub is_send_busy: bool,
    pub send_disabled_reason: Option<String>,
    pub is_environment_unavailable: bool,
    pub is_preparing_worktree: bool,
    pub has_sendable_content: bool,
    pub can_resume: bool,
}

#[derive(Clone)]
pub struct ComposerActionsSession<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    key: ThreadKey,
}

impl<P: Platform + Clone + 'static> ComposerActionsSession<P> {
    pub fn new(runtime: ClientRuntime<P>, key: ThreadKey) -> Self { Self { runtime, key } }

    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        futures::stream::select_all(vec![
            self.runtime.store().subscribe_environment(&self.key.environment).map(|_| ()).boxed_local(),
            self.runtime.composer_drafts().changes(), self.runtime.message_changes(),
        ]).boxed_local()
    }

    pub fn snapshot(&self) -> Rc<PrimaryActionState> {
        use crate::thread_views::threads::{execution, workflows};
        let snapshot = self.runtime.store().snapshot();
        let environment = snapshot.environments.get(&self.key.environment);
        let thread = environment.and_then(|e| e.threads.get(&self.key.thread));
        let projection = thread.and_then(|t| t.projection.as_deref());
        let session = self.runtime.client(&self.key.environment).and_then(|c| c.session());
        let command = serde_json::from_value::<OrchestrationV2Command>(serde_json::json!({
            "type":"thread.stop", "commandId":"availability", "threadId": self.key.thread
        })).expect("thread stop availability input");
        let can_operate = session.as_ref().is_some_and(|s| s.available::<WsOrchestrationV2DispatchCommandRpc>(&command));
        let unavailable = session.is_none();
        let millis = |date: &t3_contracts::wire::WireDateTime| crate::time::epoch_ms(&date.0).unwrap_or(0);
        let runtime = projection.and_then(|p| execution::derive_thread_structure_runtime(p, &millis));
        let resumable = projection.and_then(|p| execution::derive_resumable_run_id(p, runtime.as_ref(), &millis));
        let run = projection.and_then(workflows::resolve_active_thread_run);
        let running = run.is_some_and(|r| execution::activity_status(&r.status));
        let preparing = run.is_some_and(|r| matches!(r.status, OrchestrationV2RunStatus::Preparing | OrchestrationV2RunStatus::Starting));
        let pending = self.runtime.pending_messages(&self.key);
        let busy = pending.iter().any(|p| matches!(p.state,
            super::send::SendState::Preparing | super::send::SendState::Dispatching | super::send::SendState::AwaitingConfirmation));
        let document = self.runtime.composer_drafts().document();
        let document = document.borrow();
        let draft = &document["draftsByThreadKey"][super::draft_service::draft_key(&self.key)];
        let contexts = draft["terminalContexts"].as_array().into_iter().flatten().filter_map(|c| {
            Some(crate::view_logic::chat_view::TerminalContextDraft {
                id: c["id"].as_str()?.into(), terminal_label: c["terminalLabel"].as_str().unwrap_or_default().into(),
                line_start: c["lineStart"].as_u64().unwrap_or(0), line_end: c["lineEnd"].as_u64().unwrap_or(0),
                text: c["text"].as_str().unwrap_or_default().into(),
            })
        }).collect::<Vec<_>>();
        let count = |field: &str| draft[field].as_array().map_or(0, Vec::len);
        let content = crate::view_logic::chat_view::derive_composer_send_state(
            draft["prompt"].as_str().unwrap_or_default(), count("images"), count("files"), &contexts,
            count("previewAnnotations") + count("reviewComments") + count("threadContexts") + count("environmentContexts"),
        );
        let pending_approval = projection.is_some_and(|p| !crate::thread_views::requests::derive_pending(&p.runtime_requests, &p.turn_items).approvals.is_empty());
        let follow_up = if pending_approval { Some(FollowUpBehavior::Steer) } else {
            match document["followUpBehavior"].as_str() {
                Some("queue") => Some(FollowUpBehavior::Queue), _ => Some(FollowUpBehavior::Steer),
            }
        };
        let compaction_running = projection.is_some_and(|p| p.turn_items.iter().any(|item| {
            matches!(item, OrchestrationV2TurnItem::Compaction(c) if c.status == OrchestrationV2TurnItemStatus::Running && c.run_id == runtime.as_ref().and_then(|r| r.active_run_id.clone()))
        }));
        let can_steer = !crate::operations::compaction_blocks_steering(false, compaction_running,
            workflows::thread_steers_during_compaction(projection), None);
        let reason = if !can_operate { Some("This connection cannot change threads.") }
            else if projection.is_none() { Some("Messages loading") } else { None };
        Rc::new(PrimaryActionState {
            can_operate_thread: can_operate, is_running: running, can_interrupt: can_operate && execution::thread_runtime_has_interruptible_run(runtime.as_ref()),
            run_id: runtime.and_then(|r| match r.active_run_id { Some(IdOrIndex::Id(id)) => Some(id.0), _ => None }),
            follow_up_behavior: follow_up, can_steer, is_send_busy: busy,
            send_disabled_reason: reason.map(str::to_owned), is_environment_unavailable: unavailable,
            is_preparing_worktree: preparing, has_sendable_content: content.has_sendable_content,
            can_resume: projection.is_some_and(|p| execution::can_resume_thread(&p.runtime_requests, resumable.as_ref(),
                p.runs.iter().any(|r| r.queue_held == Some(Some(true))))),
        })
    }
}
