use crate::{
    acp_service::{AcpCatalogOperations, AcpError, AcpResult},
    environment::{Environment, Platform},
    registry::{self, Agent, Preference, RegistryCatalog, ResolvedDistribution},
    registry_installation::{
        RegistryBinaryInstaller, RegistryCommands, RegistryDownloads, directory_below,
    },
    registry_packages::{RegistryPackageInstaller, preferred_path},
    sensitive::Sensitive,
    transport::ProviderClock,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::{acp_registry::*, settings::ServerSettings, speech::True};
use tokio::sync::Mutex;
type Reason = AcpRegistryOperationErrorReason;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectionStatus {
    Unconfigured,
    Ready,
    MissingRunner,
    NotFound,
    Unsupported,
    Unprepared,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryInspection {
    pub status: InspectionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub version: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
}
impl RegistryInspection {
    fn new(status: InspectionStatus) -> Self {
        Self {
            status,
            agent_id: None,
            version: None,
            distribution: None,
            documentation_url: None,
            runner: None,
        }
    }
}
fn error(reason: Reason) -> AcpError {
    AcpError::new(reason, "The ACP Registry operation could not be completed.")
}
fn validate_local(path: &Path) -> AcpResult<()> {
    let metadata = fs::metadata(path).map_err(|_| error(Reason::RunnerUnavailable))?;
    if !metadata.is_file() {
        return Err(error(Reason::RunnerUnavailable));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(error(Reason::RunnerUnavailable));
        }
    }
    Ok(())
}
fn reject_linked_directories(base: &Path, target: &Path) -> AcpResult<()> {
    let relative = target
        .strip_prefix(base)
        .map_err(|_| error(Reason::ArchiveInvalid))?;
    let mut current = base.to_path_buf();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(error(Reason::ArchiveInvalid));
        }
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|_| error(Reason::ArchiveInvalid))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(error(Reason::ArchiveInvalid));
        }
    }
    Ok(())
}
pub struct RegistryCatalogOptions {
    pub cache_dir: PathBuf,
    pub tools_dir: PathBuf,
    pub platform: Platform,
    pub architecture: String,
    pub environment: Environment,
}
pub struct ResolvedRegistryAgent {
    pub agent: Option<Agent>,
    pub distribution: Option<AcpRegistryDistribution>,
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub environment: Sensitive<Environment>,
}
impl std::fmt::Debug for ResolvedRegistryAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResolvedRegistryAgent([REDACTED])")
    }
}
pub struct RegistryAgentCatalog {
    options: RegistryCatalogOptions,
    index: Arc<RegistryCatalog>,
    settings: Arc<t3_config::settings::SettingsService>,
    commands: Arc<dyn RegistryCommands>,
    binary: RegistryBinaryInstaller,
    packages: RegistryPackageInstaller,
    clock: Arc<dyn ProviderClock>,
    gates: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
    reservations: Mutex<BTreeMap<String, i64>>,
}
impl RegistryAgentCatalog {
    pub async fn inspect(
        &self,
        settings: &Value,
        environment: &Environment,
    ) -> AcpResult<RegistryInspection> {
        let command = settings["commandPath"].as_str().unwrap_or_default().trim();
        if settings["source"] == "local" {
            if command.is_empty() {
                return Ok(RegistryInspection::new(InspectionStatus::Unconfigured));
            }
            let executable = self.commands.resolve(command, environment);
            if let Some(path) = executable.as_ref() {
                validate_local(path)?;
            }
            let mut result = RegistryInspection::new(if executable.is_some() {
                InspectionStatus::Ready
            } else {
                InspectionStatus::MissingRunner
            });
            result.version = Some(None);
            result.distribution = Some("local".into());
            if executable.is_some() {
                result.agent_id = Some(command.into());
            }
            return Ok(result);
        }
        let id = settings["agentId"].as_str().unwrap_or_default().trim();
        if id.is_empty() {
            return Ok(RegistryInspection::new(InspectionStatus::Unconfigured));
        }
        let cached = self
            .index
            .cached()
            .await
            .ok_or_else(|| error(Reason::RegistryUnavailable))?;
        let Some(agent) = cached.agents.iter().find(|agent| agent.id == id) else {
            let mut result = RegistryInspection::new(InspectionStatus::NotFound);
            result.agent_id = Some(id.into());
            return Ok(result);
        };
        let preference = match settings["distribution"].as_str() {
            Some("binary") => Preference::Binary,
            Some("npx") => Preference::Npx,
            Some("uvx") => Preference::Uvx,
            _ => Preference::Auto,
        };
        let mut result = RegistryInspection::new(InspectionStatus::Unsupported);
        result.agent_id = Some(id.into());
        result.version = Some(Some(agent.version.clone()));
        let Ok(distribution) = self.distribution(agent, preference) else {
            return Ok(result);
        };
        result.distribution = Some(
            match distribution.kind {
                AcpRegistryDistribution::Binary => "binary",
                AcpRegistryDistribution::Npx => "npx",
                AcpRegistryDistribution::Uvx => "uvx",
            }
            .into(),
        );
        if !command.is_empty() {
            result.status = if self.commands.resolve(command, environment).is_some() {
                result.version = Some(None);
                InspectionStatus::Ready
            } else {
                result.runner = Some(command.into());
                InspectionStatus::MissingRunner
            };
        } else if distribution.kind == AcpRegistryDistribution::Binary {
            let gate = self.gate(&agent.id).await;
            let _guard = gate.lock().await;
            self.reservations.lock().await.remove(&agent.id);
            let target = distribution
                .binary
                .as_ref()
                .ok_or_else(|| error(Reason::ArchiveInvalid))?;
            let matched = agent
                .distribution
                .binary
                .as_ref()
                .and_then(|targets| {
                    targets
                        .iter()
                        .find(|(name, candidate)| {
                            (*name == &self.binary.target
                                || self.binary.target == "windows-aarch64"
                                    && *name == "windows-x86_64")
                                && candidate.archive == target.archive
                                && candidate.cmd == target.cmd
                        })
                        .map(|(name, _)| name.as_str())
                })
                .ok_or_else(|| error(Reason::ArchiveInvalid))?;
            let root = registry::install_root(&self.options.tools_dir, agent, matched)
                .map_err(|_| error(Reason::ArchiveInvalid))?;
            let executable = registry::normalize_command_path(&target.cmd)
                .ok_or_else(|| error(Reason::ArchiveInvalid))?
                .iter()
                .fold(root.clone(), |path, segment| path.join(segment));
            if executable.exists() {
                reject_linked_directories(&self.options.tools_dir, &root)?;
                crate::registry_installation::executable_in_root(&root, &executable, false)?;
                result.status = InspectionStatus::Ready;
            } else {
                let agent_root = self.options.tools_dir.join(&agent.id);
                let mut prepared = false;
                if let Ok(versions) = fs::read_dir(&agent_root) {
                    for version in versions {
                        let version = version.map_err(|_| error(Reason::InstallFailed))?;
                        if !version
                            .file_type()
                            .map_err(|_| error(Reason::InstallFailed))?
                            .is_dir()
                        {
                            continue;
                        }
                        for candidate in [
                            self.binary.target.as_str(),
                            if self.binary.target == "windows-aarch64" {
                                "windows-x86_64"
                            } else {
                                self.binary.target.as_str()
                            },
                        ] {
                            let path = version.path().join(candidate);
                            if path.is_dir() {
                                reject_linked_directories(&self.options.tools_dir, &path)?;
                                prepared = true;
                            }
                        }
                    }
                }
                result.status = if prepared {
                    InspectionStatus::Ready
                } else {
                    InspectionStatus::Unprepared
                };
            }
        } else {
            let runner = if distribution.kind == AcpRegistryDistribution::Npx {
                "npm"
            } else {
                "uv"
            };
            result.status = if self.commands.resolve(runner, environment).is_some() {
                InspectionStatus::Ready
            } else {
                result.runner = Some(runner.into());
                InspectionStatus::MissingRunner
            };
        }
        if result.status == InspectionStatus::Ready {
            result.documentation_url = agent.website.clone().or_else(|| agent.repository.clone());
        }
        Ok(result)
    }
    pub fn new(
        options: RegistryCatalogOptions,
        index: Arc<RegistryCatalog>,
        settings: Arc<t3_config::settings::SettingsService>,
        commands: Arc<dyn RegistryCommands>,
        downloads: Arc<dyn RegistryDownloads>,
        clock: Arc<dyn ProviderClock>,
    ) -> Self {
        let binary = RegistryBinaryInstaller {
            tools: options.tools_dir.clone(),
            platform: options.platform,
            target: registry::platform_target(options.platform, &options.architecture)
                .unwrap_or_default(),
            environment: options.environment.clone(),
            commands: commands.clone(),
            downloads,
            clock: clock.clone(),
        };
        let packages = RegistryPackageInstaller {
            cache: options.cache_dir.clone(),
            tools: options.tools_dir.clone(),
            platform: options.platform,
            commands: commands.clone(),
        };
        Self {
            options,
            index,
            settings,
            commands,
            binary,
            packages,
            clock,
            gates: Mutex::new(BTreeMap::new()),
            reservations: Mutex::new(BTreeMap::new()),
        }
    }
    async fn gate(&self, agent: &str) -> Arc<Mutex<()>> {
        self.gates
            .lock()
            .await
            .entry(agent.into())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
    async fn agent(&self, id: &str, fresh: bool) -> AcpResult<Agent> {
        self.index
            .recent(self.clock.now_ms(), fresh)
            .await
            .map_err(|_| error(Reason::RegistryUnavailable))?
            .agents
            .iter()
            .find(|agent| agent.id == id.trim())
            .cloned()
            .ok_or_else(|| error(Reason::AgentNotFound))
    }
    fn distribution(
        &self,
        agent: &Agent,
        preference: Preference,
    ) -> AcpResult<ResolvedDistribution> {
        registry::resolve_distribution(
            agent,
            preference,
            registry::platform_target(self.options.platform, &self.options.architecture).as_deref(),
        )
        .ok_or_else(|| error(Reason::UnsupportedDistribution))
    }
    pub async fn resolve(
        &self,
        settings: &Value,
        cwd: &Path,
        environment: &Environment,
    ) -> AcpResult<ResolvedRegistryAgent> {
        let command = settings["commandPath"].as_str().unwrap_or_default().trim();
        if settings["source"] == "local" {
            if command.is_empty() {
                return Err(error(Reason::AgentNotConfigured));
            }
            let executable = self
                .commands
                .resolve(command, environment)
                .filter(|path| path.is_file())
                .ok_or_else(|| error(Reason::RunnerUnavailable))?;
            validate_local(&executable)?;
            return Ok(ResolvedRegistryAgent {
                agent: None,
                distribution: None,
                executable,
                args: settings["commandArgs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                cwd: cwd.into(),
                environment: Sensitive::new(environment.clone()),
            });
        }
        let id = settings["agentId"].as_str().unwrap_or_default().trim();
        if id.is_empty() {
            return Err(error(Reason::AgentNotConfigured));
        }
        let agent = self.agent(id, false).await?;
        let preference = match settings["distribution"].as_str() {
            Some("binary") => Preference::Binary,
            Some("npx") => Preference::Npx,
            Some("uvx") => Preference::Uvx,
            _ => Preference::Auto,
        };
        let distribution = self.distribution(&agent, preference)?;
        let mut bin = None;
        let executable = if !command.is_empty() {
            self.commands
                .resolve(command, environment)
                .ok_or_else(|| error(Reason::RunnerUnavailable))?
        } else {
            let gate = self.gate(&agent.id).await;
            let _guard = gate.lock().await;
            match distribution.kind {
                AcpRegistryDistribution::Binary => {
                    let executable = self
                        .binary
                        .install(
                            &agent,
                            distribution
                                .binary
                                .as_ref()
                                .ok_or_else(|| error(Reason::ArchiveInvalid))?,
                        )
                        .await?;
                    self.reservations.lock().await.remove(&agent.id);
                    executable
                }
                _ => {
                    let installed = self
                        .packages
                        .ensure(&agent, &distribution, environment)
                        .await?;
                    bin = Some(installed.bin_directory);
                    installed.executable_path
                }
            }
        };
        let mut environment = environment.clone();
        for (name, value) in &distribution.env {
            if self.options.platform == Platform::Windows {
                environment.retain(|key, _| !key.eq_ignore_ascii_case(name));
                environment.insert(name.to_uppercase(), value.clone());
            } else {
                environment.insert(name.clone(), value.clone());
            }
        }
        if let Some(bin) = bin {
            environment = preferred_path(&environment, &bin, self.options.platform);
        }
        Ok(ResolvedRegistryAgent {
            agent: Some(agent),
            distribution: Some(distribution.kind),
            executable,
            args: distribution.args,
            cwd: cwd.into(),
            environment: Sensitive::new(environment),
        })
    }
    async fn remove(&self, settings: &ServerSettings, agent: &str) -> AcpResult<bool> {
        let referenced = settings.provider_instances.values().any(|instance| {
            instance.driver.0 == "acpRegistry"
                && instance.config.as_ref().is_some_and(|config| {
                    config["source"] != "local"
                        && config["agentId"]
                            .as_str()
                            .is_some_and(|id| id.trim() == agent)
                })
        });
        let mut reservations = self.reservations.lock().await;
        if referenced {
            reservations.remove(agent);
            return Ok(false);
        }
        if reservations
            .get(agent)
            .is_some_and(|expires| self.clock.now_ms() < *expires)
        {
            return Ok(false);
        }
        reservations.remove(agent);
        drop(reservations);
        let tools = directory_below(&self.options.tools_dir, &[])?;
        let root = tools.join(agent);
        let metadata = match fs::symlink_metadata(&root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(error(Reason::InstallFailed)),
        };
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || fs::canonicalize(&root)
                .map_err(|_| error(Reason::InstallFailed))?
                .parent()
                != Some(tools.as_path())
        {
            return Err(error(Reason::InstallFailed));
        }
        let mut removed = false;
        for version in fs::read_dir(&root).map_err(|_| error(Reason::InstallFailed))? {
            let version = version.map_err(|_| error(Reason::InstallFailed))?;
            if !version
                .file_type()
                .map_err(|_| error(Reason::InstallFailed))?
                .is_dir()
            {
                continue;
            }
            let version = version.path();
            for target in fs::read_dir(&version).map_err(|_| error(Reason::InstallFailed))? {
                let target = target.map_err(|_| error(Reason::InstallFailed))?;
                let name = target.file_name();
                let name = name.to_string_lossy();
                if ![
                    "darwin-aarch64",
                    "darwin-x86_64",
                    "linux-aarch64",
                    "linux-x86_64",
                    "windows-aarch64",
                    "windows-x86_64",
                ]
                .contains(&name.as_ref())
                {
                    continue;
                }
                let metadata = target
                    .file_type()
                    .map_err(|_| error(Reason::InstallFailed))?;
                if metadata.is_symlink() || !metadata.is_dir() {
                    return Err(error(Reason::InstallFailed));
                }
                let path = target.path();
                if fs::canonicalize(&path)
                    .map_err(|_| error(Reason::InstallFailed))?
                    .parent()
                    != Some(version.as_path())
                {
                    return Err(error(Reason::InstallFailed));
                }
                fs::remove_dir_all(path).map_err(|_| error(Reason::InstallFailed))?;
                removed = true;
            }
            if fs::read_dir(&version)
                .map_err(|_| error(Reason::InstallFailed))?
                .next()
                .is_none()
            {
                fs::remove_dir(version).map_err(|_| error(Reason::InstallFailed))?;
            }
        }
        if fs::read_dir(&root)
            .map_err(|_| error(Reason::InstallFailed))?
            .next()
            .is_none()
        {
            fs::remove_dir(root).map_err(|_| error(Reason::InstallFailed))?;
        }
        Ok(removed)
    }
}
#[async_trait]
impl AcpCatalogOperations for RegistryAgentCatalog {
    async fn search(&self, input: AcpRegistrySearchInput) -> AcpResult<AcpRegistrySearchResult> {
        let index = self
            .index
            .recent(self.clock.now_ms(), false)
            .await
            .map_err(|_| error(Reason::RegistryUnavailable))?;
        let managers = ["npm", "uv"]
            .into_iter()
            .filter(|manager| {
                self.commands
                    .resolve(manager, &self.options.environment)
                    .is_some()
            })
            .collect::<Vec<_>>();
        Ok(registry::search(
            &index,
            &input.query.0,
            input.limit.flatten().map(|limit| limit as usize),
            registry::platform_target(self.options.platform, &self.options.architecture).as_deref(),
            managers.contains(&"npm"),
            managers.contains(&"uv"),
        ))
    }
    async fn prepare(&self, input: AcpRegistryPrepareInput) -> AcpResult<AcpRegistryPrepareResult> {
        let agent = self.agent(&input.agent_id.0, true).await?;
        let distribution = self.distribution(&agent, Preference::Auto)?;
        let gate = self.gate(&agent.id).await;
        let _guard = gate.lock().await;
        match distribution.kind {
            AcpRegistryDistribution::Binary => {
                self.binary
                    .install(
                        &agent,
                        distribution
                            .binary
                            .as_ref()
                            .ok_or_else(|| error(Reason::ArchiveInvalid))?,
                    )
                    .await?;
                self.reservations
                    .lock()
                    .await
                    .insert(agent.id.clone(), self.clock.now_ms().saturating_add(30_000));
            }
            _ => {
                self.packages
                    .ensure(&agent, &distribution, &self.options.environment)
                    .await?;
            }
        }
        Ok(AcpRegistryPrepareResult {
            agent_id: agent.id.into(),
            version: agent.version.into(),
            distribution: distribution.kind,
            prepared: True,
        })
    }
    async fn uninstall(
        &self,
        input: AcpRegistryManagedBinaryUninstallInput,
    ) -> AcpResult<AcpRegistryManagedBinaryUninstallResult> {
        let id = input.agent_id.0;
        if id.len() > 128
            || !regex::Regex::new(r"^[a-z0-9][a-z0-9._-]*$")
                .expect("agent id regex")
                .is_match(&id)
        {
            return Err(error(Reason::InstallFailed));
        }
        let removed = self
            .settings
            .with_snapshot(async |settings| {
                let gate = self.gate(&id).await;
                let _guard = gate.lock().await;
                Ok(self.remove(settings, &id).await)
            })
            .await
            .map_err(|_| error(Reason::InstallFailed))??;
        Ok(AcpRegistryManagedBinaryUninstallResult {
            agent_id: id.into(),
            removed,
        })
    }
}
