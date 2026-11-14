use crate::{
    Result,
    environment::{Environment, Platform},
    invalid,
};
use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::Duration,
};
use t3_contracts::{
    acp_registry::{
        AcpRegistryDistribution as Distribution, AcpRegistryIntegrity, AcpRegistrySearchAgent,
        AcpRegistrySearchResult,
    },
    base_schemas::TrimmedNonEmptyString as Text,
};
use tokio::sync::Mutex;

pub const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
pub const MAX_REGISTRY_BYTES: usize = 1024 * 1024;
static AGENT_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9._-]*$").expect("agent id regex"));
static VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._+-]*$").expect("registry version regex"));
static NPX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:@[^/@\s]+/[^@\s]+|[a-z0-9][a-z0-9._-]*)@v?[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$").expect("npx regex")
});
static UVX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[a-z0-9][a-z0-9._-]*(?:@|==)v?[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$").expect("uvx regex")
});

#[derive(Clone, Serialize, Deserialize)]
pub struct PackageDistribution {
    pub package: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: Environment,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct BinaryTarget {
    pub archive: String,
    pub cmd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: Environment,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Distributions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<BTreeMap<String, BinaryTarget>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npx: Option<PackageDistribution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uvx: Option<PackageDistribution>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub distribution: Distributions,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryIndex {
    pub version: String,
    pub agents: Vec<Agent>,
}

pub fn https_url(value: &str) -> bool {
    value.len() <= 2048
        && url::Url::parse(value).is_ok_and(|u| {
            u.scheme() == "https" && u.username().is_empty() && u.password().is_none()
        })
}
fn bounded(value: &str, max: usize) -> bool {
    value.encode_utf16().count() <= max
}
fn valid_version(value: &str) -> bool {
    bounded(value, 128) && VERSION.is_match(value)
}
fn valid_arguments(args: &[String], env: &Environment) -> bool {
    args.len() <= 64
        && args.iter().all(|a| bounded(a, 1024))
        && env.iter().all(|(k, v)| bounded(k, 256) && bounded(v, 1024))
}

impl Agent {
    pub fn validate(&self) -> Result<()> {
        let valid = bounded(&self.id, 128)
            && AGENT_ID.is_match(&self.id)
            && !self.name.trim().is_empty()
            && bounded(&self.name, 160)
            && valid_version(&self.version)
            && bounded(&self.description, 1024)
            && self.authors.len() <= 16
            && self.authors.iter().all(|a| bounded(a, 256))
            && self.license.as_ref().is_none_or(|l| bounded(l, 128))
            && [&self.website, &self.repository, &self.icon]
                .into_iter()
                .all(|u| u.as_deref().is_none_or(https_url));
        if !valid {
            return Err(invalid("registry agent", "invalid metadata"));
        }
        for (kind, package) in [
            (Distribution::Npx, self.distribution.npx.as_ref()),
            (Distribution::Uvx, self.distribution.uvx.as_ref()),
        ] {
            if let Some(p) = package
                && (!bounded(&p.package, 256)
                    || !match kind {
                        Distribution::Npx => NPX.is_match(&p.package),
                        _ => UVX.is_match(&p.package),
                    }
                    || !valid_arguments(&p.args, &p.env))
            {
                return Err(invalid(
                    "registry package",
                    "expected an exact version and bounded arguments",
                ));
            }
        }
        for target in self.distribution.binary.iter().flat_map(|b| b.values()) {
            if !https_url(&target.archive)
                || !bounded(&target.cmd, 1024)
                || !valid_arguments(&target.args, &target.env)
                || target
                    .sha256
                    .as_ref()
                    .is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(invalid(
                    "registry binary",
                    "invalid URL, checksum or arguments",
                ));
            }
        }
        Ok(())
    }
}

impl RegistryIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_REGISTRY_BYTES {
            return Err(invalid("registry index", "response exceeds one MiB"));
        }
        #[derive(Deserialize)]
        struct Envelope {
            version: String,
            agents: Vec<serde_json::Value>,
        }
        let mut envelope: Envelope = serde_json::from_slice(bytes)?;
        envelope.version = envelope.version.trim().to_owned();
        if !valid_version(&envelope.version) || envelope.agents.len() > 512 {
            return Err(invalid(
                "registry index",
                "invalid version or too many agents",
            ));
        }
        let agents = envelope
            .agents
            .into_iter()
            .filter_map(|v| serde_json::from_value::<Agent>(v).ok())
            .map(|mut agent| {
                agent.name = agent.name.trim().to_owned();
                agent.version = agent.version.trim().to_owned();
                agent
            })
            .filter(|a| a.validate().is_ok())
            .collect();
        Ok(Self {
            version: envelope.version,
            agents,
        })
    }
}

