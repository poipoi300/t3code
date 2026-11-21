//! Usage service boundary; paths, probes and current settings are supplied by the host.
use crate::registry::{Failure, ServerDefect};
use std::sync::Arc;
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString, rpc::WsServerGetUsageSummaryError,
    settings::ServerSettings, usage::*,
};
pub use t3_usage::Error as UsageError;
pub use t3_usage::service::{Clock, RateFetcher, ServiceConfig, UsageService};
pub use t3_usage::sources::{NoProviderProbe, ProviderUsageProbe, Source};

pub struct UsageAdapter {
    service: Arc<UsageService>,
    rates_available: bool,
}
impl UsageAdapter {
    /// The host supplies a service with source discovery and a real rate fetcher configured.
    pub fn new(service: Arc<UsageService>) -> Self {
        Self {
            service,
            rates_available: true,
        }
    }
    pub fn rates_available(&self) -> bool {
        self.rates_available
    }
    #[cfg(feature = "local-services")]
    pub async fn read_summary_for(
        &self,
        services: &crate::services::ServiceRegistry,
        input: UsageSummaryInput,
    ) -> Result<UsageSummary, Failure<WsServerGetUsageSummaryError>> {
        let settings = services.local.settings.snapshot().await.map_err(|error| {
            Failure::Defect(ServerDefect::Internal {
                message: error.to_string(),
            })
        })?;
        self.read_summary(input, settings).await
    }
    pub async fn read_summary(
        &self,
        input: UsageSummaryInput,
        settings: ServerSettings,
    ) -> Result<UsageSummary, Failure<WsServerGetUsageSummaryError>> {
        let price_overrides =
            serde_json::to_value(settings.usage_price_overrides).map_err(|e| {
                Failure::Defect(ServerDefect::Internal {
                    message: e.to_string(),
                })
            })?;
        let settings = t3_usage::service::UsageSettings {
            model_aliases: settings
                .usage_model_aliases
                .into_iter()
                .map(|(key, value)| (key.0, value.0))
                .collect(),
            price_overrides,
        };
        self.service
            .read_summary_with_settings(input, settings)
            .await
            .map_err(|error| {
                let reason = if matches!(error, t3_usage::Error::InvalidWindow(_)) {
                    UsageReadErrorReason::InvalidWindow
                } else {
                    UsageReadErrorReason::ScanFailed
                };
                Failure::Typed(WsServerGetUsageSummaryError::UsageReadError(Box::new(
                    UsageReadError {
                        _tag: UsageReadErrorTag::Error,
                        reason,
                        detail: TrimmedNonEmptyString(error.to_string()),
                        cause: None,
                    },
                )))
            })
    }
    pub async fn refresh_rates(
        &self,
    ) -> Result<UsagePricing, Failure<t3_contracts::auth::EnvironmentAuthorizationError>> {
        self.service.refresh_rates().await.map_err(|e| {
            Failure::Defect(ServerDefect::Internal {
                message: e.to_string(),
            })
        })
    }
}

#[cfg(feature = "local-services")]
impl UsageAdapter {
    /// Composition-root hook (package A). Usage sources and provider instances come from the
    /// provider registry, which is not assembled, so both lists start empty.
    pub async fn start(context: &crate::services::DomainContext<'_>) -> crate::Result<Arc<Self>> {
        let service = UsageService::new(Self::config(context))
            .await
            .map_err(|error| crate::Error::Service(error.to_string()))?;
        let mut adapter = Self::new(Arc::new(service));
        adapter.rates_available = false;
        Ok(Arc::new(adapter))
    }

    /// With the provider domain started, the price table is fetched over its outbound HTTP
    /// transport (Node fetch's behaviour: no ambient proxy, bundled roots plus
    /// `NODE_EXTRA_CA_CERTS`), bounded by `t3_usage::http::BoundedRateFetcher` (10 s, 8 MiB,
    /// JSON shape). Provider discovery is still the empty probe `start` uses.
    pub async fn start_for(
        context: &crate::services::DomainContext<'_>,
        domains: &crate::services::Domains,
    ) -> crate::Result<Arc<Self>> {
        #[cfg(feature = "providers")]
        if let Some(providers) = domains.get::<crate::adapters::providers::ProvidersAdapter>() {
            let fetcher = t3_usage::http::BoundedRateFetcher::new(Arc::new(rates::TransportRates(
                providers.http.clone(),
            )));
            let service = UsageService::with_dependencies(
                Self::config(context),
                Arc::new(t3_usage::service::SystemClock),
                Arc::new(fetcher),
                Arc::new(NoProviderProbe),
            )
            .await
            .map_err(|error| crate::Error::Service(error.to_string()))?;
            return Ok(Arc::new(Self::new(Arc::new(service))));
        }
        let _ = domains;
        Self::start(context).await
    }

