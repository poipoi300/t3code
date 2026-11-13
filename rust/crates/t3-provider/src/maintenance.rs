use crate::{
    environment::{Environment, Platform, process_options},
    versions::{compare_muse_versions, compare_versions, satisfies_range},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString as Text,
    provider_instance::ProviderInstanceId,
    server::{ServerProviderVersionAdvisory, ServerProviderVersionAdvisoryStatus},
};
use tokio::{
    sync::{RwLock, watch},
    time::Instant,
};

#[derive(Clone)]
pub struct MaintenanceAction {
    pub command: String,
    pub executable: String,
    pub args: Vec<String>,
    pub lock_key: String,
    pub environment: Environment,
}
#[derive(Clone)]
pub struct MaintenanceCapabilities {
    pub driver: String,
    pub package: Option<String>,
    pub update: Option<MaintenanceAction>,
    pub install_path: Option<String>,
    pub manual_command: Option<String>,
}
impl std::fmt::Debug for MaintenanceAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MaintenanceAction([REDACTED])")
    }
}
impl std::fmt::Debug for MaintenanceCapabilities {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MaintenanceCapabilities([REDACTED])")
    }
}

pub fn quote_shell_word(word: &str, platform: Platform) -> String {
    if !word.is_empty()
        && word.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || c == '_'
                || "./:@=-".contains(c)
                || c == '\\' && platform == Platform::Windows
        })
    {
        return word.into();
    }
    if platform == Platform::Windows {
        format!(
            "'{}'",
            word.chars()
                .map(|c| if matches!(c, '\'' | '‘' | '’') {
                    format!("{c}{c}")
                } else {
                    c.to_string()
                })
                .collect::<String>()
        )
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

pub fn action(
    executable: &str,
    args: Vec<String>,
    lock_key: &str,
    environment: Environment,
    platform: Platform,
) -> MaintenanceAction {
    let executable_word = quote_shell_word(executable, platform);
    let executable_word = if platform == Platform::Windows && executable_word != executable {
        format!("& {executable_word}")
    } else {
        executable_word
    };
    let command = std::iter::once(executable_word)
        .chain(args.iter().map(|a| quote_shell_word(a, platform)))
        .collect::<Vec<_>>()
        .join(" ");
    MaintenanceAction {
        command,
        executable: executable.into(),
        args,
        lock_key: lock_key.into(),
        environment,
    }
}

pub fn targeted_update(
    capabilities: &MaintenanceCapabilities,
    version: &str,
) -> Option<MaintenanceAction> {
    if version.split('.').count() != 3
        || version
            .split('.')
            .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let mut update = capabilities.update.clone()?;
    let package = capabilities.package.as_ref()?;
    if !update.lock_key.starts_with("npm-global:")
        && ![
            "bun-global",
            "pnpm-global",
            "vite-plus-global",
            "yarn-global",
            "volta",
        ]
        .contains(&update.lock_key.as_str())
    {
        return None;
    }
    let index = update
        .args
        .iter()
        .position(|a| a == package || *a == format!("{package}@latest"))?;
    let previous = update.args[index].clone();
    let next = format!("{package}@{version}");
    update.args[index] = next.clone();
    if let Some(index) = update.command.rfind(&previous) {
        update
            .command
            .replace_range(index..index + previous.len(), &next);
    }
    Some(update)
}

pub fn latest_update(
    capabilities: &MaintenanceCapabilities,
    latest: Option<&str>,
) -> Option<MaintenanceAction> {
    if capabilities
        .update
        .as_ref()
        .is_some_and(|u| u.lock_key == "pnpm-global")
        && let Some(version) = latest
    {
        return targeted_update(capabilities, version).or_else(|| capabilities.update.clone());
    }
    capabilities.update.clone()
}

pub fn version_advisory(
    capabilities: &MaintenanceCapabilities,
    current: Option<&str>,
    latest: Option<&str>,
    checked_at: Option<&str>,
) -> ServerProviderVersionAdvisory {
    let behind = current.zip(latest).is_some_and(|(a, b)| {
        if capabilities.driver == "muse" {
            compare_muse_versions(a, b).is_lt()
        } else {
            compare_versions(a, b).is_lt()
        }
    });
    let status = if current.is_none_or(str::is_empty) || latest.is_none_or(str::is_empty) {
        ServerProviderVersionAdvisoryStatus::Unknown
    } else if behind {
        ServerProviderVersionAdvisoryStatus::BehindLatest
    } else {
        ServerProviderVersionAdvisoryStatus::Current
    };
    let message = if behind {
        Some(Text(if capabilities.update.is_some() {
            "Install the update now or review provider settings.".into()
        } else if let Some(path) = &capabilities.install_path {
            format!(
                "Version {} is out. T3 Code runs {path}; update it with the app or tool that installed it.",
                latest.unwrap_or_default()
            )
        } else {
            format!(
                "Version {} is out. Update it with the app or tool that installed it.",
                latest.unwrap_or_default()
            )
        }))
    } else {
        None
    };
    ServerProviderVersionAdvisory {
        status,
        current_version: current.map(|s| Text(s.into())),
        latest_version: latest.map(|s| Text(s.into())),
        update_command: latest_update(capabilities, latest)
            .map(|u| Text(u.command))
            .or_else(|| capabilities.manual_command.clone().map(Text)),
        can_update: capabilities.update.is_some(),
        can_install_version: Some(targeted_update(capabilities, "0.0.0").is_some()),
        checked_at: checked_at.map(|s| t3_contracts::wire::WireDateTime(s.into())),
        message,
    }
}

pub fn npm_global_prefix(command_path: &str, package: &str) -> Option<String> {
    let slash = command_path.replace('\\', "/");
    let normalized = slash.to_lowercase();
    let segment = format!("/lib/node_modules/{}/", package.to_lowercase());
    let index = normalized.rfind(&segment)?;
    let prefix = &normalized[..index];
    if prefix.contains("/node_modules/") {
        return None;
    }
    if let Some((_, tool)) = prefix.rsplit_once("/mise/installs/")
        && tool.split('/').next() != Some("node")
    {
        return None;
    }
    Some(if index == 0 {
        "/".into()
    } else {
        slash[..index].into()
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomebrewOwnership {
    pub cask: bool,
    pub name: String,
    pub prefix: String,
}
pub fn homebrew_ownership(command_path: &str) -> Option<HomebrewOwnership> {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(.*)/(cellar|caskroom)/([^/]+)/[^/]+/").expect("Homebrew regex")
    });
    let slash = command_path.replace('\\', "/");
    let c = PATTERN.captures(&slash)?;
    Some(HomebrewOwnership {
        cask: c[2].eq_ignore_ascii_case("caskroom"),
        name: c[3].into(),
        prefix: c[1].into(),
    })
}
pub fn parse_homebrew_latest(text: &str, cask: bool) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    let version = if cask {
        value["casks"][0]["version"].as_str()?.split(',').next()?
    } else {
        value["formulae"][0]["versions"]["stable"].as_str()?
    }
    .trim();
    (!version.is_empty() && version != "latest").then(|| version.to_owned())
}
pub fn parse_chocolatey_latest(text: &str, package: &str) -> Option<String> {
    for line in text.lines() {
        let mut pieces = line.trim().split('|');
        let name = pieces.next()?;
        if name.eq_ignore_ascii_case(package) {
            return pieces
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
        }
    }
    None
}
pub fn parse_npm_view(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    let value = value
        .as_str()
        .or_else(|| value.as_array().filter(|a| a.len() == 1)?.first()?.as_str())?
        .trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Resolve only installs with npm-prefix proof; other installer ownership remains explicit.
pub fn npm_maintenance(
    driver: &str,
    package: &str,
    real_path: &str,
    environment: Environment,
    platform: Platform,
) -> Option<MaintenanceCapabilities> {
    let prefix = npm_global_prefix(real_path, package)?;
    let update = action(
        "npm",
        vec![
            "install".into(),
            "-g".into(),
            "--prefix".into(),
            prefix.clone(),
            format!("--allow-scripts={package}"),
            format!("{package}@latest"),
        ],
        &format!("npm-global:{}", prefix.replace('\\', "/").to_lowercase()),
        environment,
        platform,
    );
    Some(MaintenanceCapabilities {
        driver: driver.into(),
        package: Some(package.into()),
        update: Some(update),
        install_path: None,
        manual_command: None,
    })
}

pub async fn configured_npm_latest(
    npm: &str,
    package: &str,
    prefix: Option<&str>,
    env: &Environment,
) -> Option<String> {
    let mut args = vec!["view".into(), "--global".into()];
    if let Some(prefix) = prefix {
        args.extend(["--prefix".into(), prefix.into()]);
    }
    args.extend([
        "--json".into(),
        format!("{package}@latest"),
        "version".into(),
    ]);
    let mut options = process_options(npm, &args, env);
    options.timeout = Some(Duration::from_secs(10));
    options.max_output_bytes = 256 * 1024;
    let output = t3_process::run(options).await.ok()?;
    parse_npm_view(&output.stdout_text())
}

pub fn pnpm_update_args(package: &str, version: Option<&str>) -> Vec<String> {
    let mut args = vec!["add".into(), "-g".into()];
    if version.is_some_and(|v| satisfies_range(v, ">=10.4.0")) {
        args.push(format!("--allow-build={package}"));
    }
    args.push(format!("{package}@latest"));
    args
}

enum VersionEntry {
    Ready {
        expires: Instant,
        value: Option<String>,
    },
    Pending(watch::Receiver<Option<Option<String>>>),
}
type VersionEntries = Arc<Mutex<BTreeMap<String, Arc<VersionEntry>>>>;
pub struct LatestVersions {
    entries: VersionEntries,
}
struct LookupGuard {
    entries: VersionEntries,
    key: String,
    entry: Arc<VersionEntry>,
    sender: Option<watch::Sender<Option<Option<String>>>>,
}
impl Drop for LookupGuard {
    fn drop(&mut self) {
        let mut entries = self.entries.lock().expect("version cache lock");
        if entries
            .get(&self.key)
            .is_some_and(|e| Arc::ptr_eq(e, &self.entry))
        {
            entries.remove(&self.key);
        }
        self.sender.take();
    }
}
impl Default for LatestVersions {
    fn default() -> Self {
        Self {
            entries: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
}
impl LatestVersions {
    pub fn seed(&self, key: String, value: Option<String>) {
        self.entries.lock().expect("version cache lock").insert(
            key,
            Arc::new(VersionEntry::Ready {
                expires: Instant::now() + Duration::from_secs(100 * 365 * 24 * 60 * 60),
                value,
            }),
        );
    }
    pub fn invalidate(&self, key: Option<&str>) {
        let mut entries = self.entries.lock().expect("version cache lock");
        if let Some(key) = key {
            entries.remove(key);
        } else {
            entries.clear();
        }
    }
    pub async fn cached<F, Fut, E>(
        &self,
        key: &str,
        fresh: bool,
        lookup: F,
    ) -> std::result::Result<Option<String>, E>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = std::result::Result<Option<String>, E>>,
    {
        loop {
            let now = Instant::now();
            let claim = {
                let mut entries = self.entries.lock().expect("version cache lock");
                let existing = if fresh {
                    None
                } else {
                    entries
                        .get(key)
                        .filter(|e| match e.as_ref() {
                            VersionEntry::Ready { expires, .. } => *expires > now,
                            VersionEntry::Pending(_) => true,
                        })
                        .cloned()
                };
                if let Some(entry) = existing {
                    Err(entry)
                } else {
                    let (sender, receiver) = watch::channel(None);
                    let entry = Arc::new(VersionEntry::Pending(receiver));
                    entries.insert(key.into(), entry.clone());
                    Ok(LookupGuard {
                        entries: self.entries.clone(),
                        key: key.into(),
                        entry,
                        sender: Some(sender),
                    })
                }
            };
            match claim {
                Err(entry) => match entry.as_ref() {
                    VersionEntry::Ready { value, .. } => return Ok(value.clone()),
                    VersionEntry::Pending(receiver) => {
                        let mut receiver = receiver.clone();
                        if receiver.wait_for(|v| v.is_some()).await.is_ok() {
                            let value = receiver.borrow().clone();
                            if let Some(value) = value {
                                return Ok(value);
                            }
                        }
                    }
                },
                Ok(mut guard) => {
                    let value = lookup().await?;
                    {
                        let mut entries = self.entries.lock().expect("version cache lock");
                        if entries
                            .get(key)
                            .is_some_and(|e| Arc::ptr_eq(e, &guard.entry))
                        {
                            entries.insert(
                                key.into(),
                                Arc::new(VersionEntry::Ready {
                                    expires: now
                                        + Duration::from_secs(if value.is_some() {
                                            3600
                                        } else {
                                            60
                                        }),
                                    value: value.clone(),
                                }),
                            );
                        }
                    }
                    if let Some(sender) = guard.sender.take() {
                        let _ = sender.send(Some(value.clone()));
                    }
                    return Ok(value);
                }
            }
        }
    }
}

#[derive(Default)]
pub struct InstallationGate {
    gates: Mutex<BTreeMap<ProviderInstanceId, Arc<RwLock<()>>>>,
}
impl InstallationGate {
    fn gate(&self, id: &ProviderInstanceId) -> Arc<RwLock<()>> {
        self.gates
            .lock()
            .expect("installation gate lock")
            .entry(id.clone())
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone()
    }
    pub async fn execute<F: Future>(&self, id: &ProviderInstanceId, work: F) -> F::Output {
        let _guard = self.gate(id).read_owned().await;
        work.await
    }
    pub async fn update<F: Future>(&self, ids: &[ProviderInstanceId], work: F) -> F::Output {
        let mut ids = ids.to_vec();
        ids.sort();
        ids.dedup();
        let mut guards = Vec::new();
        for id in ids {
            guards.push(self.gate(&id).write_owned().await);
        }
        work.await
    }
}
