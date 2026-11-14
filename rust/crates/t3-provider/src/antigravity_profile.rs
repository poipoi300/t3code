use crate::{
    environment::{Environment, Platform, process_options},
    sensitive::{Sensitive, ServiceError, ServiceResult},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
pub const AUTH_PREFIX: &str = "Open the following link to authenticate the ACP server: ";
pub const BROWSER_MARKER: &str = "__T3_ANTIGRAVITY_AUTH_URL__";
pub const SIGN_IN_REQUIRED: &str = "Sign in to Antigravity in Settings before you continue.";
pub fn text_generation_available(profile: &Path) -> bool {
    for (file, key) in [("hooks.json", "hooks"), ("mcp_config.json", "mcpServers")] {
        let path = profile.join("config").join(file);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return false,
        };
        if !metadata.is_file() || metadata.len() > 64000 {
            return false;
        }
        let Some(value) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            return false;
        };
        if !value.is_object() {
            return false;
        }
        let nested = value
            .get(key)
            .filter(|value| !value.is_null())
            .unwrap_or(&value);
        if !nested.as_object().is_some_and(|object| object.is_empty()) {
            return false;
        }
    }
    true
}
const REMOVED: &[&str] = &[
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_CLOUD_PROJECT",
    "GOOGLE_CLOUD_LOCATION",
    "GOOGLE_CLOUD_QUOTA_PROJECT",
    "GOOGLE_GENAI_USE_VERTEXAI",
    "GCLOUD_PROJECT",
    "CLOUDSDK_CORE_PROJECT",
    "AGY_ACP_CCPA_PROJECT",
    "AGY_ACP_ENABLE_OAUTH",
    "GEMINI_HOME",
    "AGY_ACP_FORCE_FILE_STORAGE",
    "ANTIGRAVITY_HARNESS_PATH",
    "BROWSER",
    "PYTHONUNBUFFERED",
    "ELECTRON_RUN_AS_NODE",
    "CHROME_CRASHPAD_PIPE_NAME",
];
#[derive(Clone)]
pub struct AuthConfig {
    pub method: String,
    pub api_key: Sensitive<String>,
    pub gcp_project: String,
    pub gcp_location: String,
}
impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AntigravityAuthConfig([REDACTED])")
    }
}
impl AuthConfig {
    pub fn from_settings(value: &Value) -> Self {
        Self {
            method: value["authMethod"]
                .as_str()
                .unwrap_or("oauth-personal")
                .into(),
            api_key: Sensitive::new(value["apiKey"].as_str().unwrap_or_default().into()),
            gcp_project: value["gcpProject"].as_str().unwrap_or_default().into(),
            gcp_location: value["gcpLocation"].as_str().unwrap_or_default().into(),
        }
    }
    pub fn uses_browser(&self) -> bool {
        matches!(self.method.as_str(), "oauth-personal" | "oauth-business")
    }
    pub fn label(&self) -> &'static str {
        match self.method.as_str() {
            "oauth-business" => "Gemini Enterprise",
            "gemini-api-key" => "Gemini API key",
            "agent-platform" => "Agent Platform",
            _ => "Google account",
        }
    }
    pub fn issue(&self) -> Option<&'static str> {
        match self.method.as_str() {
            "oauth-personal" => None,
            "oauth-business" if !self.gcp_project.is_empty() && !self.gcp_location.is_empty() => {
                None
            }
            "gemini-api-key" if !self.api_key.expose().is_empty() => None,
            "agent-platform"
                if !self.api_key.expose().is_empty()
                    || !self.gcp_project.is_empty() && !self.gcp_location.is_empty() =>
            {
                None
            }
            "oauth-business" => Some(
                "Gemini Enterprise needs a GCP project and location in the Antigravity provider settings.",
            ),
            "gemini-api-key" => {
                Some("Enter a Gemini API key in the Antigravity provider settings.")
            }
            "agent-platform" => Some(
                "Agent Platform needs an API key, or a GCP project and location, in the Antigravity provider settings.",
            ),
            _ => Some("Choose a supported Antigravity authentication method."),
        }
    }
}
pub fn instance_directories(state: &Path, instance: &str) -> (PathBuf, PathBuf) {
    let key = format!("{:x}", Sha256::digest(instance.as_bytes()));
    (
        state.join("providers").join("antigravity").join(&key),
        state.join("antigravity-tmp").join(&key[..12]),
    )
}
pub fn user_home(platform: Platform, environment: &Environment, fallback: &Path) -> PathBuf {
    if platform == Platform::Windows {
        if let Some(home) = environment
            .get("USERPROFILE")
            .filter(|home| !home.is_empty())
        {
            return home.into();
        }
        if let (Some(drive), Some(path)) = (
            environment
                .get("HOMEDRIVE")
                .filter(|value| !value.is_empty()),
            environment
                .get("HOMEPATH")
                .filter(|value| !value.is_empty()),
        ) {
            return format!("{drive}{path}").into();
        }
    } else if let Some(home) = environment.get("HOME").filter(|home| !home.is_empty()) {
        return home.into();
    }
    fallback.into()
}
#[derive(Clone)]
pub struct Profile {
    pub platform: Platform,
    pub gemini_home: PathBuf,
    pub acp_directory: PathBuf,
    pub token_path: PathBuf,
    pub temp_directory: PathBuf,
    pub browser_command: String,
    pub ca_bundle: Option<PathBuf>,
}
impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AntigravityProfile([REDACTED])")
    }
}
impl Profile {
    pub fn environment(
        &self,
        base: &Environment,
        auth: &AuthConfig,
        runtime_temp: Option<&Path>,
    ) -> Environment {
        let mut environment = base
            .iter()
            .filter(|(name, _)| !REMOVED.contains(&name.to_uppercase().as_str()))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Environment>();
        if !auth.api_key.expose().is_empty() {
            let name = match auth.method.as_str() {
                "gemini-api-key" => Some("GEMINI_API_KEY"),
                "agent-platform" => Some("GOOGLE_API_KEY"),
                _ => None,
            };
            if let Some(name) = name {
                environment.insert(name.into(), auth.api_key.expose().clone());
            }
        }
        if !base.contains_key("SSL_CERT_FILE")
            && !base.contains_key("SSL_CERT_DIR")
            && let Some(ca) = &self.ca_bundle
        {
            environment.insert("SSL_CERT_FILE".into(), ca.to_string_lossy().into_owned());
        }
        environment.extend([
            (
                "GEMINI_HOME".into(),
                self.gemini_home.to_string_lossy().into_owned(),
            ),
            ("AGY_ACP_FORCE_FILE_STORAGE".into(), "1".into()),
            ("BROWSER".into(), self.browser_command.clone()),
            ("PYTHONUNBUFFERED".into(), "1".into()),
            ("ELECTRON_RUN_AS_NODE".into(), "1".into()),
        ]);
        let temp = runtime_temp
            .unwrap_or(&self.temp_directory)
            .to_string_lossy()
            .into_owned();
        if self.platform == Platform::Windows {
            environment.insert("TEMP".into(), temp.clone());
            environment.insert("TMP".into(), temp);
        } else {
            environment.insert("TMPDIR".into(), temp);
        }
        environment
    }
    pub fn spawn(
        &self,
        executable: &Path,
        harness: Option<&Path>,
        cwd: &Path,
        environment: &Environment,
        auth: &AuthConfig,
        runtime_temp: Option<&Path>,
    ) -> crate::registry_service::ResolvedRegistryAgent {
        let mut environment = self.environment(environment, auth, runtime_temp);
        let args = if let Some(harness) = harness {
            environment.insert(
                "ANTIGRAVITY_HARNESS_PATH".into(),
                harness.to_string_lossy().into_owned(),
            );
            if self.platform == Platform::Linux {
                vec!["--uid=".into()]
            } else {
                vec![]
            }
        } else {
            vec![
                "acp".into(),
                "--gemini-dir".into(),
                self.gemini_home.to_string_lossy().into_owned(),
                "--app-data-dir".into(),
                "antigravity-acp".into(),
            ]
        };
        crate::registry_service::ResolvedRegistryAgent {
            agent: None,
            distribution: None,
            executable: executable.into(),
            args,
            cwd: cwd.into(),
            environment: Sensitive::new(environment),
        }
    }
}
fn private_directory(path: &Path) -> ServiceResult<()> {
    std::fs::create_dir_all(path).map_err(|_| ServiceError::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| ServiceError::Storage)?;
    }
    Ok(())
}
pub struct PrepareProfile<'a> {
    pub directory: &'a Path,
    pub temp_directory: Option<&'a Path>,
    pub user_home: &'a Path,
    pub browser_helper: &'a Path,
    pub environment: &'a Environment,
    pub platform: Platform,
    pub auth: &'a AuthConfig,
}
pub async fn prepare(input: PrepareProfile<'_>) -> ServiceResult<Profile> {
    let helper = input.browser_helper.to_string_lossy();
    let helper = if input.platform == Platform::Windows {
        helper.replace('\\', "/")
    } else {
        helper.into_owned()
    };
    let separator = if input.platform == Platform::Windows {
        ';'
    } else {
        ':'
    };
    if helper.contains([separator, '\r', '\n', '\0']) || helper.contains("%s") {
        return Err(ServiceError::Unavailable);
    }
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\"'\"'"));
    let directory = std::path::absolute(input.directory).map_err(|_| ServiceError::Storage)?;
    let acp = directory.join("antigravity-acp");
    let temp = input
        .temp_directory
        .map(Path::to_path_buf)
        .unwrap_or_else(|| acp.join("tmp"));
    let ca = if input.platform == Platform::Linux {
        input
            .environment
            .get("NIX_SSL_CERT_FILE")
            .map(String::as_str)
            .into_iter()
            .chain([
                "/etc/ssl/certs/ca-bundle.crt",
                "/etc/ssl/certs/ca-certificates.crt",
                "/etc/pki/tls/certs/ca-bundle.crt",
                "/etc/ssl/cert.pem",
            ])
            .map(PathBuf::from)
            .find(|path| path.is_absolute() && path.exists())
    } else {
        None
    };
    let profile = Profile {
        platform: input.platform,
        token_path: acp.join("acp_token.json"),
        gemini_home: directory.clone(),
        acp_directory: acp.clone(),
        temp_directory: temp.clone(),
        browser_command: format!("{} '%s'", quote(&helper)),
        ca_bundle: ca,
    };
    let url = "https://example.invalid/t3-antigravity-browser-preflight";
    let mut options = process_options(
        &helper,
        &[url.into()],
        &profile.environment(input.environment, input.auth, None),
    );
    options.timeout = Some(Duration::from_secs(30));
    options.max_output_bytes = 4096;
    options.check_exit = false;
    let output = t3_process::run(options)
        .await
        .map_err(|_| ServiceError::Unavailable)?;
    if output.code != Some(0)
        || !output.stdout.is_empty()
        || output.stdout_truncated
        || output.stderr_truncated
        || output.stderr_text() != format!("{BROWSER_MARKER}\"{url}\"\n")
    {
        return Err(ServiceError::Unavailable);
    }
    for path in [&directory, &acp, &temp] {
        private_directory(path)?;
    }
    let mut settings = json!({"auth":{"type":input.auth.method}});
    let mut gcp = json!({});
    if !input.auth.gcp_project.is_empty() {
        gcp["project"] = json!(input.auth.gcp_project);
    }
    if !input.auth.gcp_location.is_empty() {
        gcp["location"] = json!(input.auth.gcp_location);
    }
    if gcp.as_object().is_some_and(|gcp| !gcp.is_empty()) {
        settings["gcp"] = gcp;
    }
    let settings = serde_json::to_vec(&settings).map_err(|_| ServiceError::Storage)?;
    std::fs::write(
        acp.join("settings.json"),
        [settings.as_slice(), b"\n"].concat(),
    )
    .map_err(|_| ServiceError::Storage)?;
    for relative in ["config/skills", "antigravity-cli/skills"] {
        let link = directory.join(relative);
        let target = input.user_home.join(".gemini").join(relative);
        if !target.exists() {
            if std::fs::symlink_metadata(&link)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                let _ = std::fs::remove_dir(&link).or_else(|_| std::fs::remove_file(&link));
            }
            continue;
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&link) {
            if metadata.file_type().is_symlink() {
                let _ = std::fs::remove_dir(&link).or_else(|_| std::fs::remove_file(&link));
            } else if metadata.is_dir()
                && std::fs::read_dir(&link).is_ok_and(|mut entries| entries.next().is_none())
            {
                let _ = std::fs::remove_dir(&link);
            } else {
                continue;
            }
        }
        if let Some(parent) = link.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        #[cfg(windows)]
        let _ = std::os::windows::fs::symlink_dir(&target, &link);
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(&target, &link);
    }
    Ok(profile)
}
