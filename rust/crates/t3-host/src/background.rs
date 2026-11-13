use crate::{Error, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use std::sync::Arc;
use t3_contracts::{
    background::*,
    base_schemas::{AuthSessionId, RpcClientId},
    settings::{BackgroundActivityProfile, BackgroundActivityProfileSelection, ServerSettings},
    wire::WireDateTime,
};
use tokio::sync::{Mutex, watch};

pub const MAX_CLIENT_ACTIVITY_LEASES_PER_RPC_CLIENT: usize = 16;

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        Utc::now().timestamp_millis()
    }
}
fn iso(ms: i64) -> Result<WireDateTime> {
    DateTime::from_timestamp_millis(ms)
        .map(|d| WireDateTime(d.to_rfc3339_opts(SecondsFormat::Millis, true)))
        .ok_or(Error::Timestamp)
}
fn millis(date: &WireDateTime) -> Result<i64> {
    DateTime::parse_from_rfc3339(&date.0)
        .map(|d| d.timestamp_millis())
        .map_err(|_| Error::Timestamp)
}
pub fn scope_key(scope: &BackgroundScope) -> String {
    match scope {
        BackgroundScope::ServerConfig => "server-config".into(),
        BackgroundScope::Diagnostics => "diagnostics".into(),
        BackgroundScope::ProviderStatus { instance_id } => instance_id
            .as_ref()
            .filter(|id| !id.0.is_empty())
            .map_or_else(
                || "provider-status".into(),
                |id| format!("provider-status:{}", id.0),
            ),
        BackgroundScope::VcsStatus { cwd } => format!("vcs-status:{cwd}"),
        BackgroundScope::GitRefs { cwd } => format!("git-refs:{cwd}"),
        BackgroundScope::Thread { thread_id } => format!("thread:{}", thread_id.0),
    }
}

#[derive(Debug, Clone)]
pub struct ActivitySettings {
    pub performance: bool,
    pub pause_when_host_locked: bool,
    pub pause_when_host_low_power: bool,
    pub pause_when_client_low_power: bool,
    pub pause_when_on_battery: bool,
}
impl ActivitySettings {
    pub fn resolve(settings: &ServerSettings) -> Self {
        use BackgroundActivityProfile as P;
        use BackgroundActivityProfileSelection as S;
        let activity = &settings.background_activity;
        let legacy = activity.profile == S::Balanced
            && activity.base_profile.is_none()
            && t3_contracts::stored::to_value(&activity.overrides)
                .is_ok_and(|v| v.as_object().is_some_and(|m| m.is_empty()))
            && (settings.background_activity_profile != P::Balanced
                || settings.automatic_git_fetch_interval
                    != t3_contracts::settings::WireNumber::Finite(30_000_u32.into())
                || settings.provider_health_refresh_interval
                    != t3_contracts::settings::WireNumber::Finite(300_000_u32.into()));
        let profile = if legacy {
            settings.background_activity_profile.clone()
        } else {
            match activity.profile {
                S::Performance => P::Performance,
                S::BatterySaver => P::BatterySaver,
                S::Balanced => P::Balanced,
                S::Custom => activity.base_profile.clone().unwrap_or(P::Balanced),
            }
        };
        let mut result = Self {
            performance: profile == P::Performance,
            pause_when_host_locked: true,
            pause_when_host_low_power: profile != P::Performance,
            pause_when_client_low_power: profile != P::Performance,
            pause_when_on_battery: profile == P::BatterySaver,
        };
        if activity.profile == S::Custom && !legacy {
            let o = &activity.overrides;
            result.pause_when_host_locked = o
                .pause_when_host_locked
                .unwrap_or(result.pause_when_host_locked);
            result.pause_when_host_low_power = o
                .pause_when_host_low_power
                .unwrap_or(result.pause_when_host_low_power);
            result.pause_when_client_low_power = o
                .pause_when_client_low_power
                .unwrap_or(result.pause_when_client_low_power);
            result.pause_when_on_battery = o
                .pause_when_on_battery
                .unwrap_or(result.pause_when_on_battery);
        }
        result
    }
    fn host_constrained(&self, power: &HostPowerSnapshot) -> bool {
        !power.stale
            && (power.suspended
                || self.pause_when_host_locked && power.locked == BackgroundBooleanState::True
                || matches!(
                    power.thermal_state,
                    HostPowerThermalState::Serious | HostPowerThermalState::Critical
                )
                || self.pause_when_host_low_power
                    && power.low_power_mode == BackgroundBooleanState::True
                || self.pause_when_on_battery && power.on_battery == BackgroundBooleanState::True)
    }
    fn client_constrained(&self, lease: &ClientActivityLease) -> bool {
        self.pause_when_client_low_power
            && lease.low_power_mode == Some(BackgroundBooleanState::True)
    }
}

