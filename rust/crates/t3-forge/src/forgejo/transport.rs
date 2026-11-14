use crate::{
    FailureReason, ForgeError, Result, cli::*, forge::CheckoutContext, http::*, support::*,
};
use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
#[derive(Clone)]
pub struct ForgejoConfig {
    pub authentication_cwd: PathBuf,
    pub keys_paths: Vec<PathBuf>,
    pub environment: t3_process::Environment,
}
pub fn keys_paths(home: &Path, data_home: Option<&Path>, app_data: Option<&Path>) -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        ["forgejo-cli", "Cyborus"]
            .into_iter()
            .map(|organization| {
                home.join("Library")
                    .join("Application Support")
                    .join(format!("{organization}.forgejo-cli"))
                    .join("keys.json")
            })
            .collect()
    } else if cfg!(windows) {
        let base = app_data
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join("AppData").join("Roaming"));
        ["forgejo-cli", "Cyborus"]
            .into_iter()
            .map(|organization| {
                base.join(organization)
                    .join("forgejo-cli")
                    .join("data")
                    .join("keys.json")
            })
            .collect()
    } else {
        vec![
            data_home
                .filter(|p| p.is_absolute())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| home.join(".local").join("share"))
                .join("forgejo-cli")
                .join("keys.json"),
        ]
    }
}
#[derive(Clone, Deserialize)]
pub struct Login {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub ssh_host: Option<String>,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub valid: Option<String>,
}
#[derive(Clone)]
pub struct Remote {
    pub host: String,
    pub hostname: String,
    pub path: String,
    pub ssh: bool,
}
pub fn parse_remote(value: &str) -> Option<Remote> {
    let parsed = crate::remote::parse_remote(value)?;
    let path = if let Some(url) = url::Url::parse(value.trim())
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https" | "ssh"))
    {
        url.path()
            .trim_matches('/')
            .trim_end_matches(".git")
            .to_string()
    } else {
        value
            .trim()
            .split_once(':')?
            .1
            .trim_end_matches(".git")
            .into()
    };
    Some(Remote {
        host: parsed.host,
        hostname: parsed.hostname,
        path,
        ssh: parsed.ssh,
    })
}
pub fn match_login(
    logins: &[Login],
    remote: &Remote,
    requested: Option<&str>,
    host_only: bool,
) -> Option<Login> {
    let mut matches = BTreeMap::new();
    for login in logins {
        let Some(base) = parse_remote(&login.url) else {
            continue;
        };
        if requested.is_some_and(|h| !base.host.eq_ignore_ascii_case(h)) {
            continue;
        }
        let matched = if remote.ssh {
            login.ssh_host.as_ref().is_some_and(|h| {
                h.eq_ignore_ascii_case(&remote.host) || h.eq_ignore_ascii_case(&remote.hostname)
            }) || base.hostname == remote.hostname
        } else {
            base.host == remote.host
                && ((host_only && remote.path.is_empty())
                    || base.path.is_empty()
                    || base.path == remote.path
                    || remote.path.starts_with(&format!("{}/", base.path)))
        };
        if matched {
            matches.insert(login.name.clone(), login.clone());
        }
    }
    if matches.len() == 1 {
        return matches.into_values().next();
    }
    let urls: std::collections::HashSet<_> = matches.values().map(|l| l.url.as_str()).collect();
    if urls.len() == 1 {
        matches.values().find(|l| l.default == "true").cloned()
    } else {
        None
    }
}
#[derive(Clone, Deserialize, Default)]
struct Keys {
    hosts: BTreeMap<String, Key>,
    #[serde(default)]
    aliases: BTreeMap<String, String>,
}
#[derive(Clone, Deserialize)]
struct Key {
    #[serde(rename = "type")]
    kind: String,
    token: String,
}
#[derive(Clone)]
pub(super) struct Target {
    pub command: &'static str,
    pub login: Login,
    pub repository: String,
}
pub(super) struct ResolveInput<'a> {
    pub cwd: &'a Path,
    pub context: Option<&'a CheckoutContext>,
    pub repository: Option<&'a str>,
    pub reference: Option<&'a str>,
    pub host: Option<&'a str>,
}
fn error(operation: &str, reason: FailureReason, detail: &str) -> ForgeError {
    ForgeError::provider("forgejo", operation, reason, detail)
}
type AuthCache = Mutex<HashMap<String, (tokio::time::Instant, String)>>;
type Clients = Mutex<HashMap<String, ApiHttp>>;
pub(super) struct Transport {
    pub cli: Arc<dyn ForgeCli>,
    pub git: t3_git::Git,
    pub config: ForgejoConfig,
    pub stop: t3_process::CancellationToken,
    auth: AuthCache,
    clients: Clients,
    limits: Arc<crate::rate_limit::RateLimits>,
}
impl Transport {
    pub fn new(git: t3_git::Git, config: ForgejoConfig) -> Self {
        Self {
            cli: Arc::new(ProcessForgeCli),
            git,
            config,
            stop: t3_process::CancellationToken::new(),
            auth: Default::default(),
            clients: Default::default(),
            limits: Arc::new(crate::rate_limit::RateLimits::default()),
        }
    }
    pub async fn execute(
        &self,
        command: &'static str,
        cwd: &Path,
        op: &str,
        args: Vec<String>,
        stdin: Option<Vec<u8>>,
    ) -> Result<CliResponse> {
        let output = self
            .cli
            .run(CliRequest {
                program: command,
                provider: "forgejo",
                operation: op.into(),
                cwd: cwd.into(),
                args,
                stdin,
                environment: self.config.environment.clone(),
                max_bytes: 8 * 1024 * 1024,
                timeout: Duration::from_secs(30),
                cancellation: self.stop.clone(),
            })
            .await?;
        if output.code != 0 {
            let text = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            let reason = if text.contains("unauthorized")
                || text.contains("authentication")
                || text.contains("401")
            {
                FailureReason::Unauthenticated
            } else if text.contains("429") || text.contains("rate limit") {
                FailureReason::RateLimited
            } else {
                FailureReason::Failed
            };
            return Err(error(
                op,
                reason,
                "Forgejo CLI could not complete the operation.",
            ));
        }
        Ok(output)
    }
    async fn keys(&self) -> Result<Keys> {
        for path in &self.config.keys_paths {
            let metadata = match tokio::fs::metadata(path).await {
                Ok(value) => value,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    return Err(error(
                        "keys",
                        FailureReason::Unauthenticated,
                        "Could not read fj authentication storage.",
                    ));
                }
            };
            if metadata.len() > 1024 * 1024 {
                return Err(error(
                    "keys",
                    FailureReason::Unauthenticated,
                    "fj authentication storage exceeded the byte limit.",
                ));
            }
            let file = tokio::fs::File::open(path).await.map_err(|_| {
                error(
                    "keys",
                    FailureReason::Unauthenticated,
                    "Could not read fj authentication storage.",
                )
            })?;
            let mut bytes = Vec::new();
            file.take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| {
                    error(
                        "keys",
                        FailureReason::Unauthenticated,
                        "Could not read fj authentication storage.",
                    )
                })?;
            if bytes.len() > 1024 * 1024 {
                return Err(error(
                    "keys",
                    FailureReason::Unauthenticated,
                    "fj authentication storage exceeded the byte limit.",
                ));
            }
            let keys: Keys = serde_json::from_slice(&bytes).map_err(|_| {
                error(
                    "keys",
                    FailureReason::Unauthenticated,
                    "fj authentication storage is invalid.",
                )
            })?;
            if keys
                .hosts
                .values()
                .any(|k| !matches!(k.kind.as_str(), "Application" | "OAuth"))
            {
                return Err(error(
                    "keys",
                    FailureReason::Unauthenticated,
                    "fj authentication storage is invalid.",
                ));
            }
            return Ok(keys);
        }
        Ok(Keys::default())
    }
    pub async fn logins(
        &self,
        cwd: &Path,
        command: &'static str,
        remote_url: Option<&str>,
    ) -> Result<Vec<Login>> {
        if command == "tea" {
            let output = self
                .execute(
                    "tea",
                    cwd,
                    "listLogins",
                    vec!["login".into(), "list".into(), "--output=json".into()],
                    None,
                )
                .await?;
            if output.truncated {
                return Err(error(
                    "listLogins",
                    FailureReason::Failed,
                    "The login list exceeded the byte limit.",
                ));
            }
            return serde_json::from_slice(&output.stdout).map_err(|_| {
                error(
                    "listLogins",
                    FailureReason::Failed,
                    "Invalid tea login list.",
                )
            });
        }
        let keys = match self.keys().await {
            Ok(k) => k,
            Err(e) => {
                match self
                    .execute("fj", cwd, "version", vec!["version".into()], None)
                    .await
                {
                    Err(e) if e.reason() == Some(FailureReason::MissingTool) => return Ok(vec![]),
                    _ => return Err(e),
                }
            }
        };
        let remote = remote_url.and_then(parse_remote);
        let mut logins = vec![];
        for host in keys.hosts.keys() {
            let Some(base) = parse_remote(&format!("https://{host}")) else {
                continue;
            };
            if !base.path.is_empty() {
                continue;
            }
            let scheme = if remote
                .as_ref()
                .is_some_and(|r| !r.ssh && r.host == base.host)
                && remote_url.is_some_and(|s| s.to_ascii_lowercase().starts_with("http://"))
            {
                "http"
            } else {
                "https"
            };
            let login = Login {
                name: host.clone(),
                url: format!("{scheme}://{host}"),
                user: String::new(),
                default: "false".into(),
                ssh_host: None,
                valid: None,
            };
            let aliases: Vec<_> = keys
                .aliases
                .iter()
                .filter(|(_, target)| *target == host)
                .map(|(alias, _)| alias.clone())
                .collect();
            if aliases.is_empty() {
                logins.push(login);
            } else {
                for alias in aliases {
                    let mut login = login.clone();
                    login.ssh_host = Some(alias);
                    logins.push(login);
                }
            }
        }
        Ok(logins)
    }
    async fn authenticate(&self, login: &Login) -> Result<String> {
        let mut cache = self.auth.lock().await;
        let keys = self.keys().await?;
        let token = keys.hosts.get(&login.name).map(|k| k.token.as_str());
        if let (Some(token), Some((at, cached))) = (token, cache.get(&login.url))
            && cached == token
            && at.elapsed() < Duration::from_secs(30)
        {
            return Ok(token.into());
        }
        self.execute(
            "fj",
            &self.config.authentication_cwd,
            "authenticate",
            vec![format!("--host={}", login.url), "whoami".into()],
            None,
        )
        .await?;
        let token = self
            .keys()
            .await?
            .hosts
            .remove(&login.name)
            .map(|k| k.token)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                error(
                    "authenticate",
                    FailureReason::Unauthenticated,
                    "fj has no credentials for this server.",
                )
            })?;
        cache.insert(
            login.url.clone(),
            (tokio::time::Instant::now(), token.clone()),
        );
        Ok(token)
    }
    pub async fn resolve(&self, input: ResolveInput<'_>, host_only: bool) -> Result<Target> {
        let reference_remote = input.reference.and_then(parse_remote);
        let mut remote_url = [
            input.reference,
            input.repository,
            input.context.map(|c| c.remote_url.as_str()),
        ]
        .into_iter()
        .flatten()
        .find(|v| parse_remote(v).is_some())
        .map(str::to_string);
        let mut remote = remote_url.as_deref().and_then(parse_remote);
        if remote.is_none() && (input.repository.is_none() || input.host.is_some()) {
            let git = self.git.at(input.cwd);
            let output = git
                .execute(
                    if input.host.is_some() {
                        &["remote", "-v"]
                    } else {
                        &["remote", "get-url", "origin"]
                    },
                    t3_git::ExecuteOptions {
                        allow_nonzero: true,
                        ..Default::default()
                    },
                )
                .await
                .map_err(|_| {
                    error(
                        "resolveRepository",
                        FailureReason::Failed,
                        "Could not resolve the repository remote.",
                    )
                })?;
            remote_url = if let Some(host) = input.host {
                let urls: std::collections::BTreeSet<_> =
                    crate::registry::parse_fetch_remotes(&output.stdout_text())
                        .into_iter()
                        .map(|(_, url)| url)
                        .filter(|u| {
                            u.starts_with("http")
                                && parse_remote(u)
                                    .is_some_and(|r| r.host.eq_ignore_ascii_case(host))
                        })
                        .collect();
                if urls.len() == 1 {
                    urls.into_iter().next()
                } else {
                    let origins: std::collections::BTreeSet<_> = urls
                        .iter()
                        .filter_map(|u| {
                            url::Url::parse(u)
                                .ok()
                                .map(|u| u.origin().ascii_serialization())
                        })
                        .collect();
                    if origins.len() == 1 {
                        origins.into_iter().next()
                    } else {
                        None
                    }
                }
            } else {
                Some(output.stdout_text().trim().into())
            };
            remote = remote_url.as_deref().and_then(parse_remote);
        }
        if let Some(host) = input.host
            && remote.as_ref().is_none_or(|r| {
                !r.ssh && r.host != host.to_lowercase() && r.hostname != host.to_lowercase()
            })
        {
            remote = Some(Remote {
                host: host.to_lowercase(),
                hostname: host.split(':').next().unwrap_or(host).into(),
                ssh: false,
                path: remote.as_ref().map(|r| r.path.clone()).unwrap_or_default(),
            });
        }
        let scheme_remote = if remote.as_ref().is_some_and(|r| r.ssh) {
            input.context.map(|c| c.provider.base_url.as_str())
        } else {
            remote_url.as_deref()
        };
        let fj = self.logins(input.cwd, "fj", scheme_remote).await?;
        let requested = input
            .host
            .or_else(|| input.context.and_then(|c| c.requested_host.as_deref()));
        let only =
            host_only || input.host.is_some() && remote.as_ref().is_none_or(|r| r.path.is_empty());
        let choose = |logins: &[Login]| {
            if let Some(remote) = &remote {
                match_login(
                    logins,
                    remote,
                    if remote.ssh { requested } else { None },
                    only,
                )
            } else {
                logins
                    .iter()
                    .find(|l| l.default == "true")
                    .cloned()
                    .or_else(|| {
                        let names: std::collections::HashSet<_> =
                            logins.iter().map(|l| &l.name).collect();
                        if names.len() == 1 {
                            logins.first().cloned()
                        } else {
                            None
                        }
                    })
            }
        };
        let mut login = choose(&fj);
        let mut command = "fj";
        if login.is_none()
            && fj.iter().any(|l| {
                remote.as_ref().is_none_or(|r| {
                    match_login(
                        std::slice::from_ref(l),
                        r,
                        if r.ssh { requested } else { None },
                        only,
                    )
                    .is_some()
                })
            })
        {
            match self
                .execute("fj", input.cwd, "version", vec!["version".into()], None)
                .await
            {
                Ok(_) => {
                    return Err(error(
                        "resolveRepository",
                        FailureReason::Unauthenticated,
                        "Several fj logins match. Specify the complete server URL.",
                    ));
                }
                Err(e) if e.reason() == Some(FailureReason::MissingTool) => {}
                Err(e) => return Err(e),
            }
        }
        if let Some(held) = &login {
            match self.authenticate(held).await {
                Ok(_) => {}
                Err(e) if e.reason() == Some(FailureReason::MissingTool) => login = None,
                Err(e) => return Err(e),
            }
        }
        if login.is_none() {
            command = "tea";
            login = choose(&self.logins(input.cwd, "tea", None).await?);
        }
        let mut login = login.ok_or_else(|| {
            error(
                "resolveRepository",
                FailureReason::Unauthenticated,
                "No unambiguous Forgejo login serves this repository.",
            )
        })?;
        let base = url::Url::parse(&login.url).map_err(|_| {
            error(
                "resolveRepository",
                FailureReason::Failed,
                "Invalid serving login URL.",
            )
        })?;
        if !matches!(base.scheme(), "http" | "https")
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(error(
                "resolveRepository",
                FailureReason::Failed,
                "Invalid serving login URL.",
            ));
        }
        login.url = login.url.trim_end_matches('/').into();
        if host_only {
            return Ok(Target {
                command,
                login,
                repository: String::new(),
            });
        }
        let path = reference_remote
            .map(|r| r.path)
            .or_else(|| {
                input
                    .repository
                    .filter(|r| parse_remote(r).is_none())
                    .map(str::to_string)
            })
            .or_else(|| remote.map(|r| r.path))
            .unwrap_or_default();
        let mount = base.path().trim_matches('/');
        let path = if !mount.is_empty() && path.split('/').count() > 2 {
            path.strip_prefix(&format!("{mount}/")).unwrap_or(&path)
        } else {
            &path
        };
        let path = path
            .split("/pulls/")
            .next()
            .unwrap_or(path)
            .trim_end_matches(".git");
        if command == "fj" && !path.contains('/') {
            login.user = self.account(input.cwd, &login).await?;
        }
        let repository = if path.contains('/') {
            path.into()
        } else {
            format!("{}/{path}", login.user)
        };
        if repository.split('/').count() != 2
            || repository.split('/').any(|p| {
                p.is_empty() || matches!(p, "." | "..") || p.chars().any(char::is_whitespace)
            })
        {
            return Err(error(
                "resolveRepository",
                FailureReason::Failed,
                "Specify owner/repository or a complete server URL.",
            ));
        }
        Ok(Target {
            command,
            login,
            repository,
        })
    }
    pub async fn account(&self, cwd: &Path, login: &Login) -> Result<String> {
        let target = Target {
            command: "fj",
            login: login.clone(),
            repository: String::new(),
        };
        let response = self
            .request(&target, cwd, "user", Method::GET, None)
            .await?;
        let raw: Value = serde_json::from_slice(&response.stdout).map_err(|_| {
            error(
                "getAccount",
                FailureReason::Failed,
                "Invalid account response.",
            )
        })?;
        text(&raw, "login").map(str::to_string).ok_or_else(|| {
            error(
                "getAccount",
                FailureReason::Failed,
                "No account was returned.",
            )
        })
    }
    pub async fn request(
        &self,
        target: &Target,
        cwd: &Path,
        path: &str,
        method: Method,
        body: Option<Value>,
    ) -> Result<CliResponse> {
        let path = path.trim_start_matches('/');
        let base = url::Url::parse(&format!("{}/api/v1/", target.login.url))
            .map_err(|_| error("api", FailureReason::Failed, "Invalid API URL."))?;
        let requested = base
            .join(path)
            .map_err(|_| error("api", FailureReason::Failed, "Invalid API path."))?;
        if requested.origin() != base.origin()
            || !requested.path().starts_with(base.path())
            || !requested.username().is_empty()
            || requested.password().is_some()
        {
            return Err(error(
                "api",
                FailureReason::Failed,
                "The API path left the serving login.",
            ));
        }
        if target.command == "fj" {
            let token = self.authenticate(&target.login).await?;
            let key = format!(
                "{}:{:x}",
                target.login.url,
                Sha256::digest(token.as_bytes())
            );
            let mut clients = self.clients.lock().await;
            let http = if let Some(http) = clients.get(&key) {
                http.clone()
            } else {
                let http = ApiHttp::new(
                    "forgejo",
                    base.clone(),
                    ApiCredential::Token(token),
                    self.limits.clone(),
                )?
                .with_redirect_policy(ApiRedirectPolicy {
                    path_prefix: base.path().into(),
                    statuses: &[301, 302, 307, 308],
                    write_statuses: &[301, 302, 307, 308],
                    max_hops: 5,
                })
                .with_cancellation(self.stop.child_token());
                if clients.len() >= 16 {
                    clients.clear();
                }
                clients.insert(key, http.clone());
                http
            };
            drop(clients);
            let response = http
                .request("api", method, requested.as_str(), body, 8 * 1024 * 1024)
                .await?;
            if response.truncated || std::str::from_utf8(&response.bytes).is_err() {
                return Err(error(
                    "api",
                    FailureReason::Failed,
                    "Forgejo returned an oversized or invalid response.",
                ));
            }
            return Ok(CliResponse {
                stdout: response.bytes,
                stderr: format!(
                    "HTTP/1.1 {}\n{}",
                    response.status,
                    response
                        .link
                        .map(|l| format!("link: {l}\n"))
                        .unwrap_or_default()
                )
                .into_bytes(),
                code: 0,
                truncated: false,
            });
        }
        let mut args = vec![
            "api".into(),
            "--include".into(),
            format!("--login={}", target.login.name),
            format!("--method={method}"),
        ];
        if !target.repository.is_empty() {
            args.push(format!("--repo={}", target.repository));
        }
        if body.is_some() {
            args.push("--data=@-".into());
        }
        args.push(requested.to_string());
        let response = self
            .execute(
                "tea",
                cwd,
                "api",
                args,
                body.map(|v| v.to_string().into_bytes()),
            )
            .await?;
        let stderr = String::from_utf8_lossy(&response.stderr);
        let status = stderr.lines().find_map(|s| {
            let mut parts = s.split_whitespace();
            parts.next().filter(|s| s.starts_with("HTTP/"))?;
            parts
                .next()?
                .parse::<u16>()
                .ok()
                .filter(|s| (100..=599).contains(s))
        });
        if status.is_none_or(|s| s >= 400) {
            let reason = match status {
                Some(401) => FailureReason::Unauthenticated,
                Some(403) => FailureReason::Forbidden,
                Some(404) => FailureReason::NotFound,
                Some(429) => FailureReason::RateLimited,
                _ => FailureReason::Failed,
            };
            let mut error = error("api", reason, "Forgejo refused the API operation.");
            if let ForgeError::Provider { status: target, .. } = &mut error {
                *target = status;
            }
            return Err(error);
        }
        Ok(response)
    }
    pub async fn shutdown(&self) {
        self.stop.cancel();
        for http in self.clients.lock().await.values() {
            http.shutdown();
        }
    }
}
