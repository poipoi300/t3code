use crate::{Error, Result};
use serde_json::{Value, json};
use std::{net::IpAddr, path::Path};
use t3_contracts::{
    auth::ServerAuthPolicy, managed_policy::*, provider_policy::RuntimeMode,
    settings::ServerSettings,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "_tag")]
pub struct ManagedPolicyRefusedError {
    pub organization: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
impl std::fmt::Display for ManagedPolicyRefusedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}'s policy does not allow {}",
            self.organization, self.action
        )?;
        if let Some(detail) = &self.detail {
            write!(f, ", {detail}")?;
        }
        f.write_str(".")
    }
}
impl std::error::Error for ManagedPolicyRefusedError {}

pub struct ManagedPolicy {
    pub document: Option<ManagedPolicyDocument>,
    pub providers: t3_provider::policy::ManagedProviders,
    settings: Vec<(Vec<String>, Value, bool)>,
}
impl ManagedPolicy {
    fn refuse(&self, action: &str, detail: Option<String>) -> Error {
        ManagedPolicyRefusedError {
            organization: self
                .document
                .as_ref()
                .map_or("Your organization", |d| d.organization.0.as_str())
                .into(),
            action: action.into(),
            detail,
        }
        .into()
    }
    pub fn load(path: &Path) -> Result<Self> {
        let policy = t3_config::policy::ManagedPolicy::load_file(path)
            .map_err(|_| Error::Refused("managed policy could not be read or decoded"))?;
        Self::from_document(policy.document)
    }
    pub fn from_document(document: Option<ManagedPolicyDocument>) -> Result<Self> {
        let built_in = t3_contracts::stored::to_value(&ServerSettings::default())
            .map_err(|_| Error::Invalid)?;
        let mut applied = built_in.clone();
        let mut entries = Vec::new();
        for (path, setting) in document
            .as_ref()
            .and_then(|d| d.settings.as_ref())
            .into_iter()
            .flatten()
        {
            let segments: Vec<_> = path.split('.').map(str::to_owned).collect();
            let head = &segments[0];
            if segments.iter().any(String::is_empty)
                || built_in.get(head).is_none()
                || [
                    "projectSettingsOverrides",
                    "projectAgentBrowserAccessOverrides",
                    "projectAutoPullOverrides",
                    "projectScriptOverrides",
                    "observability",
                    "compactIdleThreads",
                ]
                .contains(&head.as_str())
            {
                return Err(Error::Refused("policy names an unmanageable setting"));
            }
            if setting.locked && head == "providerInstances" && segments.len() > 2 {
                return Err(Error::Refused(
                    "policy must lock the whole provider instance",
                ));
            }
            if setting.locked
                && (path.starts_with("storageCleanup.worktree") || path == "storageCleanup")
                && !document
                    .as_ref()
                    .and_then(|d| d.settings.as_ref())
                    .is_some_and(|s| s.get("worktreeCleanup").is_some_and(|s| s.locked))
            {
                return Err(Error::Refused(
                    "worktree cleanup overrides a managed setting",
                ));
            }
            for length in 1..segments.len() {
                let mut nullable = built_in.clone();
                set(&mut nullable, &segments[..length], Some(Value::Null));
                if serde_json::from_value::<ServerSettings>(nullable).is_ok() {
                    return Err(Error::Refused(
                        "policy setting is inside a nullable setting",
                    ));
                }
            }
            if !setting.locked
                && (get(&built_in, &segments).is_none()
                    || adds_keys(get(&built_in, &segments), &setting.value))
            {
                return Err(Error::Refused(
                    "policy default adds entries users cannot remove",
                ));
            }
            set(&mut applied, &segments, Some(setting.value.clone()));
            entries.push((segments, setting.value.clone(), setting.locked));
        }
        let decoded: ServerSettings = serde_json::from_value(applied.clone())
            .map_err(|_| Error::Refused("managed setting has the wrong type"))?;
        let encoded = t3_contracts::stored::to_value(&decoded).map_err(|_| Error::Invalid)?;
        for (path, value, _) in &entries {
            if get(&encoded, path) != Some(value) {
                return Err(Error::Refused(
                    "managed setting does not survive validation",
                ));
            }
        }
        let mut redacted = applied.clone();
        t3_config::secrets::redact_for_client(&mut redacted, true);
        if changed_secret(&applied, &redacted) {
            return Err(Error::Refused("managed setting contains a credential"));
        }
        if let Some(ceiling) = document
            .as_ref()
            .and_then(|d| d.limits.as_ref())
            .and_then(|l| l.max_runtime_mode.as_ref())
            && entries.iter().any(|(p, _, _)| p == &["defaultRuntimeMode"])
            && mode_rank(&decoded.default_runtime_mode) > mode_rank(ceiling)
        {
            return Err(Error::Refused("managed default exceeds runtime ceiling"));
        }
        let limits = document.as_ref().and_then(|d| d.limits.as_ref());
        let empty: ManagedProviderLimits =
            serde_json::from_value(json!({})).map_err(|_| Error::Invalid)?;
        let providers = t3_provider::policy::ManagedProviders::new(
            limits.and_then(|l| l.providers.as_ref()).unwrap_or(&empty),
            limits.and_then(|l| l.max_runtime_mode),
            t3_provider::environment::Platform::current(),
        )
        .map_err(|_| Error::Refused("managed provider limits cannot be enforced"))?;
        Ok(Self {
            document,
            providers,
            settings: entries,
        })
    }
    pub fn allows_auth_policy(&self, auth: &ServerAuthPolicy) -> bool {
        let Some(document) = &self.document else {
            return true;
        };
        if *auth == ServerAuthPolicy::UnsafeNoAuth {
            return false;
        }
        document
            .limits
            .as_ref()
            .and_then(|l| l.clients.as_ref())
            .is_none_or(|clients| {
                clients.iter().any(|client| {
                    matches!(
                        (client, auth),
                        (
                            ManagedPolicyClient::Desktop,
                            ServerAuthPolicy::DesktopManagedLocal
                        ) | (
                            ManagedPolicyClient::LocalBrowser,
                            ServerAuthPolicy::LoopbackBrowser
                        ) | (
                            ManagedPolicyClient::Remote,
                            ServerAuthPolicy::RemoteReachable
                        )
                    )
                })
            })
    }
    pub fn allows_remote_clients(&self) -> bool {
        self.allows_auth_policy(&ServerAuthPolicy::RemoteReachable)
    }
    pub fn allows_connect(&self) -> bool {
        self.allows_remote_clients()
            && self
                .document
                .as_ref()
                .and_then(|d| d.limits.as_ref())
                .and_then(|l| l.connect)
                != Some(false)
    }
    pub fn check_server_reachability(
        &self,
        auth: ServerAuthPolicy,
        host: Option<&str>,
        tailscale_serve: bool,
    ) -> Result<String> {
        let cannot_listen = || {
            self.refuse(
                "remote clients",
                Some(format!(
                    "so the server cannot listen on {}",
                    if host == Some("") {
                        "every interface"
                    } else {
                        host.unwrap_or("this host")
                    }
                )),
            )
        };
        if !self.allows_auth_policy(&auth) {
            return Err(match auth {
                ServerAuthPolicy::RemoteReachable => cannot_listen(),
                ServerAuthPolicy::DesktopManagedLocal => self.refuse("the desktop app", None),
                ServerAuthPolicy::LoopbackBrowser => self.refuse("browsers on this machine", None),
                ServerAuthPolicy::UnsafeNoAuth => {
                    self.refuse("running without authentication", None)
                }
            });
        }
        if self.allows_remote_clients() {
            return Ok(host.unwrap_or("127.0.0.1").into());
        }
        if tailscale_serve {
            return Err(self.refuse(
                "remote clients",
                Some("so Tailscale Serve cannot be turned on".into()),
            ));
        }
        let host = host.unwrap_or("127.0.0.1");
        if host.eq_ignore_ascii_case("localhost") {
            return Ok("127.0.0.1".into());
        }
        let ip = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .map_err(|_| cannot_listen())?;
        if !ip.is_loopback() {
            return Err(cannot_listen());
        }
        Ok(ip.to_string())
    }
    pub fn denied_by(&self, capability: &str) -> Option<&str> {
        let d = self.document.as_ref()?;
        let l = d.limits.as_ref()?;
        let value = match capability {
            "webhookTasks" => l.webhook_tasks,
            "telemetry" => l.telemetry,
            _ => None,
        };
        (value == Some(false)).then_some(d.organization.0.as_str())
    }
    pub fn update_mirror(&self) -> Option<(&str, &str)> {
        let d = self.document.as_ref()?;
        Some((
            &d.organization.0,
            &d.limits.as_ref()?.updates.as_ref()?.feed_url.as_ref()?.0,
        ))
    }
    pub fn check_update(&self, target_channel: Option<&str>) -> Result<()> {
        if let Some(updates) = self
            .document
            .as_ref()
            .and_then(|d| d.limits.as_ref())
            .and_then(|l| l.updates.as_ref())
        {
            if updates.mode == ManagedUpdatesMode::Off {
                return Err(Error::ManagedUpdateRefused(format!(
                    "{} manages T3 Code updates.",
                    self.document
                        .as_ref()
                        .map_or("Your organization", |d| d.organization.0.as_str())
                )));
            }
            if updates.allow_nightly == Some(false) && target_channel.is_some_and(|c| c != "stable")
            {
                return Err(Error::ManagedUpdateRefused(format!(
                    "{} manages T3 Code updates. Only stable releases can be installed.",
                    self.document
                        .as_ref()
                        .map_or("Your organization", |d| d.organization.0.as_str())
                )));
            }
        }
        Ok(())
    }
    pub fn with_defaults(&self, persisted: &Value) -> Value {
        let mut tree = json!({});
        for (path, value, locked) in &self.settings {
            if !locked
                && !(path[0] == "idleCompaction"
                    && persisted.get("compactIdleThreads").is_some()
                    && persisted.get("idleCompaction").is_none())
            {
                set(&mut tree, path, Some(value.clone()));
            }
        }
        fill_missing(persisted.clone(), &tree)
    }
    pub fn apply_locks(&self, current: &ServerSettings) -> Result<ServerSettings> {
        let mut value = t3_contracts::stored::to_value(current).map_err(|_| Error::Invalid)?;
        for (path, lock, locked) in &self.settings {
            if *locked {
                for target in targets(&value, path) {
                    set(&mut value, &target, Some(lock.clone()));
                }
            }
        }
        if self
            .settings
            .iter()
            .any(|(path, _, locked)| *locked && path == &["idleCompaction"])
        {
            sync_idle(&mut value);
        }
        if let Some(ceiling) = self
            .document
            .as_ref()
            .and_then(|d| d.limits.as_ref())
            .and_then(|l| l.max_runtime_mode.as_ref())
        {
            let ceiling = t3_contracts::stored::to_value(ceiling).map_err(|_| Error::Invalid)?;
            clamp(&mut value, &ceiling);
            if let Some(projects) = value
                .get_mut("projectSettingsOverrides")
                .and_then(Value::as_object_mut)
            {
                for overrides in projects.values_mut() {
                    clamp(overrides, &ceiling);
                }
            }
        }
        serde_json::from_value(value)
            .map_err(|_| Error::Refused("managed settings cannot be applied"))
    }
    pub fn vet_write(
        &self,
        current: &ServerSettings,
        next: &ServerSettings,
        patch: &Value,
    ) -> Result<ServerSettings> {
        let before = t3_contracts::stored::to_value(current).map_err(|_| Error::Invalid)?;
        let mut after = t3_contracts::stored::to_value(next).map_err(|_| Error::Invalid)?;
        for (path, lock, locked) in &self.settings {
            if !locked {
                continue;
            }
            for target in targets(patch, path) {
                if let Some(value) = get(patch, &target)
                    && !matches_lock(value, lock)
                {
                    return Err(Error::Refused("managed setting is locked"));
                }
            }
            for target in targets(&after, path) {
                if let Some(value) = get(&after, &target)
                    && Some(value) != get(&before, &target)
                    && !matches_lock(value, lock)
                {
                    return Err(Error::Refused("managed setting is locked"));
                }
            }
            for target in targets(&after, path) {
                if (1..target.len()).all(|n| get(&before, &target[..n]).is_some()) {
                    set(&mut after, &target, get(&before, &target).cloned());
                }
            }
        }
        serde_json::from_value(after).map_err(|_| Error::Invalid)
    }
    pub fn locked_paths(&self) -> Vec<String> {
        self.settings
            .iter()
            .filter(|(_, _, locked)| *locked)
            .map(|(p, _, _)| p.join("."))
            .collect()
    }
    pub fn effective_settings(&self, current: &ServerSettings) -> Result<ServerSettings> {
        Ok(self.providers.apply_limits(&self.apply_locks(current)?))
    }
    pub fn vet_settings_write(
        &self,
        current: &ServerSettings,
        next: &ServerSettings,
        patch: &Value,
    ) -> Result<ServerSettings> {
        self.providers
            .vet_write(current, &self.vet_write(current, next, patch)?)
            .map_err(|_| Error::Refused("managed provider setting is locked or disallowed"))
    }
    pub fn summary(&self) -> Option<ManagedPolicySummary> {
        let document = self.document.as_ref()?;
        let fields = self.providers.locked_instance_fields();
        Some(ManagedPolicySummary {
            organization: document.organization.clone(),
            support_url: document.support_url.clone(),
            locked_settings: self.locked_paths(),
            limits: document
                .limits
                .as_ref()
                .map(|limits| ManagedPolicySummaryLimits {
                    max_runtime_mode: limits.max_runtime_mode,
                    clients: limits.clients.clone(),
                    connect: (!self.allows_connect()).then_some(false),
                    webhook_tasks: limits.webhook_tasks,
                    telemetry: limits.telemetry,
                    browser_cookie_import: limits.browser_cookie_import,
                    providers: if limits
                        .providers
                        .as_ref()
                        .and_then(|p| p.allowed_drivers.as_ref())
                        .is_none()
                        && fields.is_empty()
                    {
                        None
                    } else {
                        Some(ManagedPolicySummaryProviderLimits {
                            allowed_drivers: limits
                                .providers
                                .as_ref()
                                .and_then(|p| p.allowed_drivers.clone()),
                            locked_instance_fields: (!fields.is_empty()).then_some(fields),
                        })
                    },
                    updates: limits
                        .updates
                        .as_ref()
                        .map(|updates| ManagedPolicySummaryUpdates {
                            mode: updates.mode.clone(),
                            allow_nightly: updates.allow_nightly,
                        }),
                }),
        })
    }
    pub fn defaults(&self) -> Result<ServerSettings> {
        let mut defaults = t3_contracts::stored::to_value(&ServerSettings::default())
            .map_err(|_| Error::Invalid)?;
        for (path, value, locked) in &self.settings {
            if !locked {
                set(&mut defaults, path, Some(value.clone()));
            }
        }
        sync_idle(&mut defaults);
        serde_json::from_value(defaults).map_err(|_| Error::Invalid)
    }
    pub fn keep_stored(
        &self,
        mut stripped: Value,
        settings: &ServerSettings,
        stored: &Value,
        is_atomic: impl Fn(&str) -> bool,
    ) -> Result<Value> {
        let encoded = t3_contracts::stored::to_value(settings).map_err(|_| Error::Invalid)?;
        let mut paths: Vec<_> = self
            .settings
            .iter()
            .filter(|(_, _, locked)| !locked)
            .map(|(p, _, _)| {
                p.iter()
                    .position(|s| is_atomic(s))
                    .map_or_else(|| p.clone(), |n| p[..=n].to_vec())
            })
            .collect();
        if self
            .settings
            .iter()
            .any(|(p, _, locked)| !locked && p == &["idleCompaction"])
        {
            paths.push(vec!["compactIdleThreads".into()]);
        }
        fn restore(
            tree: &mut Value,
            path: &[String],
            stored: &Value,
            encoded: &Value,
            atomic: &impl Fn(&str) -> bool,
        ) {
            let Some(previous) = get(stored, path) else {
                return;
            };
            if let Some(map) = previous.as_object()
                && path.last().is_none_or(|key| !atomic(key))
            {
                for key in map.keys() {
                    let mut child = path.to_vec();
                    child.push(key.clone());
                    restore(tree, &child, stored, encoded, atomic);
                }
                return;
            }
            if get(tree, path).is_none() && Some(previous) == get(encoded, path) {
                set(tree, path, Some(previous.clone()));
            }
        }
        for path in paths {
            restore(&mut stripped, &path, stored, &encoded, &is_atomic);
        }
        Ok(stripped)
    }
}
fn sync_idle(value: &mut Value) {
    if value.get("idleCompaction").is_some() {
        value["compactIdleThreads"] = Value::Bool(value["idleCompaction"] == "beforeCacheExpires");
    }
}
fn mode_rank(mode: &RuntimeMode) -> u8 {
    match mode {
        RuntimeMode::ApprovalRequired => 0,
        RuntimeMode::AutoAcceptEdits => 1,
        RuntimeMode::Auto => 2,
        RuntimeMode::FullAccess => 3,
    }
}
fn clamp(value: &mut Value, ceiling: &Value) {
    let rank = |v: &Value| match v.as_str() {
        Some("approval-required") => 0,
        Some("auto-accept-edits") => 1,
        Some("auto") => 2,
        Some("full-access") => 3,
        _ => 0,
    };
    if value
        .get("defaultRuntimeMode")
        .is_some_and(|v| rank(v) > rank(ceiling))
    {
        value["defaultRuntimeMode"] = ceiling.clone();
    }
}
fn get<'a>(tree: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(tree, |v, k| v.as_object()?.get(k))
}
fn set(tree: &mut Value, path: &[String], value: Option<Value>) {
    let Some((head, rest)) = path.split_first() else {
        if let Some(value) = value {
            *tree = value;
        }
        return;
    };
    if !tree.is_object() {
        *tree = json!({});
    }
    let map = tree.as_object_mut().expect("object was initialized");
    if rest.is_empty() {
        if let Some(value) = value {
            map.insert(head.clone(), value);
        } else {
            map.remove(head);
        }
    } else {
        set(map.entry(head).or_insert(json!({})), rest, value);
    }
}
fn fill_missing(mut value: Value, defaults: &Value) -> Value {
    if let (Some(v), Some(d)) = (value.as_object_mut(), defaults.as_object()) {
        for (k, default) in d {
            let next = v
                .remove(k)
                .map_or_else(|| default.clone(), |present| fill_missing(present, default));
            v.insert(k.clone(), next);
        }
    }
    value
}
fn adds_keys(existing: Option<&Value>, next: &Value) -> bool {
    match (existing.and_then(Value::as_object), next.as_object()) {
        (Some(a), Some(b)) => b
            .iter()
            .any(|(k, v)| !a.contains_key(k) || adds_keys(a.get(k), v)),
        _ => false,
    }
}
fn matches_lock(value: &Value, lock: &Value) -> bool {
    match (value.as_object(), lock.as_object()) {
        (Some(v), Some(l)) => v
            .iter()
            .all(|(k, v)| l.get(k).is_some_and(|l| matches_lock(v, l))),
        _ => value == lock,
    }
}
fn targets(tree: &Value, path: &[String]) -> Vec<Vec<String>> {
    let mut result = vec![path.to_vec()];
    if let Some(projects) = tree
        .get("projectSettingsOverrides")
        .and_then(Value::as_object)
    {
        for (id, overrides) in projects {
            if overrides.get(&path[0]).is_some() {
                let candidate = json!({path[0].clone():overrides[&path[0]].clone()});
                if serde_json::from_value::<t3_contracts::settings::ProjectSettingsOverrides>(
                    candidate,
                )
                .ok()
                .and_then(|v| t3_contracts::stored::to_value(&v).ok())
                .is_some_and(|v| v.get(&path[0]).is_some())
                {
                    result.push(
                        [
                            vec!["projectSettingsOverrides".into(), id.clone()],
                            path.to_vec(),
                        ]
                        .concat(),
                    );
                }
            }
        }
    }
    result
}
fn changed_secret(before: &Value, after: &Value) -> bool {
    match (before, after) {
        (Value::String(a), Value::String(b)) => !a.is_empty() && a != "••••••" && b == "••••••",
        (Value::Object(a), Value::Object(b)) => {
            a.get("sensitive") == Some(&Value::Bool(true))
                && a.get("valueRedacted") != Some(&Value::Bool(true))
                && a.get("value")
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.is_empty())
                && b.get("valueRedacted") == Some(&Value::Bool(true))
                || a.iter()
                    .any(|(k, v)| b.get(k).is_some_and(|after| changed_secret(v, after)))
        }
        (Value::Array(a), Value::Array(b)) => a.iter().zip(b).any(|(a, b)| changed_secret(a, b)),
        _ => false,
    }
}

