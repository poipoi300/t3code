use super::{
    assets::AssetService,
    config::{FetchedMachineCatalog, ServerConfigService},
    filesystem::FileMetadataService,
    policy::{command_lane, invalidated_methods, resource_policy},
    resources::{Resource, ResourceRegistry},
    scheduling::CommandScheduler,
    terminal::{TerminalBufferState, TerminalKey, TerminalResources},
};
use crate::{
    ClientError,
    rpc::RpcEndpoint,
    types::{EnvironmentId, Generation},
};
use futures::{FutureExt, future::LocalBoxFuture};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::{
    rpc::RpcMethod,
    server::{ServerConfigStreamEvent, ServerProviderUpdatedPayload},
};

#[derive(Default)]
struct State {
    resources: ResourceRegistry,
    configs: BTreeMap<EnvironmentId, ServerConfigService>,
    sessions: BTreeMap<EnvironmentId, Generation>,
    welcomes: BTreeMap<EnvironmentId, super::config::ServerWelcomeState>,
    terminals: TerminalResources,
    options: ServicesOptions,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct ServicesOptions {
    pub usage_limits_command: bool,
    pub provider_catalogs_on_demand: bool,
    pub omit_refreshed_providers: bool,
}
#[derive(Clone, Default)]
pub struct Services {
    inner: Rc<RefCell<State>>,
    pub metadata: FileMetadataService,
    pub assets: AssetService,
    pub scheduler: CommandScheduler,
    pub updates: super::updates::ServerUpdateService,
    pub review: super::review::ReviewService,
}
pub type ServiceCommand<T> = LocalBoxFuture<'static, Result<T, Rc<ClientError>>>;
impl Services {
    pub fn with_options(options: ServicesOptions) -> Self {
        let service = Self::default();
        service.inner.borrow_mut().options = options;
        service
    }
    pub fn seed_terminal(&self, key: TerminalKey) {
        self.inner.borrow_mut().terminals.seed(key);
    }
    pub fn terminal_buffer(&self, key: &TerminalKey) -> Option<TerminalBufferState> {
        self.inner.borrow().terminals.buffers.get(key).cloned()
    }
    pub fn apply_terminal(
        &self,
        key: TerminalKey,
        generation: Generation,
        event: &t3_contracts::terminal::TerminalAttachStreamEvent,
    ) -> bool {
        self.inner.borrow_mut().terminals.apply(
            key,
            generation,
            event,
            super::terminal::DEFAULT_MAX_TERMINAL_BUFFER_BYTES,
        )
    }
    pub fn apply_terminal_metadata(
        &self,
        environment: EnvironmentId,
        generation: Generation,
        event: t3_contracts::terminal::TerminalMetadataStreamEvent,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        if inner.sessions.get(&environment) != Some(&generation) {
            return false;
        }
        super::terminal::apply_terminal_metadata(
            inner.terminals.metadata.entry(environment).or_default(),
            event,
        );
        true
    }
    pub fn terminal_metadata(
        &self,
        environment: &EnvironmentId,
    ) -> Option<Vec<t3_contracts::terminal::TerminalSummary>> {
        self.inner
            .borrow()
            .terminals
            .metadata
            .get(environment)
            .cloned()
    }
    pub fn config(&self, environment: EnvironmentId) -> ServerConfigService {
        self.inner
            .borrow_mut()
            .configs
            .entry(environment.clone())
            .or_insert_with(|| ServerConfigService::new(environment))
            .clone()
    }
    pub fn begin_session(&self, environment: EnvironmentId, generation: Generation) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.sessions.insert(environment.clone(), generation);
            inner
                .welcomes
                .entry(environment.clone())
                .or_default()
                .replace_session(Some(generation));
            inner.resources.replace_session(&environment, generation);
            inner
                .terminals
                .begin_session(environment.clone(), generation);
        }
        self.config(environment.clone()).begin_session(generation);
        self.metadata.begin_session(environment.clone(), generation);
        self.review.replace_session(&environment, generation);
    }
    pub fn apply_lifecycle(
        &self,
        environment: &EnvironmentId,
        generation: Generation,
        event: t3_contracts::server::ServerLifecycleStreamEvent,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        if inner.sessions.get(environment) != Some(&generation) {
            return false;
        }
        inner
            .welcomes
            .entry(environment.clone())
            .or_default()
            .apply(generation, event);
        true
    }
    pub fn begin_welcome_subscription(&self, environment: &EnvironmentId, generation: Generation) {
        let mut inner = self.inner.borrow_mut();
        if inner.sessions.get(environment) == Some(&generation) {
            inner
                .welcomes
                .entry(environment.clone())
                .or_default()
                .subscribe(generation);
        }
    }
    pub fn welcome(
        &self,
        environment: &EnvironmentId,
    ) -> Option<t3_contracts::server::ServerLifecycleWelcomePayload> {
        self.inner
            .borrow()
            .welcomes
            .get(environment)
            .and_then(|state| state.value().cloned())
    }
    pub fn disconnect(&self, environment: &EnvironmentId) {
        let mut inner = self.inner.borrow_mut();
        inner.sessions.remove(environment);
        inner.resources.disconnect(environment);
        inner
            .welcomes
            .entry(environment.clone())
            .or_default()
            .replace_session(None);
        inner.terminals.disconnect(environment);
        drop(inner);
        self.metadata.disconnect(environment);
        self.review.disconnect(environment);
    }
    pub fn query<M: RpcMethod>(
        &self,
        environment: EnvironmentId,
        input: &M::Payload,
    ) -> Result<Resource<M::Success>, ClientError>
    where
        M::Success: 'static,
    {
        self.inner
            .borrow_mut()
            .resources
            .query::<M>(environment, input, resource_policy(M::TAG))
    }
    pub fn invalidate(&self, environment: &EnvironmentId, method: &str) {
        self.inner
            .borrow()
            .resources
            .invalidate(environment, method);
    }
    pub fn apply_server_config(
        &self,
        environment: EnvironmentId,
        generation: Generation,
        event: ServerConfigStreamEvent,
    ) -> Result<bool, ClientError> {
        if self.inner.borrow().sessions.get(&environment) != Some(&generation) {
            return Ok(false);
        }
        let config = self.config(environment.clone());
        let before = config
            .snapshot()
            .map(|projection| super::settings::usage_scan_settings_key(&projection.config.settings))
            .transpose()?;
        let changed = config.apply(generation, event)?;
        if changed {
            let after = config
                .snapshot()
                .map(|projection| {
                    super::settings::usage_scan_settings_key(&projection.config.settings)
                })
                .transpose()?;
            if before != after {
                self.invalidate(&environment, "server.getUsageSummary");
            }
        }
        Ok(changed)
    }
    pub fn collect(&self, now: u64) {
        self.inner.borrow_mut().resources.collect(now);
        self.metadata.collect(now);
        self.review.collect(now);
    }
    pub fn remove_environment(&self, environment: &EnvironmentId) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.resources.remove_environment(environment);
            if let Some(config) = inner.configs.remove(environment) {
                config.close();
            }
            inner.sessions.remove(environment);
            inner.welcomes.remove(environment);
            inner.terminals.remove_environment(environment);
        }
        self.metadata.remove_environment(environment);
        self.assets.remove_environment(environment);
        self.scheduler.remove_environment(environment);
        self.review.remove_environment(environment);
        self.updates.remove_environment(environment);
    }
    /// The endpoint enforces the current destination grants when the scheduled call starts.
    pub fn command<M, R>(
        &self,
        environment: EnvironmentId,
        endpoint: Rc<R>,
        input: M::Payload,
    ) -> Result<ServiceCommand<M::Success>, ClientError>
    where
        M: RpcMethod + 'static,
        M::Payload: 'static,
        M::Success: 'static,
        R: RpcEndpoint + 'static,
    {
        if M::STREAMS {
            return Err(ClientError::InvalidPayload(
                "stream used as unary command".into(),
            ));
        }
        if endpoint
            .destination()
            .as_ref()
            .is_some_and(|destination| destination != &environment)
        {
            return Err(ClientError::EnvironmentMismatch);
        }
        let original = serde_json::to_value(&input)?;
        let (lane, mode) = command_lane(environment.clone(), M::TAG, &original)?;
        let services = self.clone();
        let generation = self.inner.borrow().sessions.get(&environment).copied();
        let options = self.inner.borrow().options;
        if generation.is_none() {
            return Err(ClientError::Disconnected);
        }
        let future = self.scheduler.schedule(lane, mode, async move {
            if services.inner.borrow().sessions.get(&environment).copied() != generation {
                return Err(ClientError::Cancelled);
            }
            let input = if M::TAG == "server.refreshProviders" {
                let mut value = original.clone();
                value["workspaceSnapshotsOnDemand"] = json!(true);
                if options.usage_limits_command {
                    value["usageLimitsCommand"] = json!(true);
                }
                if options.provider_catalogs_on_demand {
                    value["providerCatalogsOnDemand"] = json!(true);
                }
                if options.omit_refreshed_providers {
                    value["omitProviders"] = json!(true);
                }
                if let (Some(instance), Some(_)) = (
                    original.get("instanceId").and_then(Value::as_str),
                    original.get("cwd"),
                ) && let Some(config) = services.config(environment.clone()).config(None)
                    && config.providers.0.iter().any(|provider| {
                        provider.instance_id.0 == instance
                            && provider.catalog_digest.is_some()
                            && provider.catalog_digest != provider.loaded_catalog_digest
                    })
                {
                    value["machineCatalog"] = json!(true);
                }
                serde_json::from_value(value)?
            } else {
                input
            };
            let result = endpoint.call::<M>(input).await?;
            if services.inner.borrow().sessions.get(&environment).copied() != generation {
                return Err(ClientError::Cancelled);
            }
            let value = serde_json::to_value(result)?;
            for method in invalidated_methods(M::TAG) {
                services.invalidate(&environment, method);
            }
            if M::TAG == "server.refreshProviders"
                && let (Some(instance), Some(cwd), Some(generation)) = (
                    original.get("instanceId").and_then(Value::as_str),
                    original.get("cwd").and_then(Value::as_str),
                    generation,
                )
            {
                let result: ServerProviderUpdatedPayload = serde_json::from_value(value.clone())?;
                let instance = t3_contracts::provider_instance::ProviderInstanceId(instance.into());
                let config = services.config(environment.clone());
                if let Some(provider) = result
                    .providers
                    .0
                    .iter()
                    .find(|provider| provider.instance_id == instance)
                    && let Some(digest) = &provider.catalog_digest
                {
                    config.record_catalog(
                        generation,
                        FetchedMachineCatalog {
                            instance_id: instance.clone(),
                            digest: digest.0.clone(),
                            slash_commands: provider.slash_commands.clone(),
                            skills: provider.skills.clone(),
                        },
                    );
                }
                config.record_workspace(
                    generation,
                    &instance,
                    cwd,
                    original.get("fresh").and_then(Value::as_bool) == Some(true),
                    &result.providers.0,
                );
            }
            Ok(value)
        });
        Ok(async move {
            let value = future.await?;
            serde_json::from_value(value.as_ref().clone()).map_err(|error| Rc::new(error.into()))
        }
        .boxed_local())
    }
}
