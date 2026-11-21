use crate::{
    BrowserDriver, Error, HtmlService, PreviewService, Result,
    browser::PreviewOptions,
    desktop::DesktopBrowserChannel,
    discovery::PortDiscovery,
    driver::PlaywrightOptions,
    html::{HtmlOptions, HtmlRenderRecovery},
    installer::{ManagedBrowserDriver, PreviewBrowser, UnavailableBrowserDriver},
};
use std::{path::PathBuf, sync::Arc, time::Duration};

/// Anchor module lookup at the installed server package, never a project or desktop cwd.
#[derive(Clone, Debug)]
pub struct RuntimeInputs {
    pub node: Option<PathBuf>,
    pub module_root: PathBuf,
    pub playwright_module: Option<PathBuf>,
    pub electron_run_as_node: bool,
}
#[derive(Clone, Debug)]
pub struct ValidatedRuntime {
    pub node: PathBuf,
    pub playwright_module: PathBuf,
    pub environment: t3_process::Environment,
}
pub struct HostInputs {
    pub runtime: RuntimeInputs,
    pub base_dir: PathBuf,
    pub state_dir: PathBuf,
    pub attachments_dir: PathBuf,
    pub secrets_dir: PathBuf,
    pub sandbox: bool,
    pub desktop: DesktopBrowserChannel,
    pub recovery: Arc<dyn HtmlRenderRecovery>,
}
pub struct PreviewHost {
    pub runtime: ValidatedRuntime,
    pub preview: PreviewService,
    pub html: HtmlService,
    pub discovery: PortDiscovery,
}
fn configuration(message: impl Into<String>) -> Error {
    Error::operation("PreviewHostConfigurationError", message)
}
impl RuntimeInputs {
    pub async fn validate(self) -> Result<ValidatedRuntime> {
        let node=self.node.filter(|node| !node.as_os_str().is_empty()).ok_or_else(|| configuration("Preview requires a Node runtime path (or Electron with ELECTRON_RUN_AS_NODE=1)."))?;
        if !self.module_root.is_absolute() {
            return Err(configuration(
                "Playwright module_root must be an absolute server package directory.",
            ));
        }
        if self
            .playwright_module
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err(configuration("Playwright module entry must be absolute."));
        }
        let node = resolve_node(node)?;
        if std::env::current_exe()
            .ok()
            .is_some_and(|exe| same_file::is_same_file(exe, &node).unwrap_or(false))
        {
            return Err(configuration(
                "Node runtime points to the Rust server executable.",
            ));
        }
        let mut environment = t3_process::Environment::new();
        if self.electron_run_as_node {
            environment.insert("ELECTRON_RUN_AS_NODE".into(), Some("1".into()));
        }
        let mut process = t3_process::ProcessOptions::new(node.as_os_str());
        process.args=vec!["-e".into(), r#"
const {createRequire}=require('node:module');const path=require('node:path');
if (!process.versions.node || typeof Promise.withResolvers !== 'function') {console.error('Node runtime must support Promise.withResolvers');process.exit(1);}
try {if(!require('node:fs').statSync(process.argv[1]).isDirectory())throw Error('module_root is not a directory');const resolve=createRequire(path.join(process.argv[1],'package.json'));const module=resolve.resolve(process.argv[2] || 'playwright-core');const {chromium}=resolve(module);
if (!chromium || ['launch','launchPersistentContext','connectOverCDP'].some(method=>typeof chromium[method]!=='function')) throw Error('missing chromium APIs');
console.log(JSON.stringify({node:process.execPath,module}));}catch(error){console.error('Playwright: '+error.message);process.exit(1);}
"#.into(),self.module_root.clone().into_os_string(),self.playwright_module.map(|path|path.into_os_string()).unwrap_or_default()];
        process.cwd = self
            .module_root
            .ancestors()
            .find(|path| path.is_dir())
            .map(PathBuf::from);
        process.env = environment.clone();
        process.timeout = Some(Duration::from_secs(10));
        process.max_output_bytes = 64 * 1024;
        process.check_exit = false;
        let output = t3_process::run(process)
            .await
            .map_err(|error| configuration(format!("Node runtime validation failed: {error}")))?;
        if output.code != Some(0) {
            return Err(configuration(format!(
                "Node runtime / Playwright validation failed: {}",
                output.stderr_text().trim()
            )));
        }
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|_| {
            configuration("Node runtime returned an invalid Playwright validation response.")
        })?;
        let _resolved = result["node"]
            .as_str()
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| configuration("Node runtime did not report an absolute executable."))?;
        let module = result["module"]
            .as_str()
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| configuration("Playwright did not resolve to an absolute module."))?;
        Ok(ValidatedRuntime {
            node,
            playwright_module: module,
            environment,
        })
    }
}
impl HostInputs {
    fn validate_paths(&self) -> Result<()> {
        for (name, path) in [
            ("base_dir", &self.base_dir),
            ("state_dir", &self.state_dir),
            ("attachments_dir", &self.attachments_dir),
            ("secrets_dir", &self.secrets_dir),
        ] {
            if !path.is_absolute() {
                return Err(configuration(format!("{name} must be absolute.")));
            }
        }
        Ok(())
    }
}
impl PreviewHost {
    /// Validates JavaScript dependencies now; Chrome installation remains lazy and managed.
    pub async fn new(input: HostInputs) -> Result<Self> {
        Self::new_with_discovery(
            input,
            std::env::var("T3CODE_PORT_DISCOVERY").as_deref() != Ok("0"),
        )
        .await
    }
    pub async fn new_with_discovery(input: HostInputs, discovery_enabled: bool) -> Result<Self> {
        input.validate_paths()?;
        let runtime = input.runtime.clone().validate().await?;
        let driver = PreviewServices::managed_driver(&input, &runtime).await?;
        let PreviewServices {
            preview,
            html,
            discovery,
        } = PreviewServices::build(input, Some(driver), discovery_enabled).await?;
        Ok(Self {
            runtime,
            preview,
            html,
            discovery,
        })
    }
}

