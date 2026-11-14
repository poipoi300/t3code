use crate::{
    acp_service::{AcpError, AcpResult},
    environment::{Environment, Platform},
    registry::{self, Agent, ResolvedDistribution},
    registry_installation::{RegistryCommands, directory_below},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::acp_registry::{
    AcpRegistryDistribution as Distribution, AcpRegistryOperationErrorReason as Reason,
};
fn failure() -> AcpError {
    AcpError::new(
        Reason::InstallFailed,
        "Could not resolve the installed ACP Registry package command.",
    )
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageReceipt {
    pub agent_id: String,
    pub agent_version: String,
    pub distribution: Distribution,
    pub package_spec: String,
    pub manager_path: PathBuf,
    pub bin_directory: PathBuf,
    pub executable_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_version: Option<String>,
}
impl std::fmt::Debug for PackageReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PackageReceipt([REDACTED])")
    }
}
pub fn preferred_path(
    environment: &Environment,
    directory: &Path,
    platform: Platform,
) -> Environment {
    let old = environment.get("PATH").or_else(|| {
        (platform == Platform::Windows)
            .then(|| {
                environment
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
                    .map(|(_, value)| value)
            })
            .flatten()
    });
    let separator = if platform == Platform::Windows {
        ';'
    } else {
        ':'
    };
    let first = directory.to_string_lossy();
    let mut seen = BTreeSet::new();
    let joined = std::iter::once(first.as_ref())
        .chain(old.into_iter().flat_map(|path| path.split(separator)))
        .filter(|entry| !entry.is_empty())
        .filter(|entry| {
            seen.insert(if platform == Platform::Windows {
                entry.to_lowercase()
            } else {
                (*entry).into()
            })
        })
        .collect::<Vec<_>>()
        .join(&separator.to_string());
    let mut next = environment.clone();
    if platform == Platform::Windows {
        next.retain(|key, _| !key.eq_ignore_ascii_case("PATH"));
    }
    next.insert("PATH".into(), joined);
    next
}
fn manager_path(output: &str) -> AcpResult<PathBuf> {
    let lines: Vec<_> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.len() != 1 || !Path::new(lines[0]).is_absolute() {
        return Err(failure());
    }
    Ok(lines[0].into())
}
fn command_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}
fn manifest(path: &Path, name: &str, version: &str) -> Option<Value> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > 1024 * 1024 {
        return None;
    }
    let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    if value["name"] != name || value["version"] != version {
        return None;
    }
    let valid = match &value["bin"] {
        Value::String(bin) => bin.len() <= 1024,
        Value::Object(bins) => bins.iter().all(|(name, path)| {
            command_name(name) && path.as_str().is_some_and(|path| path.len() <= 1024)
        }),
        _ => false,
    };
    valid.then_some(value)
}
fn npm_command(agent: &Agent, name: &str, manifest: &Value) -> Option<String> {
    let basename = name.rsplit('/').next()?;
    if manifest["bin"].is_string() {
        return Some(basename.into());
    }
    let bins = manifest["bin"].as_object()?;
    let entries: BTreeMap<_, _> = bins.iter().collect();
    if entries.len() == 1 {
        return entries.keys().next().map(|name| (*name).clone());
    }
    if entries.contains_key(&basename.to_owned()) {
        return Some(basename.into());
    }
    if entries.contains_key(&agent.id) {
        return Some(agent.id.clone());
    }
    let unique: BTreeSet<_> = entries.values().filter_map(|path| path.as_str()).collect();
    (unique.len() == 1)
        .then(|| entries.keys().next().map(|name| (*name).clone()))
        .flatten()
}
fn identity(spec: &str, npm: bool) -> AcpResult<(&str, &str)> {
    if npm {
        let (name, version) = spec.rsplit_once('@').ok_or_else(failure)?;
        Ok((name, version.strip_prefix('v').unwrap_or(version)))
    } else {
        let (name, version) = spec
            .rsplit_once("==")
            .or_else(|| spec.rsplit_once('@'))
            .ok_or_else(failure)?;
        Ok((name, version.strip_prefix('v').unwrap_or(version)))
    }
}
pub struct RegistryPackageInstaller {
    pub cache: PathBuf,
    pub tools: PathBuf,
    pub platform: Platform,
    pub commands: Arc<dyn RegistryCommands>,
}
impl RegistryPackageInstaller {
    fn receipt_path(
        &self,
        agent: &Agent,
        distribution: &Distribution,
        manager: &Path,
    ) -> AcpResult<PathBuf> {
        let directory = directory_below(&self.cache, &["acp-registry", "package-installs"])?;
        let kind = if *distribution == Distribution::Npx {
            "npx"
        } else {
            "uvx"
        };
        let hash = format!(
            "{:x}",
            Sha256::digest(format!("{kind}\0{}", manager.to_string_lossy()).as_bytes())
        );
        Ok(directory.join(format!("{}-{}.json", agent.id, &hash[..16])))
    }
    fn root(&self, agent: &Agent, distribution: &Distribution) -> AcpResult<PathBuf> {
        agent.validate().map_err(|_| failure())?;
        let root =
            registry::install_root(&self.tools, agent, "windows-x86_64").map_err(|_| failure())?;
        let version = root
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(failure)?;
        directory_below(
            &self.tools,
            &[
                &agent.id,
                version,
                if *distribution == Distribution::Npx {
                    "npm"
                } else {
                    "python"
                },
            ],
        )
    }
    fn bin(&self, root: &Path, distribution: &Distribution) -> PathBuf {
        if *distribution == Distribution::Npx && self.platform == Platform::Windows {
            root.into()
        } else {
            root.join("bin")
        }
    }
    fn receipt(
        &self,
        agent: &Agent,
        distribution: &Distribution,
        spec: &str,
        manager: &Path,
        bin: &Path,
    ) -> Option<PackageReceipt> {
        let path = self.receipt_path(agent, distribution, manager).ok()?;
        let receipt: PackageReceipt = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        if receipt.agent_id != agent.id
            || receipt.agent_version != agent.version
            || receipt.distribution != *distribution
            || receipt.package_spec != spec
            || receipt.manager_path != manager
            || receipt.bin_directory != bin
            || receipt.executable_path.parent() != Some(bin)
            || !receipt.executable_path.exists()
        {
            return None;
        }
        if *distribution == Distribution::Npx {
            let (name, version) = identity(spec, true).ok()?;
            if receipt.package_version.as_deref() != Some(version)
                || manifest(
                    &receipt.package_root.as_ref()?.join("package.json"),
                    name,
                    version,
                )
                .is_none()
            {
                return None;
            }
        }
        Some(receipt)
    }
    fn write_receipt(&self, agent: &Agent, receipt: &PackageReceipt) -> AcpResult<()> {
        let path = self.receipt_path(agent, &receipt.distribution, &receipt.manager_path)?;
        let mut temporary = tempfile::NamedTempFile::new_in(path.parent().ok_or_else(failure)?)
            .map_err(|_| failure())?;
        use std::io::Write;
        serde_json::to_writer(&mut temporary, receipt).map_err(|_| failure())?;
        temporary.write_all(b"\n").map_err(|_| failure())?;
        temporary.persist(path).map_err(|_| failure())?;
        Ok(())
    }
    async fn query(
        &self,
        manager: &Path,
        args: &[&str],
        environment: &Environment,
    ) -> AcpResult<String> {
        self.commands
            .run(
                manager,
                &args.iter().map(|arg| (*arg).into()).collect::<Vec<_>>(),
                environment,
                Duration::from_secs(30),
                1024 * 1024,
            )
            .await
            .map(|output| output.into_inner())
    }
    async fn discover(
        &self,
        agent: &Agent,
        distribution: &Distribution,
        spec: &str,
        manager: &Path,
        environment: &Environment,
    ) -> AcpResult<Option<PackageReceipt>> {
        let npm = *distribution == Distribution::Npx;
        let (name, version) = identity(spec, npm)?;
        let (bin, package_root, command) = if npm {
            let root = manager_path(
                &self
                    .query(manager, &["root", "--global"], environment)
                    .await?,
            )?;
            let prefix = manager_path(
                &self
                    .query(manager, &["prefix", "--global"], environment)
                    .await?,
            )?;
            let package = root.join(name);
            let Some(manifest) = manifest(&package.join("package.json"), name, version) else {
                return Ok(None);
            };
            let Some(command) = npm_command(agent, name, &manifest) else {
                return Ok(None);
            };
            (
                if self.platform == Platform::Windows {
                    prefix
                } else {
                    prefix.join("bin")
                },
                Some(package),
                vec![command],
            )
        } else {
            let tools = self.query(manager, &["tool", "list"], environment).await?;
            let normalized = |name: &str| {
                regex::Regex::new("[._-]+")
                    .expect("package separator regex")
                    .replace_all(&name.to_ascii_lowercase(), "-")
                    .into_owned()
            };
            let exact = tools.lines().any(|line| {
                let mut parts = line.split_whitespace();
                parts
                    .next()
                    .zip(parts.next())
                    .is_some_and(|(tool, current)| {
                        !line.starts_with(char::is_whitespace)
                            && normalized(tool) == normalized(name)
                            && current.strip_prefix('v') == Some(version)
                    })
            });
            if !exact {
                return Ok(None);
            }
            let bin = manager_path(
                &self
                    .query(manager, &["tool", "dir", "--bin"], environment)
                    .await?,
            )?;
            let basename = name.rsplit('/').next().unwrap_or(name);
            let mut seen = BTreeSet::new();
            let commands = [
                agent.id.as_str(),
                basename,
                basename.strip_suffix("-acp").unwrap_or(basename),
            ]
            .into_iter()
            .filter(|name| command_name(name) && seen.insert((*name).to_owned()))
            .map(str::to_owned)
            .collect();
            (bin, None, commands)
        };
        let environment = preferred_path(environment, &bin, self.platform);
        let Some(executable) = command
            .iter()
            .find_map(|command| self.commands.resolve(command, &environment))
        else {
            return Ok(None);
        };
        Ok(Some(PackageReceipt {
            agent_id: agent.id.clone(),
            agent_version: agent.version.clone(),
            distribution: distribution.clone(),
            package_spec: spec.into(),
            manager_path: manager.into(),
            bin_directory: bin,
            executable_path: executable,
            package_root,
            package_version: Some(version.into()),
        }))
    }
    pub async fn ensure(
        &self,
        agent: &Agent,
        distribution: &ResolvedDistribution,
        environment: &Environment,
    ) -> AcpResult<PackageReceipt> {
        if !matches!(distribution.kind, Distribution::Npx | Distribution::Uvx) {
            return Err(failure());
        }
        agent.validate().map_err(|_| failure())?;
        let spec = distribution.package.as_deref().ok_or_else(failure)?;
        let manager_name = if distribution.kind == Distribution::Npx {
            "npm"
        } else {
            "uv"
        };
        let manager = self
            .commands
            .resolve(manager_name, environment)
            .ok_or_else(|| {
                AcpError::new(
                    Reason::RunnerUnavailable,
                    "The ACP Registry package manager is not available on PATH.",
                )
            })?;
        let root = self.root(agent, &distribution.kind)?;
        let bin = self.bin(&root, &distribution.kind);
        let receipt = self.receipt(agent, &distribution.kind, spec, &manager, &bin);
        if distribution.kind == Distribution::Npx
            && let Some(receipt) = receipt
        {
            return Ok(receipt);
        }
        let mut package_environment = environment.clone();
        if self.platform == Platform::Windows {
            package_environment.retain(|name, _| {
                if distribution.kind == Distribution::Npx {
                    !name.eq_ignore_ascii_case("npm_config_prefix")
                } else {
                    !name.eq_ignore_ascii_case("UV_TOOL_DIR")
                        && !name.eq_ignore_ascii_case("UV_TOOL_BIN_DIR")
                }
            });
        }
        if distribution.kind == Distribution::Npx {
            package_environment.insert(
                "npm_config_prefix".into(),
                root.to_string_lossy().into_owned(),
            );
        } else {
            package_environment.insert("UV_TOOL_DIR".into(), root.to_string_lossy().into_owned());
            package_environment
                .insert("UV_TOOL_BIN_DIR".into(), bin.to_string_lossy().into_owned());
        }
        if let Some(existing) = self
            .discover(
                agent,
                &distribution.kind,
                spec,
                &manager,
                &package_environment,
            )
            .await?
        {
            self.write_receipt(agent, &existing)?;
            return Ok(existing);
        }
        let directory = fs::read_link(&manager)
            .ok()
            .map(|target| {
                if target.is_absolute() {
                    target
                } else {
                    manager.parent().unwrap_or(Path::new("")).join(target)
                }
            })
            .filter(|target| target.file_name() == manager.file_name())
            .and_then(|target| target.parent().map(Path::to_path_buf))
            .or_else(|| manager.parent().map(Path::to_path_buf))
            .ok_or_else(failure)?;
        let environment = preferred_path(&package_environment, &directory, self.platform);
        let args = if distribution.kind == Distribution::Npx {
            vec!["install".into(), "--global".into(), spec.into()]
        } else {
            vec![
                "tool".into(),
                "install".into(),
                "--force".into(),
                spec.into(),
            ]
        };
        self.commands
            .run(
                &manager,
                &args,
                &environment,
                Duration::from_secs(1200),
                1024 * 1024,
            )
            .await?;
        let installed = self
            .discover(
                agent,
                &distribution.kind,
                spec,
                &manager,
                &package_environment,
            )
            .await?
            .ok_or_else(failure)?;
        self.write_receipt(agent, &installed)?;
        Ok(installed)
    }
}