    fn config(context: &crate::services::DomainContext<'_>) -> ServiceConfig {
        let paths = &context.local.config.paths;
        let hostname = sysinfo::System::host_name().unwrap_or_else(|| "localhost".into());
        ServiceConfig::new(
            paths.base_dir.clone(),
            paths.state_dir.clone(),
            context.account_home.clone(),
            hostname,
        )
    }
}

/// `t3_usage::http::RateHttpClient` over the provider domain's `HttpTransport`. The fixed
/// price URLs come from `t3_usage::service`; nothing a client sends reaches this request.
#[cfg(all(feature = "local-services", feature = "providers"))]
mod rates {
    use std::{collections::BTreeMap, sync::Arc, time::Duration};
    use t3_provider::{
        sensitive::Sensitive,
        transport::{HttpRequest, HttpTransport},
    };
    use t3_usage::{
        http::{RateHttpBody, RateHttpClient, RateHttpResponse},
        sources::ProbeFuture,
    };

    pub(super) struct TransportRates(pub Arc<dyn HttpTransport>);
    struct Buffered(Option<Vec<u8>>);
    impl RateHttpBody for Buffered {
        fn next_chunk(&mut self) -> ProbeFuture<'_, Option<Vec<u8>>> {
            let chunk = self.0.take();
            Box::pin(async move { Ok(chunk) })
        }
    }
    impl RateHttpClient for TransportRates {
        fn get<'a>(
            &'a self,
            url: &'a str,
            accept: &'a str,
            _timeout: Duration,
        ) -> ProbeFuture<'a, RateHttpResponse> {
            Box::pin(async move {
                let response = self
                    .0
                    .raw(HttpRequest {
                        method: "GET",
                        url: url.to_owned(),
                        headers: BTreeMap::from([(
                            "accept".to_owned(),
                            Sensitive::new(accept.to_owned()),
                        )]),
                        body: None,
                    })
                    .await
                    .map_err(|_| t3_usage::Error::Probe("Price request failed".into()))?;
                let body = response.body.into_inner();
                Ok(RateHttpResponse {
                    status: response.status,
                    content_length: Some(body.len() as u64),
                    body: Box::new(Buffered(Some(body))),
                })
            })
        }
    }
}

