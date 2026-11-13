use super::{
    super::vcs::CheckoutKey,
    source_control::{RefsInvalidationGuard, SourceControl, check_destination},
};
use crate::{
    ClientError,
    platform::Platform,
    rpc::RpcEndpoint,
    services::resources::{Resource, ResourcePolicy, ResourceSubscription},
    types::Generation,
};
use futures::{StreamExt, stream::LocalBoxStream};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};
use t3_contracts::{git::*, rpc::WsGitRunStackedActionRpc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsActionOperation {
    RefreshStatus,
    RunChangeRequest,
    Pull,
    SwitchRef,
    CreateRef,
    CreateWorktree,
    Init,
    PublishRepository,
    PreparePullRequestThread,
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VcsActionState {
    pub is_running: bool,
    pub operation: Option<VcsActionOperation>,
    pub action_id: Option<String>,
    pub action: Option<GitStackedAction>,
    pub current_label: Option<String>,
    pub current_phase_label: Option<String>,
    pub hook_name: Option<String>,
    pub last_output_line: Option<String>,
    pub phase_started_at_ms: Option<u64>,
    pub hook_started_at_ms: Option<u64>,
    pub error: Option<String>,
}
pub fn begin_vcs_action_state(
    operation: VcsActionOperation,
    label: &str,
    action_id: String,
    now: u64,
) -> VcsActionState {
    VcsActionState {
        is_running: true,
        operation: Some(operation),
        action_id: Some(action_id),
        current_label: Some(label.into()),
        current_phase_label: Some(label.into()),
        phase_started_at_ms: Some(now),
        ..VcsActionState::default()
    }
}
#[derive(Debug, thiserror::Error)]
#[error("Invalid source control action target key ({key_length} characters).")]
pub struct VcsActionTargetKeyParseError {
    pub key_length: usize,
    #[source]
    pub cause: serde_json::Error,
}
pub fn parse_vcs_action_target_key(key: &str) -> Result<CheckoutKey, VcsActionTargetKeyParseError> {
    let parse = || {
        let (environment, cwd): (crate::types::EnvironmentId, String) = serde_json::from_str(key)?;
        Ok(CheckoutKey { environment, cwd })
    };
    parse().map_err(|cause| VcsActionTargetKeyParseError {
        key_length: key.encode_utf16().count(),
        cause,
    })
}
pub fn create_vcs_action_transport_id(
    target: &CheckoutKey,
    action_id: &str,
) -> Result<String, ClientError> {
    let key = serde_json::to_string(&(&target.environment, &target.cwd))?;
    Ok(format!("{}:{key}{action_id}", key.encode_utf16().count()))
}
macro_rules! event_fields {
    ($event:expr, $field:ident) => {
        match $event {
            GitActionProgressEvent::Started(e) => &e.$field,
            GitActionProgressEvent::PhaseStarted(e) => &e.$field,
            GitActionProgressEvent::HookStarted(e) => &e.$field,
            GitActionProgressEvent::HookOutput(e) => &e.$field,
            GitActionProgressEvent::HookFinished(e) => &e.$field,
            GitActionProgressEvent::Finished(e) => &e.$field,
            GitActionProgressEvent::Failed(e) => &e.$field,
        }
    };
}
pub fn normalize_vcs_action_progress_event(
    target: &CheckoutKey,
    transport_id: &str,
    action_id: &str,
    mut event: GitActionProgressEvent,
) -> Option<GitActionProgressEvent> {
    if event_fields!(&event, action_id).0 != transport_id
        || event_fields!(&event, cwd).0 != target.cwd
    {
        return None;
    }
    macro_rules! normalize {
        ($e:ident) => {
            $e.action_id.0 = action_id.into()
        };
    }
    match &mut event {
        GitActionProgressEvent::Started(e) => normalize!(e),
        GitActionProgressEvent::PhaseStarted(e) => normalize!(e),
        GitActionProgressEvent::HookStarted(e) => normalize!(e),
        GitActionProgressEvent::HookOutput(e) => normalize!(e),
        GitActionProgressEvent::HookFinished(e) => normalize!(e),
        GitActionProgressEvent::Finished(e) => normalize!(e),
        GitActionProgressEvent::Failed(e) => normalize!(e),
    }
    Some(event)
}
pub fn apply_vcs_action_progress_event(
    current: &VcsActionState,
    event: &GitActionProgressEvent,
    now: u64,
) -> VcsActionState {
    if current.action_id.as_deref() != Some(&event_fields!(event, action_id).0) {
        return current.clone();
    }
    let mut next = current.clone();
    next.is_running = true;
    next.action = Some(event_fields!(event, action).clone());
    next.operation = Some(VcsActionOperation::RunChangeRequest);
    next.error = None;
    match event {
        GitActionProgressEvent::Started(_) | GitActionProgressEvent::PhaseStarted(_) => {
            next.phase_started_at_ms = Some(now);
            next.hook_started_at_ms = None;
            next.hook_name = None;
            next.last_output_line = None;
            if let GitActionProgressEvent::PhaseStarted(event) = event {
                next.current_label = Some(event.label.0.clone());
                next.current_phase_label = next.current_label.clone();
            }
        }
        GitActionProgressEvent::HookStarted(event) => {
            next.current_label = Some(format!("Running {}...", event.hook_name.0));
            next.hook_name = Some(event.hook_name.0.clone());
            next.hook_started_at_ms = Some(now);
            next.last_output_line = None;
        }
        GitActionProgressEvent::HookOutput(event) => {
            next.last_output_line = Some(event.text.0.clone())
        }
        GitActionProgressEvent::HookFinished(_) => {
            next.current_label = next.current_phase_label.clone();
            next.hook_name = None;
            next.hook_started_at_ms = None;
            next.last_output_line = None;
        }
        GitActionProgressEvent::Finished(_) => (),
        GitActionProgressEvent::Failed(event) => next.error = Some(event.message.0.clone()),
    }
    next
}
#[derive(Debug, thiserror::Error)]
pub enum VcsActionError {
    #[error("Source control operation is unavailable.")]
    Unavailable,
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("Source control action '{action_name}' failed during {phase_name}.")]
    RemoteFailure {
        action_id: String,
        transport_action_id: String,
        action: GitStackedAction,
        target: Box<CheckoutKey>,
        phase: Option<GitActionProgressPhase>,
        remote_message_length: usize,
        action_name: &'static str,
        phase_name: &'static str,
    },
    #[error("Source control action '{action_name}' ended without a terminal result.")]
    MissingTerminalEvent {
        action_id: String,
        transport_action_id: String,
        action: GitStackedAction,
        target: Box<CheckoutKey>,
        action_name: &'static str,
    },
}
fn action_name(action: &GitStackedAction) -> &'static str {
    match action {
        GitStackedAction::Commit => "commit",
        GitStackedAction::Push => "push",
        GitStackedAction::CreatePr => "create_pr",
        GitStackedAction::CommitPush => "commit_push",
        GitStackedAction::CommitPushPr => "commit_push_pr",
    }
}
pub async fn consume_vcs_action_progress(
    mut stream: LocalBoxStream<'static, Result<GitActionProgressEvent, ClientError>>,
    target: &CheckoutKey,
    transport_id: &str,
    action_id: &str,
    action: &GitStackedAction,
    mut on_progress: impl FnMut(&GitActionProgressEvent),
) -> Result<GitRunStackedActionResult, VcsActionError> {
    let mut terminal = None;
    while let Some(event) = stream.next().await {
        if let Some(event) =
            normalize_vcs_action_progress_event(target, transport_id, action_id, event?)
        {
            on_progress(&event);
            if matches!(
                event,
                GitActionProgressEvent::Finished(_) | GitActionProgressEvent::Failed(_)
            ) {
                terminal = Some(event);
            }
        }
    }
    match terminal {
        Some(GitActionProgressEvent::Finished(event)) => Ok(event.result),
        Some(GitActionProgressEvent::Failed(event)) => {
            let phase_name = match event.phase {
                None => "execution",
                Some(GitActionProgressPhase::Branch) => "branch",
                Some(GitActionProgressPhase::Commit) => "commit",
                Some(GitActionProgressPhase::Push) => "push",
                Some(GitActionProgressPhase::Pr) => "pr",
            };
            Err(VcsActionError::RemoteFailure {
                action_id: action_id.into(),
                transport_action_id: transport_id.into(),
                action_name: action_name(&event.action),
                action: event.action,
                target: Box::new(target.clone()),
                phase: event.phase,
                remote_message_length: event.message.0.encode_utf16().count(),
                phase_name,
            })
        }
        _ => Err(VcsActionError::MissingTerminalEvent {
            action_id: action_id.into(),
            transport_action_id: transport_id.into(),
            action_name: action_name(action),
            action: action.clone(),
            target: Box::new(target.clone()),
        }),
    }
}
#[derive(Clone, Default)]
pub struct VcsActionManager {
    states: Rc<RefCell<BTreeMap<CheckoutKey, Resource<VcsActionState>>>>,
    next_id: Rc<Cell<u64>>,
}
impl VcsActionManager {
    fn resource(&self, target: &CheckoutKey) -> Resource<VcsActionState> {
        self.states
            .borrow_mut()
            .entry(target.clone())
            .or_insert_with(|| Resource::new(ResourcePolicy::default(), Generation::default()))
            .clone()
    }
    pub fn state(&self, target: Option<&CheckoutKey>) -> Rc<VcsActionState> {
        target
            .and_then(|target| self.resource(target).snapshot().value)
            .unwrap_or_default()
    }
    pub fn observe(&self, target: &CheckoutKey) -> ResourceSubscription<VcsActionState> {
        self.resource(target).observe(0)
    }
    fn set(&self, target: &CheckoutKey, state: VcsActionState) {
        let resource = self.resource(target);
        let token = resource.begin();
        resource.finish(token, state, 0, false);
    }
    pub fn begin(
        &self,
        target: &CheckoutKey,
        operation: VcsActionOperation,
        label: &str,
        action_id: Option<String>,
        now: u64,
    ) -> String {
        let id = action_id.unwrap_or_else(|| {
            self.next_id.set(self.next_id.get() + 1);
            format!("local-vcs-action:{}", self.next_id.get())
        });
        self.set(
            target,
            begin_vcs_action_state(operation, label, id.clone(), now),
        );
        id
    }
    pub fn finish_owned(&self, target: &CheckoutKey, action_id: &str, error: Option<String>) {
        let current = self.state(Some(target));
        if current.action_id.as_deref() != Some(action_id) {
            return;
        }
        self.set(
            target,
            if let Some(error) = error {
                VcsActionState {
                    operation: current.operation,
                    action_id: Some(action_id.into()),
                    error: Some(error),
                    ..VcsActionState::default()
                }
            } else {
                VcsActionState::default()
            },
        );
    }
    pub fn reset_error(&self, target: &CheckoutKey, operation: VcsActionOperation) {
        let current = self.state(Some(target));
        if !current.is_running && current.operation == Some(operation) {
            self.set(target, VcsActionState::default());
        }
    }
    pub async fn track<T, E: std::error::Error + 'static>(
        &self,
        target: &CheckoutKey,
        operation: VcsActionOperation,
        label: &str,
        platform: &(impl Platform + ?Sized),
        action: impl std::future::Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        let id = self.begin(target, operation, label, None, platform.now_epoch_ms());
        let mut guard = ActionGuard {
            manager: self.clone(),
            target: target.clone(),
            id: id.clone(),
            finished: false,
        };
        let result = action.await;
        self.finish_owned(
            target,
            &id,
            result
                .as_ref()
                .err()
                .filter(|error| !is_cancelled(*error))
                .map(ToString::to_string),
        );
        guard.finished = true;
        result
    }
    pub async fn run_stacked_action(
        &self,
        source_control: &SourceControl,
        target: &CheckoutKey,
        endpoint: &impl RpcEndpoint,
        platform: &(impl Platform + ?Sized),
        mut input: GitRunStackedActionInput,
        mut on_progress: impl FnMut(&GitActionProgressEvent) -> Result<(), ClientError>,
    ) -> Result<GitRunStackedActionResult, VcsActionError> {
        check_destination(endpoint, &target.environment)
            .map_err(|e| VcsActionError::Client(e.into_client_error()))?;
        if input.cwd.0 != target.cwd {
            return Err(VcsActionError::Unavailable);
        }
        let _lease = source_control.scheduler.acquire(target.clone()).await;
        let id = input.action_id.0.clone();
        let action = input.action.clone();
        let transport_id = create_vcs_action_transport_id(target, &id)?;
        input.action_id.0 = transport_id.clone();
        input.commit_message = input
            .commit_message
            .filter(|v| v.as_ref().is_some_and(|s| !s.0.is_empty()));
        input.feature_branch = input.feature_branch.filter(|v| *v == Some(true));
        input.file_paths = input
            .file_paths
            .filter(|v| v.as_ref().is_some_and(|v| !v.is_empty()));
        self.begin(
            target,
            VcsActionOperation::RunChangeRequest,
            "Running source control action",
            Some(id.clone()),
            platform.now_epoch_ms(),
        );
        let mut guard = ActionGuard {
            manager: self.clone(),
            target: target.clone(),
            id: id.clone(),
            finished: false,
        };
        let mut invalidation = RefsInvalidationGuard {
            refs: source_control.refs.clone(),
            environment: target.environment.clone(),
            settled: false,
        };
        let result = consume_vcs_action_progress(
            endpoint.subscribe::<WsGitRunStackedActionRpc>(input),
            target,
            &transport_id,
            &id,
            &action,
            |event| {
                let current = self.state(Some(target));
                if current.action_id.as_deref() == Some(&id) {
                    self.set(
                        target,
                        apply_vcs_action_progress_event(&current, event, platform.now_epoch_ms()),
                    );
                    let _ = on_progress(event);
                }
            },
        )
        .await;
        source_control
            .refs
            .invalidate_cached(platform, &target.environment)
            .await;
        invalidation.settled = true;
        let cancelled = matches!(&result, Err(VcsActionError::Client(ClientError::Cancelled)));
        self.finish_owned(
            target,
            &id,
            if cancelled {
                None
            } else {
                result.as_ref().err().map(ToString::to_string)
            },
        );
        guard.finished = true;
        result
    }
}
fn is_cancelled(error: &(dyn std::error::Error + 'static)) -> bool {
    matches!(
        error.downcast_ref::<ClientError>(),
        Some(ClientError::Cancelled)
    ) || matches!(
        error.downcast_ref::<crate::rpc::RpcCallError>(),
        Some(
            crate::rpc::RpcCallError::Interrupted
                | crate::rpc::RpcCallError::NotSent(ClientError::Cancelled)
        )
    ) || matches!(
        error.downcast_ref::<crate::platform::PlatformError>(),
        Some(crate::platform::PlatformError::Cancelled)
    ) || matches!(
        error.downcast_ref::<VcsActionError>(),
        Some(VcsActionError::Client(ClientError::Cancelled))
    ) || error.source().is_some_and(is_cancelled)
}
struct ActionGuard {
    manager: VcsActionManager,
    target: CheckoutKey,
    id: String,
    finished: bool,
}
impl Drop for ActionGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.manager.finish_owned(&self.target, &self.id, None);
        }
    }
}
