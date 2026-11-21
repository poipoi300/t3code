//! Device settings commands and per-environment source-list edits from DeviceSourcesSettings.
use super::{scope::SettingsScopeSearch, scoped::SettingsPatch, service::SettingsWriteAccess};
use crate::{
    ClientError,
    composition::ClientRuntime,
    navigation::SettingsPage,
    platform::Platform,
    rpc::{RpcEndpoint, SessionEndpoint},
    types::EnvironmentId,
};
use futures::{FutureExt, future::LocalBoxFuture, stream::LocalBoxStream};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::{auth::AuthEnvironmentScope, device::*, rpc::*};

#[derive(Clone)]
pub struct DeviceSession(pub SessionEndpoint);
impl DeviceSession {
    pub fn state(&self) -> LocalBoxStream<'static, Result<DeviceServiceState, ClientError>> {
        self.0.subscribe::<WsSubscribeDeviceStateRpc>(Empty {})
    }
    pub fn writable(&self) -> bool {
        self.0.grants_scope(AuthEnvironmentScope::SettingsWrite) == Some(true)
    }
    pub async fn configure(
        &self,
        hub: Option<bool>,
        agent: Option<bool>,
    ) -> Result<DeviceServiceState, ClientError> {
        if !self.writable() {
            return Err(ClientError::PermissionDenied {
                method: "device.configure".into(),
            });
        }
        self.0
            .call::<WsDeviceConfigureRpc>(DeviceConfigureInput {
                enabled: hub.map(Some),
                agent_access_enabled: agent.map(Some),
                onboarding_completed: (hub == Some(true)).then_some(Some(true)),
            })
            .await
    }
    pub async fn list(&self, input: DeviceListInput) -> Result<DeviceServiceState, ClientError> {
        self.0.call::<WsDeviceListRpc>(input).await
    }
    pub async fn machine_gateway(&self) -> Result<String, ClientError> {
        if !self.writable() {
            return Err(ClientError::PermissionDenied {
                method: "device.simlockIdentity".into(),
            });
        }
        let identity = self
            .0
            .call::<WsDeviceSimlockIdentityRpc>(DeviceSimlockIdentityInput {
                gateway_url: None,
                host_id: None,
                agent_token: None,
            })
            .await?;
        Ok(identity.gateway_url)
    }
    pub async fn check(
        &self,
        kind: HostKind,
        draft: &Value,
        token: Option<&Value>,
        saved_id: Option<&str>,
    ) -> Result<HostCheck, ClientError> {
        if !self.writable() {
            return Err(ClientError::PermissionDenied {
                method: "device host check".into(),
            });
        }
        let mut input = json!({});
        if token.is_none()
            && let Some(id) = saved_id
        {
            input["hostId"] = json!(id);
        }
        match kind {
            HostKind::Ssh => {
                let host = self
                    .0
                    .call::<WsDeviceTestHostRpc>(serde_json::from_value(draft.clone())?)
                    .await?;
                Ok(HostCheck::Ssh(host))
            }
            HostKind::Eas => {
                if let Some(token) = token {
                    input["accessToken"] = token.clone();
                }
                Ok(HostCheck::Eas(
                    self.0
                        .call::<WsDeviceEasAccountRpc>(serde_json::from_value(input)?)
                        .await?,
                ))
            }
            HostKind::Simlock => {
                input["gatewayUrl"] = draft["gatewayUrl"].clone();
                if let Some(token) = token.filter(|value| value.as_str() != Some("")) {
                    input["agentToken"] = token.clone();
                }
                Ok(HostCheck::Simlock(
                    self.0
                        .call::<WsDeviceSimlockIdentityRpc>(serde_json::from_value(input)?)
                        .await?,
                ))
            }
        }
    }
}

pub fn valid_device_token(value: &Value) -> bool {
    if value.is_string() {
        return true;
    }
    let Some(provider) = value["provider"].as_str() else {
        return false;
    };
    let Some(path) = value["reference"].as_str() else {
        return false;
    };
    provider.len() <= 64
        && provider
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && provider
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        && !path.trim().is_empty()
        && path.encode_utf16().count() <= 1024
        && !path.contains(['\r', '\n'])
}