/// Global limit sources are shared by provider refresh and every config subscriber.
#[cfg(feature = "providers")]
pub(super) mod provider_sources {
    use std::{collections::BTreeMap, sync::Arc};
    use t3_contracts::{
        server::{ServerCompatibleArray, ServerProviders},
        settings_secret_value::SettingsSecretValue,
    };
    use t3_provider::{
        secrets::SecretResolver,
        sensitive::{Sensitive, ServiceError, ServiceResult},
        service::GlobalProviderSources,
        transport::{HttpRequest, HttpTransport, ProviderClock},
    };
    use t3_usage::{
        hub::*,
        limit_sources::{SourceConfig, UsageLimitSources},
    };
    pub struct Sources {
        pub snapshots: UsageLimitSources,
        refresh_lock: tokio::sync::Mutex<()>,
        settings: Arc<t3_config::settings::SettingsService>,
        secrets: Arc<SecretResolver>,
        clock: Arc<dyn ProviderClock>,
    }
    struct Transport(Arc<dyn HttpTransport>);
    impl HubHttp for Transport {
        fn send(&self, request: HubRequest) -> HubFuture<'_> {
            Box::pin(async move {
                let mut headers = BTreeMap::from([(
                    "authorization".into(),
                    Sensitive::new(request.authorization),
                )]);
                let body = if let Some(body) = request.body {
                    headers.insert(
                        "content-type".into(),
                        Sensitive::new("application/json".into()),
                    );
                    Some(Sensitive::new(
                        String::from_utf8(body).map_err(|_| HubTransportError::default())?,
                    ))
                } else {
                    None
                };
                let response = self
                    .0
                    .raw(HttpRequest {
                        method: request.method,
                        url: request.url,
                        headers,
                        body,
                    })
                    .await
                    .map_err(|_| HubTransportError::default())?;
                Ok(HubResponse {
                    status: response.status,
                    body: response.body.into_inner(),
                })
            })
        }
    }
    impl Sources {
        pub fn new(
            settings: Arc<t3_config::settings::SettingsService>,
            secrets: Arc<SecretResolver>,
            http: Arc<dyn HttpTransport>,
            clock: Arc<dyn ProviderClock>,
            platform: &str,
            arch: &str,
        ) -> Arc<Self> {
            let probe = CliproxyApi::new(
                Arc::new(Transport(http)),
                Arc::new(t3_usage::service::SystemClock),
                platform,
                arch,
            );
            Arc::new(Self {
                snapshots: UsageLimitSources::new(Arc::new(probe)),
                refresh_lock: Default::default(),
                settings,
                secrets,
                clock,
            })
        }
    }
    impl Sources {
        pub async fn follow(
            &self,
            policy: tokio::sync::watch::Receiver<t3_provider::managed_provider::BackgroundPolicy>,
            host: Arc<crate::adapters::host::HostAdapter>,
        ) {
            let mut changes = self.settings.subscribe_changes();
            let mut last = self.settings.snapshot().await.ok();
            let period = |settings: Option<&t3_contracts::settings::ServerSettings>| {
                settings
                    .map(
                        t3_shared::background_activity::resolve_server_background_activity_settings,
                    )
                    .map(|s| s.provider_health_refresh_interval)
                    .unwrap_or(std::time::Duration::from_secs(60))
            };
            let delay = |period: std::time::Duration| {
                if period.is_zero() {
                    std::time::Duration::from_secs(60)
                } else {
                    period
                }
            };
            let mut next = tokio::time::Instant::now() + delay(period(last.as_ref()));
            let _ = self.refresh_usage().await;
            loop {
                tokio::select! {
                    settings = changes.recv() => {
                        let Some(settings) = settings else { break };
                        let refresh = last.as_ref().is_none_or(|s| s.usage_limit_sources != settings.usage_limit_sources || s.secret_providers != settings.secret_providers);
                        if period(last.as_ref()) != period(Some(&settings)) {
                            next = tokio::time::Instant::now() + delay(period(Some(&settings)));
                        }
                        last = Some(settings);
                        if refresh { let _ = self.refresh_usage().await; }
                    },
                    _ = tokio::time::sleep_until(next) => {
                        let interval = period(last.as_ref());
                        let ready = policy.borrow().server_ready;
                        if ready && !interval.is_zero() && let Some(settings) = &last {
                            let scope = t3_contracts::background::BackgroundScope::ProviderStatus { instance_id: None };
                            let activity = t3_host::background::ActivitySettings::resolve(settings);
                            if host.background.should_run_scope_work(&scope, &activity).await.unwrap_or(false) {
                                let _ = self.refresh_usage().await;
                            }
                        }
                        next = tokio::time::Instant::now() + delay(interval);
                    },
                }
            }
        }
    }
    #[async_trait::async_trait]
    impl GlobalProviderSources for Sources {
        async fn refresh_usage(&self) -> ServiceResult<()> {
            let _guard = self.refresh_lock.lock().await;
            let settings = self
                .settings
                .snapshot()
                .await
                .map_err(|_| ServiceError::Storage)?;
            let enabled = settings
                .usage_limit_sources
                .into_iter()
                .filter(|(_, s)| s.enabled)
                .collect::<Vec<_>>();
            let references: Vec<_> = enabled
                .iter()
                .filter_map(|(_, s)| match &s.management_key {
                    SettingsSecretValue::Reference(r) => Some(r.clone()),
                    _ => None,
                })
                .collect();
            let mut resolved = self
                .secrets
                .read(&settings.secret_providers, &references, false)
                .await
                .into_iter();
            let configs: Vec<_> = enabled
                .into_iter()
                .map(|(id, source)| {
                    let (management_key, key_is_reference) = match source.management_key {
                        SettingsSecretValue::Reference(_) => {
                            (resolved.next().flatten().unwrap_or_default(), true)
                        }
                        SettingsSecretValue::Literal(key) => (key, false),
                    };
                    SourceConfig {
                        id: id.0,
                        label: source.label.flatten().map(|s| s.0).unwrap_or_default(),
                        url: source.url.0,
                        management_key,
                        key_is_reference,
                        enabled: true,
                    }
                })
                .collect();
            let now = crate::adapters::orchestration::iso_from_unix_millis(self.clock.now_ms());
            self.snapshots
                .refresh(&configs, &now.0)
                .await
                .map_err(|_| ServiceError::Response)
        }
        async fn reload_secrets(&self) -> ServiceResult<()> {
            self.secrets.invalidate(None).await;
            Ok(())
        }
        async fn with_usage_commands(
            &self,
            providers: ServerProviders,
        ) -> ServiceResult<ServerProviders> {
            Ok(ServerCompatibleArray(
                t3_shared::usage_limits::with_usage_limits_commands(
                    &providers.0,
                    &self.snapshots.current(),
                ),
            ))
        }
    }
}
