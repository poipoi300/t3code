use super::{
    ThreadOperations,
    actions::{Action, ThreadActionMenu},
};
use crate::{
    ClientError,
    platform::{MimeData, Platform},
    types::ThreadKey,
    workspace::ProjectKey,
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub struct HeaderState {
    pub can_operate: bool,
    pub menu: ThreadActionMenu,
    pub is_scratch: Option<bool>,
    pub guest_share: Option<t3_contracts::guest_share::ThreadGuestShare>,
}
pub fn action_failure_title(action: &str) -> &'static str {
    match action {
        "rename" => "Failed to rename thread",
        "pin" => "Failed to pin thread",
        "unpin" => "Failed to unpin thread",
        "settle" => "Failed to settle thread",
        "unsettle" => "Failed to un-settle thread",
        "archive" => "Failed to archive thread",
        "unarchive" => "Failed to unarchive thread",
        "delete" => "Failed to delete thread",
        "regenerate-title" => "Failed to regenerate title",
        "close-guest-share" => "Failed to stop guest share",
        "mark-unread" => "Failed to mark thread unread",
        "unsnooze" => "Failed to wake thread",
        "auto-settle:enabled" | "auto-settle:disabled" => "Failed to update auto-settle",
        "copy-path" => "Failed to copy path",
        "copy-branch" => "Failed to copy branch",
        "copy-thread-id" => "Failed to copy thread ID",
        "new-thread-in-project" => "Could not create thread",
        _ => "Thread action failed",
    }
}
impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub fn project_new_thread_reason(
        &self,
        project: &ProjectKey,
        origin: &ThreadKey,
    ) -> Option<String> {
        if self.project(project).is_none() {
            return Some("The project is no longer available.".into());
        }
        let drafts = self.runtime.composer_drafts();
        if !drafts.view_key(&drafts.thread_draft_key(origin)).writable {
            return Some("The composer draft is not writable.".into());
        }
        self.runtime
            .store()
            .snapshot()
            .environments
            .get(&project.environment)
            .and_then(|e| e.config.as_ref())
            .is_none()
            .then(|| "This connection cannot change threads.".into())
    }
    pub fn watch_project_new_thread(
        &self,
        project: ProjectKey,
        origin: ThreadKey,
    ) -> LocalBoxStream<'static, Option<String>> {
        let changes = self.runtime.new_threads().changes();
        let service = self.clone();
        changes
            .map(move |_| service.project_new_thread_reason(&project, &origin))
            .boxed_local()
    }
    pub fn new_thread_in_project(
        &self,
        project: ProjectKey,
        origin: ThreadKey,
    ) -> LocalBoxFuture<'static, Result<crate::navigation::new_thread::Destination, ClientError>>
    {
        let service = self.clone();
        async move {
            if let Some(reason) = service.project_new_thread_reason(&project, &origin) {
                return Err(Self::refused(&reason));
            }
            let source = service.runtime.composer_drafts().thread_draft_key(&origin);
            let id = service
                .runtime
                .new_threads()
                .open_project(project, Some(&source), None)
                .await?;
            Ok(crate::navigation::new_thread::Destination::Draft(id))
        }
        .boxed_local()
    }
    pub fn shell_views(&self) -> Rc<crate::thread_views::threads::shell::ThreadShellViews> {
        use crate::thread_views::threads::shell::{EnvironmentShells, ThreadShellViews};
        let sources: Vec<_> = self
            .runtime
            .environments()
            .iter()
            .filter_map(|e| {
                Some((
                    e.id.clone(),
                    e.state.desired,
                    self.runtime.store().shared_shell(&e.id)?.threads.clone(),
                ))
            })
            .collect();
        let inputs: Vec<_> = sources
            .iter()
            .map(|(environment, enabled, threads)| EnvironmentShells {
                environment,
                enabled: *enabled,
                threads,
            })
            .collect();
        let prior = self.state.shells.borrow().clone();
        let mut next = ThreadShellViews {
            ordered: prior.ordered.clone(),
            rows: prior.rows.clone(),
            quiet_rows: prior.quiet_rows.clone(),
            pending_child_requests: prior.pending_child_requests.clone(),
            delegated_under_parent: prior.delegated_under_parent.clone(),
            hidden: prior.hidden.clone(),
        };
        next.update(&inputs, prior.hidden.clone());
        if next.ordered == prior.ordered
            && next.rows == prior.rows
            && next.pending_child_requests == prior.pending_child_requests
            && next.delegated_under_parent == prior.delegated_under_parent
        {
            return prior;
        }
        let next = Rc::new(next);
        *self.state.shells.borrow_mut() = next.clone();
        next
    }
    pub fn header_state(&self, key: &ThreadKey) -> HeaderState {
        let row = self.thread_shell(key);
        let is_scratch = row
            .and_then(|row| {
                self.project(&ProjectKey {
                    environment: key.environment.clone(),
                    project: row.project_id.clone(),
                })
            })
            .map(|p| p.is_scratch);
        HeaderState {
            can_operate: self.can_operate(key),
            menu: self.thread_action_menu(key),
            is_scratch,
            guest_share: self
                .runtime
                .store()
                .shared_thread(key)
                .and_then(|p| p.thread.guest_share.clone().flatten()),
        }
    }
    pub fn watch_header(&self, key: ThreadKey) -> LocalBoxStream<'static, Rc<HeaderState>> {
        let changes = self.changes(&key);
        let service = self.clone();
        changes
            .map(move |_| Rc::new(service.header_state(&key)))
            .boxed_local()
    }
    pub fn perform_action(
        &self,
        key: ThreadKey,
        id: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        use crate::optimistic::ThreadLifecycleAction::*;
        if matches!(id.as_str(), "archive" | "unarchive" | "delete") {
            return futures::future::ready(Err(ClientError::Unsupported(
                "thread removal navigation and cleanup",
            )))
            .boxed_local();
        }
        let action = match id.as_str() {
            "pin" => Some(Action::Lifecycle(Pin { order_key: None })),
            "unpin" => Some(Action::Lifecycle(Unpin)),
            "settle" => Some(Action::Lifecycle(Settle)),
            "unsettle" => Some(Action::Lifecycle(Unsettle)),
            "unsnooze" => Some(Action::Lifecycle(Unsnooze)),
            "auto-settle:enabled" => Some(Action::Lifecycle(SetAutoSettle { enabled: true })),
            "auto-settle:disabled" => Some(Action::Lifecycle(SetAutoSettle { enabled: false })),
            "regenerate-title" => Some(Action::RegenerateTitle),
            "mark-unread" => Some(Action::MarkUnread),
            "archive" => Some(Action::Archive),
            "unarchive" => Some(Action::Unarchive),
            "delete" => Some(Action::Delete),
            _ => None,
        };
        if let Some(action) = action {
            return self.perform(key, action);
        }
        let service = self.clone();
        async move {
            let row = service.thread_shell(&key).ok_or_else(|| Self::refused("This thread is no longer available."))?;
            let text = match id.as_str() {
                "copy-thread-id" => key.thread.0,
                "copy-thread" => crate::view_logic::session::markdown_link_presentation::format_hosted_thread_link(&key,&row.title,"https://app.t3.codes").ok_or_else(||Self::refused("Unable to copy thread link."))?,
                "copy-branch" => row.branch.as_ref().map(|b| b.0.clone()).ok_or_else(|| Self::refused("Branch is unavailable"))?,
                "copy-path" => row.worktree_path.as_ref().map(|p| p.0.clone()).or_else(|| service.project(&ProjectKey { environment: key.environment, project: row.project_id.clone() }).map(|p| p.project.workspace_root.0.clone())).ok_or_else(|| Self::refused("Workspace path is unavailable"))?,
                _ => return Err(ClientError::Unsupported("this thread menu action")),
            };
            service.platform.clipboard_write(vec![MimeData { mime: "text/plain".into(), bytes: text.into_bytes() }]).await?; Ok(())
        }.boxed_local()
    }
    pub fn close_guest_share(
        &self,
        key: ThreadKey,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform(key, Action::CloseGuestShare)
    }
    pub fn to_server_clock(
        &self,
        environment: &crate::types::EnvironmentId,
        client_ms: f64,
    ) -> Option<f64> {
        self.endpoint(environment)
            .ok()
            .map(|e| e.to_server_clock(client_ms))
    }
}