/// Native session management, HTML publication and port discovery outlive browser availability.
pub struct PreviewServices {
    pub preview: PreviewService,
    pub html: HtmlService,
    pub discovery: PortDiscovery,
}
impl PreviewServices {
    pub async fn start(input: HostInputs, discovery_enabled: bool) -> Result<Self> {
        input.validate_paths()?;
        let worker = async {
            let runtime = input.runtime.clone().validate().await?;
            Self::managed_driver(&input, &runtime).await
        }
        .await;
        if let Err(error) = &worker {
            tracing::info!(%error, "Server browser worker unavailable");
        }
        Self::build(input, worker.ok(), discovery_enabled).await
    }

    async fn managed_driver(
        input: &HostInputs,
        runtime: &ValidatedRuntime,
    ) -> Result<Arc<dyn BrowserDriver>> {
        let profiles = input.state_dir.join("server-browser/profiles");
        let downloads = input.state_dir.join("server-browser/downloads");
        for directory in [&profiles, &downloads, &input.attachments_dir] {
            tokio::fs::create_dir_all(directory)
                .await
                .map_err(|error| {
                    configuration(format!(
                        "Cannot prepare preview directory {}: {error}",
                        directory.display()
                    ))
                })?;
        }
        let installer = PreviewBrowser::new(input.base_dir.clone())?;
        let options = PlaywrightOptions {
            node: runtime.node.clone(),
            cwd: input.state_dir.clone(),
            executable: PathBuf::new(),
            profiles_dir: profiles,
            downloads_dir: downloads,
            attachments_dir: input.attachments_dir.clone(),
            playwright_module: Some(runtime.playwright_module.clone()),
            sandbox: input.sandbox,
        };
        Ok(Arc::new(
            ManagedBrowserDriver::new(installer, options)
                .with_node_environment(runtime.environment.clone()),
        ))
    }

    async fn build(
        input: HostInputs,
        worker: Option<Arc<dyn BrowserDriver>>,
        discovery_enabled: bool,
    ) -> Result<Self> {
        let driver = worker.clone().unwrap_or_else(|| {
            Arc::new(UnavailableBrowserDriver {
                profiles: input.state_dir.join("server-browser/profiles"),
            })
        });
        let preview = PreviewService::new(
            driver,
            PreviewOptions {
                state_dir: input.state_dir.clone(),
                desktop: input.desktop,
            },
        )
        .await;
        let html = HtmlService::new(
            HtmlOptions {
                attachments_dir: input.attachments_dir,
                state_dir: input.state_dir,
                secrets_dir: input.secrets_dir,
                recovery: input.recovery,
            },
            worker,
        );
        Ok(Self {
            preview,
            html,
            discovery: PortDiscovery::with_options(discovery_enabled, "/proc".into())?,
        })
    }
}

fn resolve_node(node: PathBuf) -> Result<PathBuf> {
    let resolved =
        t3_process::resolve_command(node.as_os_str(), &[], &t3_process::Environment::new(), true)
            .map_err(|error| configuration(format!("Node runtime resolution failed: {error}")))?;
    let program = PathBuf::from(resolved.program);
    if program.is_absolute() {
        return Ok(program);
    }
    let cwd = std::env::current_dir()
        .map_err(|error| configuration(format!("Node runtime resolution failed: {error}")))?;
    if program.components().count() > 1 {
        return Ok(cwd.join(program));
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| cwd.join(directory).join(&program))
        .find(|path| is_executable(path))
        .ok_or_else(|| configuration("Node runtime was not found on PATH."))
}
fn is_executable(path: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
