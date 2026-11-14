use crate::{
    discovery::{Driver, classify_opencode_version},
    environment::{Environment, absolute_path},
    native_provider::ExecutableProbe,
    sensitive::{Sensitive, ServiceError, ServiceResult},
};
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{io::AsyncReadExt, sync::Mutex};
pub struct RegisteredService {
    pub url: String,
}
pub struct ServiceCommand {
    pub executable: String,
    pub args: Vec<String>,
    pub shell: bool,
}
impl std::fmt::Debug for ServiceCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServiceCommand([REDACTED])")
    }
}
pub struct ServiceStart {
    pub command: ServiceCommand,
    pub environment: Sensitive<Environment>,
}
impl std::fmt::Debug for ServiceStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServiceStart([REDACTED])")
    }
}
#[async_trait]
pub trait OpenCodeServiceBackend: Send + Sync {
    async fn discover(
        &self,
        file: &Path,
        version: Option<&str>,
    ) -> ServiceResult<Option<RegisteredService>>;
    async fn wrap_resolved_command(
        &self,
        binary: &str,
        environment: &Environment,
    ) -> ServiceResult<Option<ServiceCommand>>;
    async fn stop_handoff(&self, file: &Path) -> ServiceResult<()>;
    async fn ensure(&self, file: &Path, start: ServiceStart) -> ServiceResult<()>;
}
pub fn registered_address(registered: &str, configured: &str) -> bool {
    let (Ok(registered), Ok(configured)) =
        (url::Url::parse(registered), url::Url::parse(configured))
    else {
        return false;
    };
    registered.scheme() == configured.scheme()
        && registered.port_or_known_default() == configured.port_or_known_default()
        && (registered.host_str() == configured.host_str()
            || registered.host_str() == Some("0.0.0.0")
                && configured.host_str() == Some("127.0.0.1")
            || registered.host_str() == Some("[::]") && configured.host_str() == Some("[::1]"))
}
pub struct OpenCodeServiceReplacement {
    probe: Arc<dyn ExecutableProbe>,
    backend: Arc<dyn OpenCodeServiceBackend>,
    home: PathBuf,
    cwd: PathBuf,
    gate: Arc<Mutex<()>>,
}
impl OpenCodeServiceReplacement {
    pub fn new(
        probe: Arc<dyn ExecutableProbe>,
        backend: Arc<dyn OpenCodeServiceBackend>,
        home: PathBuf,
        cwd: PathBuf,
    ) -> Self {
        Self {
            probe,
            backend,
            home,
            cwd,
            gate: Arc::new(Mutex::new(())),
        }
    }
    pub async fn replace(
        &self,
        binary: &str,
        server_url: &str,
        environment: &Environment,
    ) -> ServiceResult<()> {
        let guard = self.gate.clone().lock_owned().await;
        let probe = self
            .probe
            .version(Driver::OpenCode, binary, environment, &self.cwd)
            .await?;
        let Some(version) = probe
            .version
            .and_then(|version| classify_opencode_version(&version))
        else {
            return Ok(());
        };
        if version.generation != 2 {
            return Ok(());
        }
        let home = environment
            .get("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.clone());
        let state = environment
            .get("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"));
        let file = absolute_path(&state.join("opencode/service.json"), &self.cwd);
        let Some(service) = self.backend.discover(&file, None).await? else {
            return Ok(());
        };
        if !registered_address(&service.url, server_url) {
            return Ok(());
        }
        let Some(command) = self
            .backend
            .wrap_resolved_command(binary, environment)
            .await?
        else {
            return Ok(());
        };
        if command.shell {
            return Ok(());
        }
        let config = environment
            .get("OPENCODE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                environment
                    .get("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".config"))
                    .join("opencode")
            });
        let config = absolute_path(&config.join("service.json"), &self.cwd);
        let mut env = environment.clone();
        if let Some(configured) = configured_environment(&config).await {
            for (name, value) in configured {
                if !name.eq_ignore_ascii_case("XDG_STATE_HOME") {
                    let name = if crate::environment::Platform::current()
                        == crate::environment::Platform::Windows
                    {
                        env.retain(|current, _| !current.eq_ignore_ascii_case(&name));
                        name.to_uppercase()
                    } else {
                        name
                    };
                    env.insert(name, value);
                }
            }
        }
        if self
            .backend
            .discover(&file, Some(&version.version))
            .await?
            .is_some()
        {
            return Ok(());
        }
        env.retain(|name, _| !name.eq_ignore_ascii_case("ELECTRON_RUN_AS_NODE"));
        let backend = self.backend.clone();
        tokio::spawn(async move {
            let _guard = guard;
            backend.stop_handoff(&file).await?;
            backend
                .ensure(
                    &file,
                    ServiceStart {
                        command,
                        environment: Sensitive::new(env),
                    },
                )
                .await
        })
        .await
        .map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Maintenance,
        ))?
    }
}
async fn configured_environment(path: &Path) -> Option<Environment> {
    let file = tokio::fs::File::open(path).await.ok()?;
    let mut bytes = vec![];
    file.take(1_048_577).read_to_end(&mut bytes).await.ok()?;
    if bytes.len() > 1_048_576 {
        return None;
    }
    let config = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    config
        .get("env")?
        .as_object()?
        .iter()
        .map(|(name, value)| Some((name.clone(), value.as_str()?.into())))
        .collect()
}
