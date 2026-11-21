//! Session-scoped usage reports presented by the usage route.
use super::*;
use crate::services::usage::{read_usage, refresh_usage};
use t3_contracts::{
    auth::AuthEnvironmentScope,
    server::ServerConfig,
    usage::{UsageSummary, UsageSummaryInput},
};

#[derive(Clone, Debug, PartialEq)]
pub struct UsageTarget {
    pub id: EnvironmentId,
    pub label: String,
    pub generation: Generation,
    pub connected: bool,
    pub diagnostics: bool,
    pub settings_write: bool,
    pub config: Option<Rc<ServerConfig>>,
}

impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    pub fn usage_targets(&self) -> Vec<UsageTarget> {
        let snapshot = self.store().snapshot();
        self.environments()
            .into_iter()
            .map(|env| {
                let session = self.client(&env.id).and_then(|c| c.session());
                let grants =
                    |scope| session.as_ref().and_then(|s| s.grants_scope(scope)) == Some(true);
                let config = snapshot
                    .environments
                    .get(&env.id)
                    .and_then(|s| s.config.clone());
                UsageTarget {
                    id: env.id,
                    label: env.label,
                    generation: env.state.generation,
                    connected: session.is_some() && env.state.phase == Phase::Connected,
                    diagnostics: grants(AuthEnvironmentScope::DiagnosticsRead),
                    settings_write: grants(AuthEnvironmentScope::SettingsWrite),
                    config,
                }
            })
            .collect()
    }
    pub fn refresh_usage_limits(
        &self,
        coordinator: &crate::services::usage::UsageLimitsRefresh,
        environment: EnvironmentId,
        automatic: bool,
    ) -> crate::services::usage::LimitsRefreshResult {
        let providers = self.first_run();
        let id = environment.clone();
        coordinator.refresh(
            environment,
            Rc::new(self.inner.platform.clone()),
            automatic,
            async move {
                providers.refresh_providers(&id, None).await?;
                Ok(serde_json::Value::Null)
            },
        )
    }
    pub fn usage_clock_ms(&self) -> u64 {
        self.inner.platform.now_epoch_ms()
    }
    /// Dropping the report stream (period, destination or session change) drops the in-flight read.
    pub fn usage_report(
        &self,
        id: &EnvironmentId,
        input: UsageSummaryInput,
        refresh: bool,
    ) -> LocalBoxStream<'static, crate::types::QueryState<Rc<UsageSummary>>> {
        let Some(endpoint) = self
            .client(id)
            .and_then(|c| c.session())
            .filter(|s| s.grants_scope(AuthEnvironmentScope::DiagnosticsRead) == Some(true))
        else {
            return futures::stream::empty().boxed_local();
        };
        let platform = self.inner.platform.clone();
        let resource = Resource::new(ResourcePolicy::default(), Generation(0));
        let changes = resource.observe(platform.now_epoch_ms());
        let snapshots = resource.clone();
        let updates = changes.map(move |_| snapshots.snapshot());
        let drive = async move {
            if refresh {
                let _ = refresh_usage(&resource, &endpoint, &platform, input).await;
            } else {
                let _ = read_usage(&resource, &endpoint, &platform, input).await;
            }
            resource.close();
        };
        futures::stream::select(
            updates,
            futures::stream::once(drive).filter_map(|()| futures::future::ready(None)),
        )
        .boxed_local()
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePreferences {
    pub metric: UsageMetric,
    pub window_days: u32,
    #[serde(default)]
    pub hidden_providers: Vec<t3_contracts::usage::UsageProviderKind>,
}
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageMetric {
    Limits,
    Cost,
    Tokens,
}
impl Default for UsagePreferences {
    fn default() -> Self {
        Self {
            metric: UsageMetric::Limits,
            window_days: 30,
            hidden_providers: Vec::new(),
        }
    }
}
fn preference_key() -> crate::platform::StorageKey {
    crate::platform::StorageKey::new(
        crate::view_logic::shell::ui_state::PREFERENCES_NAMESPACE,
        None,
        "t3code:usage-page-preferences:v1",
    )
}
impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    pub async fn usage_preferences(&self) -> UsagePreferences {
        self.inner
            .platform
            .get(crate::platform::StorageArea::Ordinary, preference_key())
            .await
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<UsagePreferences>(&b).ok())
            .filter(|p| matches!(p.window_days, 1 | 7 | 30 | 90))
            .unwrap_or_default()
    }
    pub async fn save_usage_preferences(
        &self,
        prefs: &UsagePreferences,
    ) -> Result<(), ClientError> {
        use crate::platform::{StorageArea, StorageWrite};
        self.inner
            .platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: preference_key(),
                    value: Some(serde_json::to_vec(prefs)?),
                }],
            )
            .await?;
        Ok(())
    }
}
