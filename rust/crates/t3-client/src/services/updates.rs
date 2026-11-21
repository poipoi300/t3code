pub mod providers;
use crate::ClientError;
use crate::{
    platform::Platform,
    types::{EnvironmentId, Generation},
};
use futures::{
    FutureExt, StreamExt,
    future::{Either, LocalBoxFuture, select},
    stream::LocalBoxStream,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::{environment::ServerSelfUpdateMethod, server::*};

pub const UPDATE_RESUME_TIMEOUT_MS: u64 = 32 * 60_000;
pub const UPDATE_ACCEPTING_READY_TIMEOUT_MS: u64 = 5_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerUpdateStage {
    Downloading,
    Installing,
    Resuming,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ServerUpdateState {
    #[default]
    Idle,
    Running {
        stage: ServerUpdateStage,
        from_version: String,
        target_version: String,
    },
    Failed {
        stage: ServerUpdateStage,
        from_version: String,
        target_version: String,
        message: String,
    },
}
pub fn update_state_for_progress(
    from: &str,
    target: &str,
    event: &ServerSelfUpdateProgressEvent,
) -> ServerUpdateState {
    let stage = match event {
        ServerSelfUpdateProgressEvent::Progress(event) => match event.stage {
            ServerSelfUpdateProgressStage::Downloading => ServerUpdateStage::Downloading,
            ServerSelfUpdateProgressStage::Installing => ServerUpdateStage::Installing,
        },
        ServerSelfUpdateProgressEvent::Complete(event)
            if event.result.method == ServerSelfUpdateMethod::DesktopApp
                && event.result.desktop_update_token.is_some() =>
        {
            ServerUpdateStage::Installing
        }
        ServerSelfUpdateProgressEvent::Complete(_) => ServerUpdateStage::Resuming,
    };
    ServerUpdateState::Running {
        stage,
        from_version: from.into(),
        target_version: target.into(),
    }
}
pub fn update_state_for_server_version(
    state: ServerUpdateState,
    version: Option<&str>,
) -> ServerUpdateState {
    if let ServerUpdateState::Failed { from_version, .. } = &state
        && version.is_some_and(|version| version != from_version)
    {
        return ServerUpdateState::Idle;
    }
    state
}
pub fn matches_update_ready(
    result: &ServerSelfUpdateResult,
    event: &ServerLifecycleStreamReadyEvent,
) -> bool {
    result.update_id.as_ref().map_or_else(
        || event.payload.environment.server_version == result.target_version,
        |id| {
            event
                .payload
                .update_outcome
                .as_ref()
                .is_some_and(|outcome| outcome.id == *id)
        },
    )
}
pub fn matches_update_resume(
    result: &ServerSelfUpdateResult,
    event: &ServerLifecycleStreamReadyEvent,
    accepted_at: Option<&str>,
) -> bool {
    result.method == ServerSelfUpdateMethod::DesktopApp && result.desktop_update_token.is_some()
        || matches_update_ready(result, event)
        || result.update_id.is_some() && accepted_at.is_some_and(|at| event.payload.at.0 != at)
}
pub fn validate_update_ready(
    result: &ServerSelfUpdateResult,
    event: &ServerLifecycleStreamReadyEvent,
) -> Result<(), ClientError> {
    let Some(id) = &result.update_id else {
        return Ok(());
    };
    let outcome = event
        .payload
        .update_outcome
        .as_ref()
        .filter(|outcome| &outcome.id == id);
    if outcome.is_some_and(|outcome| {
        outcome.status == ServerSelfUpdateOutcomeStatus::Committed
            && outcome.target_version == result.target_version
    }) && event.payload.environment.server_version == result.target_version
    {
        return Ok(());
    }
    Err(ClientError::InvalidPayload(
        outcome
            .and_then(|outcome| outcome.reason.as_ref())
            .map_or_else(
                || {
                    "The service launcher resumed without committing the requested server version."
                        .into()
                },
                |reason| reason.0.clone(),
            ),
    ))
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStreamEnd {
    Complete,
    TransportHandoff,
    Failed,
    Cancelled,
}
pub fn resolve_update_progress_result(
    target: &str,
    terminal: Option<ServerSelfUpdateResult>,
    end: UpdateStreamEnd,
    accepted: Option<ServerSelfUpdateResult>,
) -> Result<ServerSelfUpdateResult, ClientError> {
    if matches!(
        end,
        UpdateStreamEnd::Complete | UpdateStreamEnd::TransportHandoff | UpdateStreamEnd::Cancelled
    ) && let Some(terminal) = terminal
    {
        return Ok(terminal);
    }
    if matches!(
        end,
        UpdateStreamEnd::TransportHandoff | UpdateStreamEnd::Cancelled
    ) && let Some(accepted) = accepted
    {
        return Ok(accepted);
    }
    match end {
        UpdateStreamEnd::Cancelled => Err(ClientError::Cancelled),
        UpdateStreamEnd::Complete => Err(ClientError::InvalidPayload(format!(
            "The t3@{target} update ended before the server accepted the restart."
        ))),
        _ => Err(ClientError::Disconnected),
    }
}
#[derive(Default)]
pub struct ReconnectObserver {
    disconnected: bool,
}
impl ReconnectObserver {
    pub fn update(&mut self, connected: bool) -> bool {
        if !connected {
            self.disconnected = true;
        }
        connected && self.disconnected
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateConnectionState {
    pub phase: super::sessions::QueryConnectionPhase,
    pub authentication_failure: bool,
}
#[derive(Debug, Clone)]
pub struct UpdateSession {
    pub generation: Generation,
    pub version: String,
    pub method: ServerSelfUpdateMethod,
    pub progress: bool,
}
/// The adapter resolves current endpoints and follows registry replacements without replaying commands.
pub trait UpdateHost {
    fn session(&self) -> Result<UpdateSession, ClientError>;
    fn lifecycle(
        &self,
    ) -> LocalBoxStream<'static, Result<(Generation, ServerLifecycleStreamReadyEvent), ClientError>>;
    fn states(&self) -> LocalBoxStream<'static, Result<UpdateConnectionState, ClientError>>;
    fn update(
        &self,
        generation: Generation,
        input: ServerSelfUpdateInput,
    ) -> LocalBoxFuture<'_, Result<ServerSelfUpdateResult, ClientError>>;
    fn progress(
        &self,
        generation: Generation,
        input: ServerSelfUpdateInput,
    ) -> LocalBoxStream<'static, Result<ServerSelfUpdateProgressEvent, ClientError>>;
    fn commit(&self, token: String) -> LocalBoxFuture<'_, Result<(), ClientError>>;
    fn retry_now(&self) -> LocalBoxFuture<'_, Result<(), ClientError>>;
}
pub fn legacy_handoff(error: &ClientError) -> bool {
    matches!(error, ClientError::Disconnected | ClientError::Cancelled)
}
fn resume_timeout(target: &str) -> ClientError {
    ClientError::InvalidPayload(format!(
        "Could not confirm that the server resumed on t3@{target}. Reconnect to check the update outcome."
    ))
}
pub async fn wait_for_next_reconnect(
    mut states: LocalBoxStream<'static, Result<UpdateConnectionState, ClientError>>,
) -> Result<(), ClientError> {
    let mut observer = ReconnectObserver::default();
    while let Some(state) = states.next().await {
        if observer.update(state?.phase == super::sessions::QueryConnectionPhase::Connected) {
            return Ok(());
        }
    }
    Err(ClientError::Disconnected)
}
pub async fn run_desktop_commit_with_reconnect_observer(
    host: &impl UpdateHost,
    platform: &(impl Platform + ?Sized),
    token: String,
) -> Result<(), ClientError> {
    let mut states = host.states();
    let first = states.next().await.ok_or(ClientError::Disconnected)??;
    let observer = wait_for_next_reconnect(
        futures::stream::once(futures::future::ready(Ok(first)))
            .chain(states)
            .boxed_local(),
    );
    let observer = select(
        observer.boxed_local(),
        platform.sleep(UPDATE_RESUME_TIMEOUT_MS),
    );
    futures::pin_mut!(observer);
    match select(host.commit(token), observer.as_mut()).await {
        Either::Left((Ok(()), _)) => Ok(()),
        Either::Left((Err(error), observer)) => {
            if !legacy_handoff(&error) {
                return Err(error);
            }
            match observer.await {
                Either::Left((result, _)) => result?,
                Either::Right(_) => return Err(ClientError::Disconnected),
            }
            Err(error)
        }
        Either::Right((reconnected, commit)) => match commit.await {
            Ok(()) => Ok(()),
            Err(error) if !legacy_handoff(&error) => Err(error),
            Err(error) => {
                match reconnected {
                    Either::Left((result, _)) => result?,
                    Either::Right(_) => return Err(ClientError::Disconnected),
                }
                Err(error)
            }
        },
    }
}
pub async fn nudge_reconnect_during_update(
    host: &impl UpdateHost,
    platform: &(impl Platform + ?Sized),
    interval_ms: u64,
) {
    use super::sessions::QueryConnectionPhase::{Backoff, Blocked};
    let nudge = async {
        let mut states = host.states();
        while let Some(Ok(state)) = states.next().await {
            if state.phase == Backoff || state.phase == Blocked && state.authentication_failure {
                platform.sleep(interval_ms.max(1)).await;
                let _ = host.retry_now().await;
            }
        }
    }
    .boxed_local();
    let _ = select(nudge, platform.sleep(UPDATE_RESUME_TIMEOUT_MS)).await;
}
async fn next_ready(
    host: &impl UpdateHost,
    platform: &(impl Platform + ?Sized),
    result: &ServerSelfUpdateResult,
    accepted_at: Option<&str>,
) -> Result<ServerLifecycleStreamReadyEvent, ClientError> {
    let read = async {
        let mut lifecycle = host.lifecycle();
        while let Some(event) = lifecycle.next().await {
            let (_, event) = event?;
            if matches_update_resume(result, &event, accepted_at) {
                return Ok(event);
            }
        }
        Err(resume_timeout(&result.target_version.0))
    }
    .boxed_local();
    match select(read, platform.sleep(UPDATE_RESUME_TIMEOUT_MS)).await {
        Either::Left((result, _)) => result,
        Either::Right(_) => Err(resume_timeout(&result.target_version.0)),
    }
}
pub async fn wait_for_desktop_update_target(
    host: &impl UpdateHost,
    platform: &(impl Platform + ?Sized),
    result: &ServerSelfUpdateResult,
    accepted_at: Option<&str>,
) -> Result<ServerLifecycleStreamReadyEvent, ClientError> {
    for attempt in 1..=3 {
        let ready = next_ready(host, platform, result, accepted_at).await?;
        if ready.payload.environment.server_version == result.target_version {
            return Ok(ready);
        }
        if attempt == 3 {
            break;
        }
        let Some(token) = &result.desktop_update_token else {
            break;
        };
        match run_desktop_commit_with_reconnect_observer(host, platform, token.0.clone()).await {
            Ok(()) => break,
            Err(error) if legacy_handoff(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Err(ClientError::InvalidPayload(
        "The desktop app resumed without installing the prepared update.".into(),
    ))
}

#[derive(Default)]
struct UpdateInner {
    states: BTreeMap<EnvironmentId, super::resources::Resource<ServerUpdateState>>,
    locks: BTreeMap<EnvironmentId, Rc<futures::lock::Mutex<()>>>,
}
#[derive(Clone, Default)]
pub struct ServerUpdateService(Rc<RefCell<UpdateInner>>);
impl ServerUpdateService {
    pub fn remove_environment(&self, environment: &EnvironmentId) {
        let mut inner = self.0.borrow_mut();
        if let Some(state) = inner.states.remove(environment) {
            state.close();
        }
        inner.locks.remove(environment);
    }
    pub fn state(
        &self,
        environment: EnvironmentId,
    ) -> super::resources::Resource<ServerUpdateState> {
        self.0
            .borrow_mut()
            .states
            .entry(environment)
            .or_insert_with(|| {
                let resource =
                    super::resources::Resource::new(Default::default(), Generation::default());
                set_update_state(&resource, ServerUpdateState::Idle);
                resource
            })
            .clone()
    }
    pub async fn run(
        &self,
        environment: EnvironmentId,
        host: &impl UpdateHost,
        platform: &(impl Platform + ?Sized),
        input: ServerSelfUpdateInput,
    ) -> Result<ServerSelfUpdateResult, ClientError> {
        let lock = self
            .0
            .borrow_mut()
            .locks
            .entry(environment.clone())
            .or_default()
            .clone();
        let state = self.state(environment);
        let _lock = lock.lock().await;
        if state.is_closed() {
            return Err(ClientError::Cancelled);
        }
        let mut guard = UpdateGuard {
            state: state.clone(),
            settled: false,
        };
        let initial = host.session()?;
        let from = initial.version.clone();
        let mut target = input.target_version.0.clone();
        let mut stage = ServerUpdateStage::Downloading;
        set_update_state(
            &state,
            ServerUpdateState::Running {
                stage,
                from_version: from.clone(),
                target_version: target.clone(),
            },
        );
        let operation = until_removed(&state, async {
            let mut accepting = None;
            if initial.method == ServerSelfUpdateMethod::BootService {
                let mut lifecycle = host.lifecycle();
                let baseline = async { lifecycle.next().await }.boxed_local();
                if let Either::Left((Some(Ok((generation, event))), _)) =
                    select(baseline, platform.sleep(UPDATE_ACCEPTING_READY_TIMEOUT_MS)).await
                {
                    accepting = Some((generation, event.payload.at.0));
                }
            }
            let session = host.session()?;
            let accepted_at = accepting
                .as_ref()
                .filter(|(generation, _)| *generation == session.generation)
                .map(|(_, at)| at.as_str());
            let result = if session.progress {
                let mut stream = host.progress(session.generation, input.clone());
                let mut terminal = None;
                let mut received_progress = false;
                let mut error = None;
                while let Some(event) = stream.next().await {
                    match event {
                        Ok(event) => {
                            received_progress |=
                                matches!(event, ServerSelfUpdateProgressEvent::Progress(_));
                            let next = update_state_for_progress(&from, &target, &event);
                            if let ServerUpdateState::Running {
                                stage: next_stage, ..
                            } = &next
                            {
                                stage = *next_stage;
                            }
                            set_update_state(&state, next);
                            if let ServerSelfUpdateProgressEvent::Complete(event) = event {
                                terminal = Some(event.result);
                            }
                        }
                        Err(failure) => {
                            error = Some(failure);
                            break;
                        }
                    }
                }
                let end = if let Some(failure) = error {
                    if !legacy_handoff(&failure) {
                        return Err(failure);
                    }
                    UpdateStreamEnd::TransportHandoff
                } else {
                    UpdateStreamEnd::Complete
                };
                let accepted = (received_progress
                    && matches!(
                        session.method,
                        ServerSelfUpdateMethod::BootService | ServerSelfUpdateMethod::Respawn
                    ))
                .then(|| ServerSelfUpdateResult {
                    target_version: input.target_version.clone(),
                    method: session.method.clone(),
                    update_id: None,
                    desktop_update_token: None,
                });
                resolve_update_progress_result(&target, terminal, end, accepted)?
            } else {
                match host.update(session.generation, input).await {
                    Ok(result) => result,
                    Err(error)
                        if legacy_handoff(&error)
                            && matches!(
                                session.method,
                                ServerSelfUpdateMethod::BootService
                                    | ServerSelfUpdateMethod::Respawn
                            ) =>
                    {
                        ServerSelfUpdateResult {
                            target_version: t3_contracts::base_schemas::TrimmedNonEmptyString(
                                target.clone(),
                            ),
                            method: session.method,
                            update_id: None,
                            desktop_update_token: None,
                        }
                    }
                    Err(error) => return Err(error),
                }
            };
            if result.method == ServerSelfUpdateMethod::DesktopApp
                && let Some(token) = &result.desktop_update_token
            {
                let commit =
                    run_desktop_commit_with_reconnect_observer(host, platform, token.0.clone())
                        .boxed_local();
                let nudge = async {
                    nudge_reconnect_during_update(host, platform, 1_000).await;
                    futures::future::pending::<()>().await
                }
                .boxed_local();
                let committed = match select(commit, nudge).await {
                    Either::Left((result, _)) => result,
                    Either::Right(_) => unreachable!(),
                };
                match committed {
                    Ok(()) => {}
                    Err(error) if legacy_handoff(&error) => {}
                    Err(error) => return Err(error),
                }
            }
            target = result.target_version.0.clone();
            if result.method == ServerSelfUpdateMethod::DesktopApp
                && result.desktop_update_token.is_none()
                && target == from
            {
                return Ok(result);
            }
            stage = ServerUpdateStage::Resuming;
            set_update_state(
                &state,
                ServerUpdateState::Running {
                    stage,
                    from_version: from.clone(),
                    target_version: target.clone(),
                },
            );
            let resume = async {
                let ready = if result.method == ServerSelfUpdateMethod::DesktopApp
                    && result.desktop_update_token.is_some()
                {
                    wait_for_desktop_update_target(host, platform, &result, accepted_at).await?
                } else {
                    next_ready(host, platform, &result, accepted_at).await?
                };
                validate_update_ready(&result, &ready)?;
                Ok(result)
            }
            .boxed_local();
            let nudge = async {
                nudge_reconnect_during_update(host, platform, 1_000).await;
                futures::future::pending::<()>().await
            }
            .boxed_local();
            match select(resume, nudge).await {
                Either::Left((result, _)) => result,
                Either::Right(_) => unreachable!(),
            }
        })
        .await;
        let next = match &operation {
            Ok(_) | Err(ClientError::Cancelled) => ServerUpdateState::Idle,
            Err(error) => ServerUpdateState::Failed {
                stage,
                from_version: from,
                target_version: target,
                message: error.to_string(),
            },
        };
        set_update_state(&state, next);
        guard.settled = true;
        operation
    }
    pub async fn run_outdated<F: crate::connection::SessionFactory>(
        &self,
        resolver: &crate::connection::ConnectionResolver,
        factory: &F,
        target: OutdatedServerUpdateTarget,
        after_update: impl Fn() -> LocalBoxFuture<'static, Result<(), ClientError>>,
    ) -> Result<crate::connection::OutdatedHostUpdateResult, ClientError> {
        let environment = target.identity.environment.clone();
        let lock = self
            .0
            .borrow_mut()
            .locks
            .entry(environment.clone())
            .or_default()
            .clone();
        let state = self.state(environment);
        let _lock = lock.lock().await;
        if state.is_closed() {
            return Err(ClientError::Cancelled);
        }
        let mut guard = UpdateGuard {
            state: state.clone(),
            settled: false,
        };
        let from = target
            .from_version
            .unwrap_or_else(|| target.input.target_version.0.clone());
        let version = target.input.target_version.0.clone();
        let stage = std::cell::Cell::new(ServerUpdateStage::Downloading);
        set_update_state(
            &state,
            ServerUpdateState::Running {
                stage: stage.get(),
                from_version: from.clone(),
                target_version: version.clone(),
            },
        );
        let result = until_removed(
            &state,
            resolver.update_outdated_host(
                factory,
                &target.routes,
                &target.identity,
                target.input,
                |next| {
                    stage.set(match next {
                        crate::connection::OutdatedHostUpdateStage::Downloading => {
                            ServerUpdateStage::Downloading
                        }
                        crate::connection::OutdatedHostUpdateStage::Installing => {
                            ServerUpdateStage::Installing
                        }
                        crate::connection::OutdatedHostUpdateStage::Resuming => {
                            ServerUpdateStage::Resuming
                        }
                    });
                    set_update_state(
                        &state,
                        ServerUpdateState::Running {
                            stage: stage.get(),
                            from_version: from.clone(),
                            target_version: version.clone(),
                        },
                    );
                },
                after_update,
            ),
        )
        .await;
        set_update_state(
            &state,
            match &result {
                Ok(_) | Err(ClientError::Cancelled) => ServerUpdateState::Idle,
                Err(error) => ServerUpdateState::Failed {
                    stage: stage.get(),
                    from_version: from,
                    target_version: version,
                    message: error.to_string(),
                },
            },
        );
        guard.settled = true;
        result
    }
}
async fn until_removed<T>(
    state: &super::resources::Resource<ServerUpdateState>,
    work: impl std::future::Future<Output = Result<T, ClientError>>,
) -> Result<T, ClientError> {
    let removed = async {
        let mut changes = state.observe(0);
        while !state.is_closed() {
            changes.next().await;
        }
        Err(ClientError::Cancelled)
    }
    .boxed_local();
    match select(removed, Box::pin(work)).await {
        Either::Left((result, _)) | Either::Right((result, _)) => result,
    }
}
pub struct OutdatedServerUpdateTarget {
    pub identity: crate::environment::SessionIdentity,
    pub routes: Vec<crate::connection::Route>,
    pub input: ServerSelfUpdateInput,
    pub from_version: Option<String>,
}
pub async fn describe_outdated_server(
    resolver: &crate::connection::ConnectionResolver,
    platform: &(impl Platform + ?Sized),
    routes: &[crate::connection::Route],
    identity: &crate::environment::SessionIdentity,
) -> Result<crate::authorization::AuthorizedEnvironment, ClientError> {
    let describe = resolver
        .describe_outdated_host(routes, identity)
        .boxed_local();
    match select(describe, platform.sleep(20_000)).await {
        Either::Left((result, _)) => {
            result.map_err(|failure| ClientError::InvalidPayload(failure.detail))
        }
        Either::Right(_) => Err(ClientError::InvalidPayload(
            "Outdated host description timed out.".into(),
        )),
    }
}
fn set_update_state(
    state: &super::resources::Resource<ServerUpdateState>,
    value: ServerUpdateState,
) {
    let token = state.begin();
    state.finish(token, value, 0, false);
}
struct UpdateGuard {
    state: super::resources::Resource<ServerUpdateState>,
    settled: bool,
}
impl Drop for UpdateGuard {
    fn drop(&mut self) {
        if !self.settled {
            set_update_state(&self.state, ServerUpdateState::Idle);
        }
    }
}
