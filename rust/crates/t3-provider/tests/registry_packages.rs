#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_provider::{
    acp_service::AcpResult,
    environment::{Environment, Platform},
    registry::*,
    registry_installation::RegistryCommands,
    registry_packages::*,
    sensitive::Sensitive,
};
use tokio::sync::Mutex;
struct Commands {
    manager: PathBuf,
    calls: Mutex<Vec<Vec<String>>>,
    uv: bool,
    installed: Mutex<bool>,
}
#[async_trait]
impl RegistryCommands for Commands {
    fn resolve(&self, command: &str, environment: &Environment) -> Option<PathBuf> {
        if command == if self.uv { "uv" } else { "npm" } {
            return Some(self.manager.clone());
        }
        let directory = environment["PATH"].split(';').next().unwrap();
        let path = Path::new(directory).join(format!("{command}.exe"));
        path.is_file().then_some(path)
    }
    async fn run(
        &self,
        _: &Path,
        args: &[String],
        environment: &Environment,
        _: Duration,
        _: usize,
    ) -> AcpResult<Sensitive<String>> {
        self.calls.lock().await.push(args.into());
        let mut installed = self.installed.lock().await;
        let output = if self.uv {
            match args
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice()
            {
                ["tool", "list"] => {
                    if *installed {
                        "agent-acp v1.2.3\n    - agent\n".into()
                    } else {
                        "".into()
                    }
                }
                ["tool", "dir", "--bin"] => environment["UV_TOOL_BIN_DIR"].clone(),
                ["tool", "install", "--force", "agent-acp==1.2.3"] => {
                    *installed = true;
                    let directory = Path::new(&environment["UV_TOOL_BIN_DIR"]);
                    std::fs::create_dir_all(directory).unwrap();
                    std::fs::write(directory.join("agent.exe"), b"command").unwrap();
                    String::new()
                }
                _ => panic!("unexpected uv command"),
            }
        } else {
            let root = Path::new(&environment["npm_config_prefix"]);
            match args
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice()
            {
                ["root", "--global"] => root.join("node_modules").to_string_lossy().into_owned(),
                ["prefix", "--global"] => root.to_string_lossy().into_owned(),
                ["install", "--global", "@example/agent@1.2.3"] => {
                    *installed = true;
                    let package = root.join("node_modules/@example/agent");
                    std::fs::create_dir_all(&package).unwrap();
                    std::fs::write(package.join("package.json"),json!({"name":"@example/agent","version":"1.2.3","bin":{"other":"other.js","agent":"main.js","assistant":"assistant.js"}}).to_string()).unwrap();
                    std::fs::write(root.join("agent.exe"), b"command").unwrap();
                    String::new()
                }
                _ => panic!("unexpected npm command"),
            }
        };
        Ok(Sensitive::new(output))
    }
}
fn agent() -> Agent {
    serde_json::from_value(json!({"id":"assistant","name":"Assistant","version":"1.0.0","description":"Fixture","distribution":{"npx":{"package":"@example/agent@1.2.3"},"uvx":{"package":"agent-acp==1.2.3"}}})).unwrap()
}
#[tokio::test]
async fn npm_receipts_require_exact_manifests_and_select_the_package_command_before_the_agent_alias()
 {
    let root = tempfile::tempdir().unwrap();
    let commands = Arc::new(Commands {
        manager: root.path().join("npm.exe"),
        calls: Mutex::new(vec![]),
        uv: false,
        installed: Mutex::new(false),
    });
    let installer = RegistryPackageInstaller {
        tools: root.path().join("tools"),
        cache: root.path().join("cache"),
        platform: Platform::Windows,
        commands: commands.clone(),
    };
    let agent = agent();
    let distribution = resolve_distribution(&agent, Preference::Npx, None).unwrap();
    let receipt = installer
        .ensure(&agent, &distribution, &Environment::new())
        .await
        .unwrap();
    assert_eq!(receipt.executable_path.file_name().unwrap(), "agent.exe");
    assert_eq!(receipt.package_version.as_deref(), Some("1.2.3"));
    let calls = commands.calls.lock().await.len();
    installer
        .ensure(&agent, &distribution, &Environment::new())
        .await
        .unwrap();
    assert_eq!(commands.calls.lock().await.len(), calls);
    let package = receipt.package_root.as_ref().unwrap().join("package.json");
    std::fs::write(
        &package,
        json!({"name":"@example/agent","version":"9.9.9","bin":"main.js"}).to_string(),
    )
    .unwrap();
    installer
        .ensure(&agent, &distribution, &Environment::new())
        .await
        .unwrap();
    assert_eq!(
        commands
            .calls
            .lock()
            .await
            .iter()
            .filter(|args| args.first().is_some_and(|arg| arg == "install"))
            .count(),
        2
    );
    let environment = preferred_path(
        &Environment::from([("Path".into(), "C:/existing;C:/preferred".into())]),
        Path::new("C:/preferred"),
        Platform::Windows,
    );
    assert_eq!(environment["PATH"], "C:/preferred;C:/existing");
    assert!(!environment.contains_key("Path"));
}
#[tokio::test]
async fn uv_receipts_are_rechecked_against_tool_list_and_resolve_the_unscoped_acp_command() {
    let root = tempfile::tempdir().unwrap();
    let commands = Arc::new(Commands {
        manager: root.path().join("uv.exe"),
        calls: Mutex::new(vec![]),
        uv: true,
        installed: Mutex::new(false),
    });
    let installer = RegistryPackageInstaller {
        tools: root.path().join("tools"),
        cache: root.path().join("cache"),
        platform: Platform::Windows,
        commands: commands.clone(),
    };
    let agent = agent();
    let distribution = resolve_distribution(&agent, Preference::Uvx, None).unwrap();
    let first = installer
        .ensure(&agent, &distribution, &Environment::new())
        .await
        .unwrap();
    assert_eq!(first.executable_path.file_name().unwrap(), "agent.exe");
    let calls = commands.calls.lock().await.len();
    installer
        .ensure(&agent, &distribution, &Environment::new())
        .await
        .unwrap();
    assert!(commands.calls.lock().await.len() > calls);
    assert_eq!(
        commands
            .calls
            .lock()
            .await
            .iter()
            .filter(|args| args.get(1).is_some_and(|arg| arg == "install"))
            .count(),
        1
    );
}
