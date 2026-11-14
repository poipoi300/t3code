//! The only contact surface with `t3-device`; compiled with the `device` feature.
//!
//! One `DeviceRuntime` per environment (`device/DeviceService.ts` and `DeviceHubProxy.ts`): its
//! service serves the device RPCs, its router the hub proxy, and its shutdown retires every
//! host. It is assembled only when a Node runtime is found on the server's PATH, which the
//! agent launchers require; without one the device methods answer unsupported.
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    path::PathBuf,
    sync::Arc,
};
use t3_contracts::{
    auth::{AuthEnvironmentScope, AuthWebSocketTicketUse},
    settings_secret_value::SettingsSecretValue,
};
use t3_device::{
    DeviceRuntime, RuntimeInputs, agent, cloud, io, local,
    proxy::{AuthFailure, Authenticator, NativeSocketConnector, Scope},
    ssh, tools,
};

pub use t3_device::Error as DeviceError;

pub struct DeviceAdapter {
    pub runtime: DeviceRuntime,
    pub workspaces: Arc<crate::workspaces::EnvironmentWorkspaces>,
}

impl DeviceAdapter {
    /// `None` when no Node runtime is on PATH (`resolveNodeExecutable` fails in TypeScript).
    #[cfg(feature = "local-services")]
    pub async fn start(
        context: &crate::services::DomainContext<'_>,
        workspaces: Arc<crate::workspaces::EnvironmentWorkspaces>,
    ) -> crate::Result<Option<Arc<Self>>> {
        let Some(node) = resolve_node(&context.environment) else {
            tracing::info!("device support is unavailable: no Node runtime on PATH");
            return Ok(None);
        };
        let node = node.to_string_lossy().into_owned();
        let service = |e: t3_device::Error| crate::Error::Service(e.to_string());
        let config = &context.local.config;
        let base = config.paths.base_dir.clone();
        let state = config.paths.state_dir.clone();
        // Subscribed before the runtime reads its first settings snapshot.
        let settings_changes = context.local.settings.subscribe_changes();
        let settings = Arc::new(Settings(context.local.settings.clone()));
        let secrets: Arc<dyn cloud::Secrets> = Arc::new(Secrets {
            settings: context.local.settings.clone(),
            resolver: t3_provider::secrets::SecretResolver::new(
                t3_provider::secrets::OnePassword::new(context.environment.clone()),
            ),
        });
        let commands = Arc::new(io::NativeCommands);
        let spawner = Arc::new(io::NativeSpawner);
        let http = Arc::new(io::NativeHttp::new().map_err(service)?);
        let ports = Arc::new(local::NativePorts);
        let toolchain = Arc::new(tools::Toolchain::new(base.clone(), commands.clone()));
        let entry = tools::Tool::Agent
            .paths(&base, tools::Tool::Agent.version())
            .entry;
        let local_host = Arc::new(local::LocalDeviceHost::new(local::LocalOptions {
            state: state.clone(),
            cwd: config.cwd.clone(),
            node: node.clone(),
            tools: toolchain.clone(),
            http: http.clone(),
            commands: commands.clone(),
            spawner: spawner.clone(),
            ports: ports.clone(),
            machine: Arc::new(local::NativeMachine),
        }));
        let host_factory = Arc::new(cloud::NativeHostFactory {
            state: state.clone(),
            environment_id: context.local.environment.environment_id.0.clone(),
            node: node.clone(),
            agent_entry: entry.clone(),
            tools: toolchain.clone(),
            http: http.clone(),
            commands: commands.clone(),
            spawner,
            ports,
            secrets: secrets.clone(),
            environment: context.environment.clone(),
            expo_home: context.account_home.clone(),
            dns: Arc::new(ssh::NativeDns),
            local_addresses: local_addresses(),
        });
        let launchers = Arc::new(agent::FileLaunchers {
            state,
            node: node.clone(),
            entry,
            cli_provider: Some(Arc::new(tools::ToolchainAgentCli {
                node,
                tools: toolchain.clone(),
            })),
            commands,
        });
        let accounts = Arc::new(cloud::AccountService {
            settings: settings.clone(),
            secrets,
            http: http.clone(),
            environment: context.environment.clone(),
            expo_home: context.account_home.clone(),
        });
        let runtime = DeviceRuntime::new(RuntimeInputs {
            local_host,
            host_factory,
            launchers,
            installer: toolchain,
            accounts,
            settings,
            settings_changes,
            http,
            auth: Arc::new(MediaAuth(context.local.auth.inner().clone())),
            sockets: Arc::new(NativeSocketConnector),
        })
        .await
        .map_err(service)?;
        let mut status = runtime.settings_status();
        tokio::spawn(async move {
            while status.changed().await.is_ok() {
                if let Some(error) = status.borrow_and_update().as_ref() {
                    tracing::warn!(%error, "device host reconciliation failed");
                }
            }
        });
        Ok(Some(Arc::new(Self {
            runtime,
            workspaces,
        })))
    }

