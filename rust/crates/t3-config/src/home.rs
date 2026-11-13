use crate::{Result, files};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeLayout {
    pub base_dir: PathBuf,
    pub state_dir: PathBuf,
}

macro_rules! accessors {
    ($($name:ident => $base:ident / $path:literal),* $(,)?) => { $(pub fn $name(&self) -> PathBuf { self.$base.join($path) })* };
}

impl HomeLayout {
    pub fn derive(
        base_dir: impl Into<PathBuf>,
        dev_url_present: bool,
        base_dir_explicit: bool,
    ) -> Self {
        let base_dir = base_dir.into();
        let state_dir = base_dir.join(if dev_url_present && !base_dir_explicit {
            "dev"
        } else {
            "userdata"
        });
        Self {
            base_dir,
            state_dir,
        }
    }
    accessors! {
        database => state_dir / "statev2.sqlite", legacy_database => state_dir / "state.sqlite",
        database_wal => state_dir / "statev2.sqlite-wal", database_shm => state_dir / "statev2.sqlite-shm",
        legacy_database_wal => state_dir / "state.sqlite-wal", legacy_database_shm => state_dir / "state.sqlite-shm",
        ownership_database => state_dir / "server-owner.sqlite", settings => state_dir / "settings.json",
        keybindings => state_dir / "keybindings.json", keybindings_migrations => state_dir / "keybindings-migrations",
        runtime_state => state_dir / "server-runtime.json", themes => state_dir / "themes",
        attachments => state_dir / "attachments", browser_artifacts => state_dir / "browser-artifacts",
        logs => state_dir / "logs", server_trace => state_dir / "logs/server.trace.ndjson",
        provider_logs => state_dir / "logs/provider", provider_events => state_dir / "logs/provider/events.log",
        terminal_logs => state_dir / "logs/terminals", anonymous_id => state_dir / "anonymous-id",
        environment_id => state_dir / "environment-id", environment_id_recovery => state_dir / "environment-id.recovery",
        secrets => state_dir / "secrets", provider_processes => state_dir / "provider-processes",
        usage_rates => state_dir / "usage-model-rates.json", model_manifest => state_dir / "model-manifest.json",
        speech_models => state_dir / "speech/models", server_browser => state_dir / "server-browser",
        browser_profiles => state_dir / "server-browser/profiles", browser_launchers => state_dir / "server-browser/launchers",
        browser_downloads => state_dir / "server-browser/downloads", device => state_dir / "device",
        caches => base_dir / "caches", native_app_icons => base_dir / "caches/native-app-icons",
        pull_request_cache => base_dir / "caches/pull-requests", worktrees => base_dir / "worktrees",
        worktree_checkouts => base_dir / "caches/worktree-checkouts", runtime => base_dir / "runtime",
        runtime_versions => base_dir / "runtime/versions", service_state => base_dir / "runtime/service-state.json",
        service_launcher_lock => base_dir / "runtime/service-launcher.lock", service_stop_marker => base_dir / "runtime/.service-stopping",
        service_restart_pending => base_dir / "runtime/.restart-pending", service_launchers => base_dir / "runtime/launchers",
        database_backups => base_dir / "runtime/db-backup", service_env => base_dir / "service.env",
        tools => base_dir / "tools", scratch => base_dir / "scratch", projects => base_dir / "projects",
        source => base_dir / "source", usage_host_id => base_dir / "usage-host-id",
        usage_scan_cache => state_dir / "usage-scan-cache-v15.json",
        service_log => state_dir / "logs/boot-service.log", relay_log => state_dir / "logs/relay-client.ndjson",
        desktop_update_restart => base_dir / "runtime/desktop-update-restart",
        device_agent => state_dir / "device/agent-device", device_bin => state_dir / "device/bin",
        device_targets => state_dir / "device/targets", device_hosts => state_dir / "device/hosts",
        device_eas => state_dir / "device/eas", device_simlock => state_dir / "device/simlock",
        provider_homes => state_dir / "providers", antigravity_temp => state_dir / "antigravity-tmp"
    }
    pub fn secret(&self, name: &str) -> PathBuf {
        self.secrets().join(format!("{name}.bin"))
    }
    pub fn usage_scan_cache_version(&self, version: Option<u8>) -> PathBuf {
        self.state_dir.join(
            version
                .map(|v| format!("usage-scan-cache-v{v}.json"))
                .unwrap_or_else(|| "usage-scan-cache.json".into()),
        )
    }
    pub fn heap_snapshot(&self, pid: u32, timestamp: &str) -> PathBuf {
        self.logs()
            .join(format!("server-{pid}-{timestamp}.heapsnapshot"))
    }
    pub fn antigravity_profile(&self, key: &str) -> PathBuf {
        self.provider_homes().join("antigravity").join(key)
    }
    pub fn device_host_config(&self, hash: &str) -> PathBuf {
        self.device_hosts().join(format!("{hash}.json"))
    }
    pub fn eas_host_ledger(&self, id: &str) -> PathBuf {
        self.device_eas().join(format!("{id}.json"))
    }
    pub fn simlock_host(&self, id: &str) -> PathBuf {
        self.device_simlock().join(id)
    }
    pub fn runtime_version(&self, version: &str) -> PathBuf {
        self.runtime_versions().join(version)
    }
    pub fn runtime_executable(&self, version: &str, windows: bool) -> PathBuf {
        self.runtime_version(version)
            .join(if windows { "t3.exe" } else { "t3" })
    }
    pub fn runtime_sentinel(&self, version: &str) -> PathBuf {
        self.runtime_version(version).join(".install-complete")
    }
    pub fn tool_version(&self, name: &str, version: &str) -> PathBuf {
        self.tools().join(name).join(version)
    }
    pub fn codex_shadow_home(&self, instance_id: &str) -> PathBuf {
        self.provider_homes()
            .join("codex")
            .join(instance_id)
            .join("shadow")
    }
    pub fn terminal_history(&self, thread_id: &str, terminal_id: &str) -> PathBuf {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        use sha2::{Digest, Sha256};
        let filename = |thread: String, terminal: String| {
            if terminal_id == "term-1" {
                format!("{thread}.log")
            } else {
                format!("{thread}_{terminal}.log")
            }
        };
        let encoded = filename(
            format!("terminal_{}", B64.encode(thread_id)),
            B64.encode(terminal_id),
        );
        let name = if encoded.len() <= 143 {
            encoded
        } else {
            filename(
                format!(
                    "terminal_sha256.{}",
                    B64.encode(Sha256::digest(thread_id.as_bytes()))
                ),
                B64.encode(Sha256::digest(terminal_id.as_bytes())),
            )
        };
        self.terminal_logs().join(name)
    }
    pub fn legacy_terminal_history(&self, thread_id: &str) -> PathBuf {
        let name: String = thread_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.terminal_logs().join(format!("{name}.log"))
    }
    pub fn ensure_directories(&self) -> Result<()> {
        for dir in [
            &self.state_dir,
            &self.logs(),
            &self.provider_logs(),
            &self.terminal_logs(),
            &self.attachments(),
            &self.worktrees(),
            &self.caches(),
        ] {
            fs::create_dir_all(dir).map_err(|e| files::io("create directory", dir, e))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(error) =
                fs::set_permissions(&self.state_dir, fs::Permissions::from_mode(0o700))
            {
                tracing::warn!(%error, "Could not restrict state directory permissions");
            }
        }
        Ok(())
    }
    pub fn sweep_stale_pending_attachments(&self, now: std::time::SystemTime) -> usize {
        let Ok(entries) = fs::read_dir(self.attachments()) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let partial = name.ends_with(".part");
                if !partial && !pending_attachment(&name) {
                    return false;
                }
                let max_age = std::time::Duration::from_secs(if partial { 3600 } else { 86400 });
                let expired = fs::metadata(entry.path())
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|m| now.duration_since(m).ok())
                    .is_some_and(|age| age > max_age);
                expired && fs::remove_file(entry.path()).is_ok()
            })
            .count()
    }
}

