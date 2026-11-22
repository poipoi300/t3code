//! Port of apps/web/src/components/chat/useWorktreeSetup.ts for a server thread: the setup
//! snapshot a thread's timeline shows, its cancellation and its terminal.
//!
//! The snapshot follows `subscribeWorktreeSetup` while the thread's run is preparing or the
//! thread has a worktree, exactly when the web hook enables its query. A dropped or older
//! snapshot is held until a newer one arrives, and the visible card is chosen by the
//! client's `resolve_worktree_setup_progress` and `resolve_visible_worktree_setup`.
use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::Platform,
    rpc::{RpcEndpoint, RpcMethod},
    services::sessions::EnvironmentStream,
    types::ThreadKey,
    view_logic::chat_view::{resolve_visible_worktree_setup, resolve_worktree_setup_progress},
};
use futures::{
    FutureExt, StreamExt,
    channel::mpsc,
    future::{AbortHandle, LocalBoxFuture},
    stream::LocalBoxStream,
};
use std::rc::Rc;
use t3_contracts::{
    orchestration_v2::{
        ConversationMessageRole, OrchestrationV2RunStatus, OrchestrationV2ShellThreadStatus,
    },
    rpc::{WsSubscribeWorktreeSetupRpc, WsWorktreeSetupCancelRpc},
    worktree_setup::{
        WorktreeSetupCancelInput, WorktreeSetupPhase, WorktreeSetupSnapshot,
        WorktreeSetupSubscribeInput,
    },
};

/// What the timeline needs from `useWorktreeSetup`: the card's snapshot (as the
/// MessagesTimeline row input carries it) and whether the worktree is still being prepared.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorktreeSetupView {
    pub snapshot: Option<Rc<WorktreeSetupSnapshot>>,
    /// The snapshot as `MessagesTimelineRowsInput::worktree_setup` takes it, encoded once.
    pub row_input: Option<serde_json::Value>,
    pub preparing: bool,
}

/// The thread facts the web hook reads from its active thread and activity run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ThreadFacts {
    known: bool,
    has_worktree: bool,
    run_preparing: bool,
    turn_started: bool,
}

#[derive(Clone)]
pub struct WorktreeSetup<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    platform: P,
}

struct Abort(AbortHandle);
impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<P: Platform + Clone + 'static> WorktreeSetup<P> {
    pub fn new(runtime: ClientRuntime<P>, platform: P) -> Self {
        Self { runtime, platform }
    }

    /// The thread's visible setup: the current value first, then every change. The
    /// subscription runs only while the stream is held and the thread needs it.
    pub fn watch(&self, key: ThreadKey) -> LocalBoxStream<'static, WorktreeSetupView> {
        let (sender, receiver) = mpsc::unbounded();
        let runtime = self.runtime.clone();
        let platform = self.platform.clone();
        let driver = async move {
            let store = runtime.store().clone();
            let mut changes = store.subscribe_environment(&key.environment).fuse();
            let mut frames: Option<
                LocalBoxStream<'_, Result<Option<WorktreeSetupSnapshot>, ClientError>>,
            > = None;
            let mut latest: Option<Rc<WorktreeSetupSnapshot>> = None;
            let mut held: Option<Rc<WorktreeSetupSnapshot>> = None;
            let mut sent: Option<WorktreeSetupView> = None;
            loop {
                let facts = thread_facts(&runtime, &key);
                let enabled = facts.known && (facts.run_preparing || facts.has_worktree);
                if enabled && frames.is_none() {
                    let input = WorktreeSetupSubscribeInput {
                        thread_id: key.thread.clone(),
                    };
                    let sessions = runtime
                        .query_connections(&key.environment)
                        .map(|connection| connection.session)
                        .boxed_local();
                    frames = Some(
                        EnvironmentStream::recovering(
                            key.environment.clone(),
                            sessions,
                            &platform,
                            <WsSubscribeWorktreeSetupRpc as RpcMethod>::NAME,
                            move |session| {
                                session
                                    .endpoint
                                    .subscribe::<WsSubscribeWorktreeSetupRpc>(input.clone())
                            },
                        )
                        .boxed_local(),
                    );
                } else if !enabled && frames.is_some() {
                    // The web query is switched off: its data becomes undefined, the hold stays.
                    frames = None;
                    latest = None;
                }
                let view = resolve_view(&runtime, &key, facts, latest.as_ref(), held.as_ref());
                if sent.as_ref() != Some(&view) {
                    if sender.unbounded_send(view.clone()).is_err() {
                        return;
                    }
                    sent = Some(view);
                }
                let frame = match frames.as_mut() {
                    Some(frames) => futures::select! {
                        change = changes.next() => match change {
                            Some(_) => None,
                            None => return,
                        },
                        frame = frames.next().fuse() => Some(frame),
                    },
                    None => match changes.next().await {
                        Some(_) => None,
                        None => return,
                    },
                };
                match frame {
                    None => {}
                    Some(Some(Ok(value))) => {
                        latest = value.map(Rc::new);
                        if let Some(next) = &latest {
                            held = Some(match held.take() {
                                Some(current)
                                    if current.thread_id == next.thread_id
                                        && current.sequence > next.sequence =>
                                {
                                    current
                                }
                                _ => next.clone(),
                            });
                        }
                    }
                    // A failed or ended stream leaves the last data in place, as the query does.
                    Some(Some(Err(_))) => {}
                    Some(None) => frames = None,
                }
            }
        };
        let abort = Abort(self.runtime.spawn(driver.boxed_local()));
        receiver
            .map(move |view| {
                let _ = &abort;
                view
            })
            .boxed_local()
    }

    /// `useWorktreeSetup().cancel`: true when the server cancelled the running setup. Failures
    /// stay typed here; the web reports none of them (`reportFailure: false`).
    pub fn cancel(
        &self,
        key: &ThreadKey,
        setup: &WorktreeSetupSnapshot,
    ) -> LocalBoxFuture<'static, Result<bool, ClientError>> {
        if setup.phase != WorktreeSetupPhase::Running {
            return async { Ok(false) }.boxed_local();
        }
        let endpoint = self
            .runtime
            .client(&key.environment)
            .and_then(|client| client.session());
        let input = WorktreeSetupCancelInput {
            thread_id: setup.thread_id.clone(),
        };
        async move {
            let endpoint = endpoint.ok_or(ClientError::Disconnected)?;
            endpoint
                .command::<WsWorktreeSetupCancelRpc>(input)
                .await
                .map(|result| result.cancelled)
                .map_err(|error| error.into_client_error())
        }
        .boxed_local()
    }

    /// `openSetupTerminal`: offered only for the setup of the thread being shown (a failed
    /// bootstrap deletes other threads' terminals). Opens and activates the terminal tab.
    pub fn open_terminal(
        &self,
        key: &ThreadKey,
        setup: &WorktreeSetupSnapshot,
        terminal: &str,
    ) -> bool {
        if setup.thread_id != key.thread {
            return false;
        }
        self.runtime.terminals().ensure(key, terminal, true, true);
        true
    }
}

