//! Current composer actions, derived from the destination and the authoritative draft.
use crate::{composition::ClientRuntime, platform::Platform, types::ThreadKey};
use futures::{StreamExt, stream::LocalBoxStream};
use std::{cell::RefCell, rc::Rc};
use t3_contracts::{
    orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
    rpc::WsOrchestrationV2DispatchCommandRpc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FollowUpBehavior {
    Queue,
    Steer,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingActionState {
    pub question_index: usize,
    pub is_last_question: bool,
    pub can_advance: bool,
    pub is_responding: bool,
    pub is_complete: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrimaryActionState {
    pub pending_action: Option<PendingActionState>,
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
    /// ChatView's `resumeCompactionTokens`: the context the next send compacts first.
    pub compact_tokens: Option<u64>,
    pub keep_full_history: bool,
    /// `freshSessionCommandAvailable`: messages exist, the server supports fresh context, and no
    /// question or approval owns the composer.
    pub fresh_session_available: bool,
    pub fresh_session_armed: bool,
    pub is_resuming: bool,
}

/// What `planComposerTurn` reads for one send, taken when the send starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TurnOptions {
    pub fresh_arm: Option<u64>,
    pub compact_before_send: bool,
}

/// `formatContextWindowTokens` (apps/web/src/lib/contextWindow.ts).
pub fn format_context_window_tokens(value: u64) -> String {
    let fixed = |scaled: f64| {
        let text = format!("{scaled:.1}");
        text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
    };
    if value < 1_000 {
        value.to_string()
    } else if value < 10_000 {
        format!("{}k", fixed(value as f64 / 1_000.0))
    } else if value < 999_500 {
        format!("{}k", (value as f64 / 1_000.0).round())
    } else {
        format!("{}m", fixed(value as f64 / 1_000_000.0))
    }
}

type CachedThreadFacts = (Rc<OrchestrationV2ThreadProjection>, Rc<ThreadFacts>);

#[derive(Clone)]
pub struct ComposerActionsSession<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    key: ThreadKey,
    view: Rc<RefCell<Option<Rc<PrimaryActionState>>>>,
    facts: Rc<RefCell<Option<CachedThreadFacts>>>,
    derivations: Rc<std::cell::Cell<u64>>,
    compaction: Rc<RefCell<Option<(CompactionKey, Option<u64>)>>>,
    compaction_derivations: Rc<std::cell::Cell<u64>>,
}

/// Inputs of `compact_before_send_tokens` other than the draft (whose typed `/compact` the send
/// plan checks): identities, the selection, idleness and the current minute.
#[derive(PartialEq)]
struct CompactionKey {
    projection: *const OrchestrationV2ThreadProjection,
    config: *const t3_contracts::server::ServerConfig,
    selection: t3_contracts::model_selection::ModelSelection,
    idle: bool,
    minute: u64,
}

impl<P: Platform + Clone + 'static> ComposerActionsSession<P> {
    pub fn new(runtime: ClientRuntime<P>, key: ThreadKey) -> Self {
        Self {
            runtime,
            key,
            view: Rc::default(),
            facts: Rc::default(),
            derivations: Rc::default(),
            compaction: Rc::default(),
            compaction_derivations: Rc::default(),
        }
    }

    fn compact_tokens(
        &self,
        projection: Option<&Rc<OrchestrationV2ThreadProjection>>,
        config: Option<&Rc<t3_contracts::server::ServerConfig>>,
        selection: &t3_contracts::model_selection::ModelSelection,
        idle: bool,
    ) -> Option<u64> {
        let (projection, config) = (projection?, config?);
        let key = CompactionKey {
            projection: Rc::as_ptr(projection),
            config: Rc::as_ptr(config),
            selection: selection.clone(),
            idle,
            minute: self.runtime.now_epoch_ms() / 60_000,
        };
        let mut cached = self.compaction.borrow_mut();
        if let Some((prior, tokens)) = cached.as_ref()
            && *prior == key
        {
            return *tokens;
        }
        self.compaction_derivations
            .set(self.compaction_derivations.get() + 1);
        let now = crate::time::iso_from_epoch_ms(key.minute * 60_000);
        let tokens = crate::thread_views::threads::context_window::compact_before_send_tokens(
            crate::thread_views::threads::context_window::CompactBeforeSendInput {
                projection,
                selection,
                providers: &config.providers,
                settings: &config.settings,
                text: "",
                thread_idle: idle,
                now: &now,
            },
            crate::time::epoch_ms,
        );
        *cached = Some((key, tokens));
        tokens
    }

    /// The fresh-session arm and compaction for a send of `text`, read when it starts.
    pub fn turn_options(&self, text: &str) -> TurnOptions {
        let state = self.snapshot();
        let fresh_arm = state
            .fresh_session_available
            .then(|| {
                self.runtime
                    .composer_thread_flags()
                    .fresh_session_arm(&self.key)
            })
            .flatten();
        let typed_compact = text
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("/compact"))
            && text[8..]
                .chars()
                .next()
                .is_none_or(crate::view_logic::is_js_whitespace);
        TurnOptions {
            fresh_arm,
            compact_before_send: fresh_arm.is_none()
                && state.compact_tokens.is_some()
                && !state.keep_full_history
                && !typed_compact,
        }
    }

    /// After an accepted send: the arm it read is spent and full history is no longer kept.
    pub fn sent(&self, options: TurnOptions) {
        let flags = self.runtime.composer_thread_flags();
        flags.set_keep_full_history(&self.key, false);
        if let Some(arm) = options.fresh_arm {
            flags.disarm_fresh_session(&self.key, Some(arm));
        }
    }

    fn thread_facts(
        &self,
        projection: Option<&Rc<OrchestrationV2ThreadProjection>>,
    ) -> Rc<ThreadFacts> {
        let Some(projection) = projection else {
            return Rc::new(derive_thread_facts(None));
        };
        let mut cached = self.facts.borrow_mut();
        if let Some((source, facts)) = cached.as_ref()
            && Rc::ptr_eq(source, projection)
        {
            return facts.clone();
        }
        self.derivations.set(self.derivations.get() + 1);
        let facts = Rc::new(derive_thread_facts(Some(&**projection)));
        *cached = Some((projection.clone(), facts.clone()));
        facts
    }

    pub async fn set_follow_up_behavior(
        &self,
        behavior: FollowUpBehavior,
    ) -> Result<(), crate::ClientError> {
        self.runtime
            .composer_preferences()
            .update(|settings| {
                settings.follow_up_behavior = match behavior {
                    FollowUpBehavior::Queue => {
                        t3_contracts::settings::ClientSettingsSchemaFollowUpBehavior::Queue
                    }
                    FollowUpBehavior::Steer => {
                        t3_contracts::settings::ClientSettingsSchemaFollowUpBehavior::Steer
                    }
                }
            })
            .await
    }

    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        futures::stream::select_all(vec![
            self.runtime.timeline_rewind().changes(&self.key),
            self.runtime.composer_drafts().changes(),
            self.runtime.message_changes(),
            self.runtime.composer_preferences().changes(),
            self.runtime.pending_user_input(&self.key).changes(),
            self.runtime.composer_thread_flags().changes(),
        ])
        .boxed_local()
    }

    pub fn snapshot(&self) -> Rc<PrimaryActionState> {
        let question = self.runtime.pending_user_input(&self.key).view();
        let models = self.runtime.composer_models(&self.key).snapshot();
        let target = question.as_ref().map_or_else(
            || super::target::ComposerTarget::Thread(self.key.clone()),
            |view| super::target::ComposerTarget::Draft {
                draft_id: view.question_draft_key.clone(),
                environment_id: Some(self.key.environment.clone()),
            },
        );
        let attachments = self.runtime.composer_attachments(&target).ok();
        let attachment_reason = attachments.as_ref().and_then(|attachments| {
            attachments.block_reason().or_else(|| {
                attachments
                    .draft_attachments()
                    .err()
                    .map(|error| error.to_string())
            })
        });
        let prepared = attachments
            .as_ref()
            .and_then(|attachments| attachments.draft_attachments().ok())
            .unwrap_or_default();
        let snapshot = self.runtime.store().snapshot();
        let environment = snapshot.environments.get(&self.key.environment);
        let thread = environment.and_then(|e| e.threads.get(&self.key.thread));
        let projection = thread.and_then(|t| t.projection.as_deref());
        let session = self
            .runtime
            .client(&self.key.environment)
            .and_then(|c| c.session());
        let command = serde_json::from_value::<OrchestrationV2Command>(serde_json::json!({
            "type":"thread.stop", "commandId":"availability", "threadId": self.key.thread
        }))
        .expect("thread stop availability input");
        let can_operate = session
            .as_ref()
            .is_some_and(|s| s.available::<WsOrchestrationV2DispatchCommandRpc>(&command));
        let unavailable = session.is_none();
        let facts = self.thread_facts(thread.and_then(|t| t.projection.as_ref()));
        let flags = self.runtime.composer_thread_flags();
        let config = environment.and_then(|e| e.config.as_ref());
        let fresh_session_available = facts.has_messages
            && !facts.pending_approval
            && question.is_none()
            && config.is_some_and(|c| c.environment.capabilities.fresh_context == Some(true));
        let compact_tokens = self.compact_tokens(
            thread.and_then(|t| t.projection.as_ref()),
            config,
            &models.selection,
            !facts.running,
        );
        let pending = self.runtime.pending_messages(&self.key);
        let busy = pending.iter().any(|p| {
            matches!(
                p.state,
                super::send::SendState::Preparing
                    | super::send::SendState::Dispatching
                    | super::send::SendState::AwaitingConfirmation
            )
        });
        let document = self.runtime.composer_drafts().document();
        let document = document.borrow();
        let draft = &document["draftsByThreadKey"]
            [self.runtime.composer_drafts().editor_draft_key(&self.key)];
        let contexts = draft["terminalContexts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                Some(crate::view_logic::chat_view::TerminalContextDraft {
                    id: c["id"].as_str()?.into(),
                    terminal_label: c["terminalLabel"].as_str().unwrap_or_default().into(),
                    line_start: c["lineStart"].as_u64().unwrap_or(0),
                    line_end: c["lineEnd"].as_u64().unwrap_or(0),
                    text: c["text"].as_str().unwrap_or_default().into(),
                })
            })
            .collect::<Vec<_>>();
        let count = |field: &str| draft[field].as_array().map_or(0, Vec::len);
        let content = crate::view_logic::chat_view::derive_composer_send_state(
            draft["prompt"].as_str().unwrap_or_default(),
            count("images"),
            count("files"),
            &contexts,
            count("previewAnnotations") + count("reviewComments"),
        );
        let follow_up = if facts.pending_approval {
            Some(FollowUpBehavior::Steer)
        } else {
            match self
                .runtime
                .composer_preferences()
                .snapshot()
                .follow_up_behavior
            {
                t3_contracts::settings::ClientSettingsSchemaFollowUpBehavior::Queue => {
                    Some(FollowUpBehavior::Queue)
                }
                t3_contracts::settings::ClientSettingsSchemaFollowUpBehavior::Steer => {
                    Some(FollowUpBehavior::Steer)
                }
            }
        };
        let compact_request_active = (busy
            && pending.last().is_some_and(|message| {
                crate::view_logic::chat_view::is_compact_command_message(
                    true,
                    &message.message.text,
                    message.message.attachments.len(),
                )
            }))
            || facts.compact_message_active;
        let can_steer = !crate::operations::compaction_blocks_steering(
            compact_request_active && !facts.compaction_settled,
            facts.compaction_running,
            facts.steers_during_compaction,
            facts
                .run_message
                .as_ref()
                .map(|(text, attachments)| (text.as_str(), *attachments)),
        );
        let reason = if !can_operate {
            Some("This connection cannot change threads.".into())
        } else if self
            .runtime
            .timeline_rewind()
            .rewind_state(&self.key)
            .reverting
        {
            Some("Rewinding conversation".into())
        } else if projection.is_none() {
            Some("Messages loading".into())
        } else {
            environment
                .and_then(|e| e.config.as_ref())
                .zip(projection)
                .and_then(|(config, projection)| {
                    let settings = super::delivery_policy::ThreadSettings {
                        model_selection: models.selection.clone(),
                        runtime_mode: projection.thread.runtime_mode,
                        interaction_mode: projection.thread.interaction_mode,
                    };
                    let project = match &projection.thread.project_id {
                        IdOrIndex::Id(id) => Some(id),
                        IdOrIndex::Index(_) => None,
                    };
                    attachment_reason.clone().or_else(|| {
                        super::send_policy::composer_block_reason(
                            config, project, &prepared, &settings,
                        )
                    })
                })
        };
        let next = PrimaryActionState {
            pending_action: question.as_ref().map(|view| PendingActionState {
                question_index: view.question_index,
                is_last_question: view.is_last_question,
                can_advance: !view.response_disabled,
                is_responding: view.is_responding,
                is_complete: view.is_complete,
            }),
            can_operate_thread: can_operate,
            is_running: facts.running,
            can_interrupt: can_operate && facts.can_interrupt_running,
            run_id: facts.run_id.clone(),
            follow_up_behavior: follow_up,
            can_steer,
            is_send_busy: busy,
            send_disabled_reason: reason,
            is_environment_unavailable: unavailable,
            is_preparing_worktree: facts.preparing,
            has_sendable_content: content.has_sendable_content,
            can_resume: facts.can_resume,
            compact_tokens,
            keep_full_history: flags.keep_full_history(&self.key),
            fresh_session_available,
            fresh_session_armed: fresh_session_available
                && flags.fresh_session_arm(&self.key).is_some(),
            is_resuming: flags.is_resuming(&self.key),
        };
        let mut cached = self.view.borrow_mut();
        if let Some(prior) = cached.as_ref().filter(|prior| ***prior == next) {
            return prior.clone();
        }
        let next = Rc::new(next);
        *cached = Some(next.clone());
        next
    }
}

