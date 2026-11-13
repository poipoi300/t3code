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
        let paths = &context.local.config.paths;
        let hostname = sysinfo::System::host_name().unwrap_or_else(|| "localhost".into());
        let config = ServiceConfig::new(
            paths.base_dir.clone(),
            paths.state_dir.clone(),
            context.account_home.clone(),
            hostname,
        );
        let service = UsageService::new(config)
            .await
            .map_err(|error| crate::Error::Service(error.to_string()))?;
        let mut adapter = Self::new(Arc::new(service));
        adapter.rates_available = false;
        Ok(Arc::new(adapter))
    }
}