#[async_trait::async_trait]
pub trait ServeRoutes: Send + Sync {
    async fn can_bind(&self, host: &str, port: u16) -> Result<bool>;
    async fn routes(&self, host: &str, port: u16) -> Result<Vec<ServeRoute>>;
    async fn disable(&self, route: &ServeRoute) -> Result<()>;
}
#[derive(Debug, Clone)]
pub struct ServeRoute {
    pub serve_port: u16,
    pub removable: bool,
}
impl ManagedPolicy {
    pub async fn close_tailscale_serve_to(
        &self,
        host: &str,
        port: u16,
        routes: &dyn ServeRoutes,
    ) -> Result<()> {
        if self.allows_remote_clients() {
            return Ok(());
        }
        if !tokio::time::timeout(
            std::time::Duration::from_secs(30),
            routes.can_bind(host, port),
        )
        .await
        .map_err(|_| Error::Refused("bind availability check timed out"))??
        {
            return Err(Error::Refused(
                "cannot check Tailscale routes while the port is owned",
            ));
        }
        let found = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            routes.routes(host, port),
        )
        .await
        .map_err(|_| Error::Refused("Tailscale Serve check timed out"))??;
        for route in found.iter().filter(|r| r.removable) {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(30), routes.disable(route))
                .await;
        }
        if !tokio::time::timeout(
            std::time::Duration::from_secs(30),
            routes.routes(host, port),
        )
        .await
        .map_err(|_| Error::Refused("Tailscale Serve recheck timed out"))??
        .is_empty()
        {
            return Err(Error::Refused(
                "Tailscale Serve still forwards to the server",
            ));
        }
        Ok(())
    }
}
