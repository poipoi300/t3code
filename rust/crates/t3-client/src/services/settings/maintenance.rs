//! Scoped storage status, recommendations and commands used by Maintenance settings.
use super::scoped::resolve_worktree_cleanup;
use crate::{
    ClientError,
    rpc::{RpcEndpoint, SessionEndpoint},
};
use futures::{future::LocalBoxFuture, stream::LocalBoxStream};
use serde_json::Value;
use t3_contracts::{
    rpc::*,
    settings::{DatabaseStorageStatus, StorageCleanupReport, WireNumber},
};

pub struct StorageSession(pub SessionEndpoint);
impl StorageSession {
    pub fn database(
        &self,
    ) -> LocalBoxStream<'static, Result<Option<DatabaseStorageStatus>, ClientError>> {
        self.0.subscribe::<WsServerGetDatabaseStorageRpc>(Empty {})
    }
    pub fn reports(
        &self,
    ) -> LocalBoxStream<'static, Result<Option<StorageCleanupReport>, ClientError>> {
        self.0
            .subscribe::<WsServerGetStorageCleanupReportRpc>(Empty {})
    }
    pub fn can_reclaim(&self) -> bool {
        self.0
            .available::<WsServerReclaimDatabaseSpaceRpc>(&Empty {})
    }
    pub fn can_clean(&self) -> bool {
        self.0.available::<WsServerRunStorageCleanupRpc>(&Empty {})
    }
    pub fn reclaim(&self) -> LocalBoxFuture<'_, Result<(), ClientError>> {
        self.0.call::<WsServerReclaimDatabaseSpaceRpc>(Empty {})
    }
    pub fn clean(&self) -> LocalBoxFuture<'_, Result<StorageCleanupReport, ClientError>> {
        self.0.call::<WsServerRunStorageCleanupRpc>(Empty {})
    }
}

pub fn number(n: &WireNumber) -> f64 {
    match n {
        WireNumber::Finite(n) => n.value(),
        WireNumber::NonFinite(_) => 0.0,
    }
}

pub fn recommend(status: &DatabaseStorageStatus) -> bool {
    !status.incremental_vacuum_enabled && number(&status.reclaimable_bytes) >= 1024_f64.powi(3)
}

pub fn minutes(status: &DatabaseStorageStatus) -> u64 {
    ((number(&status.database_bytes) - number(&status.reclaimable_bytes)) / 1024_f64.powi(3) / 2.)
        .ceil()
        .max(1.) as u64
}

pub fn has_rules(settings: &Value) -> bool {
    fn enabled(rules: &Value) -> bool {
        ["worktreeAfterDays", "worktreeSettledAfterDays"]
            .iter()
            .any(|k| rules[k].is_number())
            || ["worktreeOnMerge", "worktreeOnDelete", "worktreeUnchanged"]
                .iter()
                .any(|k| rules[k] == true)
    }
    let storage = &settings["storageCleanup"];
    storage["browserArtifactsAfterDays"].is_number()
        || storage["logsAfterDays"].is_number()
        || enabled(&resolve_worktree_cleanup(settings, settings))
        || settings["projectSettingsOverrides"]
            .as_object()
            .is_some_and(|overrides| {
                overrides.values().any(|p| {
                    p["worktreeCleanup"]["mode"] == "custom"
                        && enabled(&p["worktreeCleanup"]["rules"])
                })
            })
}

/// Effective background activity fields, including legacy profiles and intervals.
pub struct BackgroundPolicy {
    pub profile: &'static str,
    pub advanced: bool,
    pub values: Value,
    pub overrides: Value,
}
impl BackgroundPolicy {
    pub fn read(settings: &t3_contracts::settings::ServerSettings) -> Self {
        use t3_contracts::settings::BackgroundActivityProfile;
        use t3_shared::background_activity::{
            ResolvedBackgroundActivitySettings as Resolved, preset,
            resolve_server_background_activity_settings,
        };
        fn fields(p: &Resolved) -> Value {
            let ms = |d: std::time::Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
            serde_json::json!({
                "automaticGitFetchInterval":ms(p.automatic_git_fetch_interval),
                "providerHealthRefreshInterval":ms(p.provider_health_refresh_interval),
                "hostPowerMonitorActiveInterval":ms(p.host_power_monitor_active_interval),
                "hostPowerMonitorIdleInterval":ms(p.host_power_monitor_idle_interval),
                "pauseWhenHostLocked":p.pause_when_host_locked,"pauseWhenHostLowPower":p.pause_when_host_low_power,
                "pauseWhenClientLowPower":p.pause_when_client_low_power,"pauseWhenOnBattery":p.pause_when_on_battery
            })
        }
        let policy = resolve_server_background_activity_settings(settings);
        let defaults = fields(&preset(&policy.profile));
        let values = fields(&policy);
        let mut overrides = values.clone();
        if let Some(map) = overrides.as_object_mut() {
            map.retain(|key, value| &defaults[key] != value);
        }
        Self {
            profile: match policy.profile {
                BackgroundActivityProfile::Balanced => "balanced",
                BackgroundActivityProfile::Performance => "performance",
                BackgroundActivityProfile::BatterySaver => "battery-saver",
            },
            advanced: values != defaults,
            values,
            overrides,
        }
    }
}
