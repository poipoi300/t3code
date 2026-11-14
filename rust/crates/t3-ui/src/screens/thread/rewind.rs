//! Port of apps/web/src/components/chat/useTimelineRewind.ts useTimelineRewind.

use std::rc::Rc;

use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use t3_client::thread_views::messages_timeline::{MessagesTimelineRow, MessagesTimelineRowContent};
use t3_client::types::ThreadKey;
use t3_contracts::base_schemas::{MessageId, RunId};

use super::rows::user::UserRowContext;
use crate::i18n::{Text, t};
use crate::state::nav::{Navigator, Route, navigator};
use crate::ui::confirm::{
    ConfirmDialogInitialFocus, ConfirmDialogOptions, ConfirmDialogVariant, request_confirm_dialog,
};

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewindState {
    pub reverting: bool,
    pub can_fork: bool,
}

/// Composed operation requested in briefs/client-seam-requests.md, TL-REWIND.
/// Implementations restore draft/attachments only after persisted rollback and recheck all guards.
pub trait TimelineRewindClient {
    fn state(&self, key: &ThreadKey) -> RewindState;
    fn availability(&self, key: &ThreadKey, target: &RewindTarget) -> RewindAvailability;
    fn changes(&self, key: &ThreadKey) -> LocalBoxStream<'static, ()>;
    fn rewind(
        &self,
        key: ThreadKey,
        target: RewindTarget,
        restore_files: bool,
    ) -> LocalBoxFuture<'static, Result<String, RewindError>>;
    fn fork(
        &self,
        key: ThreadKey,
        run_id: RunId,
    ) -> LocalBoxFuture<'static, Result<ThreadKey, RewindError>>;
}

/// The runtime integrator provides the composed service once, never a raw rollback RPC.
#[derive(Clone)]
pub struct RewindService(pub Rc<dyn TimelineRewindClient>);

#[derive(Clone, Copy)]
pub struct TimelineRewind {
    key: Stored<ThreadKey>,
    client: Stored<Option<Rc<dyn TimelineRewindClient>>>,
    owner: Owner,
    nav: Navigator,
    state: Signal<RewindState>,
    pending: Signal<bool>,
    pub reverting: Memo<bool>,
    pub error: Signal<Option<RewindError>>,
    pub restored_prompt: Signal<Option<String>>,
}

pub fn use_timeline_rewind(key: ThreadKey) -> TimelineRewind {
    TimelineRewind::with_client(key, use_context::<RewindService>().map(|service| service.0))
}

impl TimelineRewind {
    /// Construct above the virtual list so recycling does not cancel a confirmed operation.
    pub fn with_client(key: ThreadKey, client: Option<Rc<dyn TimelineRewindClient>>) -> Self {
        let state = signal(
            client
                .as_ref()
                .map_or_else(RewindState::default, |client| client.state(&key)),
        );
        if let Some(client) = &client {
            let mut changes = client.changes(&key);
            let client = client.clone();
            let key = key.clone();
            spawn(async move {
                while changes.next().await.is_some() {
                    state.set(client.state(&key));
                }
            });
        }
        let pending = signal(false);
        Self {
            key: stored(key),
            client: stored(client),
            owner: Owner::current(),
            nav: navigator(),
            state,
            pending,
            reverting: computed(move || pending.get() || state.get().reverting),
            error: signal(None),
            restored_prompt: signal(None),
        }
    }

    pub fn bind_user_row(self, context: &mut UserRowContext) {
        context.reverting = self.reverting;
        context.revert = self.client.with(|client| {
            client.as_ref().map(|_| {
                Rc::new(
                    move |row: &t3_client::thread_views::user_message::UserRow, count: usize| {
                        self.request_revert(
                            RewindTarget {
                                message_id: match &row.message.id {
                                    t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id) => {
                                        id.clone()
                                    }
                                    t3_contracts::orchestration_v2_id_table::IdOrIndex::Index(
                                        _,
                                    ) => return,
                                },
                                turn_count: count as u64,
                            },
                            false,
                        );
                    },
                ) as super::rows::user::RevertAction
            })
        });
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

    pub fn request_revert(self, target: RewindTarget, restore_files: bool) {
        if self.reverting.get_untracked() {
            return;
        }
        let availability = self.availability(&target);
        if !availability.can_rewind || (restore_files && !availability.can_restore_files) {
            self.error.set(availability.reason);
            return;
        }
        let Some(client) = self.client.get() else {
            return;
        };
        // UI-CONFIRM currently exposes only Confirm/Cancel; the source's three-choice seam is requested.
        let Some(answer) = request_confirm_dialog(
            format!(
                "{}\n{}",
                t("Edit from here?"),
                t(
                    "Rewind chat to before this message. Your prompt and attachments return to the composer. “Revert files too” also undoes your own edits."
                )
            ),
            Some(ConfirmDialogOptions {
                variant: ConfirmDialogVariant::Destructive,
                initial_focus: Some(ConfirmDialogInitialFocus::Cancel),
            }),
        ) else {
            return;
        };
        self.pending.set(true);
        self.owner.run(|| {
            spawn(async move {
                let _pending = Pending(self.pending);
                if !answer.await {
                    return;
                }
                let key = self.key.get();
                let availability = client.availability(&key, &target);
                if !availability.can_rewind || (restore_files && !availability.can_restore_files) {
                    self.error.set(availability.reason);
                    return;
                }
                self.error.set(None);
                self.restored_prompt.set(None);
                match client.rewind(key.clone(), target, restore_files).await {
                    Ok(prompt) if self.nav.top().thread() == Some(&key) => {
                        self.restored_prompt.set(Some(prompt))
                    }
                    Ok(_) => {}
                    Err(error) => self.error.set(Some(error)),
                }
            });
        });
    }

    pub fn on_fork_from_run(self, run_id: RunId) {
        if self.reverting.get_untracked() || !self.state.get_untracked().can_fork {
            return;
        }
        let Some(client) = self.client.get() else {
            return;
        };
        let key = self.key.get();
        self.pending.set(true);
        self.owner.run(|| {
            spawn(async move {
                let _pending = Pending(self.pending);
                self.error.set(None);
                match client.fork(key, run_id).await {
                    Ok(target) => self.nav.open(Route::Thread(target)),
                    Err(error) => self.error.set(Some(error)),
                }
            });
        });
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
    fn only_the_clients_rewindable_user_rows_produce_a_target() {
        for (role, supported, expected) in [
            ("user", true, true),
            ("user", false, false),
            ("assistant", true, false),
        ] {
            let entry: TimelineEntry = serde_json::from_value(serde_json::json!({
                "id":"prompt", "kind":"message", "createdAt":"2026-11-13T12:00:00Z",
                "message":{"id":"prompt", "role":role, "text":"Prompt", "runId":"run", "inputIntent":"turn_start",
                    "streaming":false, "createdAt":"2026-11-13T12:00:00Z", "updatedAt":"2026-11-13T12:00:00Z"}
            })).expect("canonical entry");
            let rows = derive_messages_timeline_rows(&MessagesTimelineRowsInput {
                timeline_entries: vec![Rc::new(entry)],
                supports_conversation_rollback: supported,
                thread_start_checkpoint_available: true,
                available_revert_turn_counts: Some([0].into()),
                ..Default::default()
            });
            let targets: Vec<_> = rows
                .iter()
                .filter_map(|row| RewindTarget::from_row(row))
                .collect();
            assert_eq!(
                targets,
                if expected {
                    vec![RewindTarget {
                        message_id: MessageId("prompt".into()),
                        turn_count: 0,
                    }]
                } else {
                    vec![]
                }
            );
        }
    }
}