fn pending_attachment(name: &str) -> bool {
    let name = name
        .strip_suffix(".txt")
        .filter(|s| s.contains('.'))
        .unwrap_or(name);
    let Some((id, _extension)) = name.rsplit_once('.') else {
        return false;
    };
    let Some(rest) = id.strip_prefix("pending-") else {
        return false;
    };
    if rest.len() < 36 || !rest.is_char_boundary(36) {
        return false;
    }
    let uuid = &rest[..36];
    if uuid::Uuid::parse_str(uuid).is_err() || uuid.chars().filter(|c| *c == '-').count() != 4 {
        return false;
    }
    let suffix = &rest[36..];
    suffix.is_empty()
        || suffix.strip_prefix('-').is_some_and(|s| {
            !s.is_empty() && s.len() <= 10 && s.bytes().all(|c| c.is_ascii_alphanumeric())
        })
}

pub fn expand_home(input: &str, home: &Path) -> PathBuf {
    if input == "~" {
        return home.to_owned();
    }
    if input.starts_with("~/") || input.starts_with("~\\") {
        return home.join(input[2..].trim_start_matches(std::path::is_separator));
    }
    input.into()
}

pub fn resolve_path(input: &str, home: &Path, cwd: &Path) -> PathBuf {
    let expanded = expand_home(input, home);
    normalize(if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    })
}

fn normalize(path: PathBuf) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => (),
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// Dev runner precedence is intentionally different from the ordinary server CLI.
pub fn resolve_dev_home(
    flag: Option<&str>,
    worktree_home: Option<&Path>,
    env_home: Option<&str>,
    home: &Path,
    cwd: &Path,
) -> (PathBuf, bool) {
    if let Some(value) = flag.filter(|v| !v.trim().is_empty()) {
        return (resolve_path(value.trim(), home, cwd), true);
    }
    if let Some(value) = worktree_home {
        return (value.to_owned(), true);
    }
    if let Some(value) = env_home.filter(|v| !v.trim().is_empty()) {
        return (resolve_path(value.trim(), home, cwd), true);
    }
    (home.join(".t3"), false)
}

pub fn resolve_worktrees_directory(setting: &str, default: &Path, home: &Path) -> Option<PathBuf> {
    if setting.is_empty() {
        return Some(default.to_owned());
    }
    let expanded = expand_home(setting, home);
    if !expanded.is_absolute() {
        return None;
    }
    let path = normalize(expanded);
    path.parent().map(|_| path.clone())
}