    /// The thread's own workspace for `device.open`; never a path from the client.
    pub async fn open_context(&self, thread_id: &str) -> t3_device::host::OpenContext {
        t3_device::host::OpenContext {
            workspace_root: self.workspaces.thread_workspace_root(thread_id).await,
        }
    }
}

impl crate::services::DomainRuntime for DeviceAdapter {
    fn name(&self) -> &'static str {
        "device"
    }
    /// Retires every host and releases launchers; device support stays enabled in settings.
    fn shutdown(&self) -> futures::future::BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            self.runtime
                .shutdown()
                .await
                .map_err(|e| crate::Error::Service(e.to_string()))
        })
    }
}

/// `resolveNodeExecutable`: an absolute Node path from the server's environment PATH.
fn resolve_node(environment: &BTreeMap<String, String>) -> Option<PathBuf> {
    let path = environment
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value)?;
    let names: &[&str] = if cfg!(windows) {
        &["node.exe"]
    } else {
        &["node"]
    };
    std::env::split_paths(path)
        .filter(|directory| directory.is_absolute())
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|candidate| candidate.is_file())
}

/// Every assigned interface address, tailnet ones included (`LocalDeviceHostAddresses`).
fn local_addresses() -> BTreeSet<IpAddr> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|interface| interface.ip())
        .collect()
}

struct Settings(Arc<t3_config::settings::SettingsService>);
#[async_trait::async_trait]
impl t3_device::service::Settings for Settings {
    async fn read(&self) -> t3_device::Result<t3_device::service::DeviceSettings> {
        let value = self.0.snapshot().await.map_err(|_| {
            t3_device::Error::operation(
                "settings",
                t3_contracts::device::DeviceOperationErrorReason::SettingsFailed,
            )
        })?;
        Ok(t3_device::service::DeviceSettings::from_server(&value))
    }
    async fn update(
        &self,
        input: &t3_contracts::device::DeviceConfigureInput,
    ) -> t3_device::Result<t3_device::service::DeviceSettings> {
        let value = self
            .0
            .update(&t3_device::service::configuration_patch(input))
            .await
            .map_err(|_| {
                t3_device::Error::operation(
                    "configure",
                    t3_contracts::device::DeviceOperationErrorReason::SettingsFailed,
                )
            })?;
        Ok(t3_device::service::DeviceSettings::from_server(&value))
    }
}

/// `requireSettingsSecret` over the shared 1Password resolver and the private settings.
struct Secrets {
    settings: Arc<t3_config::settings::SettingsService>,
    resolver: t3_provider::secrets::SecretResolver,
}
#[async_trait::async_trait]
impl cloud::Secrets for Secrets {
    async fn resolve(&self, value: &SettingsSecretValue) -> t3_device::Result<io::Secret> {
        let reference = match value {
            SettingsSecretValue::Literal(value) => return Ok(io::Secret::new(value.clone())),
            SettingsSecretValue::Reference(reference) => reference,
        };
        let unavailable = || {
            t3_device::Error::Unavailable(format!(
                "Could not read secret provider \"{}\". Unlock or reconnect it, then retry.",
                reference.provider.0
            ))
        };
        let settings = self.settings.snapshot().await.map_err(|_| unavailable())?;
        let mut answers = self
            .resolver
            .read(
                &settings.secret_providers,
                std::slice::from_ref(reference),
                false,
            )
            .await;
        match answers.pop().flatten() {
            Some(secret) if answers.is_empty() && !secret.is_empty() => Ok(io::Secret::new(secret)),
            _ => Err(unavailable()),
        }
    }
}

/// `authenticateMediaRequest`: the live session or a one-use media ticket, then the scope.
struct MediaAuth(t3_auth::EnvironmentAuth);
#[async_trait::async_trait]
impl Authenticator for MediaAuth {
    async fn authorize(
        &self,
        headers: &axum::http::HeaderMap,
        query: &[(String, String)],
        scope: Scope,
    ) -> Result<(), AuthFailure> {
        use axum::http::StatusCode;
        let uri: axum::http::Uri = {
            let mut url = url::form_urlencoded::Serializer::new(String::from("/api/device-hub?"));
            for (key, value) in query {
                url.append_pair(key, value);
            }
            url.finish()
        }
        .parse()
        .map_err(|_| AuthFailure(StatusCode::BAD_REQUEST))?;
        let request = crate::adapters::auth::request("GET", &uri, headers, None);
        let session = self
            .0
            .authenticate_websocket(&request, AuthWebSocketTicketUse::Media)
            .await
            .map_err(|error| {
                AuthFailure(if error.is_credential() {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                })
            })?;
        let required = match scope {
            Scope::Read => AuthEnvironmentScope::OrchestrationRead,
            Scope::Operate => AuthEnvironmentScope::OrchestrationOperate,
        };
        if session.scopes.contains(&required) {
            Ok(())
        } else {
            Err(AuthFailure(StatusCode::FORBIDDEN))
        }
    }
}
