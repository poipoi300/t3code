use crate::{
    claude_skills::parse_lenient_json,
    environment::{Environment, Platform, process_options},
    maintenance::{self, LatestVersions, MaintenanceCapabilities, action},
    sensitive::{ServiceError, ServiceResult},
    transport::{HttpRequest, HttpTransport, ProviderClock},
};
use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
#[async_trait]
pub trait MaintenanceProbe: Send + Sync {
    fn resolve(&self, command: &str, environment: &Environment) -> Option<String>;
    fn canonical(&self, path: &str) -> Option<String> {
        std::fs::canonicalize(path).ok().map(|p| {
            let s = p.to_string_lossy();
            s.strip_prefix("\\\\?\\").unwrap_or(&s).to_owned()
        })
    }
    async fn run(
        &self,
        command: &str,
        args: &[String],
        environment: &Environment,
        ok: &[i32],
    ) -> Option<String>;
}
pub struct ProcessMaintenanceProbe {
    pub cwd: PathBuf,
}
#[async_trait]
impl MaintenanceProbe for ProcessMaintenanceProbe {
    fn resolve(&self, command: &str, environment: &Environment) -> Option<String> {
        let options = process_options(command, &[], environment);
        t3_process::resolve_command(&options.program, &[], &options.env, false)
            .ok()
            .map(|v| v.program.to_string_lossy().into_owned())
    }
    async fn run(
        &self,
        command: &str,
        args: &[String],
        environment: &Environment,
        ok: &[i32],
    ) -> Option<String> {
        let mut options = process_options(command, args, environment);
        options.cwd = Some(self.cwd.clone());
        options.timeout = Some(Duration::from_secs(10));
        options.max_output_bytes = 256 * 1024;
        options.check_exit = false;
        let output = t3_process::run(options).await.ok()?;
        (ok.contains(&output.code?) && !output.stdout_truncated).then(|| output.stdout_text())
    }
}
pub struct MaintenanceContext {
    pub driver: String,
    pub binary: String,
    pub environment: Environment,
    pub host_environment: Environment,
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub platform: Platform,
    pub shared_codex_home: PathBuf,
}
#[derive(Clone)]
struct Resolved {
    capabilities: MaintenanceCapabilities,
    lookup: Lookup,
    public_fallback: bool,
}
#[derive(Clone)]
enum Lookup {
    Npm,
    NativeCodex,
    NativeClaude,
    Mise {
        command: String,
        tool: String,
    },
    Pnpm,
    Bun,
    VitePlus,
    Homebrew {
        command: String,
        name: String,
        cask: bool,
    },
    Chocolatey,
}
pub struct NativeMaintenance {
    context: MaintenanceContext,
    probe: Arc<dyn MaintenanceProbe>,
    http: Arc<dyn HttpTransport>,
    clock: Arc<dyn ProviderClock>,
    versions: Arc<LatestVersions>,
    cache: Mutex<Option<(i64, Resolved)>>,
}
impl NativeMaintenance {
    pub fn new(
        context: MaintenanceContext,
        probe: Arc<dyn MaintenanceProbe>,
        http: Arc<dyn HttpTransport>,
        clock: Arc<dyn ProviderClock>,
        versions: Arc<LatestVersions>,
    ) -> Self {
        Self {
            context,
            probe,
            http,
            clock,
            versions,
            cache: Mutex::new(None),
        }
    }
    fn package(&self) -> ServiceResult<&str> {
        match self.context.driver.as_str() {
            "codex" => Ok("@openai/codex"),
            "claudeAgent" => Ok("@anthropic-ai/claude-code"),
            _ => Err(ServiceError::Unsupported),
        }
    }
    fn manual(&self, path: Option<String>) -> ServiceResult<Resolved> {
        Ok(Resolved {
            capabilities: MaintenanceCapabilities {
                driver: self.context.driver.clone(),
                package: Some(self.package()?.into()),
                update: None,
                install_path: path,
                manual_command: None,
            },
            lookup: Lookup::Npm,
            public_fallback: true,
        })
    }
    fn managed(
        &self,
        command: &str,
        args: Vec<String>,
        lock: &str,
        environment: Environment,
        lookup: Lookup,
        public_fallback: bool,
    ) -> ServiceResult<Resolved> {
        Ok(Resolved {
            capabilities: MaintenanceCapabilities {
                driver: self.context.driver.clone(),
                package: Some(self.package()?.into()),
                update: Some(action(
                    command,
                    args,
                    lock,
                    environment,
                    self.context.platform,
                )),
                install_path: None,
                manual_command: None,
            },
            lookup,
            public_fallback,
        })
    }
    async fn run(&self, command: &str, args: &[&str]) -> Option<String> {
        self.probe
            .run(
                command,
                &args.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
                &self.context.environment,
                &[0],
            )
            .await
    }
    pub async fn capabilities(&self, fresh: bool) -> ServiceResult<MaintenanceCapabilities> {
        Ok(self.resolved(fresh).await?.capabilities)
    }
    async fn resolved(&self, fresh: bool) -> ServiceResult<Resolved> {
        let mut cache = self.cache.lock().await;
        let now = self.clock.now_ms();
        if !fresh
            && let Some((_, value)) = cache
                .as_ref()
                .filter(|(at, _)| now >= *at && now - *at < 3_600_000)
            {
                return Ok(value.clone());
        }
        let value = self.resolve().await?;
        *cache = Some((now, value.clone()));
        Ok(value)
    }
    async fn resolve(&self) -> ServiceResult<Resolved> {
        let context = &self.context;
        let package = self.package()?;
        let Some(mut resolved) = self.probe.resolve(&context.binary, &context.environment) else {
            return self.manual(None);
        };
        let Some(mut real) = self.probe.canonical(&resolved) else {
            return self.manual(None);
        };
        if vite(&resolved) {
            let shim = Path::new(&resolved)
                .parent()
                .ok_or(ServiceError::Maintenance)?;
            let data = shim.parent().ok_or(ServiceError::Maintenance)?;
            let mut config_paths = vec![];
            if !context.environment.contains_key("VP_HOME") {
                let normalized = data.to_string_lossy().replace('\\', "/");
                if let Some(home) = normalized.strip_suffix("/.local/share/vite-plus") {
                    config_paths.push(
                        context
                            .environment
                            .get("XDG_CONFIG_HOME")
                            .map(PathBuf::from)
                            .unwrap_or_else(|| PathBuf::from(home).join(".config"))
                            .join("vite-plus/config.json"),
                    );
                }
            }
            config_paths.push(data.join("config.json"));
            let config = config_paths
                .iter()
                .find_map(|p| std::fs::read_to_string(p).ok())
                .and_then(|s| serde_json::from_str::<Value>(&s).ok());
            if config
                .as_ref()
                .is_some_and(|v| v["shimMode"] == "system_first")
            {
                let mut environment = context.environment.clone();
                for (key, value) in &mut environment {
                    if key.eq_ignore_ascii_case("PATH") {
                        let delimiter = if context.platform == Platform::Windows {
                            ';'
                        } else {
                            ':'
                        };
                        *value = value
                            .split(delimiter)
                            .filter(|p| {
                                normalize(p).trim_end_matches('/')
                                    != normalize(&shim.to_string_lossy()).trim_end_matches('/')
                            })
                            .collect::<Vec<_>>()
                            .join(&delimiter.to_string());
                    }
                }
                let name = command_name(&resolved);
                if let Some(next) = self.probe.resolve(&name, &environment) {
                    real = self.probe.canonical(&next).unwrap_or_else(|| next.clone());
                    resolved = next;
                }
            }
        }
        let paths = [resolved.as_str(), real.as_str()];
        if context.driver == "claudeAgent" && context.platform == Platform::Windows {
            if paths.iter().any(|p|regex::Regex::new(r"^[a-z]:/(?:users/[^/]+/appdata/local/microsoft|program files(?: \(x86\))?)/winget/(?:links|packages/anthropic\.claudecode_[^/]+)/claude\.exe$").is_ok_and(|re|re.is_match(&normalize(p)))){return self.manual(Some(real));}
            if paths.iter().any(|p| chocolatey(p, &context.environment)) {
                let mut value = self.manual(Some(real))?;
                value.capabilities.manual_command = Some("choco upgrade claude-code".into());
                value.lookup = Lookup::Chocolatey;
                value.public_fallback = false;
                return Ok(value);
            }
        }
        if let Some(mise) = self.mise(&resolved, &real).await? {
            return Ok(mise);
        }
        if paths
            .iter()
            .any(|p| normalize(p).starts_with("/nix/store/"))
        {
            return self.manual(Some(real));
        }
        let native = if context.driver == "codex" {
            paths.iter().any(|p| codex_standalone_home(p).is_some())
        } else {
            paths.iter().any(|p| {
                let p = normalize(p);
                p.ends_with("/.local/bin/claude.exe")
                    || p.contains("/.local/share/claude/")
                    || p.contains("/claude/versions/")
            })
        };
        if native && maintenance::npm_global_prefix(&real, package).is_none() {
            let mut environment = context.environment.clone();
            if context.driver == "codex" {
                environment.retain(|key, _| !key.eq_ignore_ascii_case("CODEX_HOME"));
                environment.insert(
                    "CODEX_HOME".into(),
                    codex_standalone_home(&real)
                        .or_else(|| codex_standalone_home(&resolved))
                        .unwrap_or_else(|| {
                            context.shared_codex_home.to_string_lossy().into_owned()
                        }),
                );
            }
            return self.managed(
                &resolved,
                vec!["update".into()],
                &format!("{}-native", context.driver),
                environment,
                if context.driver == "codex" {
                    Lookup::NativeCodex
                } else {
                    Lookup::NativeClaude
                },
                false,
            );
        }
        if paths.iter().any(|p| vite(p)) {
            return self.managed(
                "vp",
                vec!["i".into(), "-g".into(), package.into()],
                "vite-plus-global",
                context.environment.clone(),
                Lookup::VitePlus,
                true,
            );
        }
        if paths.iter().any(|p| normalize(p).contains("/.bun/bin/")) {
            return self.managed(
                "bun",
                vec!["i".into(), "-g".into(), format!("{package}@latest")],
                "bun-global",
                context.environment.clone(),
                Lookup::Bun,
                true,
            );
        }
        if paths.iter().any(|p| pnpm(p)) {
            let mut args = vec!["add".into(), "-g".into()];
            if self
                .run("pnpm", &["--version"])
                .await
                .as_deref()
                .is_some_and(|v| crate::versions::satisfies_range(v.trim(), ">=10.4.0"))
            {
                args.push(format!("--allow-build={package}"));
            }
            args.push(format!("{package}@latest"));
            return self.managed(
                "pnpm",
                args,
                "pnpm-global",
                context.environment.clone(),
                Lookup::Pnpm,
                true,
            );
        }
        if paths.iter().any(|p| {
            regex::Regex::new(r"/yarn/(?:data/)?global/node_modules/")
                .is_ok_and(|re| re.is_match(&normalize(p)))
        }) {
            return self.managed(
                "yarn",
                vec!["global".into(), "add".into(), format!("{package}@latest")],
                "yarn-global",
                context.environment.clone(),
                Lookup::Npm,
                true,
            );
        }
        if command_name(&real) == "volta-shim"
            && Path::new(&resolved)
                .parent()
                .and_then(Path::parent)
                .is_some_and(|root| root.join("tools/image/packages").join(package).exists())
        {
            return self.managed(
                "volta",
                vec!["install".into(), format!("{package}@latest")],
                "volta",
                context.environment.clone(),
                Lookup::Npm,
                true,
            );
        }
        let prefix = maintenance::npm_global_prefix(&real, package).or_else(|| {
            (context.platform == Platform::Windows)
                .then(|| Path::new(&resolved).parent())
                .flatten()
                .filter(|dir| {
                    dir.join("node_modules")
                        .join(package)
                        .join("package.json")
                        .exists()
                })
                .map(|p| p.to_string_lossy().into_owned())
        });
        if let Some(prefix) = prefix {
            return self.managed(
                "npm",
                vec![
                    "install".into(),
                    "-g".into(),
                    "--prefix".into(),
                    prefix.clone(),
                    format!("--allow-scripts={package}"),
                    format!("{package}@latest"),
                ],
                &format!("npm-global:{}", normalize(&prefix)),
                context.environment.clone(),
                Lookup::Npm,
                true,
            );
        }
        if let Some(brew) = maintenance::homebrew_ownership(&real) {
            let keg = Path::new(&brew.prefix).join("bin/brew");
            let command = if keg.exists() {
                Some(keg.to_string_lossy().into_owned())
            } else {
                self.probe.resolve("brew", &context.environment)
            };
            if let Some(command) = command {
                let prefix = self
                    .run(&command, &["--prefix"])
                    .await
                    .map(|s| s.trim().to_owned());
                if prefix.is_some_and(|p| {
                    normalize(&self.probe.canonical(&p).unwrap_or(p)) == normalize(&brew.prefix)
                }) {
                    let mut args = vec!["upgrade".into()];
                    if brew.cask {
                        args.push("--cask".into());
                    }
                    args.push(brew.name.clone());
                    let mut value = self.managed(
                        &command,
                        args,
                        "homebrew",
                        context.environment.clone(),
                        Lookup::Homebrew {
                            command: command.clone(),
                            name: brew.name,
                            cask: brew.cask,
                        },
                        false,
                    )?;
                    if let Some(action) = value.capabilities.update.as_mut() {
                        action.command = std::iter::once("brew")
                            .chain(action.args.iter().map(String::as_str))
                            .collect::<Vec<_>>()
                            .join(" ");
                    }
                    return Ok(value);
                }
            }
        }
        self.manual(Some(real))
    }
    async fn mise(&self, resolved: &str, real: &str) -> ServiceResult<Option<Resolved>> {
        let tool = if self.context.driver == "claudeAgent" {
            "claude"
        } else {
            "codex"
        };
        let paths = [resolved, real];
        let shim = command_name(real) == "mise";
        let wrapper = std::fs::metadata(real).is_ok_and(|m| m.len() <= 16 * 1024)
            && std::fs::read_to_string(real).is_ok_and(|s| {
                s.starts_with("#!")
                    && regex::Regex::new(r"\bmise\s+(?:x|exec)\b").is_ok_and(|re| re.is_match(&s))
            });
        let looks = shim
            || wrapper
            || paths.iter().any(|p| {
                let p = normalize(p);
                p.contains("/mise/installs/") && !p.contains("/mise/installs/node/")
                    || p.contains("/mise/shims/")
                    || p.contains("/mise/command-wrappers/")
            });
        if shim && command_name(resolved) != tool {
            return self.manual(Some(real.into())).map(Some);
        }
        let Some(command) = self
            .probe
            .resolve("mise", &self.context.environment)
            .or_else(|| shim.then(|| real.into()))
        else {
            return if looks {
                self.manual(Some(real.into())).map(Some)
            } else {
                Ok(None)
            };
        };
        let which = self
            .run(&command, &["which", tool])
            .await
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .map(|s| self.probe.canonical(&s).unwrap_or(s));
        let binary = if shim { which.as_deref() } else { Some(real) };
        let Some(binary) = binary else {
            return self.manual(Some(real.into())).map(Some);
        };
        let runs = which
            .as_ref()
            .is_some_and(|p| normalize(p) == normalize(binary));
        let listed = self
            .run(&command, &["ls", "--json", "--installed", tool])
            .await
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let Some(listed) = listed else {
            return if runs || looks {
                self.manual(Some(binary.into())).map(Some)
            } else {
                Ok(None)
            };
        };
        let installs = listed
            .as_array()
            .or_else(|| {
                listed
                    .as_object()
                    .filter(|m| m.keys().all(|k| k == tool))
                    .and_then(|m| m.get(tool))
                    .and_then(Value::as_array)
            })
            .filter(|installs| {
                installs.iter().all(|install| {
                    install["install_path"].is_string()
                        && install.get("active").is_none_or(Value::is_boolean)
                        && install
                            .get("requested_version")
                            .is_none_or(|version| version.is_null() || version.is_string())
                })
            });
        let owned: Vec<_> = installs
            .into_iter()
            .flatten()
            .filter(|v| {
                v["install_path"].as_str().is_some_and(|path| {
                    let path = self.probe.canonical(path).unwrap_or_else(|| path.into());
                    normalize(binary)
                        .starts_with(&format!("{}/", normalize(&path).trim_end_matches('/')))
                })
            })
            .collect();
        if owned.is_empty() {
            return if looks {
                self.manual(Some(binary.into())).map(Some)
            } else {
                Ok(None)
            };
        }
        let Some(install) = owned
            .iter()
            .find(|v| v["active"] == true && v["requested_version"] == "latest")
            .filter(|_| runs)
        else {
            return self.manual(Some(binary.into())).map(Some);
        };
        if !shim {
            let directory = Path::new(install["install_path"].as_str().unwrap_or_default())
                .parent()
                .ok_or(ServiceError::Maintenance)?;
            let normalized = normalize(resolved);
            let dirs = [
                directory.to_string_lossy().into_owned(),
                self.probe
                    .canonical(&directory.to_string_lossy())
                    .unwrap_or_else(|| directory.to_string_lossy().into_owned()),
            ];
            if !dirs
                .iter()
                .any(|dir| normalized.starts_with(&format!("{}/latest/", normalize(dir))))
            {
                return self.manual(Some(binary.into())).map(Some);
            }
        }
        let mut value = self.managed(
            &command,
            vec!["upgrade".into(), tool.into()],
            "mise",
            self.context.environment.clone(),
            Lookup::Mise {
                command: command.clone(),
                tool: tool.into(),
            },
            false,
        )?;
        if let Some(action) = value.capabilities.update.as_mut() {
            action.command = format!("mise upgrade {tool}");
        }
        Ok(Some(value))
    }
    pub async fn latest(&self, fresh: bool) -> ServiceResult<Option<String>> {
        let resolved = self.resolved(fresh).await?;
        let bytes = serde_json::to_vec(&self.context.environment.iter().collect::<Vec<_>>())
            .map_err(|_| ServiceError::Response)?;
        let channel = if matches!(resolved.lookup, Lookup::NativeClaude) {
            claude_update_channel(
                &self.context.host_environment,
                &self.context.home,
                &self.context.cwd,
                self.context.platform,
            )
        } else {
            String::new()
        };
        let key = format!(
            "{}\0{}\0{}\0{}\0{:x}",
            self.context.driver,
            self.context.binary,
            resolved
                .capabilities
                .update
                .as_ref()
                .map(|a| a.lock_key.as_str())
                .unwrap_or("manual"),
            channel,
            Sha256::digest(bytes)
        );
        self.versions
            .cached(&key, fresh, || self.lookup(&resolved))
            .await
    }
    async fn lookup(&self, resolved: &Resolved) -> ServiceResult<Option<String>> {
        let package = self.package()?;
        let context = &self.context;
        let found = match &resolved.lookup {
            Lookup::NativeCodex => {
                let request = get("https://releases.openai.com/codex/channels/latest".into());
                match tokio::time::timeout(Duration::from_secs(4), self.http.send(request)).await {
                    Ok(Ok(response)) if (200..300).contains(&response.status) => {
                        response.body.expose()["tag_name"]
                            .as_str()
                            .and_then(|s| s.strip_prefix("rust-v"))
                            .filter(|s| numeric_version(s))
                            .map(str::to_owned)
                    }
                    _ => None,
                }
            }
            Lookup::NativeClaude => {
                let channel = claude_update_channel(
                    &context.host_environment,
                    &context.home,
                    &context.cwd,
                    context.platform,
                );
                match tokio::time::timeout(
                    Duration::from_secs(4),
                    self.http.text(get(format!(
                        "https://downloads.claude.ai/claude-code-releases/{channel}"
                    ))),
                )
                .await
                {
                    Ok(Ok(response)) if (200..300).contains(&response.status) => {
                        Some(response.body.expose().trim().to_owned())
                            .filter(|v| numeric_version(v))
                    }
                    _ => None,
                }
            }
            Lookup::Mise { command, tool } => self
                .run(command, &["latest", tool])
                .await
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty() && !s.chars().any(char::is_whitespace)),
            Lookup::Homebrew {
                command,
                name,
                cask,
            } => self
                .run(command, &["info", "--json=v2", name])
                .await
                .and_then(|s| maintenance::parse_homebrew_latest(&s, *cask)),
            Lookup::Chocolatey => self
                .run(
                    "choco",
                    &["search", "claude-code", "--exact", "--limit-output"],
                )
                .await
                .and_then(|s| maintenance::parse_chocolatey_latest(&s, "claude-code")),
            Lookup::Pnpm => {
                if let Some(command) = self.probe.resolve("pnpm", &context.environment) {
                    let args = [
                        "outdated",
                        "-g",
                        "--reporter=silent",
                        "--format",
                        "json",
                        package,
                    ]
                    .map(str::to_owned);
                    let outdated = self
                        .probe
                        .run(&command, &args, &context.environment, &[0, 1])
                        .await
                        .and_then(|s| serde_json::from_str::<Value>(&s).ok());
                    if let Some(value) = outdated.filter(Value::is_object) {
                        let latest = value[package]["latest"]
                            .as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned);
                        if latest.is_some() {
                            latest
                        } else {
                            self.run(
                                &command,
                                &[
                                    "ls",
                                    "-g",
                                    "--reporter=silent",
                                    "--depth",
                                    "0",
                                    "--json",
                                    package,
                                ],
                            )
                            .await
                            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                            .and_then(|v| {
                                v[0]["dependencies"][package]["version"]
                                    .as_str()
                                    .map(str::to_owned)
                            })
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            Lookup::Bun => {
                if let Some(command) = self.probe.resolve("bun", &context.environment) {
                    let mut env = context.environment.clone();
                    env.insert("FORCE_COLOR".into(), "0".into());
                    self.probe
                        .run(
                            &command,
                            &["outdated".into(), "-g".into(), package.into()],
                            &env,
                            &[0],
                        )
                        .await
                        .and_then(|s| {
                            s.lines().find_map(|line| {
                                let cells: Vec<_> = line.split('|').map(str::trim).collect();
                                (cells.len() == 6 && cells[1] == package)
                                    .then(|| cells[4].trim_end_matches(" *").to_owned())
                                    .filter(|s| !s.is_empty())
                            })
                        })
                } else {
                    None
                }
            }
            Lookup::VitePlus => {
                let node = self
                    .run("vp", &["env", "current", "--json"])
                    .await
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok());
                if let Some(node) = node.filter(|v| {
                    v["node"]["installed"] == true && v["node"]["source"] != "system PATH"
                }) {
                    if let Some(binary) = node["node"]["bin_path"]
                        .as_str()
                        .filter(|s| Path::new(s).is_absolute())
                    {
                        let command = Path::new(binary).parent().map(|p| {
                            p.join(if context.platform == Platform::Windows {
                                "npm.cmd"
                            } else {
                                "npm"
                            })
                        });
                        if let Some(command) = command {
                            let spec = format!("{package}@latest");
                            self.run(
                                &command.to_string_lossy(),
                                &["pack", "--dry-run", "--json=false", "--global", &spec],
                            )
                            .await
                            .and_then(|s| {
                                s.trim()
                                    .strip_prefix(&format!(
                                        "{}-",
                                        package.trim_start_matches('@').replace('/', "-")
                                    ))
                                    .and_then(|s| s.strip_suffix(".tgz"))
                                    .filter(|s| {
                                        !s.is_empty() && !s.chars().any(char::is_whitespace)
                                    })
                                    .map(str::to_owned)
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            Lookup::Npm => None,
        };
        if found.is_some() || !resolved.public_fallback {
            return Ok(found);
        }
        let mut environment = context.host_environment.clone();
        environment.extend(context.environment.clone());
        if let Some(npm) = self.probe.resolve("npm", &environment) {
            let mut args = vec!["view".into(), "--global".into()];
            if let Some(action) = resolved
                .capabilities
                .update
                .as_ref()
                .filter(|a| a.lock_key.starts_with("npm-global:"))
                && let Some(index) = action.args.iter().position(|s| s == "--prefix")
                && let Some(prefix) = action.args.get(index + 1)
            {
                args.extend(["--prefix".into(), prefix.clone()]);
            }
            args.extend([
                "--json".into(),
                format!("{package}@latest"),
                "version".into(),
            ]);
            return Ok(self
                .probe
                .run(&npm, &args, &environment, &[0])
                .await
                .and_then(|s| maintenance::parse_npm_view(&s)));
        }
        let encoded: String = url::form_urlencoded::byte_serialize(package.as_bytes()).collect();
        let response = tokio::time::timeout(
            Duration::from_secs(4),
            self.http
                .send(get(format!("https://registry.npmjs.org/{encoded}/latest"))),
        )
        .await;
        Ok(match response {
            Ok(Ok(response)) if (200..300).contains(&response.status) => {
                response.body.expose()["version"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            }
            _ => None,
        })
    }
}
fn get(url: String) -> HttpRequest {
    HttpRequest {
        method: "GET",
        url,
        headers: BTreeMap::new(),
        body: None,
    }
}
fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}
fn command_name(path: &str) -> String {
    let name = normalize(path)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    [".exe", ".cmd", ".bat", ".ps1"]
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix).map(str::to_owned))
        .unwrap_or(name)
}
fn vite(path: &str) -> bool {
    let path = normalize(path);
    [
        "/.vite-plus/bin/",
        "/.local/share/vite-plus/bin/",
        "/appdata/local/vite-plus/bin/",
    ]
    .iter()
    .any(|segment| path.contains(segment))
}
fn pnpm(path: &str) -> bool {
    let path = normalize(path);
    [
        "/.local/share/pnpm/",
        "/library/pnpm/",
        "/local/share/pnpm/",
        "/appdata/local/pnpm/",
        "/pnpm/global/",
    ]
    .iter()
    .any(|segment| path.contains(segment))
}
fn chocolatey(path: &str, environment: &Environment) -> bool {
    let path = normalize(path);
    if path.ends_with("/chocolatey/bin/claude.exe")
        || path.ends_with("/chocolatey/lib/claude-code/tools/claude.exe")
    {
        return true;
    }
    environment
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("ChocolateyInstall"))
        .is_some_and(|(_, root)| {
            let root = normalize(root);
            let root = root.trim_end_matches('/');
            path == format!("{root}/bin/claude.exe")
                || path == format!("{root}/lib/claude-code/tools/claude.exe")
        })
}
pub fn codex_standalone_home(path: &str) -> Option<String> {
    let slash = path.replace('\\', "/");
    let lower = slash.to_ascii_lowercase();
    let index = lower.rfind("/packages/standalone/")?;
    let home = &slash[..index];
    let root = home.is_empty()
        || home.len() == 2 && home.ends_with(':') && home.as_bytes()[0].is_ascii_alphabetic();
    Some(path[..index + usize::from(root)].into())
}
fn numeric_version(version: &str) -> bool {
    let mut count = 0;
    version.split('.').all(|part| {
        count += 1;
        !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())
    }) && count == 3
}
pub fn claude_update_channel(
    environment: &Environment,
    home: &Path,
    cwd: &Path,
    platform: Platform,
) -> String {
    let managed = match platform {
        Platform::Macos => PathBuf::from("/Library/Application Support/ClaudeCode"),
        Platform::Windows => PathBuf::from(
            environment
                .get("PROGRAMFILES")
                .filter(|s| !s.is_empty())
                .map_or("C:\\Program Files", String::as_str),
        )
        .join("ClaudeCode"),
        Platform::Linux => PathBuf::from("/etc/claude-code"),
    };
    let config = crate::environment::claude_home("", environment, home, cwd);
    let mut paths = vec![
        config.join("settings.json"),
        managed.join("managed-settings.json"),
    ];
    let mut drop_ins: Vec<_> = std::fs::read_dir(managed.join("managed-settings.d"))
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|s| !s.starts_with('.') && s.ends_with(".json"))
        })
        .map(|e| e.path())
        .collect();
    drop_ins.sort();
    paths.extend(drop_ins);
    paths
        .iter()
        .filter_map(|p| {
            std::fs::read_to_string(p)
                .ok()
                .and_then(|s| parse_lenient_json(&s))
        })
        .filter_map(|v| {
            v["autoUpdatesChannel"]
                .as_str()
                .filter(|s| matches!(*s, "latest" | "stable"))
                .map(str::to_owned)
        })
        .next_back()
        .unwrap_or_else(|| "latest".into())
}