/// What the primary actions read from the thread projection alone. Derived once per projection
/// (`Rc` identity), so a draft keystroke does not re-derive the thread's runtime.
#[derive(Debug, Default, PartialEq)]
struct ThreadFacts {
    running: bool,
    preparing: bool,
    has_messages: bool,
    pending_approval: bool,
    compact_message_active: bool,
    compaction_settled: bool,
    compaction_running: bool,
    steers_during_compaction: bool,
    run_message: Option<(String, usize)>,
    can_interrupt_running: bool,
    run_id: Option<String>,
    can_resume: bool,
}

fn derive_thread_facts(projection: Option<&OrchestrationV2ThreadProjection>) -> ThreadFacts {
    use crate::thread_views::threads::{execution, workflows};
    let millis =
        |date: &t3_contracts::wire::WireDateTime| crate::time::epoch_ms(&date.0).unwrap_or(0);
    let runtime = projection.and_then(|p| execution::derive_thread_structure_runtime(p, &millis));
    let resumable =
        projection.and_then(|p| execution::derive_resumable_run_id(p, runtime.as_ref(), &millis));
    let run = projection.and_then(workflows::resolve_active_thread_run);
    let runtime_view = runtime
        .as_ref()
        .map(|r| crate::view_logic::session::RuntimeView {
            status: r.status.clone(),
            active_run_id: r.active_run_id.clone(),
            activity_started_at: r
                .activity_started_at
                .as_ref()
                .map(|value| Some(value.0.clone())),
        });
    let phase = crate::view_logic::session::derive_phase(runtime_view.as_ref());
    let running = matches!(
        phase,
        crate::view_logic::session::SessionPhase::Connecting
            | crate::view_logic::session::SessionPhase::Running
    );
    let preparing = run.is_some_and(|r| {
        matches!(
            r.status,
            OrchestrationV2RunStatus::Preparing | OrchestrationV2RunStatus::Starting
        )
    });
    let pending_approval = projection.is_some_and(|p| {
        !crate::thread_views::requests::derive_pending(&p.runtime_requests, &p.turn_items)
            .approvals
            .is_empty()
    });
    let activity = projection.and_then(|p| execution::derive_thread_activity_run(p, &millis));
    let activity_id = activity.as_ref().map(|run| &run.run_id);
    let activity_view = activity
        .as_ref()
        .map(|run| crate::view_logic::session::RunView {
            run_id: run.run_id.clone(),
            status: run.status.clone(),
            started_at: run.started_at.as_ref().map(|at| at.0.clone()),
            requested_at: Some(run.requested_at.0.clone()),
            completed_at: run.completed_at.as_ref().map(|at| at.0.clone()),
        });
    let compact_message_active = projection.is_some_and(|p| {
        p.visible_turn_items
            .iter()
            .rev()
            .find_map(|row| match &row.item {
                OrchestrationV2TurnItem::UserMessage(message) => Some(message),
                _ => None,
            })
            .is_some_and(|message| {
                crate::view_logic::chat_view::is_compact_command_message(
                    true,
                    &message.text,
                    message.attachments.len(),
                ) && activity_id.is_some()
                    && message.run_id.as_ref() == activity_id
                    && !crate::view_logic::session::is_latest_run_settled(
                        activity_view.as_ref(),
                        runtime_view.as_ref(),
                    )
            })
    });
    let compaction_settled = projection.is_some_and(|p| p.visible_turn_items.iter().any(|row| {
            matches!(&row.item, OrchestrationV2TurnItem::Compaction(item)
                if activity_id.is_some() && item.run_id.as_ref() == activity_id && matches!(item.status,
                    OrchestrationV2TurnItemStatus::Completed | OrchestrationV2TurnItemStatus::Failed
                    | OrchestrationV2TurnItemStatus::Interrupted | OrchestrationV2TurnItemStatus::Cancelled))
        }));
    let active_run_id = runtime.as_ref().and_then(|run| run.active_run_id.as_ref());
    let compaction_running = projection.is_some_and(|p| {
        p.visible_turn_items.iter().any(|row| {
            matches!(&row.item, OrchestrationV2TurnItem::Compaction(item)
                if item.run_id.is_some() && item.run_id.as_ref() == active_run_id
                    && item.status == OrchestrationV2TurnItemStatus::Running)
        })
    });
    let run_message = projection.and_then(|p| {
        let run = p.runs.iter().find(|run| Some(&run.id) == active_run_id)?;
        p.messages
            .iter()
            .find(|message| message.id == run.user_message_id)
    });
    ThreadFacts {
        running,
        preparing,
        has_messages: projection.is_some_and(|p| !p.messages.is_empty()),
        pending_approval,
        compact_message_active,
        compaction_settled,
        compaction_running,
        steers_during_compaction: workflows::thread_steers_during_compaction(projection),
        run_message: run_message.map(|message| (message.text.clone(), message.attachments.len())),
        can_interrupt_running: crate::view_logic::session::derive_can_interrupt_running_thread(
            projection.is_some(),
            runtime_view.as_ref(),
        ),
        run_id: runtime.as_ref().and_then(|r| match &r.active_run_id {
            Some(IdOrIndex::Id(id)) => Some(id.0.clone()),
            _ => None,
        }),
        can_resume: projection.is_some_and(|p| {
            execution::can_resume_thread(
                &p.runtime_requests,
                resumable.as_ref(),
                p.runs.iter().any(|r| r.queue_held == Some(Some(true))),
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{executor::LocalPool, task::LocalSpawnExt};

    fn seeded(
        threads: &[(&str, &str)],
    ) -> (
        LocalPool,
        crate::ClientStore,
        ClientRuntime<crate::testing::FakePlatform>,
        ThreadKey,
        crate::types::Generation,
    ) {
        use crate::testing::environment;
        use serde_json::{from_value, json};
        let pool = LocalPool::new();
        let spawner = pool.spawner();
        let store = crate::ClientStore::default();
        let key = ThreadKey {
            environment: crate::types::EnvironmentId("env".into()),
            thread: crate::types::ThreadId("thread".into()),
        };
        let session = store.begin_session(key.environment.clone());
        store.apply_shell(
            &key.environment,
            session,
            vec![
                from_value(json!({"kind":"snapshot","snapshot":environment::shell(threads)}))
                    .expect("shell snapshot"),
                from_value(json!({"kind":"synchronized"})).expect("synchronized"),
            ],
        );
        store
            .apply_thread(
                key.clone(),
                session,
                vec![
                    from_value(json!({"kind":"snapshot","snapshotSequence":1,
                        "projection":environment::projection("thread","Thread",json!([]),json!([]))}))
                    .expect("thread snapshot"),
                    from_value(json!({"kind":"synchronized"})).expect("synchronized"),
                ],
            )
            .expect("thread applies");
        let runtime = ClientRuntime::new(
            crate::testing::FakePlatform::default(),
            store.clone(),
            t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
            Rc::new(move |future| {
                spawner.spawn_local(future).expect("spawn");
            }),
        );
        (pool, store, runtime, key, session)
    }

    #[test]
    fn typing_reuses_the_threads_derived_facts_until_its_projection_changes() {
        let (mut pool, store, runtime, key, session) = seeded(&[("thread", "Thread")]);
        pool.run_until_stalled();
        let actions = runtime.composer_primary_actions(&key);
        assert!(!actions.snapshot().has_sendable_content);
        let drafts = runtime.composer_drafts();
        for (index, prompt) in ["h", "he", "hel", "hell", "hello"].into_iter().enumerate() {
            drafts
                .set_prompt(&key, prompt.into(), index + 1)
                .expect("draft is writable");
            assert!(actions.snapshot().has_sendable_content);
        }
        assert_eq!(
            actions.derivations.get(),
            1,
            "one derivation for five keystrokes"
        );
        store.update_thread(&key, session, |thread| {
            thread.projection.as_mut().map(Rc::make_mut).is_some()
        });
        actions.snapshot();
        actions.snapshot();
        assert_eq!(
            actions.derivations.get(),
            2,
            "a new projection is derived once"
        );
    }
}