pub fn platform_target(platform: Platform, architecture: &str) -> Option<String> {
    let arch = match architecture {
        "arm64" | "aarch64" => "aarch64",
        "x64" | "x86_64" => "x86_64",
        _ => return None,
    };
    let os = match platform {
        Platform::Windows => "windows",
        Platform::Macos => "darwin",
        Platform::Linux => "linux",
    };
    Some(format!("{os}-{arch}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preference {
    Auto,
    Binary,
    Npx,
    Uvx,
}
#[derive(Clone)]
pub struct ResolvedDistribution {
    pub kind: Distribution,
    pub args: Vec<String>,
    pub env: Environment,
    pub binary: Option<BinaryTarget>,
    pub package: Option<String>,
}

pub fn resolve_distribution(
    agent: &Agent,
    preference: Preference,
    target: Option<&str>,
) -> Option<ResolvedDistribution> {
    let binary = target.and_then(|t| {
        agent.distribution.binary.as_ref().and_then(|b| {
            b.get(t).or_else(|| {
                if t == "windows-aarch64" {
                    b.get("windows-x86_64")
                } else {
                    None
                }
            })
        })
    });
    let candidates = match preference {
        Preference::Auto => vec![Preference::Binary, Preference::Npx, Preference::Uvx],
        p => vec![p],
    };
    for candidate in candidates {
        if candidate == Preference::Binary
            && let Some(b) = binary
        {
            return Some(ResolvedDistribution {
                kind: Distribution::Binary,
                args: b.args.clone(),
                env: b.env.clone(),
                binary: Some(b.clone()),
                package: None,
            });
        }
        let package = match candidate {
            Preference::Npx => agent.distribution.npx.as_ref(),
            Preference::Uvx => agent.distribution.uvx.as_ref(),
            _ => None,
        };
        if let Some(p) = package {
            return Some(ResolvedDistribution {
                kind: if candidate == Preference::Npx {
                    Distribution::Npx
                } else {
                    Distribution::Uvx
                },
                args: p.args.clone(),
                env: p.env.clone(),
                binary: None,
                package: Some(p.package.clone()),
            });
        }
    }
    None
}

pub fn normalize_command_path(command: &str) -> Option<Vec<String>> {
    let normalized = command.trim().replace('\\', "/");
    let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);
    let segments: Vec<_> = normalized
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if segments.is_empty()
        || normalized.starts_with('/')
        || normalized.as_bytes().get(1) == Some(&b':')
            && normalized.as_bytes()[0].is_ascii_alphabetic()
        || segments.iter().any(|s| matches!(s.as_str(), "." | ".."))
    {
        None
    } else {
        Some(segments)
    }
}
pub fn validate_archive_entries(listing: &str) -> bool {
    listing
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .all(|s| s == "./" || normalize_command_path(s).is_some())
}
pub fn verify_checksum(bytes: &[u8], expected: Option<&str>) -> bool {
    expected.is_none_or(|hash| format!("{:x}", Sha256::digest(bytes)).eq_ignore_ascii_case(hash))
}

pub fn install_root(tools: &Path, agent: &Agent, target: &str) -> Result<PathBuf> {
    agent.validate()?;
    if ![
        "darwin-aarch64",
        "darwin-x86_64",
        "linux-aarch64",
        "linux-x86_64",
        "windows-aarch64",
        "windows-x86_64",
    ]
    .contains(&target)
    {
        return Err(invalid("registry platform", target));
    }
    let version: String = agent
        .version
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    Ok(tools.join(&agent.id).join(version).join(target))
}

pub fn search_rank(agent: &Agent, query: &str) -> Option<u8> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(100);
    }
    let id = agent.id.to_lowercase();
    let name = agent.name.to_lowercase();
    let authors = agent.authors.join(" ").to_lowercase();
    let description = agent.description.to_lowercase();
    if id == query || name == query {
        return Some(0);
    }
    if id.starts_with(&query) || name.starts_with(&query) {
        return Some(10);
    }
    let terms: Vec<_> = query.split_whitespace().collect();
    let identity = format!("{id} {name}");
    let tokens: Vec<_> = identity
        .split(|c: char| !c.is_ascii_lowercase() && !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .collect();
    if terms
        .iter()
        .all(|term| tokens.iter().any(|token| token.starts_with(term)))
    {
        return Some(20);
    }
    if terms.iter().all(|t| id.contains(t) || name.contains(t)) {
        return Some(30);
    }
    if terms.iter().all(|t| authors.contains(t)) {
        return Some(40);
    }
    let all = format!("{id} {name} {authors} {description}");
    terms.iter().all(|t| all.contains(t)).then_some(50)
}