pub struct HostPowerMonitor {
    current: watch::Sender<HostPowerSnapshot>,
    gate: Mutex<()>,
}
impl HostPowerMonitor {
    pub fn new(initial: HostPowerSnapshot) -> Self {
        Self {
            current: watch::channel(initial).0,
            gate: Mutex::new(()),
        }
    }
    pub fn unknown(now_ms: i64) -> Result<Self> {
        Ok(Self::new(HostPowerSnapshot {
            source: HostPowerSource::Unknown,
            idle: BackgroundBooleanState::Unknown,
            idle_seconds: None,
            locked: BackgroundBooleanState::Unknown,
            suspended: false,
            on_battery: BackgroundBooleanState::Unknown,
            low_power_mode: BackgroundBooleanState::Unknown,
            thermal_state: HostPowerThermalState::Unknown,
            stale: true,
            updated_at: iso(now_ms)?,
        }))
    }
    pub fn snapshot(&self) -> HostPowerSnapshot {
        self.current.borrow().clone()
    }
    pub fn stream_changes(&self) -> watch::Receiver<HostPowerSnapshot> {
        self.current.subscribe()
    }
    pub async fn report(&self, next: HostPowerSnapshot) -> Result<()> {
        let _guard = self.gate.lock().await;
        let mut current = self.current.borrow().clone();
        if millis(&next.updated_at)? < millis(&current.updated_at)? {
            return Ok(());
        }
        current.updated_at = next.updated_at.clone();
        current.idle_seconds = next.idle_seconds.clone();
        if current == next {
            self.current.send_if_modified(|stored| {
                *stored = next;
                false
            });
        } else {
            self.current.send_replace(next);
        }
        Ok(())
    }
}

struct Lease {
    value: ClientActivityLease,
    updated_ms: i64,
    expires_ms: i64,
}
pub struct BackgroundPolicy {
    clock: Arc<dyn Clock>,
    power: Arc<HostPowerMonitor>,
    leases: Mutex<Vec<Lease>>,
}
impl BackgroundPolicy {
    pub fn new(clock: Arc<dyn Clock>, power: Arc<HostPowerMonitor>) -> Self {
        Self {
            clock,
            power,
            leases: Mutex::new(Vec::new()),
        }
    }
    pub async fn report_client_activity(
        &self,
        session_id: AuthSessionId,
        rpc_client_id: RpcClientId,
        input: ClientActivityReportInput,
    ) -> Result<()> {
        let now = self.clock.now_ms();
        let ttl = match input.ttl_ms.as_ref() {
            None => 45_000.0,
            Some(t3_contracts::speech::SpeechNumber::Finite(v)) => v.value(),
            Some(_) => return Err(Error::Invalid),
        }
        .clamp(1_000.0, 120_000.0);
        let expires = now.checked_add(ttl as i64).ok_or(Error::Timestamp)?;
        let value = ClientActivityLease {
            session_id,
            rpc_client_id,
            client_id: input.client_id,
            client_kind: input.client_kind,
            visible: input.visible,
            focused: input.focused,
            recently_interacted: input.recently_interacted,
            app_state: input.app_state,
            low_power_mode: input.low_power_mode,
            scopes: input.scopes,
            updated_at: iso(now)?,
            expires_at: iso(expires)?,
        };
        let mut leases = self.leases.lock().await;
        leases.retain(|l| l.expires_ms > now);
        if let Some(existing) = leases.iter_mut().find(|l| {
            l.value.session_id == value.session_id
                && l.value.rpc_client_id == value.rpc_client_id
                && l.value.client_id == value.client_id
        }) {
            *existing = Lease {
                value,
                updated_ms: now,
                expires_ms: expires,
            };
            return Ok(());
        }
        let matching: Vec<_> = leases
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.value.session_id == value.session_id
                    && l.value.rpc_client_id == value.rpc_client_id
            })
            .map(|(i, l)| (i, l.updated_ms))
            .collect();
        if matching.len() >= MAX_CLIENT_ACTIVITY_LEASES_PER_RPC_CLIENT
            && let Some((i, _)) = matching.into_iter().min_by_key(|(i, time)| (*time, *i))
        {
            leases.remove(i);
        }
        leases.push(Lease {
            value,
            updated_ms: now,
            expires_ms: expires,
        });
        Ok(())
    }
    pub async fn remove_rpc_client(&self, session: &AuthSessionId, client: &RpcClientId) {
        self.leases
            .lock()
            .await
            .retain(|l| &l.value.session_id != session || &l.value.rpc_client_id != client);
    }
    pub async fn snapshot(&self, settings: &ActivitySettings) -> Result<BackgroundPolicySnapshot> {
        let now = self.clock.now_ms();
        let mut leases = self.leases.lock().await;
        leases.retain(|l| l.expires_ms > now);
        let active: Vec<_> = leases.iter().map(|l| l.value.clone()).collect();
        let foreground: Vec<_> = active
            .iter()
            .filter(|l| l.visible && (l.focused || l.recently_interacted))
            .collect();
        let power = self.power.snapshot();
        let scopes: std::collections::BTreeSet<_> = active
            .iter()
            .flat_map(|l| l.scopes.iter().map(scope_key))
            .collect();
        Ok(BackgroundPolicySnapshot {
            should_run_opportunistic_work: foreground
                .iter()
                .any(|l| !settings.client_constrained(l))
                && !settings.host_constrained(&power),
            active_foreground_lease_count: t3_contracts::speech::SpeechNumber::Finite(
                foreground.len().into(),
            ),
            host_power: power,
            leases: active,
            active_scope_keys: scopes.into_iter().collect(),
            updated_at: iso(now)?,
        })
    }
    pub async fn should_run_scope_work(
        &self,
        scope: &BackgroundScope,
        settings: &ActivitySettings,
    ) -> Result<bool> {
        let snapshot = self.snapshot(settings).await?;
        Ok(!settings.host_constrained(&snapshot.host_power)
            && snapshot.leases.iter().any(|l| {
                !settings.client_constrained(l)
                    && l.scopes.iter().any(|s| scope_key(s) == scope_key(scope))
                    && (settings.performance || l.visible && (l.focused || l.recently_interacted))
            }))
    }
    pub async fn should_run_opportunistic_work(&self, settings: &ActivitySettings) -> Result<bool> {
        Ok(self.snapshot(settings).await?.should_run_opportunistic_work)
    }
}
