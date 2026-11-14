use crate::{
    catalog::{self, ManifestSource},
    cursor_auth::CursorAuth,
    cursor_usage,
    instances::InstanceSpec,
    native_provider::NativeProviderContext,
    sensitive::{ServiceError, ServiceResult},
    service::ProviderInstanceService,
    transport::{HttpTransport, ProviderClock},
};
use async_trait::async_trait;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use t3_contracts::{provider_instance::ProviderInstanceId, server::ServerProvider};

pub struct CursorProvider {
    pub spec: Arc<InstanceSpec>,
    pub context: NativeProviderContext,
    pub auth: Arc<CursorAuth>,
    pub http: Arc<dyn HttpTransport>,
    pub manifest: ManifestSource,
    pub clock: Arc<dyn ProviderClock>,
    machine: tokio::sync::Mutex<Option<ServerProvider>>,
}
impl CursorProvider {
    pub fn new(
        spec: Arc<InstanceSpec>,
        context: NativeProviderContext,
        auth: Arc<CursorAuth>,
        http: Arc<dyn HttpTransport>,
        manifest: ManifestSource,
        clock: Arc<dyn ProviderClock>,
    ) -> Self {
        Self {
            spec,
            context,
            auth,
            http,
            manifest,
            clock,
            machine: tokio::sync::Mutex::new(None),
        }
    }
    async fn check(&self) -> ServiceResult<ServerProvider> {
        let now = self.clock.now_ms();
        let checked = chrono::DateTime::from_timestamp_millis(now)
            .ok_or(ServiceError::Response)?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let custom = self.spec.config["customModels"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let fallback = catalog::capabilities(vec![]);
        let mut models = catalog::custom_models(&[], &custom, Some(&fallback));
        let browser = !self.auth.uses_api_key();
        let mut value = json!({"instanceId":self.spec.instance_id,"driver":"cursor","displayName":self.spec.display_name.as_deref().unwrap_or("Cursor"),"enabled":self.spec.enabled,"installed":self.spec.enabled,"version":null,"status":if self.spec.enabled{"warning"}else{"disabled"},"auth":{"status":"unknown","canLogout":browser},"models":models,"checkedAt":checked,"setup":{"canAuthenticate":browser,"canInstall":false},"continuation":{"groupKey":self.spec.default_continuation_key()},"supportsConversationRollback":false,"showInteractionModeToggle":true,"supportedRuntimeModes":["approval-required","auto-accept-edits","auto","full-access"]});
        if let Some(color) = self.spec.accent_color.as_ref() {
            value["accentColor"] = json!(color);
        }
        if !self.spec.enabled {
            value["message"] = json!("Cursor is disabled in T3 Code settings.");
        } else if let Some(key) = self
            .auth
            .read_api_key()
            .await
            .ok()
            .flatten()
            .filter(|key| !key.expose().trim().is_empty())
        {
            let environment = self.spec.environment(
                &self.context.environment,
                &self.context.home,
                self.context.platform,
            );
            let timeout = Duration::from_millis(crate::versions::probe_timeout_ms(
                "cursor",
                15_000,
                &environment,
            ));
            let first = tokio::time::timeout(timeout, self.auth.catalog.read(&key)).await;
            let catalog = if first.is_err() {
                tokio::time::timeout(timeout, self.auth.catalog.read(&key)).await
            } else {
                first
            };
            match catalog {
                Ok(Ok(snapshot)) => {
                    let discovered = crate::models::cursor_models(&snapshot.models)
                        .map_err(|_| ServiceError::Response)?;
                    value["status"] = json!(if discovered.is_empty() {
                        "warning"
                    } else {
                        "ready"
                    });
                    let user = snapshot.user.as_ref();
                    let key_name = user
                        .and_then(|user| user["apiKeyName"].as_str())
                        .map(str::trim)
                        .filter(|name| !name.is_empty());
                    value["auth"] = json!({"status":"authenticated","type":if browser{"browser"}else{"api-key"},"label":if browser{"Cursor account".to_owned()}else{key_name.map_or_else(||"Cursor API key".into(), |name|format!("Cursor API key ({name})"))},"canLogout":browser});
                    if let Some(email) = user
                        .and_then(|user| user["userEmail"].as_str())
                        .map(str::trim)
                        .filter(|email| !email.is_empty())
                    {
                        value["auth"]["email"] = json!(email);
                    }
                    models = catalog::custom_models(&discovered, &custom, Some(&fallback));
                    if discovered.is_empty() {
                        value["message"] =
                            json!("Cursor SDK model discovery returned no built-in models.");
                    }
                    let mut environment = environment;
                    environment.insert("CURSOR_API_KEY".into(), key.into_inner());
                    value["usageLimits"] = json!(
                        cursor_usage::read(
                            &*self.http,
                            self.spec.config["apiEndpoint"].as_str(),
                            &environment,
                            &checked
                        )
                        .await?
                    );
                }
                Ok(Err(error)) => {
                    value["status"] = json!("error");
                    value["auth"]["status"] = json!(if error.authentication_failure {
                        "unauthenticated"
                    } else {
                        "unknown"
                    });
                    value["message"] = json!(if error.authentication_failure {
                        if browser {
                            "Cursor sign-in expired or was rejected. Sign in again in provider settings."
                        } else {
                            "Cursor SDK authentication failed. Check CURSOR_API_KEY."
                        }
                    } else if error.plan_required {
                        if browser {
                            "Cursor needs a paid plan to run in T3 Code. Upgrade this account or sign in with a different one."
                        } else {
                            "Cursor needs a paid plan to run in T3 Code. Upgrade the account that owns CURSOR_API_KEY or use a key from a paid account."
                        }
                    } else {
                        "Cursor SDK catalog request failed. Check server logs for details."
                    });
                }
                Err(_) => {
                    value["status"] = json!("error");
                    value["message"] = json!(format!(
                        "Cursor SDK catalog request timed out after {}ms.",
                        timeout.as_millis()
                    ));
                }
            }
        } else {
            value["status"] = json!("error");
            value["auth"]["status"] = json!("unauthenticated");
            value["message"] =
                json!("Sign in with Cursor or add CURSOR_API_KEY in provider settings.");
        }
        value["models"] = json!(models);
        if self.spec.enabled {
            let environment = self.spec.environment(
                &self.context.environment,
                &self.context.home,
                self.context.platform,
            );
            let skills = crate::cursor_skills::probe(None, &self.context.home, &environment)?;
            value["skills"] = json!(skills);
        }
        if let Some(advisory) = crate::versions::resolve_compatibility(
            self.manifest
                .current()
                .compatibility
                .as_deref()
                .unwrap_or_default(),
            "cursor",
            None,
            &self.context.server_version,
        ) {
            value["compatibilityAdvisory"] = json!(advisory);
        }
        serde_json::from_value(value).map_err(|_| ServiceError::Response)
    }
}
#[async_trait]
impl ProviderInstanceService for CursorProvider {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.spec.instance_id
    }
    fn catalogs_per_cwd(&self) -> bool {
        true
    }
    async fn invalidate_models(&self, _: Option<&str>, _: bool) -> ServiceResult<()> {
        self.auth.catalog.invalidate().await;
        Ok(())
    }
    async fn maintenance(
        &self,
        _: bool,
    ) -> ServiceResult<crate::maintenance::MaintenanceCapabilities> {
        Ok(crate::maintenance::MaintenanceCapabilities {
            driver: "cursor".into(),
            package: None,
            update: None,
            install_path: None,
            manual_command: None,
        })
    }
    async fn snapshot(&self, cwd: Option<&str>, fresh: bool) -> ServiceResult<ServerProvider> {
        let mut machine = self.machine.lock().await;
        if let Some(cwd) = cwd {
            if machine.is_none() {
                *machine = Some(self.check().await?);
            }
            let base = machine.as_mut().ok_or(ServiceError::Unavailable)?;
            if !base.enabled {
                return Ok(base.clone());
            }
            if !fresh
                && base.workspace_snapshots.as_ref().is_some_and(|workspaces| {
                    workspaces.iter().any(|workspace| workspace.cwd.0 == cwd)
                })
            {
                return Ok(base.clone());
            }
            let environment = self.spec.environment(
                &self.context.environment,
                &self.context.home,
                self.context.platform,
            );
            let skills = crate::cursor_skills::probe(
                Some(std::path::Path::new(cwd)),
                &self.context.home,
                &environment,
            )?;
            let checked = chrono::DateTime::from_timestamp_millis(self.clock.now_ms())
                .ok_or(ServiceError::Response)?
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let snapshot=serde_json::from_value(json!({"cwd":cwd,"checkedAt":checked,"slashCommands":base.slash_commands,"skills":skills,"slashCommandsPending":false})).map_err(|_|ServiceError::Response)?;
            let snapshots = base.workspace_snapshots.get_or_insert_with(Vec::new);
            snapshots.retain(|snapshot| snapshot.cwd.0 != cwd);
            snapshots.push(snapshot);
            if snapshots.len() > 16 {
                snapshots.remove(0);
            }
            return Ok(base.clone());
        }
        let mut next = self.check().await?;
        if let Some(previous) = machine.as_ref() {
            next.workspace_snapshots = previous.workspace_snapshots.clone();
        }
        *machine = Some(next.clone());
        Ok(next)
    }
}