pub fn search(
    index: &RegistryIndex,
    query: &str,
    limit: Option<usize>,
    target: Option<&str>,
    npm_available: bool,
    uv_available: bool,
) -> AcpRegistrySearchResult {
    let mut ranked: Vec<_> = index
        .agents
        .iter()
        .filter_map(|agent| {
            let distribution = resolve_distribution(agent, Preference::Auto, target)?;
            let rank = search_rank(agent, query)?;
            if distribution.kind == Distribution::Npx && !npm_available
                || distribution.kind == Distribution::Uvx && !uv_available
            {
                return None;
            }
            Some((rank, agent, distribution))
        })
        .collect();
    ranked.sort_by(|(ar, a, _), (br, b, _)| {
        ar.cmp(br)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    AcpRegistrySearchResult {
        agents: ranked
            .into_iter()
            .take(limit.unwrap_or(20))
            .map(|(_, a, d)| AcpRegistrySearchAgent {
                id: Text(a.id.clone()),
                name: Text(a.name.clone()),
                version: Text(a.version.clone()),
                description: a.description.clone(),
                authors: a.authors.clone(),
                license: a.license.clone(),
                website: a.website.clone(),
                repository: a.repository.clone(),
                icon: a.icon.clone(),
                integrity: if d.binary.as_ref().is_some_and(|b| b.sha256.is_some()) {
                    AcpRegistryIntegrity::Sha256
                } else {
                    AcpRegistryIntegrity::Registry
                },
                distribution: d.kind,
            })
            .collect(),
    }
}

struct RegistryState {
    index: Option<Arc<RegistryIndex>>,
    fetched_at_ms: Option<i64>,
}
pub struct RegistryCatalog {
    fetch: Arc<dyn RegistryFetch>,
    url: String,
    state: Mutex<RegistryState>,
    revision: AtomicU64,
    cache_file: Option<PathBuf>,
}
#[async_trait]
pub trait RegistryFetch: Send + Sync {
    async fn bytes(&self, url: &str) -> crate::sensitive::ServiceResult<Vec<u8>>;
}
pub struct ReqwestRegistryFetch(pub reqwest::Client);
#[async_trait]
impl RegistryFetch for ReqwestRegistryFetch {
    async fn bytes(&self, url: &str) -> crate::sensitive::ServiceResult<Vec<u8>> {
        use crate::sensitive::ServiceError;
        let mut response = self
            .0
            .get(url)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|_| ServiceError::Network)?
            .error_for_status()
            .map_err(|_| ServiceError::Network)?;
        let mut bytes = vec![];
        while let Some(chunk) = response.chunk().await.map_err(|_| ServiceError::Network)? {
            if bytes.len().saturating_add(chunk.len()) > MAX_REGISTRY_BYTES {
                return Err(ServiceError::Response);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}
impl RegistryCatalog {
    pub async fn cached(&self) -> Option<Arc<RegistryIndex>> {
        let mut state = self.state.lock().await;
        self.load_cached(&mut state).await;
        state.index.clone()
    }
    pub fn with_cache_file(mut self, path: PathBuf) -> Self {
        self.cache_file = Some(path);
        self
    }
    async fn load_cached(&self, state: &mut RegistryState) {
        if state.index.is_some() {
            return;
        }
        let Some(path) = self.cache_file.as_ref() else {
            return;
        };
        if tokio::fs::metadata(path)
            .await
            .is_ok_and(|metadata| metadata.len() <= MAX_REGISTRY_BYTES as u64)
            && let Ok(bytes) = tokio::fs::read(path).await
            && let Ok(index) = RegistryIndex::parse(&bytes)
        {
            state.index = Some(Arc::new(index));
        }
    }
    async fn write_cached(&self, index: &RegistryIndex) {
        let Some(path) = self.cache_file.clone() else {
            return;
        };
        let Ok(bytes) = serde_json::to_vec(index) else {
            return;
        };
        let _ = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            use std::io::Write;
            let parent = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
            std::fs::create_dir_all(parent)?;
            let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
            temporary.write_all(&bytes)?;
            temporary.persist(path).map_err(|error| error.error)?;
            Ok(())
        })
        .await;
    }
    pub fn new(client: reqwest::Client, cache: Option<RegistryIndex>) -> Self {
        Self::with_fetcher(Arc::new(ReqwestRegistryFetch(client)), cache)
    }
    pub fn with_fetcher(fetch: Arc<dyn RegistryFetch>, cache: Option<RegistryIndex>) -> Self {
        Self {
            fetch,
            url: REGISTRY_URL.into(),
            state: Mutex::new(RegistryState {
                index: cache.map(Arc::new),
                fetched_at_ms: None,
            }),
            revision: AtomicU64::new(0),
            cache_file: None,
        }
    }
    pub fn with_url(mut self, url: String) -> Self {
        self.url = url;
        self
    }
    pub async fn recent(&self, now_ms: i64, force: bool) -> Result<Arc<RegistryIndex>> {
        let observed = self.revision.load(Ordering::Acquire);
        let mut state = self.state.lock().await;
        self.load_cached(&mut state).await;
        if self.revision.load(Ordering::Acquire) != observed
            && let Some(index) = state.index.as_ref()
        {
            return Ok(index.clone());
        }
        if !force
            && state.fetched_at_ms.is_some_and(|t| now_ms < t + 300_000)
            && let Some(index) = &state.index
        {
            return Ok(index.clone());
        }
        let fetched = async {
            if !https_url(&self.url) {
                return Err(invalid(
                    "registry URL",
                    "expected HTTPS without credentials",
                ));
            }
            let bytes = tokio::time::timeout(Duration::from_secs(30), self.fetch.bytes(&self.url))
                .await
                .map_err(|_| invalid("registry index", "request timed out"))?
                .map_err(|_| invalid("registry index", "request failed"))?;
            if bytes.len() > MAX_REGISTRY_BYTES {
                return Err(invalid("registry index", "response exceeds one MiB"));
            }
            RegistryIndex::parse(&bytes)
        }
        .await;
        match fetched {
            Ok(index) => {
                self.write_cached(&index).await;
                let index = Arc::new(index);
                state.index = Some(index.clone());
                state.fetched_at_ms = Some(now_ms);
                self.revision.fetch_add(1, Ordering::Release);
                Ok(index)
            }
            Err(error) => {
                self.revision.fetch_add(1, Ordering::Release);
                state.index.clone().ok_or(error)
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct LaunchDescription {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub environment: Environment,
}
macro_rules! redacted_debug {
    ($($name:ty),+)=>{$(impl std::fmt::Debug for $name{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str(concat!(stringify!($name),"([REDACTED])"))}})+};
}
redacted_debug!(
    PackageDistribution,
    BinaryTarget,
    ResolvedDistribution,
    LaunchDescription
);

pub fn launch_description(
    distribution: &ResolvedDistribution,
    installed_executable: &Path,
    command_override: Option<&Path>,
    base: &Environment,
) -> LaunchDescription {
    let mut environment = base.clone();
    environment.extend(distribution.env.clone());
    LaunchDescription {
        executable: command_override
            .unwrap_or(installed_executable)
            .to_path_buf(),
        args: distribution.args.clone(),
        environment,
    }
}

pub fn package_install_description(
    distribution: &ResolvedDistribution,
    root: &Path,
    base: &Environment,
) -> Result<LaunchDescription> {
    let package = distribution
        .package
        .as_ref()
        .ok_or_else(|| invalid("registry package", "missing package"))?;
    let mut env = base.clone();
    match distribution.kind {
        Distribution::Npx => {
            if !NPX.is_match(package) {
                return Err(invalid("registry package", "expected exact npm version"));
            }
            env.insert(
                "npm_config_prefix".into(),
                root.to_string_lossy().into_owned(),
            );
            Ok(LaunchDescription {
                executable: "npm".into(),
                args: vec!["install".into(), "--global".into(), package.clone()],
                environment: env,
            })
        }
        Distribution::Uvx => {
            if !UVX.is_match(package) {
                return Err(invalid("registry package", "expected exact uv version"));
            }
            env.insert("UV_TOOL_DIR".into(), root.to_string_lossy().into_owned());
            env.insert(
                "UV_TOOL_BIN_DIR".into(),
                root.join("bin").to_string_lossy().into_owned(),
            );
            Ok(LaunchDescription {
                executable: "uv".into(),
                args: vec![
                    "tool".into(),
                    "install".into(),
                    "--force".into(),
                    package.clone(),
                ],
                environment: env,
            })
        }
        Distribution::Binary => Err(invalid(
            "registry package",
            "binary distributions have no package manager",
        )),
    }
}
