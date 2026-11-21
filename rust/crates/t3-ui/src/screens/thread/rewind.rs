//! Port of apps/web/src/components/chat/useTimelineRewind.ts useTimelineRewind.

use std::rc::Rc;

use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use t3_client::thread_views::messages_timeline::{MessagesTimelineRow, MessagesTimelineRowContent};
use t3_client::types::{ThreadId, ThreadKey};
use t3_contracts::base_schemas::{MessageId, RunId, TurnItemId};

use super::rows::assistant::{ForkHandler, ForkRequest};
use super::rows::user::RevertAction;
use crate::i18n::Text;
use crate::state::nav::{Navigator, Route, navigator};
use crate::ui::confirm::{RevertChoice, request_revert_dialog};
use futures::FutureExt;
use t3_contracts::orchestration_v2_id_table::IdOrIndex;

use crate::runtime::tasks::{TaskOwner, watch};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewindTarget {
    pub message_id: MessageId,
    pub turn_count: u64,
}

impl RewindTarget {
    pub fn from_row(row: &MessagesTimelineRow) -> Option<Self> {
        let MessagesTimelineRowContent::Message {
            message,
            revert_turn_count: Some(turn_count),
            ..
        } = &row.content
        else {
            return None;
        };
        let t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(message_id) = &message.id else {
            return None;
        };
        (message.role == "user").then(|| Self {
            message_id: message_id.clone(),
            turn_count: *turn_count,
        })
    }
}

/// Client sentences are keys; server details remain literal content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewindError {
    pub message: String,
    pub detail: Option<String>,
}

