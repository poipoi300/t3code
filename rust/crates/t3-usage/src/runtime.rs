use crate::{
    Error,
    http::{BoundedRateFetcher, RateHttpClient},
    limit_sources::{SourceConfig, UsageLimitSources},
    limits::LimitSourceProbe,
    service::{Clock, ServiceConfig, UsageService},
    sources::{
        ExternalSource, HashMapTiers, MuseHome, ProbeFuture, ProviderInstance, ProviderUsageProbe,
        TierRequest,
    },
};
use std::sync::Arc;
use t3_contracts::{
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    provider_usage_limits::{
        ProviderConsumeResetCreditResult, UsageLimitSourceSnapshot, UsageLimitSourceSnapshots,
    },
    server::{
        ServerConfig, ServerConfigProviderStatusesPayload,
        ServerConfigUsageLimitSourcesUpdatedPayload, ServerProviders,
    },
};
use tokio::sync::watch;

#[derive(Clone)]
pub struct ProviderUsageSnapshot {
    pub providers: ServerProviders,
    pub instances: Vec<ProviderInstance>,
}
/// One coherent registry publication: safe provider statuses plus resolved launch-time history inputs.
pub trait ProviderUsageDiscovery: ProviderUsageProbe {
    fn subscribe_usage(&self) -> watch::Receiver<ProviderUsageSnapshot>;
}
pub struct UsageRuntimeInputs {
    pub config: ServiceConfig,
    pub clock: Arc<dyn Clock>,
    pub http: Arc<dyn RateHttpClient>,
    pub providers: Arc<dyn ProviderUsageDiscovery>,
    pub hub: Arc<dyn LimitSourceProbe>,
}
#[derive(Clone)]
pub struct UsageRuntime {
    service: Arc<UsageService>,
    sources: Arc<UsageLimitSources>,
    providers: watch::Receiver<ProviderUsageSnapshot>,
    clock: Arc<dyn Clock>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct UsageConfigSnapshot {
    pub providers: ServerProviders,
    pub sources: Vec<UsageLimitSourceSnapshot>,
}
impl UsageConfigSnapshot {
    pub fn apply(&self, config: &mut ServerConfig) {
        config.providers = self.providers.clone();
        config.usage_limit_sources = Some(Some(UsageLimitSourceSnapshots(self.sources.clone())));
    }
}
pub enum UsageConfigChange {
    Snapshot(UsageConfigSnapshot),
    Providers(ServerConfigProviderStatusesPayload),
    Sources(ServerConfigUsageLimitSourcesUpdatedPayload),
}
pub struct UsageConfigSubscription {
    providers: watch::Receiver<ProviderUsageSnapshot>,
    sources: watch::Receiver<Vec<UsageLimitSourceSnapshot>>,
    initial: Option<UsageConfigSnapshot>,
}
impl UsageConfigSubscription {
    pub async fn next(&mut self) -> Option<UsageConfigChange> {
        if let Some(initial) = self.initial.take() {
            return Some(UsageConfigChange::Snapshot(initial));
        }
        tokio::select! {
            changed=self.providers.changed()=>{
                changed.ok()?;
                Some(UsageConfigChange::Providers(ServerConfigProviderStatusesPayload {providers:self.providers.borrow_and_update().providers.clone()}))
            }
            changed=self.sources.changed()=>{
                changed.ok()?;
                Some(UsageConfigChange::Sources(ServerConfigUsageLimitSourcesUpdatedPayload {sources:UsageLimitSourceSnapshots(self.sources.borrow_and_update().clone())}))
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEndpoint {
    pub driver: ProviderDriverKind,
    pub origin: String,
}
impl UsageRuntime {
    pub async fn new(inputs: UsageRuntimeInputs) -> Result<Self, Error> {
        let providers = inputs.providers.subscribe_usage();
        let probe = Arc::new(DiscoveredProbe {
            host: inputs.providers,
            providers: providers.clone(),
        });
        let service = UsageService::with_dependencies(
            inputs.config,
            inputs.clock.clone(),
            Arc::new(BoundedRateFetcher::new(inputs.http)),
            probe,
        )
        .await?;
        Ok(Self {
            service: Arc::new(service),
            sources: Arc::new(UsageLimitSources::new(inputs.hub)),
            providers,
            clock: inputs.clock,
        })
    }
    pub fn service(&self) -> Arc<UsageService> {
        self.service.clone()
    }
    pub fn limit_sources(&self) -> Arc<UsageLimitSources> {
        self.sources.clone()
    }
    pub fn current_config(&self) -> UsageConfigSnapshot {
        UsageConfigSnapshot {
            providers: self.providers.borrow().providers.clone(),
            sources: self.sources.current(),
        }
    }
    pub fn subscribe_config(&self) -> UsageConfigSubscription {
        let mut providers = self.providers.clone();
        let mut sources = self.sources.subscribe();
        let initial = UsageConfigSnapshot {
            providers: providers.borrow_and_update().providers.clone(),
            sources: sources.borrow_and_update().clone(),
        };
        UsageConfigSubscription {
            providers,
            sources,
            initial: Some(initial),
        }
    }
    pub async fn refresh_sources(&self, configs: &[SourceConfig]) -> Result<(), Error> {
        self.sources
            .refresh(configs, &crate::iso(self.clock.now_ms()))
            .await
    }
    pub async fn consume_reset_credit(
        &self,
        configs: &[SourceConfig],
        source: &str,
        account: &str,
        credit: &str,
    ) -> Result<ProviderConsumeResetCreditResult, Error> {
        self.sources
            .consume_reset_credit(
                configs,
                source,
                account,
                credit,
                &crate::iso(self.clock.now_ms()),
            )
            .await
    }
    /// Capture with the turn's selection, before another registry update can change its endpoint.
    pub fn endpoint(&self, instance: &ProviderInstanceId) -> Option<UsageEndpoint> {
        let providers = self.providers.borrow();
        let provider = providers
            .providers
            .0
            .iter()
            .find(|p| &p.instance_id == instance)?;
        let url = url::Url::parse(&provider.auth.endpoint.as_ref()?.as_ref()?.0).ok()?;
        matches!(url.scheme(), "http" | "https").then(|| UsageEndpoint {
            driver: provider.driver.clone(),
            origin: url.origin().ascii_serialization(),
        })
    }
    pub async fn reset_at(
        &self,
        configs: &[SourceConfig],
        endpoint: &UsageEndpoint,
        model: Option<&str>,
    ) -> Option<String> {
        self.sources
            .reset_at(
                configs,
                &endpoint.driver.0,
                &endpoint.origin,
                model,
                self.clock.as_ref(),
            )
            .await
    }
}
struct DiscoveredProbe {
    host: Arc<dyn ProviderUsageDiscovery>,
    providers: watch::Receiver<ProviderUsageSnapshot>,
}
impl ProviderUsageProbe for DiscoveredProbe {
    fn instances(&self) -> Option<Vec<ProviderInstance>> {
        Some(self.providers.borrow().instances.clone())
    }
    fn muse_home<'a>(
        &'a self,
        instance: &'a ProviderInstance,
        await_refresh: bool,
    ) -> ProbeFuture<'a, MuseHome> {
        self.host.muse_home(instance, await_refresh)
    }
    fn scan<'a>(
        &'a self,
        instances: &'a [ProviderInstance],
        since: f64,
        until: f64,
        await_refresh: bool,
    ) -> ProbeFuture<'a, Vec<ExternalSource>> {
        self.host.scan(instances, since, until, await_refresh)
    }
    fn codex_tiers<'a>(&'a self, requests: &'a [TierRequest]) -> ProbeFuture<'a, HashMapTiers> {
        self.host.codex_tiers(requests)
    }
}
