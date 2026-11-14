use crate::{
    Error, Result,
    host::{AgentEndpoint, SessionRelease},
    io::{Http, HttpRequest, Secret, request_bytes},
};
use async_trait::async_trait;
use http::Method;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..24].into()
}
pub fn session(thread: &str, host: &str, device: &str) -> String {
    format!("t3-{}", hash(&json!([thread, host, device]).to_string()))
}
pub fn config_path(state: &Path, host: &str, device: Option<&str>) -> PathBuf {
    let key = device.map_or_else(
        || host.to_owned(),
        |device| json!([host, device]).to_string(),
    );
    state
        .join("device/hosts")
        .join(format!("{}.json", hash(&key)))
}

pub async fn write_config(path: &Path, endpoint: &AgentEndpoint) -> Result<()> {
    let mut value =
        json!({"daemonBaseUrl":endpoint.base_url,"daemonAuthToken":endpoint.token.expose()});
    if let Some(state) = &endpoint.state_dir {
        value["stateDir"] = json!(state);
    }
    atomic_write(path, value.to_string().as_bytes()).await
}

pub async fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| crate::error::invalid("write config"))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|_| Error::Unavailable("Could not prepare device state directory.".into()))?;
    if tokio::fs::read(path).await.ok().as_deref() == Some(content) {
        return Ok(());
    }
    let temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| Error::Unavailable("Could not prepare device state file.".into()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| Error::Unavailable("Could not protect device state file.".into()))?;
    }
    tokio::fs::write(temporary.path(), content)
        .await
        .map_err(|_| Error::Unavailable("Could not write device state file.".into()))?;
    temporary
        .persist(path)
        .map_err(|_| Error::Unavailable("Could not publish device state file.".into()))?;
    Ok(())
}

pub async fn release_session(http: &dyn Http, endpoint: &AgentEndpoint, name: &str) -> Result<()> {
    let request=HttpRequest::new(Method::POST,&format!("{}/rpc",endpoint.base_url),Duration::from_secs(15))?.bearer(&endpoint.token).json(&json!({"jsonrpc":"2.0","id":name,"method":"agent_device.command","params":{"command":"close","session":name,"positionals":[],"flags":{}}}))?;
    let (status, bytes) = request_bytes(http, request).await?;
    if !(200..300).contains(&status) && status != 404 {
        return Err(Error::operation(
            "close",
            t3_contracts::device::DeviceOperationErrorReason::RequestFailed,
        ));
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| crate::error::invalid("close"))?;
    if value.pointer("/result/ok") == Some(&json!(true))
        || value.pointer("/error/data/code") == Some(&json!("SESSION_NOT_FOUND"))
    {
        Ok(())
    } else {
        Err(crate::error::invalid("close"))
    }
}

pub struct DaemonRelease {
    pub http: Arc<dyn Http>,
    pub endpoint: AgentEndpoint,
}
#[async_trait]
impl SessionRelease for DaemonRelease {
    async fn release(&self, session: &str, _: &str) -> Result<()> {
        release_session(self.http.as_ref(), &self.endpoint, session).await
    }
}

#[derive(Clone, Debug)]
pub struct AgentTarget {
    pub agent_device_id: Option<String>,
    pub session: String,
    pub args: Vec<String>,
    pub install_directory: Option<PathBuf>,
    pub host_session: bool,
}

#[async_trait]
pub trait Launchers: Send + Sync {
    async fn configure(
        &self,
        host: &str,
        device: Option<&str>,
        endpoint: &AgentEndpoint,
    ) -> Result<String>;
    async fn bind(&self, session: &str, args: &[String]) -> Result<String>;
    async fn release(&self, launcher: &str) -> Result<()>;
    async fn prune(&self, live: &BTreeSet<String>) -> Result<()>;
}