#[derive(Clone, PartialEq)]
pub enum HostCheck {
    Ssh(DeviceHostSummary),
    Eas(DeviceEasAccount),
    Simlock(DeviceSimlockIdentity),
}

pub fn busy(state: &DeviceServiceState) -> bool {
    let mut statuses =
        std::iter::once(&state.host_status).chain(state.host_statuses.values().map(|h| &h.status));
    !statuses.clone().any(|s| *s == DeviceHostStatus::Ready)
        && statuses.any(|s| matches!(s, DeviceHostStatus::Starting | DeviceHostStatus::Installing))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKind {
    Ssh,
    Eas,
    Simlock,
}
impl HostKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Ssh => "deviceHosts",
            Self::Eas => "easDeviceHosts",
            Self::Simlock => "simlockDeviceHosts",
        }
    }
    pub fn token(self) -> Option<&'static str> {
        match self {
            Self::Ssh => None,
            Self::Eas => Some("accessToken"),
            Self::Simlock => Some("agentToken"),
        }
    }
    pub fn new_host(self, id: String) -> Value {
        match self {
            Self::Ssh => json!({"kind":"ssh","id":id,"label":"","target":""}),
            Self::Eas => {
                json!({"kind":"eas","id":id,"label":"EAS Simulator","accessToken":"","maxSessionMinutes":30,"idleMinutes":10,"maxConcurrentSessions":2})
            }
            Self::Simlock => {
                json!({"kind":"simlock","id":id,"label":"Simlock","gatewayUrl":"","agentToken":"","maxLeases":2,"maxLeaseMinutes":60})
            }
        }
    }
    pub fn valid(self, host: &Value) -> bool {
        if host["label"]
            .as_str()
            .is_none_or(|label| label.trim().is_empty())
        {
            return false;
        }
        let within = |key: &str, lo, hi| host[key].as_u64().is_some_and(|n| (lo..=hi).contains(&n));
        match self {
            Self::Ssh => {
                host["target"].as_str().is_some_and(|s| {
                    !s.is_empty() && !s.starts_with('-') && !s.chars().any(char::is_whitespace)
                }) && (host.get("port").is_none() || within("port", 1, 65535))
            }
            Self::Eas => {
                within("maxSessionMinutes", 1, 115)
                    && within("idleMinutes", 1, 60)
                    && within("maxConcurrentSessions", 1, 20)
                    && host["idleMinutes"].as_u64() < host["maxSessionMinutes"].as_u64()
            }
            Self::Simlock => {
                within("maxLeaseMinutes", 5, 240)
                    && within("maxLeases", 1, 20)
                    && host["gatewayUrl"]
                        .as_str()
                        .and_then(|s| {
                            s.strip_prefix("https://")
                                .or_else(|| s.strip_prefix("http://"))
                        })
                        .is_some_and(|s| {
                            let s = s.strip_suffix('/').unwrap_or(s);
                            !s.is_empty() && !s.contains('/') && !s.chars().any(char::is_whitespace)
                        })
            }
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct HostRow {
    pub kind: HostKind,
    pub host: Value,
    pub holders: BTreeMap<EnvironmentId, String>,
    pub separate: bool,
}
/// Conflicting SSH identities stay per machine. Compatible SSH aliases keep the local ID on write.
pub fn host_rows(members: &[(EnvironmentId, Value)], eas: bool, simlock: bool) -> Vec<HostRow> {
    let mut rows: Vec<HostRow> = vec![];
    for kind in [HostKind::Ssh, HostKind::Eas, HostKind::Simlock] {
        if (kind == HostKind::Eas && !eas) || (kind == HostKind::Simlock && !simlock) {
            continue;
        }
        let mut configs = BTreeMap::new();
        let mut targets = BTreeMap::new();
        let mut separate = false;
        let mut hosts = vec![];
        for (env, settings) in members {
            let mut destinations = BTreeSet::new();
            for host in settings[kind.key()].as_array().into_iter().flatten() {
                let id = host["id"].as_str().unwrap_or_default();
                let dest = ssh_destination(host);
                if kind == HostKind::Ssh {
                    let config = json!([host["label"], dest]);
                    separate |= !destinations.insert(dest.to_string())
                        || configs
                            .insert(id, config.clone())
                            .is_some_and(|s| s != config)
                        || targets
                            .insert(host["target"].as_str(), config.clone())
                            .is_some_and(|s| s != config);
                }
                hosts.push((env, id, host));
            }
        }
        for (env, id, host) in hosts {
            let found = rows.iter_mut().find(|r| {
                r.kind == kind
                    && !separate
                    && if kind == HostKind::Ssh {
                        ssh_destination(&r.host) == ssh_destination(host)
                    } else {
                        r.host["id"] == id
                    }
            });
            if let Some(row) = found {
                row.holders.insert(env.clone(), id.to_owned());
            } else {
                rows.push(HostRow {
                    kind,
                    host: host.clone(),
                    holders: BTreeMap::from([(env.clone(), id.to_owned())]),
                    separate,
                });
            }
        }
    }
    rows
}
fn ssh_destination(host: &Value) -> Value {
    json!([host["target"], host["port"], host["identityFile"]])
}

#[derive(Clone)]
pub struct SourceEdit {
    pub kind: HostKind,
    pub draft: Value,
    pub original: Option<HostRow>,
    pub remove: bool,
    /// Absent preserves this machine's credential. Empty explicitly clears it.
    pub tokens: BTreeMap<EnvironmentId, Value>,
}
impl SourceEdit {
    fn patch(&self, env: &EnvironmentId, settings: &Value) -> Result<Option<SettingsPatch>, ()> {
        let field = self.kind.key();
        let mut hosts = settings[field].as_array().cloned().unwrap_or_default();
        let held = self.original.as_ref().and_then(|r| r.holders.get(env));
        if held.is_none() && (self.remove || self.original.as_ref().is_some_and(|r| r.separate)) {
            return Ok(None);
        }
        let id = held
            .map(String::as_str)
            .or_else(|| self.draft["id"].as_str())
            .ok_or(())?;
        let mut pos = hosts.iter().position(|h| h["id"] == id);
        if pos.is_none() && self.kind == HostKind::Ssh && !self.remove {
            for dest in [self.original.as_ref().map(|r| &r.host), Some(&self.draft)]
                .into_iter()
                .flatten()
            {
                let mut matches = hosts
                    .iter()
                    .enumerate()
                    .filter(|(_, h)| ssh_destination(h) == ssh_destination(dest));
                pos = matches.next().map(|(i, _)| i);
                if matches.next().is_some() {
                    return Err(());
                }
                if pos.is_some() {
                    break;
                }
            }
        }
        if self.remove {
            if let Some(i) = pos {
                hosts.remove(i);
            } else {
                return Ok(None);
            }
        } else {
            let mut next = self.draft.clone();
            if let Some(key) = self.kind.token() {
                next[key] = self
                    .tokens
                    .get(env)
                    .cloned()
                    .or_else(|| pos.map(|i| hosts[i][key].clone()))
                    .unwrap_or(json!(""));
            }
            if let Some(i) = pos {
                next["id"] = hosts[i]["id"].clone();
                hosts[i] = next;
            } else {
                hosts.push(next);
            }
        }
        Ok(Some(SettingsPatch::from_iter([(
            field.to_owned(),
            Value::Array(hosts),
        )])))
    }
}

type SaveSources =
    dyn Fn(Vec<EnvironmentId>, SourceEdit) -> LocalBoxFuture<'static, Result<(), Vec<String>>>;
#[derive(Clone)]
pub struct SourceWriter(Rc<SaveSources>);
impl SourceWriter {
    pub fn new<P: Platform + Clone + 'static>(runtime: &ClientRuntime<P>) -> Self {
        let settings = runtime.settings();
        Self(Rc::new(move |ids, edit| {
            let settings = settings.clone();
            async move {
                if !edit.remove
                    && (!edit.kind.valid(&edit.draft)
                        || !edit.tokens.values().all(valid_device_token))
                {
                    return Err(vec![]);
                }
                let mut plans = vec![];
                let mut denied = vec![];
                for id in ids {
                    let scope = settings.scope_view(
                        SettingsPage::Devices,
                        &SettingsScopeSearch {
                            machine: Some(id.0.clone()),
                            ..Default::default()
                        },
                    );
                    let Some(target) = scope.targets.iter().find(|t| t.environment_id == id) else {
                        denied.push(id.0);
                        continue;
                    };
                    let patch = edit.patch(&id, &target.json);
                    if matches!(patch, Ok(None)) {
                        continue;
                    }
                    if !matches!(settings.write_access(&scope), SettingsWriteAccess::Writable) {
                        denied.push(target.label.clone());
                        continue;
                    }
                    match patch {
                        Ok(Some(patch)) => plans.push((target.label.clone(), scope, patch)),
                        _ => denied.push(target.label.clone()),
                    }
                }
                if !denied.is_empty() {
                    return Err(denied);
                }
                let outcomes =
                    futures::future::join_all(plans.into_iter().map(|(label, scope, patch)| {
                        let save = settings.update(&scope, patch);
                        async move { save.await.failed.then_some(label) }
                    }))
                    .await;
                let failed: Vec<_> = outcomes.into_iter().flatten().collect();
                if failed.is_empty() {
                    Ok(())
                } else {
                    Err(failed)
                }
            }
            .boxed_local()
        }))
    }
    pub fn save(
        &self,
        ids: Vec<EnvironmentId>,
        edit: SourceEdit,
    ) -> LocalBoxFuture<'static, Result<(), Vec<String>>> {
        (self.0)(ids, edit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn simlock_gateway_requires_an_http_origin() {
        let mut host = HostKind::Simlock.new_host("pool".into());
        for url in ["https://pool.test", "http://pool.test:8080/"] {
            host["gatewayUrl"] = json!(url);
            assert!(HostKind::Simlock.valid(&host), "{url}");
        }
        for url in [
            "",
            "pool.test",
            "ftp://pool.test",
            "https://",
            "https:///",
            "https://two hosts",
            "https://pool.test/leases",
        ] {
            host["gatewayUrl"] = json!(url);
            assert!(!HostKind::Simlock.valid(&host), "{url}");
        }
    }

    #[test]
    fn ssh_target_and_port_reject_invalid_values_independently() {
        let mut host = json!({"label":"Mac", "target":"user@mac"});
        assert!(HostKind::Ssh.valid(&host));
        for target in ["", "-option", "two hosts", "host\nname"] {
            host["target"] = json!(target);
            assert!(!HostKind::Ssh.valid(&host), "{target:?}");
        }
        host["target"] = json!("user@mac");
        for port in [1, 65535] {
            host["port"] = json!(port);
            assert!(HostKind::Ssh.valid(&host), "{port}");
        }
        for port in [json!(0), json!(65536), json!(-1), json!(2.5), json!("22")] {
            host["port"] = port;
            assert!(!HostKind::Ssh.valid(&host), "{}", host["port"]);
        }
    }

    #[test]
    fn device_token_references_validate_provider_and_utf16_path() {
        let reference = |provider: &str, path: &str| json!({"provider":provider, "reference":path});
        assert!(valid_device_token(&json!("")));
        let max_provider = "a".repeat(64);
        let max_path = "🤖".repeat(512);
        assert!(valid_device_token(&reference(&max_provider, &max_path)));
        for invalid in [
            json!(false),
            json!({"provider":"vault"}),
            json!({"reference":"field"}),
            reference("", "field"),
            reference("_vault", "field"),
            reference("bad.id", "field"),
            reference(&format!("{max_provider}a"), "field"),
            reference("vault", " "),
            reference("vault", "line\rbreak"),
            reference("vault", "line\nbreak"),
            reference("vault", &format!("{max_path}x")),
        ] {
            assert!(!valid_device_token(&invalid), "{invalid}");
        }
    }
    #[test]
    fn denied_source_member_prevents_every_write() {
        use crate::{ClientStore, composition::Spawner, testing::environment::FakeEnvironment};
        use futures::{executor::LocalPool, task::LocalSpawnExt};
        use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

        let mut pool = LocalPool::new();
        let spawner = pool.spawner();
        let spawn: Spawner =
            Rc::new(move |task| spawner.spawn_local(task).expect("spawn client task"));
        let first = FakeEnvironment::new("allowed", spawn.clone());
        let other = FakeEnvironment::at_origin("denied", "https://other.test", spawn.clone());
        first.set_permissions(&["orchestration:read", "settings:write"]);
        other.set_permissions(&["orchestration:read"]);
        let client = ClientRuntime::new(
            first.platform().with_environment(&other),
            ClientStore::default(),
            OrchestrationV2CreationSource::Web,
            spawn,
        );
        pool.run_until(client.pair(first.pairing()))
            .expect("pair writable member");
        pool.run_until(client.pair(other.pairing()))
            .expect("pair restricted member");
        pool.run_until_stalled();
        let edit = SourceEdit {
            kind: HostKind::Eas,
            draft: HostKind::Eas.new_host("expo".into()),
            original: None,
            remove: false,
            tokens: BTreeMap::new(),
        };
        let writer = SourceWriter::new(&client);
        assert!(
            pool.run_until(writer.save(
                vec![
                    EnvironmentId("allowed".into()),
                    EnvironmentId("denied".into())
                ],
                edit.clone()
            ))
            .is_err()
        );
        assert!(first.calls("server.updateSettings").is_empty());
        assert!(other.calls("server.updateSettings").is_empty());
        let _ = pool.run_until(writer.save(vec![EnvironmentId("allowed".into())], edit));
        assert_eq!(first.calls("server.updateSettings").len(), 1);
    }
    #[test]
    fn unchanged_cloud_credentials_resolve_on_each_member() {
        let env_a = EnvironmentId("first".into());
        let env_b = EnvironmentId("second".into());
        let env_c = EnvironmentId("new".into());
        for kind in [HostKind::Eas, HostKind::Simlock] {
            let key = kind.token().unwrap();
            let mut a = kind.new_host("pool".into());
            a[key] = json!({"provider":"first","reference":"first/path"});
            let mut b = a.clone();
            b[key] = json!({"provider":"other","reference":"second/path"});
            let configs = vec![
                (env_a.clone(), json!({kind.key():[a]})),
                (env_b.clone(), json!({kind.key():[b]})),
                (env_c.clone(), json!({kind.key():[]})),
            ];
            let row = host_rows(&configs, true, true).pop().unwrap();
            let edit = SourceEdit {
                kind,
                draft: row.host.clone(),
                original: Some(row),
                remove: false,
                tokens: BTreeMap::new(),
            };
            for ((env, config), expected) in
                configs
                    .iter()
                    .zip([a[key].clone(), b[key].clone(), json!("")])
            {
                let patch = edit.patch(env, config).unwrap().unwrap();
                assert_eq!(patch[kind.key()][0][key], expected);
            }
        }
    }
    #[test]
    fn conflicting_ssh_aliases_stay_machine_specific() {
        let a = EnvironmentId("a".into());
        let b = EnvironmentId("b".into());
        let mut ssh = HostKind::Ssh.new_host("host-a".into());
        ssh["label"] = json!("Mac");
        ssh["target"] = json!("build");
        let mut other = ssh.clone();
        other["id"] = json!("host-b");
        let configs = vec![
            (a.clone(), json!({"deviceHosts":[ssh]})),
            (b.clone(), json!({"deviceHosts":[other]})),
        ];
        let grouped = host_rows(&configs, false, false);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].holders[&b], "host-b");
        let mut conflict = configs.clone();
        conflict[1].1["deviceHosts"][0]["port"] = json!(2222);
        let groups = host_rows(&conflict, false, false);
        assert!(groups.len() == 2 && groups.iter().all(|r| r.separate));
        let edit = SourceEdit {
            kind: HostKind::Ssh,
            draft: groups[0].host.clone(),
            original: Some(groups[0].clone()),
            remove: true,
            tokens: BTreeMap::new(),
        };
        assert!(edit.patch(&b, &conflict[1].1).unwrap().is_none());
    }
}
