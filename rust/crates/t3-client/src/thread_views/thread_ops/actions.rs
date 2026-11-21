use super::{ThreadOperations, menu};
use crate::{
    ClientError,
    operations::{CommandOutcome, OperationQueue, StoreProgress},
    optimistic::{ProjectionKey, ThreadLifecycleAction, ThreadLifecycleIntent, ViewChange},
    platform::Platform,
    rpc::{RpcCallError, RpcEndpoint, RpcMethod, SessionEndpoint},
    types::{EnvironmentId, ThreadKey},
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::{
    orchestration_v2::*, rpc::WsOrchestrationV2DispatchCommandRpc, wire::WireDateTime,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Action {
    Rename(String),
    Archive,
    Unarchive,
    Delete,
    Lifecycle(ThreadLifecycleAction),
    MarkRead,
    MarkUnread,
    RegenerateTitle,
    CloseGuestShare,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Intent {
    key: ThreadKey,
    action: Action,
    now: WireDateTime,
    now_ms: u64,
}
struct Queue {
    queue: OperationQueue<Intent>,
    progress: RefCell<StoreProgress>,
}
#[derive(Default)]
pub struct ActionState {
    queues: RefCell<BTreeMap<EnvironmentId, Rc<Queue>>>,
    listeners: RefCell<Vec<futures::channel::mpsc::UnboundedSender<()>>>,
}
impl ActionState {
    pub(super) fn forget_environment(&self, environment: &EnvironmentId) {
        if let Some(queue) = self.queues.borrow_mut().remove(environment) {
            queue.queue.disconnect();
        }
    }
    pub(super) fn notify(&self) {
        self.listeners
            .borrow_mut()
            .retain(|s| s.unbounded_send(()).is_ok());
    }
    pub(super) fn changes(&self) -> LocalBoxStream<'static, ()> {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        self.listeners.borrow_mut().push(tx);
        rx.boxed_local()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThreadActionMenu {
    pub items: Vec<t3_contracts::ipc::ContextMenuItemSchema>,
    pub unavailable_reason: Option<String>,
}
#[derive(Default)]
pub struct BatchReport {
    pub completed: Vec<ThreadKey>,
    pub failed: Vec<(ThreadKey, ClientError)>,
}

#[derive(Clone, Copy)]
enum MenuSurface {
    Header,
    Sidebar,
    Batch,
}

pub type ConfirmationPort =
    Rc<dyn Fn(String, bool) -> LocalBoxFuture<'static, Result<bool, ClientError>>>;

impl Intent {
    fn apply(
        &self,
        snapshot: &mut OrchestrationV2ShellSnapshot,
        status: &crate::operations::CommandStatus,
    ) {
        if let Action::Lifecycle(action) = &self.action {
            ThreadLifecycleIntent {
                thread: self.key.thread.clone(),
                action: action.clone(),
                now: self.now.clone(),
                now_epoch_ms: self.now_ms,
            }
            .apply(snapshot, status);
            return;
        }
        if matches!(self.action, Action::Archive) {
            if let Some(index) = snapshot
                .threads
                .iter()
                .position(|t| t.id == self.key.thread)
            {
                let mut row = snapshot.threads.remove(index);
                row.archived_at = Some(self.now.clone());
                snapshot.archived_threads.push(row);
            }
            return;
        }
        if matches!(self.action, Action::Unarchive) {
            if let Some(index) = snapshot
                .archived_threads
                .iter()
                .position(|t| t.id == self.key.thread)
            {
                let mut row = snapshot.archived_threads.remove(index);
                row.archived_at = None;
                snapshot.threads.push(row);
            }
            return;
        }
        if matches!(self.action, Action::Delete) {
            snapshot.threads.retain(|t| t.id != self.key.thread);
            snapshot
                .archived_threads
                .retain(|t| t.id != self.key.thread);
            return;
        }
        if let Some(row) = snapshot
            .threads
            .iter_mut()
            .chain(snapshot.archived_threads.iter_mut())
            .find(|t| t.id == self.key.thread)
        {
            match &self.action {
                Action::Rename(title) => row.title = title.clone(),
                Action::MarkRead => row.last_visited_at = Some(Some(self.now.clone())),
                Action::MarkUnread => row.last_visited_at = Some(None),
                _ => {}
            }
        }
    }
}

// Preserve typed guard/unsupported outcomes while the durable queue records delivery status.
struct RecordedEndpoint {
    endpoint: SessionEndpoint,
    failure: Rc<RefCell<Option<ClientError>>>,
}
impl RpcEndpoint for RecordedEndpoint {
    fn destination(&self) -> Option<EnvironmentId> {
        self.endpoint.destination()
    }
    fn call<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>> {
        self.endpoint.call::<M>(input)
    }
    fn subscribe<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxStream<'static, Result<M::Output, ClientError>> {
        self.endpoint.subscribe::<M>(input)
    }
    fn command<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, RpcCallError>>
    where
        M::Output: 'static,
    {
        let request = self.endpoint.command::<M>(input);
        async move {
            let result = request.await;
            *self.failure.borrow_mut() = match &result {
                Err(RpcCallError::NotSent(ClientError::Unsupported(reason))) => {
                    Some(ClientError::Unsupported(reason))
                }
                Err(RpcCallError::NotSent(ClientError::PermissionDenied { method })) => {
                    Some(ClientError::PermissionDenied {
                        method: method.clone(),
                    })
                }
                Err(RpcCallError::Interrupted) => Some(ClientError::Cancelled),
                _ => None,
            };
            result
        }
        .boxed_local()
    }
}

impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    fn queue(&self, environment: &EnvironmentId) -> Rc<Queue> {
        let queue = self
            .state
            .actions
            .queues
            .borrow_mut()
            .entry(environment.clone())
            .or_insert_with(|| {
                Rc::new(Queue {
                    queue: OperationQueue::new(environment.clone()),
                    progress: RefCell::default(),
                })
            })
            .clone();
        queue
            .queue
            .observe_store(self.runtime.store(), &mut queue.progress.borrow_mut());
        queue
    }
    pub fn shell(&self, environment: &EnvironmentId) -> Option<Rc<OrchestrationV2ShellSnapshot>> {
        let base = self
            .runtime
            .store()
            .snapshot()
            .environments
            .get(environment)?
            .shell
            .snapshot
            .clone()?;
        Some(self.queue(environment).queue.view(
            &base,
            &ProjectionKey::shell(environment.clone()),
            |view, change, status| change.apply.apply(view, status),
        ))
    }
    pub fn thread_shell(&self, key: &ThreadKey) -> Option<Rc<OrchestrationV2ThreadShell>> {
        let shell = self.shell(&key.environment)?;
        shell
            .threads
            .iter()
            .chain(shell.archived_threads.iter())
            .find(|t| t.id == key.thread)
            .cloned()
            .map(Rc::new)
    }
    pub fn thread_action_menu(&self, key: &ThreadKey) -> ThreadActionMenu {
        self.action_menu(key, None, MenuSurface::Header)
    }

    pub fn sidebar_action_menu(
        &self,
        key: &ThreadKey,
        calendar: &crate::thread_views::shelves::SnoozeCalendar<'_>,
    ) -> ThreadActionMenu {
        self.action_menu(key, Some(calendar), MenuSurface::Sidebar)
    }

    fn action_menu(
        &self,
        key: &ThreadKey,
        calendar: Option<&crate::thread_views::shelves::SnoozeCalendar<'_>>,
        surface: MenuSurface,
    ) -> ThreadActionMenu {
        let Some(row) = self.thread_shell(key) else {
            return ThreadActionMenu {
                items: vec![],
                unavailable_reason: Some("This thread is no longer available.".into()),
            };
        };
        let snapshot = self.runtime.store().snapshot();
        let config = snapshot
            .environments
            .get(&key.environment)
            .and_then(|e| e.config.as_ref());
        let capabilities = config.map(|c| &c.environment.capabilities);
        let view = crate::services::models::present_thread_shell(
            key.environment.clone(),
            row.clone(),
            None,
        );
        let shelf =
            crate::view_logic::shell::sidebar::presentation::ThreadInput::from_environment_shell(
                &view,
                None,
                &crate::time::epoch_ms,
            )
            .shelf;
        let state = menu::MenuState {
            can_operate: self.can_operate(key),
            can_find: false,
            branch: row.branch.as_ref().map(|b| b.0.clone()),
            pinned: row.pinned_at.as_ref().is_some_and(Option::is_some),
            settled: row.settled_override == Some(AppThreadSettledOverride::Settled),
            auto_settle: row
                .auto_settle_disabled_at
                .as_ref()
                .is_none_or(Option::is_none),
            snoozed: !matches!(surface, MenuSurface::Batch)
                && crate::thread_views::shelves::effective_snoozed(
                    &shelf,
                    self.platform.now_epoch_ms() as i64,
                ),
            can_snooze: crate::thread_views::shelves::can_snooze(
                &shelf,
                self.platform.now_epoch_ms() as i64,
            ),
            running: !crate::services::models::thread_runtime_can_archive(view.runtime.as_ref()),
            settlement: capabilities.is_some_and(|c| c.thread_settlement == Some(true)),
            auto_settle_opt_out: capabilities
                .is_some_and(|c| c.thread_auto_settle_opt_out == Some(true)),
            snooze: capabilities.is_some_and(|c| c.thread_snooze == Some(true)),
            pinning: capabilities.is_some_and(|c| c.thread_pinning == Some(true)),
            title_regeneration: capabilities
                .is_some_and(|c| c.thread_title_regeneration == Some(true)),
            regenerating: row.title_regeneration.as_ref().is_some_and(Option::is_some),
            presets: calendar.map_or_else(Vec::new, |calendar| {
                self.resolve_snooze_presets(&key.environment, calendar)
            }),
            ..Default::default()
        };
        let mut items = menu::build(&state);
        for item in &mut items {
            if calendar.is_none()
                && item.id == "snooze"
                && let Some(children) = &mut item.children
            {
                for child in children {
                    child.disabled = Some(true);
                    child.detail = Some("This action is not available on this client yet.".into());
                }
            }
            if matches!(
                item.id.as_str(),
                "new-thread-on-branch" | "project-settings"
            ) {
                item.disabled = Some(true);
                item.detail = Some("This action is not available on this client yet.".into());
            }
            if row.archived_at.is_some() && item.id == "archive" {
                item.id = "unarchive".into();
                item.label = "Unarchive thread".into();
                item.disabled = Some(!state.can_operate);
            }
            if matches!(surface, MenuSurface::Header)
                && matches!(item.id.as_str(), "archive" | "unarchive" | "delete")
            {
                item.disabled = Some(true);
                item.detail = Some("Thread actions are not available on this client.".into());
            }
        }
        ThreadActionMenu {
            items,
            unavailable_reason: self
                .endpoint(&key.environment)
                .err()
                .map(|_| "This environment is not connected.".into()),
        }
    }
    pub fn sidebar_batch_menu(
        &self,
        keys: &[ThreadKey],
        calendar: &crate::thread_views::shelves::SnoozeCalendar<'_>,
    ) -> ThreadActionMenu {
        menu::build_batch(&self.sidebar_batch_menus(keys, Some(calendar)))
    }

    fn sidebar_batch_menus(
        &self,
        keys: &[ThreadKey],
        calendar: Option<&crate::thread_views::shelves::SnoozeCalendar<'_>>,
    ) -> Vec<(ThreadKey, ThreadActionMenu)> {
        keys.iter()
            .filter(|key| {
                self.thread_shell(key)
                    .is_some_and(|row| row.archived_at.is_none() && row.deleted_at.is_none())
            })
            .map(|key| {
                (
                    key.clone(),
                    self.action_menu(key, calendar, MenuSurface::Batch),
                )
            })
            .collect()
    }

    pub fn perform_sidebar_batch(
        &self,
        keys: Vec<ThreadKey>,
        id: String,
        until: Option<WireDateTime>,
    ) -> LocalBoxFuture<'static, Result<BatchReport, ClientError>> {
        use ThreadLifecycleAction::*;
        let service = self.clone();
        async move {
            let action = match id.as_str() {
                "unpin" => Action::Lifecycle(Unpin),
                "settle" => Action::Lifecycle(Settle),
                "snooze" => Action::Lifecycle(Snooze {
                    until: until.ok_or_else(|| Self::refused("Choose a snooze time."))?,
                }),
                "regenerate-title" => Action::RegenerateTitle,
                "mark-unread" => Action::MarkUnread,
                "delete" => Action::Delete,
                _ => return Err(ClientError::Unsupported("this batch action")),
            };
            let targets = menu::batch_targets(&service.sidebar_batch_menus(&keys, None), &id)?;
            let settings = service.runtime.composer_preferences().snapshot();
            let count = targets.len();
            let confirmation = match id.as_str() {
                "delete" if settings.confirm_thread_delete => Some((
                    format!("Delete {count} {}?\nThis permanently clears conversation history for these threads.", if count == 1 { "thread" } else { "threads" }),
                    true,
                )),
                "unpin" if settings.confirm_thread_unpin => Some((
                    format!("Unpin {count} {}?\nThis will move the threads out of your pinned section.", if count == 1 { "thread" } else { "threads" }),
                    false,
                )),
                _ => None,
            };
            if let Some((message, destructive)) = confirmation {
                let confirm = service.state.confirmation.borrow().clone()
                    .ok_or(ClientError::Unsupported("thread action confirmation"))?;
                if !confirm(message, destructive).await? {
                    return Err(ClientError::Cancelled);
                }
            }
            let targets = menu::batch_targets(&service.sidebar_batch_menus(&targets, None), &id)?;
            let mut report = BatchReport::default();
            for key in targets {
                match menu::batch_targets(
                    &service.sidebar_batch_menus(std::slice::from_ref(&key), None),
                    &id,
                ) {
                    Ok(keys) if keys.is_empty() => continue,
                    Err(error) => {
                        report.failed.push((key, error));
                        continue;
                    }
                    Ok(_) => {}
                }
                match service.dispatch_action(key.clone(), action.clone(), false, true).await {
                    Ok(()) => report.completed.push(key),
                    Err(error) => report.failed.push((key, error)),
                }
            }
            Ok(report)
        }.boxed_local()
    }

    fn command(
        &self,
        key: &ThreadKey,
        action: &Action,
    ) -> Result<OrchestrationV2Command, ClientError> {
        use ThreadLifecycleAction::*;
        let (kind, fields) = match action {
            Action::Rename(title) => ("thread.metadata.update", json!({"title":title})),
            Action::RegenerateTitle => ("thread.metadata.update", json!({"regenerateTitle":true})),
            Action::CloseGuestShare => ("thread.guest-share.close", json!({})),
            Action::Archive => ("thread.archive", json!({})),
            Action::Unarchive => ("thread.unarchive", json!({})),
            Action::Delete => ("thread.delete", json!({})),
            Action::MarkRead => (
                "thread.visit",
                json!({"visitedAt":crate::time::iso_from_epoch_ms(self.platform.now_epoch_ms())}),
            ),
            Action::MarkUnread => ("thread.mark-unread", json!({})),
            Action::Lifecycle(action) => match action {
                Settle => ("thread.settle", json!({})),
                Unsettle => ("thread.unsettle", json!({"reason":"user"})),
                Snooze { until } => ("thread.snooze", json!({"snoozedUntil":until})),
                Unsnooze => ("thread.unsnooze", json!({"reason":"user"})),
                SetAutoSettle { enabled } => ("thread.auto-settle.set", json!({"enabled":enabled})),
                Pin { order_key } => (
                    "thread.pin",
                    order_key
                        .as_ref()
                        .map_or(json!({}), |k| json!({"orderKey":k})),
                ),
                Unpin => ("thread.unpin", json!({})),
                ReorderPin { order_key } => ("thread.pin.reorder", json!({"orderKey":order_key})),
                ReorderActive { order_key } => {
                    ("thread.active.reorder", json!({"orderKey":order_key}))
                }
                SetModel { selection } => (
                    "thread.model-selection.set",
                    json!({"modelSelection":selection}),
                ),
            },
        };
        let mut payload = json!({"type":kind,"commandId":crate::operations::allocate_command_id(&self.platform,None)?,"threadId":key.thread});
        if let Some(fields) = fields.as_object() {
            for (key, value) in fields {
                payload[key] = value.clone();
            }
        }
        serde_json::from_value(payload).map_err(Into::into)
    }
    pub fn perform(
        &self,
        key: ThreadKey,
        action: Action,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform_impl(key, action, true)
    }
    pub fn bind_confirmation(&self, port: ConfirmationPort) {
        *self.state.confirmation.borrow_mut() = Some(port);
    }
    pub(super) fn perform_impl(
        &self,
        key: ThreadKey,
        action: Action,
        notices: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.dispatch_action(key, action, notices, notices)
    }

    fn dispatch_action(
        &self,
        key: ThreadKey,
        mut action: Action,
        confirm_action: bool,
        notices: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        async move {
            let row = service.thread_shell(&key).ok_or_else(|| Self::refused("This thread is no longer available."))?;
            if service.unsupported(&key.environment,"orchestration.dispatchCommand") { return Err(ClientError::Unsupported("orchestration.dispatchCommand")); }
            if !matches!(action,Action::MarkRead|Action::MarkUnread) && !service.can_operate(&key) {return Err(ClientError::PermissionDenied{method:"orchestration.dispatchCommand".into()});}
            if confirm_action {
                let settings = service.runtime.composer_preferences().snapshot();
                let request = match &action {
                    Action::Delete if settings.confirm_thread_delete => Some((format!("Delete thread \"{}\"?\nThis permanently clears conversation history for this thread.",row.title),true)),
                    Action::Archive if settings.confirm_thread_archive => Some((format!("Archive thread \"{}\"?",row.title),false)),
                    Action::Lifecycle(ThreadLifecycleAction::Unpin) if settings.confirm_thread_unpin => Some((format!("Unpin thread \"{}\"?\nThis will move the thread out of your pinned section.",row.title),false)),
                    _ => None,
                };
                if let Some((message,destructive))=request {
                    let confirm=service.state.confirmation.borrow().clone().ok_or(ClientError::Unsupported("thread action confirmation"))?;
                    if !confirm(message,destructive).await? {return Err(ClientError::Cancelled);}
                }
            }
            if let Action::Lifecycle(action) = &action {
                let snapshot = service.runtime.store().snapshot();
                let caps = snapshot.environments.get(&key.environment).and_then(|e| e.config.as_ref()).map(|c| &c.environment.capabilities);
                let supported = caps.is_some_and(|c| match action {
                    ThreadLifecycleAction::Settle | ThreadLifecycleAction::Unsettle => c.thread_settlement == Some(true),
                    ThreadLifecycleAction::Snooze { .. } | ThreadLifecycleAction::Unsnooze => c.thread_snooze == Some(true),
                    ThreadLifecycleAction::Pin { .. } | ThreadLifecycleAction::Unpin => c.thread_pinning == Some(true),
                    ThreadLifecycleAction::SetAutoSettle { .. } => c.thread_auto_settle_opt_out == Some(true),
                    ThreadLifecycleAction::ReorderPin { .. } => c.thread_pin_reorder == Some(true),
                    ThreadLifecycleAction::ReorderActive { .. } => c.thread_active_reorder == Some(true),
                    ThreadLifecycleAction::SetModel { .. } => true,
                });
                if !supported { return Err(ClientError::Unsupported("this thread action")); }
                if let ThreadLifecycleAction::Snooze { until } = action
                    && !crate::time::epoch_ms(&until.0).is_some_and(|wake| wake > service.platform.now_epoch_ms() as i64) {
                    return Err(Self::refused("That snooze time has passed. Choose another time."));
                }
            }
            if let Action::Rename(title) = &mut action {
                *title = t3_contracts::wire::trim_js(title).into();
                if title.is_empty() { return Err(Self::refused("Thread title cannot be empty")); }
                if title == &row.title { return Ok(()); }
            }
            if let Action::Lifecycle(ThreadLifecycleAction::Pin {order_key}) = &mut action
                && order_key.is_none() {
                let first = service.runtime.environments().iter().filter(|e|e.state.desired).filter_map(|e|service.shell(&e.id)).flat_map(|s|s.threads.clone()).filter(|t|t.pinned_at.as_ref().is_some_and(Option::is_some)).filter_map(|t|t.pin_order_key.flatten().map(|key|key.0)).min();
                *order_key=crate::thread_views::threads::sort::pin_order_key_between(None,first.as_deref()).map(t3_contracts::base_schemas::TrimmedNonEmptyString);
            }
            let menu = service.thread_action_menu(&key);
            if matches!(action,Action::Archive) && !crate::services::models::thread_runtime_can_archive(crate::services::models::present_thread_shell(key.environment.clone(),row.clone(),None).runtime.as_ref()) {
                return Err(Self::refused("Interrupt the current turn before changing this thread."));
            }
            let id = match &action { Action::Lifecycle(ThreadLifecycleAction::Snooze { .. }) => Some("snooze"), _ => None };
            if id.is_some_and(|id| menu.items.iter().any(|i| i.id == id && i.disabled == Some(true))) { return Err(Self::refused("Interrupt the current turn before changing this thread.")); }
            let command = service.command(&key, &action)?;
            let endpoint = service.endpoint(&key.environment)?;
            if !endpoint.available::<WsOrchestrationV2DispatchCommandRpc>(&command) { return Err(ClientError::PermissionDenied { method: "orchestration.dispatchCommand".into() }); }
            let generation = service.runtime.store().snapshot().environments.get(&key.environment).map(|e| e.shell.generation).ok_or(ClientError::Disconnected)?;
            let undo = service.begin_thread_undo(&key,&action,&row,notices);
            let intent = Intent { key: key.clone(), action:action.clone(), now: WireDateTime(crate::time::iso_from_epoch_ms(service.platform.now_epoch_ms())), now_ms: service.platform.now_epoch_ms() };
            let queue = service.queue(&key.environment);
            let id = queue.queue.enqueue::<WsOrchestrationV2DispatchCommandRpc>(&service.platform, &command, ProjectionKey::shell(key.environment.clone()), generation,
                ViewChange { apply: intent.clone(), revert: intent }).await.map_err(operation_error)?;
            service.state.actions.notify();
            let failure = Rc::new(RefCell::new(None));
            let endpoint = RecordedEndpoint { endpoint, failure: failure.clone() };
            let session=service.session_generation(&key.environment);
            let result = queue.queue.dispatch(&service.platform, &endpoint, &id).await;
            service.state.actions.notify();
            if let Some(error) = failure.borrow_mut().take() { service.remember_unsupported(&key.environment,"orchestration.dispatchCommand",session,&error); return Err(error); }
            let report = result.map_err(operation_error)?;
            if let Some(error) = report.persistence_error { return Err(error); }
            match report.outcome {
                CommandOutcome::Accepted { .. } => { service.commit_thread_undo(undo); Ok(()) },
                CommandOutcome::Rejected { reason } => Err(ClientError::Rejected {message:reason,detail:None}),
                CommandOutcome::NotSent | CommandOutcome::UnknownAfterDisconnect => Err(ClientError::Disconnected),
            }
        }.boxed_local()
    }
    pub fn rename_thread(
        &self,
        key: ThreadKey,
        title: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform(key, Action::Rename(title))
    }
    pub fn archive_thread(
        &self,
        key: ThreadKey,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform(key, Action::Archive)
    }
    pub fn unarchive_thread(
        &self,
        key: ThreadKey,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform(key, Action::Unarchive)
    }
    pub fn delete_thread(
        &self,
        key: ThreadKey,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform(key, Action::Delete)
    }
    pub fn settle_thread(
        &self,
        key: ThreadKey,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform(key, Action::Lifecycle(ThreadLifecycleAction::Settle))
    }
    pub fn unsettle_thread(
        &self,
        key: ThreadKey,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform(key, Action::Lifecycle(ThreadLifecycleAction::Unsettle))
    }
    pub fn snooze_thread(
        &self,
        key: ThreadKey,
        until: WireDateTime,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform(
            key,
            Action::Lifecycle(ThreadLifecycleAction::Snooze { until }),
        )
    }
    pub fn unsnooze_thread(
        &self,
        key: ThreadKey,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform(key, Action::Lifecycle(ThreadLifecycleAction::Unsnooze))
    }
    pub fn set_thread_pinned(
        &self,
        key: ThreadKey,
        pinned: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform(
            key,
            Action::Lifecycle(if pinned {
                ThreadLifecycleAction::Pin { order_key: None }
            } else {
                ThreadLifecycleAction::Unpin
            }),
        )
    }
    pub fn mark_read(&self, key: ThreadKey) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform(key, Action::MarkRead)
    }
    pub fn mark_unread(&self, key: ThreadKey) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform(key, Action::MarkUnread)
    }
}
fn operation_error(error: crate::operations::OperationError) -> ClientError {
    match error {
        crate::operations::OperationError::Client(error) => error,
        error => ClientError::InvalidPayload(error.to_string()),
    }
}