pub struct FileLaunchers {
    pub state: PathBuf,
    pub node: String,
    pub entry: PathBuf,
    pub cli_provider: Option<Arc<dyn crate::host::AgentCliProvider>>,
    pub commands: Arc<dyn crate::io::Commands>,
}
impl FileLaunchers {
    pub async fn ensure_shim(&self) -> Result<PathBuf> {
        let cli = if let Some(provider) = &self.cli_provider {
            provider.ensure().await?
        } else {
            crate::host::AgentCli {
                node: self.node.clone(),
                entry: self.entry.clone(),
            }
        };
        let dir = self.state.join("device/bin");
        self.write_launcher(dir.clone(), &cli, &[]).await?;
        Ok(dir)
    }
    async fn write_launcher(
        &self,
        dir: PathBuf,
        cli: &crate::host::AgentCli,
        args: &[String],
    ) -> Result<String> {
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|_| Error::Unavailable("Could not create agent launcher.".into()))?;
        let source = launcher_source(&cli.node, &cli.entry, args);
        atomic_write(&dir.join("agent-device-launcher.mjs"), source.as_bytes()).await?;
        let script = if cfg!(windows) {
            dir.join("agent-device.cmd")
        } else {
            dir.join("agent-device")
        };
        let js = dir.join("agent-device-launcher.mjs");
        const STALE_RUNTIME: &str = "This agent-device command is out of date because T3 Code moved or updated its runtime. Call device_open again for a fresh one.";
        let content = if cfg!(windows) {
            if cli.node.contains(['%', '\r', '\n', '"'])
                || js.to_string_lossy().contains(['%', '\r', '\n', '"'])
            {
                return Err(crate::error::invalid("launcher path"));
            }
            format!(
                "@echo off\r\nsetlocal EnableExtensions DisableDelayedExpansion\r\nif not exist \"{}\" (echo {} 1>&2 & exit /b 1)\r\nset \"ELECTRON_RUN_AS_NODE=1\"\r\n\"{}\" \"{}\" %*\r\n",
                cli.node,
                STALE_RUNTIME,
                cli.node,
                js.display()
            )
        } else {
            format!(
                "#!/bin/sh\n[ -x {} ] || {{ echo {} >&2; exit 1; }}\nexport ELECTRON_RUN_AS_NODE=1\nexec {} {} \"$@\"\n",
                crate::ssh::quote(&cli.node),
                crate::ssh::quote(STALE_RUNTIME),
                crate::ssh::quote(&cli.node),
                crate::ssh::quote(&js.to_string_lossy())
            )
        };
        atomic_write(&script, content.as_bytes()).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .await
                .map_err(|_| crate::error::invalid("launcher permissions"))?;
        }
        Ok(script.to_string_lossy().into_owned())
    }
}
#[async_trait]
impl Launchers for FileLaunchers {
    async fn configure(
        &self,
        host: &str,
        device: Option<&str>,
        endpoint: &AgentEndpoint,
    ) -> Result<String> {
        let path = config_path(&self.state, host, device);
        write_config(&path, endpoint).await?;
        Ok(path.to_string_lossy().into_owned())
    }
    async fn bind(&self, name: &str, args: &[String]) -> Result<String> {
        if !name.starts_with("t3-") || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(crate::error::invalid("launcher session"));
        }
        let cli = if let Some(provider) = &self.cli_provider {
            provider.ensure().await?
        } else {
            crate::host::AgentCli {
                node: self.node.clone(),
                entry: self.entry.clone(),
            }
        };
        self.write_launcher(self.state.join("device/bin"), &cli, &[])
            .await?;
        self.write_launcher(self.state.join("device/targets").join(name), &cli, args)
            .await
    }
    async fn release(&self, launcher: &str) -> Result<()> {
        let parent = Path::new(launcher)
            .parent()
            .ok_or_else(|| crate::error::invalid("launcher"))?;
        let mut command = crate::io::Command::new(
            &self.node,
            [
                parent
                    .join("agent-device-launcher.mjs")
                    .to_string_lossy()
                    .into_owned(),
                "close".into(),
            ],
        );
        command.timeout = Duration::from_secs(15);
        command
            .env
            .insert("ELECTRON_RUN_AS_NODE".into(), Some(Secret::new("1")));
        let output = self.commands.run(command).await?;
        if output.code != 0 && !output.stderr.contains("SESSION_NOT_FOUND") {
            return Err(Error::command("close agent session", output.code));
        }
        Ok(())
    }
    async fn prune(&self, live: &BTreeSet<String>) -> Result<()> {
        let dir = self.state.join("device/targets");
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(crate::error::invalid("launcher directory")),
        };
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| crate::error::invalid("launcher directory"))?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !live.contains(&name)
                && entry
                    .file_type()
                    .await
                    .map_err(|_| crate::error::invalid("launcher"))?
                    .is_dir()
            {
                let _ = tokio::fs::remove_dir_all(entry.path()).await;
            }
        }
        Ok(())
    }
}

fn launcher_source(node: &str, entry: &Path, args: &[String]) -> String {
    include_str!("launcher.mjs")
        .replace("__NODE__", &json!(node).to_string())
        .replace("__ENTRY__", &json!(entry).to_string())
        .replace("__BOUND__", &json!(args).to_string())
}
