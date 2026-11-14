use crate::{Error, limits::LimitSourceProbe};
use serde_json::json;
use std::sync::Arc;
use t3_contracts::provider_usage_limits::{
    ProviderConsumeResetCreditResult, UsageLimitSourceSnapshot,
};
use tokio::sync::{Mutex, watch};

/// The caller resolves secret references before supplying the management key.
#[derive(Clone)]
pub struct SourceConfig {
    pub id: String,
    pub label: String,
    pub url: String,
    pub management_key: String,
    pub enabled: bool,
}

pub struct UsageLimitSources {
    probe: Arc<dyn LimitSourceProbe>,
    lock: Mutex<()>,
    snapshots: watch::Sender<Vec<UsageLimitSourceSnapshot>>,
}
pub fn hub_pool_reset_at(
    sources: &[UsageLimitSourceSnapshot],
    driver: &str,
    model: Option<&str>,
    now: f64,
) -> Option<String> {
    let mut earliest: Option<f64> = None;
    for account in sources
        .iter()
        .flat_map(|s| &s.accounts)
        .filter(|a| a.driver.0 == driver)
    {
        if account
            .usage_limits
            .unavailable
            .as_ref()
            .is_some_and(Option::is_some)
        {
            return None;
        }
        let mut recovers: Option<f64> = None;
        for window in account.usage_limits.windows.iter().filter(|w| matches!(&w.used_percent, t3_contracts::speech::SpeechNumber::Finite(n) if f64::from(n.clone()) >= 100.0) && crate::limits::window_limits_model(w,model)) {
            let reset = crate::timestamp_ms(&window.resets_at.as_ref()?.as_ref()?.0)?;
            recovers = Some(recovers.map_or(reset, |r| r.max(reset)));
        }
        let recovers = recovers.filter(|r| *r > now)?;
        earliest = Some(earliest.map_or(recovers, |r| r.min(recovers)));
    }
    earliest.map(crate::iso)
}
impl UsageLimitSources {
    pub async fn reset_at(
        &self,
        configs: &[SourceConfig],
        driver: &str,
        endpoint: &str,
        model: Option<&str>,
        clock: &dyn crate::service::Clock,
    ) -> Option<String> {
        let at_endpoint = || {
            self.current()
                .into_iter()
                .filter(|s| {
                    s.origin
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_some_and(|o| o.0 == endpoint)
                })
                .collect::<Vec<_>>()
        };
        if at_endpoint().is_empty() {
            return None;
        }
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.refresh(configs, &crate::iso(clock.now_ms())),
        )
        .await;
        hub_pool_reset_at(&at_endpoint(), driver, model, clock.now_ms())
    }
    pub fn new(probe: Arc<dyn LimitSourceProbe>) -> Self {
        let (snapshots, _) = watch::channel(vec![]);
        Self {
            probe,
            lock: Mutex::new(()),
            snapshots,
        }
    }
    pub fn current(&self) -> Vec<UsageLimitSourceSnapshot> {
        self.snapshots.borrow().clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<Vec<UsageLimitSourceSnapshot>> {
        self.snapshots.subscribe()
    }
    fn publish(&self, next: Vec<UsageLimitSourceSnapshot>) {
        self.snapshots.send_if_modified(|held| {
            if held == &next {
                false
            } else {
                *held = next;
                true
            }
        });
    }
    async fn read_source(
        &self,
        config: &SourceConfig,
        checked_at: &str,
    ) -> Result<UsageLimitSourceSnapshot, Error> {
        let label = if !config.label.is_empty() {
            config.label.clone()
        } else {
            url::Url::parse(&config.url)
                .ok()
                .and_then(|u| {
                    u.host_str()
                        .map(|h| u.port().map(|p| format!("{h}:{p}")).unwrap_or(h.into()))
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    if config.id.trim().is_empty() {
                        "Usage limit source".into()
                    } else {
                        config.id.trim().into()
                    }
                })
        };
        let mut snapshot = json!({"id":config.id,"kind":"cliproxy","label":label,"checkedAt":checked_at,"accounts":[]});
        if let Ok(url) = url::Url::parse(&config.url)
            && matches!(url.scheme(), "http" | "https")
        {
            snapshot["origin"] = json!(url.origin().ascii_serialization());
        }
        if config.management_key.is_empty() {
            snapshot["error"] = json!("No management key configured.");
        } else {
            match tokio::time::timeout(
                std::time::Duration::from_secs(15),
                self.probe.accounts(&config.url, &config.management_key),
            )
            .await
            {
                Ok(Ok(accounts)) => snapshot["accounts"] = serde_json::to_value(accounts)?,
                _ => {
                    tracing::warn!("Usage limit source request failed");
                    snapshot["error"] = json!("The hub management request failed.");
                }
            }
        }
        Ok(serde_json::from_value(snapshot)?)
    }
    pub async fn refresh(&self, configs: &[SourceConfig], checked_at: &str) -> Result<(), Error> {
        let _guard = self.lock.lock().await;
        let mut snapshots = vec![];
        // Configuration order owns display order, even when an earlier source fails.
        for config in configs.iter().filter(|c| c.enabled) {
            snapshots.push(self.read_source(config, checked_at).await?);
        }
        self.publish(snapshots);
        Ok(())
    }
    pub async fn consume_reset_credit(
        &self,
        configs: &[SourceConfig],
        source_id: &str,
        account_id: &str,
        credit_id: &str,
        checked_at: &str,
    ) -> Result<ProviderConsumeResetCreditResult, Error> {
        let _guard = self.lock.lock().await;
        let config = configs
            .iter()
            .find(|c| c.id == source_id && c.enabled)
            .ok_or_else(|| Error::Probe("The usage limit source is missing or disabled.".into()))?;
        if config.management_key.is_empty() {
            return Err(Error::Probe("No management key configured.".into()));
        }
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            self.probe
                .consume(&config.url, &config.management_key, account_id, credit_id),
        )
        .await
        .map_err(|_| Error::Probe("The hub did not answer within 15 seconds.".into()))?
        .map_err(|_| Error::Probe("The hub management request failed.".into()))?;
        let snapshot = self.read_source(config, checked_at).await?;
        let next = self
            .current()
            .into_iter()
            .map(|s| {
                if s.id.0 == source_id {
                    snapshot.clone()
                } else {
                    s
                }
            })
            .collect();
        self.publish(next);
        Ok(result)
    }
}