impl RewindError {
    pub fn message(&self) -> Text {
        Text::client_key(self.message.clone())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewindAvailability {
    pub can_rewind: bool,
    /// The client includes worktree isolation AND checkpoint readiness in this answer.
    pub can_restore_files: bool,
    pub reason: Option<RewindError>,
}

pub use t3_client::thread_views::thread_ops::rewind::RewindState;

/// Thin binding over the client's composed rewind and fork operations.
/// Implementations restore draft/attachments only after persisted rollback and recheck all guards.
pub trait TimelineRewindClient {
    fn state(&self, key: &ThreadKey) -> RewindState;
    fn has_operate_scope(&self, _key: &ThreadKey) -> bool {
        false
    }
    fn availability(&self, key: &ThreadKey, target: &RewindTarget) -> RewindAvailability;
    fn cancel_confirmation(&self, _key: &ThreadKey) {}
    fn changes(&self, key: &ThreadKey) -> LocalBoxStream<'static, ()>;
    fn rewind(
        &self,
        key: ThreadKey,
        target: RewindTarget,
        restore_files: bool,
    ) -> LocalBoxFuture<'static, Result<String, RewindError>>;
    fn fork(
        &self,
        viewed: ThreadKey,
        source: ThreadId,
        response: TurnItemId,
        run_id: RunId,
    ) -> LocalBoxFuture<'static, Result<ThreadKey, RewindError>>;
}

/// Optional override; normal screens bind from Backend::client().
#[derive(Clone)]
pub struct RewindService(pub Rc<dyn TimelineRewindClient>);

struct PendingRewindConfirmation {
    client: Rc<dyn TimelineRewindClient>,
    key: ThreadKey,
    dispatched: bool,
}

impl Drop for PendingRewindConfirmation {
    fn drop(&mut self) {
        if !self.dispatched {
            self.client.cancel_confirmation(&self.key);
        }
    }
}

#[derive(Clone, Copy)]
pub struct TimelineRewind {
    key: Stored<ThreadKey>,
    client: Stored<Option<Rc<dyn TimelineRewindClient>>>,
    owner: TaskOwner,
    nav: Navigator,
    pending: Signal<bool>,
    pub reverting: Memo<bool>,
    pub can_operate: Memo<bool>,
    pub error: Signal<Option<RewindError>>,
    pub restored_prompt: Signal<Option<String>>,
}

pub fn use_timeline_rewind(key: ThreadKey) -> TimelineRewind {
    let service = use_context::<RewindService>().or_else(|| {
        use_context::<crate::runtime::Rt>()?
            .backend()
            .client()
            .map(|client| crate::runtime::services::thread_ops::timeline_rewind(&client))
    });
    TimelineRewind::with_client(key, service.map(|service| service.0))
}

impl TimelineRewind {
    /// Construct above the virtual list so recycling does not cancel a confirmed operation.
    pub fn with_client(key: ThreadKey, client: Option<Rc<dyn TimelineRewindClient>>) -> Self {
        let read = |client: &Rc<dyn TimelineRewindClient>, key: &ThreadKey| {
            (client.state(key).reverting, client.has_operate_scope(key))
        };
        let state = signal(
            client
                .as_ref()
                .map_or((false, false), |client| read(client, &key)),
        );
        if let Some(client) = &client {
            let mut changes = client.changes(&key);
            let client = client.clone();
            let key = key.clone();
            watch(async move {
                while changes.next().await.is_some() {
                    state.set(read(&client, &key));
                }
            });
        }
        let pending = signal(false);
        Self {
            key: stored(key),
            client: stored(client),
            owner: TaskOwner::here(),
            nav: navigator(),
            pending,
            reverting: computed(move || pending.get() || state.get().0),
            can_operate: computed(move || state.get().1),
            error: signal(None),
            restored_prompt: signal(None),
        }
    }

    /// The user row's "Edit from here" (`onRevertToTurnCount`); none without a rewind service.
    pub fn revert_action(self) -> Option<RevertAction> {
        self.client.with(|client| {
            client.as_ref().map(|_| {
                Rc::new(
                    move |row: &t3_client::thread_views::user_message::UserRow, count: usize| {
                        let IdOrIndex::Id(message_id) = &row.message.id else {
                            return;
                        };
                        self.request_revert(RewindTarget {
                            message_id: message_id.clone(),
                            turn_count: count as u64,
                        });
                    },
                ) as RevertAction
            })
        })
    }

    /// The assistant row's `onForkFromRun`: forks the item's source thread at `run_id`, opens the
    /// fork, and leaves a failure in [`TimelineRewind::error`] as the web's thread error.
    pub fn fork_action(self) -> Option<ForkHandler> {
        self.client.with(|client| {
            client.as_ref().map(|_| {
                Rc::new(move |request: ForkRequest| {
                    let (IdOrIndex::Id(source), IdOrIndex::Id(response), IdOrIndex::Id(run_id)) = (
                        request.source_thread_id,
                        request.source_item_id,
                        request.run_id,
                    ) else {
                        return futures::future::ready(Ok(())).boxed_local();
                    };
                    self.fork_thread(source, response, run_id).boxed_local()
                }) as ForkHandler
            })
        })
    }

    pub fn availability(self, target: &RewindTarget) -> RewindAvailability {
        self.client.with(|client| {
            client
                .as_ref()
                .map_or_else(RewindAvailability::default, |client| {
                    client.availability(&self.key.get(), target)
                })
        })
    }

    pub fn request_revert(self, target: RewindTarget) {
        if self.reverting.get_untracked() {
            return;
        }
        let availability = self.availability(&target);
        if !availability.can_rewind {
            self.error.set(availability.reason);
            return;
        }
        let Some(client) = self.client.get() else {
            return;
        };
        let confirmation = PendingRewindConfirmation {
            client,
            key: self.key.get(),
            dispatched: false,
        };
        let Some(answer) = request_revert_dialog(availability.can_restore_files) else {
            return;
        };
        self.pending.set(true);
        self.owner.spawn(async move {
            let _pending = Pending(self.pending);
            let mut confirmation = confirmation;
            let Some(choice) = answer.await else {
                return;
            };
            let restore_files = choice == RevertChoice::RestoreFiles;
            let key = &confirmation.key;
            let availability = confirmation.client.availability(key, &target);
            if !availability.can_rewind || (restore_files && !availability.can_restore_files) {
                self.error.set(availability.reason);
                return;
            }
            self.error.set(None);
            self.restored_prompt.set(None);
            confirmation.dispatched = true;
            match confirmation
                .client
                .rewind(key.clone(), target, restore_files)
                .await
            {
                Ok(prompt) if self.nav.top().thread() == Some(key) => {
                    self.restored_prompt.set(Some(prompt))
                }
                Ok(_) => {}
                Err(error) => self.error.set(Some(error)),
            }
        });
    }

    pub fn on_fork_from_run(self, response: TurnItemId, run_id: RunId) {
        let source = self.key.get().thread;
        let fork = self.fork_thread(source, response, run_id);
        self.owner.spawn(async move {
            let _ = fork.await;
        });
    }

    fn fork_thread(
        self,
        source: ThreadId,
        response: TurnItemId,
        run_id: RunId,
    ) -> impl std::future::Future<Output = Result<(), String>> + 'static {
        let client = (!self.reverting.get_untracked())
            .then(|| self.client.get())
            .flatten();
        if client.is_some() {
            self.pending.set(true);
        }
        let viewed = self.key.get();
        async move {
            let Some(client) = client else {
                return Ok(());
            };
            let _pending = Pending(self.pending);
            let _ = self.error.try_set(None);
            match client.fork(viewed, source, response, run_id).await {
                Ok(target) => {
                    self.nav.open(Route::Thread(target));
                    Ok(())
                }
                Err(error) => {
                    let message = error.message.clone();
                    let _ = self.error.try_set(Some(error));
                    Err(message)
                }
            }
        }
    }
}

struct Pending(Signal<bool>);
impl Drop for Pending {
    fn drop(&mut self) {
        let _ = self.0.try_set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_client::thread_views::messages_timeline::{
        MessagesTimelineRowsInput, derive_messages_timeline_rows,
    };
    use t3_client::view_logic::session::TimelineEntry;

    #[test]
    fn targets_preserve_client_identity_and_reject_non_user_or_unresolved_payloads() {
        let entry: TimelineEntry = serde_json::from_value(serde_json::json!({
                "id":"prompt", "kind":"message", "createdAt":"2026-11-13T12:00:00Z",
                "message":{"id":"prompt", "role":"user", "text":"Prompt", "runId":"run", "inputIntent":"turn_start",
                    "streaming":false, "createdAt":"2026-11-13T12:00:00Z", "updatedAt":"2026-11-13T12:00:00Z"}
            })).expect("canonical entry");
        let rows = derive_messages_timeline_rows(&MessagesTimelineRowsInput {
            timeline_entries: vec![Rc::new(entry)],
            supports_conversation_rollback: true,
            thread_start_checkpoint_available: true,
            available_revert_turn_counts: Some([0].into()),
            ..Default::default()
        });
        let row = rows
            .iter()
            .find(|row| RewindTarget::from_row(row).is_some())
            .expect("client target");
        assert_eq!(
            RewindTarget::from_row(row),
            Some(RewindTarget {
                message_id: MessageId("prompt".into()),
                turn_count: 0
            })
        );
        for unresolved in [false, true] {
            let mut invalid = row.as_ref().clone();
            let MessagesTimelineRowContent::Message { message, .. } = &mut invalid.content else {
                panic!("user row")
            };
            let message = Rc::make_mut(message);
            if unresolved {
                message.id = t3_contracts::orchestration_v2_id_table::IdOrIndex::Index(0);
            } else {
                message.role = "assistant".into();
            }
            assert_eq!(RewindTarget::from_row(&invalid), None);
        }
    }
}
