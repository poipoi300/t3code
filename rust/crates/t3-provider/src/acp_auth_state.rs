use crate::{environment::Environment, sensitive::ServiceResult};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use t3_contracts::{
    provider_instance::{ProviderInstanceEnvironment, ProviderInstanceId},
    wire::trim_js,
};
use tokio::sync::Mutex;
#[derive(Serialize)]
struct Local<'a> {
    source: &'static str,
    #[serde(rename = "commandArgs")]
    command_args: &'a Value,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Binding<'a> {
    #[serde(flatten)]
    local: Option<Local<'a>>,
    agent_id: &'a str,
    command_path: &'a str,
    distribution: &'a str,
    auth_method_id: &'a str,
    environment: Vec<&'a t3_contracts::provider_instance::ProviderInstanceEnvironmentVariable>,
    profiles: Vec<Option<&'a str>>,
}
pub struct AcpAuthenticationState {
    file: PathBuf,
    binding: String,
    confirmed: AtomicBool,
    lock: Arc<Mutex<()>>,
    changes: tokio::sync::watch::Sender<bool>,
}
impl std::fmt::Debug for AcpAuthenticationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AcpAuthenticationState([REDACTED])")
    }
}
impl AcpAuthenticationState {
    pub async fn load(
        cache: &Path,
        instance: &ProviderInstanceId,
        settings: &Value,
        environment: &ProviderInstanceEnvironment,
        process: &Environment,
    ) -> Arc<Self> {
        let hash = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
        let instance = serde_json::to_vec(instance).expect("instance id JSON");
        let file = cache.join(format!("acp-auth-{}.json", hash(&instance)));
        let mut environment = environment.iter().collect::<Vec<_>>();
        environment.sort_by(|left, right| left.name.cmp(&right.name));
        let empty_args = serde_json::json!([]);
        let binding = Binding {
            local: (settings["source"] == "local").then(|| Local {
                source: "local",
                command_args: settings.get("commandArgs").unwrap_or(&empty_args),
            }),
            agent_id: trim_js(settings["agentId"].as_str().unwrap_or_default()),
            command_path: trim_js(settings["commandPath"].as_str().unwrap_or_default()),
            distribution: settings["distribution"].as_str().unwrap_or("auto"),
            auth_method_id: trim_js(settings["authMethodId"].as_str().unwrap_or_default()),
            environment,
            profiles: [
                "HOME",
                "XDG_CONFIG_HOME",
                "XDG_DATA_HOME",
                "XDG_STATE_HOME",
                "APPDATA",
                "LOCALAPPDATA",
            ]
            .iter()
            .map(|name| process.get(*name).map(String::as_str))
            .collect(),
        };
        let binding = hash(&serde_json::to_vec(&binding).expect("binding JSON"));
        let saved = tokio::fs::read(&file)
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(|value| value["binding"].is_string() && value["authenticated"].is_boolean());
        let confirmed = saved
            .as_ref()
            .is_some_and(|saved| saved["binding"] == binding && saved["authenticated"] == true);
        let (changes, _) = tokio::sync::watch::channel(confirmed);
        let state = Arc::new(Self {
            file,
            binding,
            confirmed: AtomicBool::new(confirmed),
            lock: Arc::new(Mutex::new(())),
            changes,
        });
        if saved.is_some_and(|saved| saved["binding"] != state.binding) {
            let _ = state.set(false).await;
        }
        state
    }
    pub fn get(&self) -> bool {
        self.confirmed.load(Ordering::Acquire)
    }
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<bool> {
        self.changes.subscribe()
    }
    pub async fn set(&self, confirmed: bool) -> ServiceResult<()> {
        let guard = self.lock.clone().lock_owned().await;
        self.confirmed.store(confirmed, Ordering::Release);
        self.changes.send_if_modified(|current| {
            if *current != confirmed {
                *current = confirmed;
                true
            } else {
                false
            }
        });
        let path = self.file.clone();
        let binding = self.binding.clone();
        let _ = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let _guard = guard;
            use std::io::Write;
            let parent = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
            std::fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            serde_json::to_writer(
                &mut file,
                &serde_json::json!({"binding":binding,"authenticated":confirmed}),
            )?;
            file.write_all(b"\n")?;
            file.persist(path).map_err(|error| error.error)?;
            Ok(())
        })
        .await;
        Ok(())
    }
}
