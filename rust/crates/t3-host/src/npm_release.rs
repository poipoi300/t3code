use async_trait::async_trait;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::server::{ServerNpmRelease, ServerNpmReleaseError, ServerNpmReleaseErrorTag};
use tokio::{
    sync::{Mutex, OnceCell, Semaphore},
    time::Instant,
};
pub type LookupResult<T> = std::result::Result<T, ServerNpmReleaseError>;
fn failure(reason: &str) -> ServerNpmReleaseError {
    ServerNpmReleaseError {
        tag: ServerNpmReleaseErrorTag::ServerNpmReleaseError,
        reason: reason.into(),
    }
}
pub fn valid_spec(spec: &str) -> bool {
    matches!(spec, "latest" | "nightly") || exact_version(spec)
}
pub fn exact_version(version: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$")
            .expect("constant version regex")
    })
    .is_match(version)
}
pub struct NpmOutput {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub incomplete: bool,
}
#[async_trait]
pub trait NpmReleaseHost: Send + Sync {
    async fn npm_command(&self) -> Option<PathBuf>;
    async fn npm_view(&self, command: &Path, spec: &str) -> LookupResult<NpmOutput>;
    async fn public_registry(&self, spec: &str) -> LookupResult<(u16, Vec<u8>)>;
}
pub struct NativeNpmReleaseHost {
    client: reqwest::Client,
}
impl Default for NativeNpmReleaseHost {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}
#[async_trait]
impl NpmReleaseHost for NativeNpmReleaseHost {
    async fn npm_command(&self) -> Option<PathBuf> {
        #[cfg(windows)]
        return t3_process::resolve_command(
            std::ffi::OsStr::new("npm"),
            &[],
            &Default::default(),
            true,
        )
        .ok()
        .map(|r| PathBuf::from(r.program));
        #[cfg(not(windows))]
        std::env::var_os("PATH").and_then(|p| {
            std::env::split_paths(&p)
                .map(|p| p.join("npm"))
                .find(|p| p.is_file())
        })
    }
    async fn npm_view(&self, command: &Path, spec: &str) -> LookupResult<NpmOutput> {
        let mut options = t3_process::ProcessOptions::new(command.as_os_str()).args([
            "view",
            "--global",
            "--json",
            &format!("t3@{spec}"),
            "version",
            "t3code",
        ]);
        options.timeout = Some(Duration::from_secs(15));
        options.max_output_bytes = 256 * 1024;
        options.output_mode = t3_process::OutputMode::Truncate;
        options.check_exit = false;
        options.timed_out_result = true;
        let output = t3_process::run(options)
            .await
            .map_err(|_| failure("npm could not complete the lookup."))?;
        Ok(NpmOutput {
            code: output.code.unwrap_or(-1),
            stdout: output.stdout,
            incomplete: output.timed_out || output.stdout_truncated || output.stdout_invalid_utf8,
        })
    }
    async fn public_registry(&self, spec: &str) -> LookupResult<(u16, Vec<u8>)> {
        use futures_util::StreamExt;
        let response = self
            .client
            .get(format!("https://registry.npmjs.org/t3/{spec}"))
            .send()
            .await
            .map_err(|_| {
                failure("npm is not installed and the public npm registry could not be reached.")
            })?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Ok((status, Vec::new()));
        }
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| {
                failure("npm is not installed and the public npm registry could not be reached.")
            })?;
            if body.len() + chunk.len() > 256 * 1024 {
                return Err(failure("npm returned an incomplete response."));
            }
            body.extend_from_slice(&chunk);
        }
        Ok((status, body))
    }
}
type Cached = Arc<OnceCell<(Instant, LookupResult<Option<ServerNpmRelease>>)>>;
pub struct NpmRelease {
    host: Arc<dyn NpmReleaseHost>,
    cache: Mutex<VecDeque<(String, Cached)>>,
    permits: Semaphore,
}
impl NpmRelease {
    pub fn new(host: Arc<dyn NpmReleaseHost>) -> Self {
        Self {
            host,
            cache: Mutex::new(VecDeque::new()),
            permits: Semaphore::new(2),
        }
    }
    pub async fn get(&self, spec: &str) -> LookupResult<Option<ServerNpmRelease>> {
        if !valid_spec(spec) {
            return Err(failure("Invalid release version or track."));
        }
        let cell = {
            let mut cache = self.cache.lock().await;
            let now = Instant::now();
            cache.retain(|(_, cell)| cell.get().is_none_or(|(expires, _)| *expires > now));
            if let Some((_, cell)) = cache.iter().find(|(key, _)| key == spec) {
                cell.clone()
            } else {
                if cache.len() == 32 {
                    cache.pop_front();
                }
                let cell = Arc::new(OnceCell::new());
                cache.push_back((spec.into(), cell.clone()));
                cell
            }
        };
        cell.get_or_init(|| async {
            let result = self.lookup(spec).await;
            (Instant::now() + Duration::from_secs(60), result)
        })
        .await
        .1
        .clone()
    }
    async fn lookup(&self, spec: &str) -> LookupResult<Option<ServerNpmRelease>> {
        let lookup = async {
            if let Some(command) = self.host.npm_command().await {
                let _permit = self
                    .permits
                    .acquire()
                    .await
                    .map_err(|_| failure("npm could not complete the lookup."))?;
                let output = self.host.npm_view(&command, spec).await?;
                if output.incomplete
                    || output.stdout.len() > 256 * 1024
                    || std::str::from_utf8(&output.stdout).is_err()
                {
                    return Err(failure("npm returned an incomplete response."));
                }
                if output.code != 0 {
                    let error: serde_json::Value =
                        serde_json::from_slice(&output.stdout).unwrap_or_default();
                    return if error["error"]["code"] == "E404" {
                        Ok(None)
                    } else {
                        Err(failure("npm could not read release metadata."))
                    };
                }
                decode(spec, &output.stdout).map(Some)
            } else {
                let (status, body) = self.host.public_registry(spec).await?;
                if status == 404 {
                    return Ok(None);
                }
                if !(200..300).contains(&status) {
                    return Err(failure(
                        "npm is not installed and the public npm registry could not be reached.",
                    ));
                }
                decode(spec, &body).map(Some)
            }
        };
        tokio::time::timeout(Duration::from_secs(15), lookup)
            .await
            .map_err(|_| failure("npm returned an incomplete response."))?
    }
}
fn decode(spec: &str, body: &[u8]) -> LookupResult<ServerNpmRelease> {
    let invalid = || failure("npm returned invalid release metadata.");
    if body.len() > 256 * 1024 {
        return Err(invalid());
    }
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| invalid())?;
    let manifest = match &value {
        serde_json::Value::Array(a) if a.len() == 1 => &a[0],
        _ => &value,
    };
    let version = manifest
        .as_str()
        .or_else(|| manifest.get("version").and_then(|v| v.as_str()))
        .ok_or_else(invalid)?;
    if !exact_version(version) || (!matches!(spec, "latest" | "nightly") && spec != version) {
        return Err(invalid());
    }
    let database_migration = manifest["t3code"]["databaseMigration"]
        .as_f64()
        .filter(|n| n.is_finite() && n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0)
        .map(|n| n as i64);
    Ok(ServerNpmRelease {
        version: version.into(),
        database_migration,
    })
}