fn thread_facts<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
    key: &ThreadKey,
) -> ThreadFacts {
    let Some(shell) = runtime.store().shared_shell(&key.environment) else {
        return ThreadFacts::default();
    };
    let Some(thread) = shell.threads.iter().find(|thread| thread.id == key.thread) else {
        return ThreadFacts::default();
    };
    ThreadFacts {
        known: true,
        has_worktree: thread.worktree_path.is_some(),
        run_preparing: thread.latest_run_id.is_some()
            && thread.status == OrchestrationV2ShellThreadStatus::Preparing,
        turn_started: thread.latest_run_id.is_some()
            && thread
                .latest_run_started_at
                .as_ref()
                .is_some_and(Option::is_some),
    }
}

fn resolve_view<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
    key: &ThreadKey,
    facts: ThreadFacts,
    latest: Option<&Rc<WorktreeSetupSnapshot>>,
    held: Option<&Rc<WorktreeSetupSnapshot>>,
) -> WorktreeSetupView {
    let run_status = facts
        .run_preparing
        .then_some(OrchestrationV2RunStatus::Preparing);
    let (live, preparing) = resolve_worktree_setup_progress(
        &key.thread,
        false,
        run_status.as_ref(),
        latest.map(Rc::as_ref),
        held.map(Rc::as_ref),
    );
    // Read only while a card could show; the second user message is near the start.
    let follow_up_sent = live.is_some() && user_messages_exceed_one(runtime, key);
    let visible = resolve_visible_worktree_setup(live, None, facts.turn_started, follow_up_sent);
    let snapshot = visible.and_then(|visible| {
        [latest, held]
            .into_iter()
            .flatten()
            .find(|candidate| std::ptr::eq(candidate.as_ref(), visible))
            .cloned()
    });
    WorktreeSetupView {
        row_input: snapshot
            .as_ref()
            .and_then(|snapshot| serde_json::to_value(snapshot.as_ref()).ok()),
        snapshot,
        preparing,
    }
}

fn user_messages_exceed_one<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
    key: &ThreadKey,
) -> bool {
    runtime.store().shared_thread(key).is_some_and(|thread| {
        thread
            .messages
            .iter()
            .filter(|message| message.role == ConversationMessageRole::User)
            .nth(1)
            .is_some()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(phase: &str, sequence: u64) -> WorktreeSetupSnapshot {
        serde_json::from_value(json!({"threadId":"t1","phase":phase,"startedAt":"2026-10-26T12:00:00.000Z",
            "endedAt":null,"branch":"t3/feature","baseRef":"main","worktreePath":null,"setupScript":null,
            "stages":[{"id":"agent","status":"pending","startedAt":null,"endedAt":null,"percent":null,"detail":null,"tail":[]}],
            "error":null,"sequence":sequence}))
        .unwrap()
    }

    #[test]
    fn the_row_input_is_the_snapshot_wire_shape() {
        let value = serde_json::to_value(snapshot("running", 1)).unwrap();
        assert_eq!(value["phase"], "running");
        assert_eq!(value["setupScript"], serde_json::Value::Null);
        assert_eq!(value["stages"][0]["id"], "agent");
    }
}
