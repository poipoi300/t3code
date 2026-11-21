pub mod limits;
pub mod overview;
pub mod prices;
use super::{filesystem::ReadAccess, resources::Resource};
use crate::types::EnvironmentId;
use crate::{ClientError, platform::Platform, rpc::RpcEndpoint};
use futures::{
    FutureExt, StreamExt,
    future::{Either, LocalBoxFuture, select},
    stream::LocalBoxStream,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::{
    auth::{AuthEnvironmentScope, AuthSessionState},
    rpc::{Empty, WsServerGetUsageSummaryRpc, WsServerRefreshUsageRatesRpc},
    usage::*,
};
#[derive(Clone, Default)]
pub struct UsageLimitsRefresh {
    scheduler: super::scheduling::CommandScheduler,
    refresh_after: Rc<RefCell<BTreeMap<EnvironmentId, u64>>>,
}
pub type LimitsRefreshResult =
    LocalBoxFuture<'static, Result<Option<Rc<serde_json::Value>>, Rc<ClientError>>>;
impl UsageLimitsRefresh {
    pub fn refresh(
        &self,
        environment: EnvironmentId,
        platform: Rc<dyn Platform>,
        automatic: bool,
        refresh: impl std::future::Future<Output = Result<serde_json::Value, ClientError>> + 'static,
    ) -> LimitsRefreshResult {
        let lane = super::scheduling::CommandLane {
            environment: environment.clone(),
            group: "usage-limits-refresh".into(),
            arguments: String::new(),
        };
        if automatic
            && (self.scheduler.is_pending(&lane)
                || platform.now_epoch_ms()
                    < self
                        .refresh_after
                        .borrow()
                        .get(&environment)
                        .copied()
                        .unwrap_or(0))
        {
            return futures::future::ready(Ok(None)).boxed_local();
        }
        let refresh_after = self.refresh_after.clone();
        let future = self.scheduler.schedule(
            lane,
            super::scheduling::Concurrency::SingleFlight,
            async move {
                let result = refresh.await;
                refresh_after
                    .borrow_mut()
                    .insert(environment, platform.now_epoch_ms().saturating_add(300_000));
                result
            },
        );
        async move { future.await.map(Some) }.boxed_local()
    }
}

pub fn usage_access(
    phase: &str,
    session: Option<&AuthSessionState>,
    session_error: bool,
) -> ReadAccess {
    if session_error {
        return ReadAccess {
            allowed: false,
            pending: false,
            error: Some(
                "Could not check this connection's access to diagnostics and usage.".into(),
            ),
        };
    }
    if let Some(session) = session {
        let allowed = crate::permissions::session_grants_scope(
            session,
            AuthEnvironmentScope::DiagnosticsRead,
        );
        return ReadAccess {
            allowed,
            pending: false,
            error: (!allowed)
                .then(|| "This connection does not have access to diagnostics and usage.".into()),
        };
    }
    let pending = matches!(phase, "connected" | "connecting" | "reconnecting");
    ReadAccess {
        allowed: false,
        pending,
        error: (!pending).then(|| "This environment is not connected.".into()),
    }
}
pub fn usage_summary_input(
    mut input: UsageSummaryInput,
    filter: Option<UsageModelFilter>,
) -> UsageSummaryInput {
    if let Some(filter) = filter {
        input.model_filter = Some(Some(filter));
    }
    input
}
pub fn summary_settled(summary: &UsageSummary) -> bool {
    !summary
        .sources
        .iter()
        .any(|source| source.refreshing.is_some())
}
pub async fn read_usage(
    resource: &Resource<UsageSummary>,
    endpoint: &impl RpcEndpoint,
    platform: &(impl Platform + ?Sized),
    input: UsageSummaryInput,
) -> Result<(), ClientError> {
    let previous = resource.snapshot().value.is_some();
    resource
        .run(platform, |token| async move {
            if !previous {
                let mut provisional = input.clone();
                provisional.provisional = Some(Some(true));
                if let Ok(saved) = endpoint
                    .call::<WsServerGetUsageSummaryRpc>(provisional)
                    .await
                {
                    if !resource.is_current(token) {
                        return Err(ClientError::Cancelled);
                    }
                    if summary_settled(&saved) {
                        return Ok(saved);
                    }
                    resource.emit(token, saved);
                    platform.yield_to_host().await;
                }
            }
            let summary = endpoint
                .call::<WsServerGetUsageSummaryRpc>(input.clone())
                .await?;
            if !resource.is_current(token) {
                return Err(ClientError::Cancelled);
            }
            if summary_settled(&summary) {
                return Ok(summary);
            }
            resource.emit(token, summary.clone());
            platform.yield_to_host().await;
            let mut awaited = input;
            awaited.await_refresh = Some(Some(true));
            match endpoint.call::<WsServerGetUsageSummaryRpc>(awaited).await {
                Ok(summary) => Ok(summary),
                Err(_) => Ok(summary),
            }
        })
        .await
}
pub async fn refresh_usage(
    resource: &Resource<UsageSummary>,
    endpoint: &impl RpcEndpoint,
    platform: &(impl Platform + ?Sized),
    input: UsageSummaryInput,
) -> Result<(), ClientError> {
    let rates = endpoint
        .call::<WsServerRefreshUsageRatesRpc>(Empty {})
        .await;
    resource.invalidate();
    if matches!(
        rates,
        Err(ClientError::Disconnected | ClientError::Cancelled)
    ) {
        return Ok(());
    }
    read_usage(resource, endpoint, platform, input).await
}
pub async fn refresh_usage_while_connected(
    resource: &Resource<UsageSummary>,
    endpoint: &impl RpcEndpoint,
    platform: &(impl Platform + ?Sized),
    input: UsageSummaryInput,
    mut phases: LocalBoxStream<'static, super::sessions::QueryConnectionPhase>,
) -> Result<(), ClientError> {
    let disconnected = async {
        while let Some(phase) = phases.next().await {
            if phase != super::sessions::QueryConnectionPhase::Connected {
                return;
            }
        }
    }
    .boxed_local();
    let refresh = refresh_usage(resource, endpoint, platform, input).boxed_local();
    match select(disconnected, refresh).await {
        Either::Left(_) => {
            resource.invalidate();
            Ok(())
        }
        Either::Right((Err(ClientError::Disconnected | ClientError::Cancelled), _)) => Ok(()),
        Either::Right((result, _)) => result,
    }
}
#[derive(Clone)]
pub struct UsageEnvironment<'a> {
    pub label: &'a str,
    pub connected: bool,
    pub pending: bool,
    pub error: Option<&'a str>,
    pub summary: Option<&'a UsageSummary>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum UsageEnvironmentProgress {
    Inactive,
    Loading,
    Stale,
    Ready,
    Partway(Vec<UsageProviderKind>),
}
pub fn usage_environment_progress(
    environment: &UsageEnvironment<'_>,
    refreshing: bool,
) -> UsageEnvironmentProgress {
    if !environment.connected || environment.error.is_some() {
        return UsageEnvironmentProgress::Inactive;
    }
    let Some(summary) = environment.summary else {
        return UsageEnvironmentProgress::Loading;
    };
    if refreshing {
        return UsageEnvironmentProgress::Stale;
    }
    if !environment.pending {
        return UsageEnvironmentProgress::Ready;
    }
    let mut providers = Vec::new();
    for source in &summary.sources {
        if source.refreshing.is_some() && !providers.contains(&source.fingerprint.provider) {
            providers.push(source.fingerprint.provider.clone());
        }
    }
    if providers.is_empty() {
        UsageEnvironmentProgress::Stale
    } else {
        UsageEnvironmentProgress::Partway(providers)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageProgress {
    pub dimmed: bool,
    pub label: Option<String>,
}
pub fn updating_providers_label(
    providers: &[UsageProviderKind],
    label: impl Fn(&UsageProviderKind) -> String,
) -> String {
    if let [provider] = providers {
        format!("Updating {}…", label(provider))
    } else {
        format!("Updating {} providers…", providers.len())
    }
}
pub fn usage_progress(
    environments: &[UsageEnvironment<'_>],
    refreshing: bool,
    label: impl Fn(&UsageProviderKind) -> String,
) -> UsageProgress {
    let progress: Vec<_> = environments
        .iter()
        .map(|environment| usage_environment_progress(environment, refreshing))
        .collect();
    let waiting: Vec<_> = environments
        .iter()
        .zip(&progress)
        .filter(|(_, progress)| {
            matches!(
                progress,
                UsageEnvironmentProgress::Loading | UsageEnvironmentProgress::Stale
            )
        })
        .map(|(environment, _)| environment.label)
        .collect();
    let mut providers = Vec::new();
    for progress in &progress {
        if let UsageEnvironmentProgress::Partway(entries) = progress {
            for provider in entries {
                if !providers.contains(provider) {
                    providers.push(provider.clone());
                }
            }
        }
    }
    let answered = progress.iter().any(|progress| {
        matches!(
            progress,
            UsageEnvironmentProgress::Partway(_) | UsageEnvironmentProgress::Ready
        )
    });
    let label = if waiting.is_empty() {
        (!providers.is_empty()).then(|| updating_providers_label(&providers, label))
    } else if environments.len() == 1 {
        Some("Updating…".into())
    } else if let [waiting] = waiting.as_slice() {
        Some(format!("Updating {waiting}…"))
    } else {
        Some(format!("Updating {} environments…", waiting.len()))
    };
    UsageProgress {
        dimmed: !answered && progress.contains(&UsageEnvironmentProgress::Stale),
        label,
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct UsageLoadingState {
    pub partial: bool,
    pub every_provider: bool,
    pub providers: Vec<UsageProviderKind>,
}
pub fn usage_loading_state(
    environments: &[UsageEnvironment<'_>],
    refreshing: bool,
) -> UsageLoadingState {
    let mut providers = Vec::new();
    let mut every_provider = false;
    for environment in environments {
        match usage_environment_progress(environment, refreshing) {
            UsageEnvironmentProgress::Loading | UsageEnvironmentProgress::Stale => {
                every_provider = true
            }
            UsageEnvironmentProgress::Partway(entries) => {
                for provider in entries {
                    if !providers.contains(&provider) {
                        providers.push(provider);
                    }
                }
            }
            _ => {}
        }
    }
    UsageLoadingState {
        partial: every_provider || !providers.is_empty(),
        every_provider,
        providers,
    }
}
