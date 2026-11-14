use super::{GitHub, transport::*};
use crate::{FailureReason, ForgeError, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use t3_contracts::source_control::{
    SourceControlAppAccessStatus, SourceControlSignInFailure as SignInFailure,
    SourceControlSignInPrompt,
};
use tokio::sync::{Mutex, watch};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSession {
    pub access_token: String,
    pub access_token_expires_at: Option<u64>,
    pub refresh_token: Option<String>,
    pub refresh_token_expires_at: Option<u64>,
}
impl std::fmt::Debug for AppSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppSession").finish_non_exhaustive()
    }
}

/// Compare-and-replace must be atomic with settings updates and sign-out.
#[async_trait]
pub trait AppSessionStore: Send + Sync {
    fn revision(&self) -> u64 {
        0
    }
    async fn read(&self, host: &str) -> Result<Option<AppSession>>;
    async fn write(&self, host: &str, session: AppSession) -> Result<()>;
    async fn replace(
        &self,
        host: &str,
        previous_token: &str,
        session: Option<AppSession>,
    ) -> Result<bool>;
}

#[derive(Default)]
pub struct MemoryAppSessions(
    Mutex<HashMap<String, AppSession>>,
    std::sync::atomic::AtomicU64,
);
#[async_trait]
impl AppSessionStore for MemoryAppSessions {
    fn revision(&self) -> u64 {
        self.1.load(std::sync::atomic::Ordering::Acquire)
    }
    async fn read(&self, host: &str) -> Result<Option<AppSession>> {
        Ok(self.0.lock().await.get(host).cloned())
    }
    async fn write(&self, host: &str, session: AppSession) -> Result<()> {
        self.0.lock().await.insert(host.into(), session);
        self.1.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Ok(())
    }
    async fn replace(&self, host: &str, previous: &str, next: Option<AppSession>) -> Result<bool> {
        let mut sessions = self.0.lock().await;
        if sessions
            .get(host)
            .is_none_or(|s| s.access_token != previous)
        {
            return Ok(false);
        }
        if let Some(next) = next {
            sessions.insert(host.into(), next);
        } else {
            sessions.remove(host);
        }
        self.1.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Ok(true)
    }
}

struct Pending {
    result: watch::Sender<Option<Result<()>>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Pending {
    fn fail_if_pending(&self, host: &str, reason: SignInFailure, detail: &str) {
        self.result.send_if_modified(|result| {
            if result.is_some() {
                return false;
            }
            *result = Some(Err(failure(host, reason, detail)));
            true
        });
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
pub struct AppSignIn {
    stop: t3_process::CancellationToken,
    client_id: String,
    store: Arc<dyn AppSessionStore>,
    client: Client,
    endpoint: Option<String>,
    refresh_gate: Mutex<()>,
    refused: Mutex<HashSet<String>>,
    pending: Mutex<HashMap<String, Pending>>,
}
fn failure(host: &str, reason: SignInFailure, detail: &str) -> ForgeError {
    ForgeError::SignIn {
        host: host.into(),
        reason,
        detail: detail.into(),
    }
}
fn normalized(host: &str) -> String {
    host.trim().to_lowercase()
}
impl AppSignIn {
    pub fn revision(&self) -> u64 {
        self.store.revision()
    }
    pub fn new(client_id: String, store: Arc<dyn AppSessionStore>) -> Result<Self> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| {
                error(
                    "signIn",
                    FailureReason::Failed,
                    "Could not create the sign-in client.",
                )
            })?;
        Ok(Self {
            stop: Default::default(),
            client_id: client_id.trim().into(),
            store,
            client,
            endpoint: None,
            refresh_gate: Mutex::new(()),
            refused: Mutex::new(HashSet::new()),
            pending: Mutex::new(HashMap::new()),
        })
    }
    pub fn with_loopback_endpoint(mut self, endpoint: &str) -> Result<Self> {
        validate_loopback_endpoint(endpoint)?;
        self.endpoint = Some(endpoint.trim_end_matches('/').into());
        Ok(self)
    }
    pub fn enabled(&self) -> bool {
        !self.client_id.is_empty()
    }
    fn allowed(&self, host: &str) -> bool {
        self.enabled() && host == "github.com"
    }
    async fn post(&self, host: &str, path: &str, form: &[(&str, &str)]) -> Result<Value> {
        let endpoint = self
            .endpoint
            .clone()
            .unwrap_or_else(|| format!("https://{host}"));
        let request = self
            .client
            .post(format!("{endpoint}{path}"))
            .header("accept", "application/json")
            .header("user-agent", "t3code")
            .form(form);
        let response = tokio::time::timeout(Duration::from_secs(15), async {
            let response = request.send().await.map_err(|_| ())?;
            if !response.status().is_success() {
                return Err(());
            }
            use futures_util::StreamExt;
            let mut chunks = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk.map_err(|_| ())?;
                if bytes.len() + chunk.len() > 64 * 1024 {
                    return Err(());
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).map_err(|_| ())
        })
        .await;
        response
            .map_err(|_| failure(host, SignInFailure::Failed, "GitHub sign-in timed out."))?
            .map_err(|_| {
                failure(
                    host,
                    SignInFailure::Failed,
                    "The server could not reach GitHub for sign-in.",
                )
            })
    }
    fn session(value: &Value) -> Option<AppSession> {
        let token = value["access_token"]
            .as_str()
            .filter(|s| !s.trim().is_empty())?
            .to_string();
        let expires = |key: &str| {
            value[key]
                .as_u64()
                .map(|seconds| now().saturating_add(seconds.saturating_mul(1000)))
        };
        Some(AppSession {
            access_token: token,
            access_token_expires_at: expires("expires_in"),
            refresh_token: value["refresh_token"].as_str().map(str::to_string),
            refresh_token_expires_at: expires("refresh_token_expires_in"),
        })
    }
    pub async fn token(&self, raw_host: &str) -> Result<Option<Credential>> {
        let host = normalized(raw_host);
        if !self.allowed(&host) {
            return Ok(None);
        }
        if let Some(session) = self.store.read(&host).await?
            && !self.refused.lock().await.contains(&session.access_token)
            && session
                .access_token_expires_at
                .is_none_or(|at| now() < at.saturating_sub(300_000))
        {
            return Ok(Some(Credential::new(&host, session.access_token, "app")));
        }
        let _gate = self.refresh_gate.lock().await;
        let Some(session) = self.store.read(&host).await? else {
            return Ok(None);
        };
        let refused = self.refused.lock().await.contains(&session.access_token);
        if !refused
            && session
                .access_token_expires_at
                .is_none_or(|at| now() < at.saturating_sub(300_000))
        {
            return Ok(Some(Credential::new(&host, session.access_token, "app")));
        }
        let Some(refresh) = session
            .refresh_token
            .as_deref()
            .filter(|_| session.refresh_token_expires_at.is_none_or(|at| at > now()))
        else {
            self.store
                .replace(&host, &session.access_token, None)
                .await?;
            return Ok(None);
        };
        let usable = !refused && session.access_token_expires_at.is_none_or(|at| now() < at);
        let value = self
            .post(
                &host,
                "/login/oauth/access_token",
                &[
                    ("client_id", &self.client_id),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh),
                ],
            )
            .await;
        let value = match value {
            Ok(value) => value,
            Err(_) if usable => {
                return Ok(Some(Credential::new(&host, session.access_token, "app")));
            }
            Err(error) => return Err(error),
        };
        if value["error"] == "bad_refresh_token" {
            self.store
                .replace(&host, &session.access_token, None)
                .await?;
            return Ok(None);
        }
        let next = match Self::session(&value) {
            Some(next) => next,
            None if usable => return Ok(Some(Credential::new(&host, session.access_token, "app"))),
            None => {
                return Err(failure(
                    &host,
                    SignInFailure::Failed,
                    "GitHub did not refresh the sign-in.",
                ));
            }
        };
        let mut saved = None;
        for attempt in 0..3 {
            match self
                .store
                .replace(&host, &session.access_token, Some(next.clone()))
                .await
            {
                Ok(replaced) => {
                    saved = Some(replaced);
                    break;
                }
                Err(_) if attempt < 2 => tokio::time::sleep(Duration::from_millis(250)).await,
                Err(_) => {}
            }
        }
        if saved != Some(false) {
            return Ok(Some(Credential::new(&host, next.access_token, "app")));
        }
        Ok(self
            .store
            .read(&host)
            .await?
            .map(|s| Credential::new(&host, s.access_token, "app")))
    }
    pub async fn invalidate(&self, host: &str, rejected: &Credential) -> Result<()> {
        if self
            .store
            .read(&normalized(host))
            .await?
            .is_some_and(|s| s.access_token == rejected.token)
        {
            self.refused.lock().await.insert(rejected.token.clone());
        }
        Ok(())
    }
    pub async fn start(self: &Arc<Self>, raw_host: &str) -> Result<SourceControlSignInPrompt> {
        let host = normalized(raw_host);
        if self.stop.is_cancelled() {
            return Err(failure(
                &host,
                SignInFailure::Cancelled,
                "The sign-in was cancelled.",
            ));
        }
        if !self.allowed(&host) {
            return Err(failure(
                &host,
                SignInFailure::Unsupported,
                "This server cannot sign in to this host with the GitHub app.",
            ));
        }
        let value = self
            .post(
                &host,
                "/login/device/code",
                &[("client_id", &self.client_id)],
            )
            .await?;
        let invalid = || {
            failure(
                &host,
                SignInFailure::Failed,
                "GitHub would not start the sign-in.",
            )
        };
        let device = value["device_code"]
            .as_str()
            .ok_or_else(invalid)?
            .to_string();
        let code = value["user_code"].as_str().ok_or_else(invalid)?;
        let uri = value["verification_uri"].as_str().ok_or_else(invalid)?;
        if uri.len() > 16_384
            || url::Url::parse(uri.trim()).ok().is_none_or(|u| {
                !matches!(u.scheme(), "http" | "https")
                    || u.host_str().is_none()
                    || !u.username().is_empty()
                    || u.password().is_some()
            })
        {
            return Err(invalid());
        }
        let expiry = now().saturating_add(
            value["expires_in"]
                .as_u64()
                .ok_or_else(invalid)?
                .saturating_mul(1000),
        );
        let interval = value["interval"].as_u64().unwrap_or(5);
        let prompt = super::decode::parse(
            json!({"userCode":code,"verificationUri":uri,"expiresAt":expiry}),
            "signIn",
        )?;
        let mut pending = self.pending.lock().await;
        if self.stop.is_cancelled() {
            return Err(failure(
                &host,
                SignInFailure::Cancelled,
                "The sign-in was cancelled.",
            ));
        }
        let (result, _) = watch::channel(None);
        let sender = result.clone();
        let this = Arc::downgrade(self);
        let task_host = host.clone();
        let task = tokio::spawn(async move {
            let mut interval = interval;
            let outcome = loop {
                tokio::time::sleep(Duration::from_secs(interval)).await;
                if now() >= expiry {
                    break Err(failure(
                        &task_host,
                        SignInFailure::Expired,
                        "The sign-in code expired. Start again.",
                    ));
                }
                let Some(this) = this.upgrade() else {
                    return;
                };
                let Ok(value) = this
                    .post(
                        &task_host,
                        "/login/oauth/access_token",
                        &[
                            ("client_id", &this.client_id),
                            ("device_code", &device),
                            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                        ],
                    )
                    .await
                else {
                    continue;
                };
                if let Some(session) = Self::session(&value) {
                    let result = this.store.write(&task_host, session).await.map_err(|_| {
                        failure(
                            &task_host,
                            SignInFailure::Failed,
                            "The server could not save the sign-in.",
                        )
                    });
                    if result.is_ok() {
                        this.refused.lock().await.clear();
                    }
                    break result;
                }
                match value["error"].as_str() {
                    Some("authorization_pending") => {}
                    Some("slow_down") => {
                        interval = value["interval"]
                            .as_u64()
                            .unwrap_or(interval.saturating_add(5))
                    }
                    Some("expired_token") => {
                        break Err(failure(
                            &task_host,
                            SignInFailure::Expired,
                            "The sign-in code expired. Start again.",
                        ));
                    }
                    Some("access_denied") => {
                        break Err(failure(
                            &task_host,
                            SignInFailure::Denied,
                            "The sign-in was cancelled on GitHub.",
                        ));
                    }
                    _ => {
                        break Err(failure(
                            &task_host,
                            SignInFailure::Failed,
                            "GitHub refused the sign-in.",
                        ));
                    }
                }
            };
            sender.send_replace(Some(outcome));
        });
        if let Some(previous) = pending.insert(
            host.clone(),
            Pending {
                result,
                task: Some(task),
            },
        ) {
            previous.fail_if_pending(
                &host,
                SignInFailure::Failed,
                "A newer sign-in replaced this one.",
            );
        }
        Ok(prompt)
    }
    pub async fn wait(&self, raw_host: &str) -> Result<()> {
        let host = normalized(raw_host);
        let mut receiver = self
            .pending
            .lock()
            .await
            .get(&host)
            .map(|p| p.result.subscribe())
            .ok_or_else(|| {
                failure(
                    &host,
                    SignInFailure::NotStarted,
                    "No sign-in is in progress. Start again.",
                )
            })?;
        loop {
            if let Some(result) = receiver.borrow_and_update().clone() {
                return result;
            }
            receiver.changed().await.map_err(|_| {
                failure(
                    &host,
                    SignInFailure::Cancelled,
                    "The sign-in was cancelled.",
                )
            })?;
        }
    }
    pub async fn cancel(&self, raw_host: &str) {
        let host = normalized(raw_host);
        if let Some(pending) = self.pending.lock().await.get_mut(&host) {
            pending.fail_if_pending(
                &host,
                SignInFailure::Cancelled,
                "The sign-in was cancelled.",
            );
            if let Some(task) = pending.task.take() {
                task.abort();
                let _ = task.await;
            }
        }
    }
    pub async fn shutdown(&self) {
        self.stop.cancel();
        let pending = std::mem::take(&mut *self.pending.lock().await);
        let mut tasks = vec![];
        for (host, mut pending) in pending {
            pending.fail_if_pending(
                &host,
                SignInFailure::Cancelled,
                "The sign-in was cancelled.",
            );
            if let Some(task) = pending.task.take() {
                task.abort();
                tasks.push(task);
            }
        }
        for task in tasks {
            let _ = task.await;
        }
    }
}

#[derive(Clone)]
pub(super) struct Coverage {
    pub at: u64,
    pub result: Option<(HashSet<String>, HashSet<String>)>,
    pub failed: bool,
}
impl GitHub {
    pub fn with_app_sign_in(mut self, sign_in: Arc<AppSignIn>) -> Self {
        self.http = Arc::new(self.http.as_ref().clone().with_app(sign_in.clone()));
        self.app_sign_in = Some(sign_in);
        self
    }
    pub(super) async fn app_reach(
        &self,
        host: &str,
        repository: &str,
    ) -> Result<SourceControlAppAccessStatus> {
        let host = normalized(host);
        let Some(app) = &self.app_sign_in else {
            return Ok(SourceControlAppAccessStatus::SignedOut);
        };
        let credential = match app.token(&host).await {
            Ok(Some(credential)) => credential,
            Ok(None) => return Ok(SourceControlAppAccessStatus::SignedOut),
            Err(_) => return Ok(SourceControlAppAccessStatus::Covered),
        };
        let key = credential.fingerprint.clone();
        let gate = {
            let mut gates = self.coverage_gates.lock().await;
            gates.retain(|_, weak| weak.strong_count() > 0);
            if let Some(gate) = gates.get(&key).and_then(std::sync::Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(Mutex::new(()));
                gates.insert(key.clone(), Arc::downgrade(&gate));
                gate
            }
        };
        let _gate = gate.lock().await;
        let cached = self.coverage.lock().await.get(&key).cloned();
        let entry = if cached.as_ref().is_none_or(|entry| {
            now().saturating_sub(entry.at) >= if entry.failed { 30_000 } else { 120_000 }
        }) {
            let read = async {
                let http = self.http.pinned(host.clone(), credential);
                let mut accounts = HashSet::new();
                let mut repositories = HashSet::new();
                let mut installations = Vec::new();
                for page in 1..=10 {
                    let response = http
                        .rest(
                            &host,
                            ".",
                            "appInstallations",
                            reqwest::Method::GET,
                            &format!("user/installations?per_page=100&page={page}"),
                            None,
                        )
                        .await?;
                    let value: Value = serde_json::from_slice(&response.body).map_err(|_| {
                        error(
                            "appInstallations",
                            FailureReason::Failed,
                            "Invalid installation list.",
                        )
                    })?;
                    let rows = value["installations"].as_array().ok_or_else(|| {
                        error(
                            "appInstallations",
                            FailureReason::Failed,
                            "Invalid installation list.",
                        )
                    })?;
                    installations.extend(rows.iter().cloned());
                    if rows.len() < 100 {
                        break;
                    }
                }
                for installation in installations {
                    let Some(login) = installation["account"]["login"].as_str() else {
                        continue;
                    };
                    if installation["repository_selection"] == "all" {
                        accounts.insert(login.to_lowercase());
                        continue;
                    }
                    let id = installation["id"].as_u64().ok_or_else(|| {
                        error(
                            "appInstallations",
                            FailureReason::Failed,
                            "Invalid installation id.",
                        )
                    })?;
                    for page in 1..=11 {
                        if page > 10 {
                            accounts.insert(login.to_lowercase());
                            break;
                        }
                        let response = http
                            .rest(
                                &host,
                                ".",
                                "appInstallationRepositories",
                                reqwest::Method::GET,
                                &format!(
                                    "user/installations/{id}/repositories?per_page=100&page={page}"
                                ),
                                None,
                            )
                            .await?;
                        let value: Value =
                            serde_json::from_slice(&response.body).map_err(|_| {
                                error(
                                    "appInstallations",
                                    FailureReason::Failed,
                                    "Invalid repository list.",
                                )
                            })?;
                        let rows = value["repositories"].as_array().ok_or_else(|| {
                            error(
                                "appInstallations",
                                FailureReason::Failed,
                                "Invalid repository list.",
                            )
                        })?;
                        for row in rows {
                            if let Some(name) = row["full_name"].as_str() {
                                repositories.insert(name.to_lowercase());
                            }
                        }
                        if rows.len() < 100 {
                            break;
                        }
                    }
                }
                Ok::<_, ForgeError>((accounts, repositories))
            }
            .await;
            let mut coverage = self.coverage.lock().await;
            if coverage.len() >= 8 {
                coverage.clear();
            }
            let entry = Coverage {
                at: now(),
                failed: read.is_err(),
                result: read.ok(),
            };
            coverage.insert(key, entry.clone());
            Some(entry)
        } else {
            cached
        };
        let Some((accounts, repositories)) = entry.as_ref().and_then(|e| e.result.as_ref()) else {
            return Ok(SourceControlAppAccessStatus::Covered);
        };
        let repo = super::repository::parse_repository_selector(repository, &host)
            .ok_or_else(|| error("appAccess", FailureReason::Failed, "Invalid repository."))?;
        Ok(
            if accounts.contains(&repo.owner.to_lowercase())
                || repositories.contains(&format!("{}/{}", repo.owner, repo.name).to_lowercase())
            {
                SourceControlAppAccessStatus::Covered
            } else {
                SourceControlAppAccessStatus::NotCovered
            },
        )
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;
    use futures_util::FutureExt;
    fn session() -> AppSession {
        AppSession {
            access_token: "fixture-app-token".into(),
            access_token_expires_at: None,
            refresh_token: None,
            refresh_token_expires_at: None,
        }
    }
    #[tokio::test]
    async fn fresh_token_read_does_not_wait_for_an_unrelated_refresh() {
        let store = Arc::new(MemoryAppSessions::default());
        store.write("github.com", session()).await.unwrap();
        let app = AppSignIn::new("fixture-client".into(), store).unwrap();
        let _refresh = app.refresh_gate.lock().await;
        assert!(
            app.token("github.com")
                .now_or_never()
                .expect("fresh read waited for refresh gate")
                .unwrap()
                .is_some()
        );
    }
    #[tokio::test]
    async fn installation_network_io_does_not_hold_the_coverage_cache_lock() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
            sync::oneshot,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (observed, request) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = vec![];
            let mut chunk = [0; 2048];
            while !bytes.windows(4).any(|v| v == b"\r\n\r\n") {
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
            }
            observed.send(()).unwrap();
            resume.await.unwrap();
            let body = r#"{"installations":[]}"#;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let store = Arc::new(MemoryAppSessions::default());
        store.write("github.com", session()).await.unwrap();
        let app = Arc::new(AppSignIn::new("fixture-client".into(), store).unwrap());
        let http = GitHubHttp::new(
            Arc::new(GitHubCredentials::new(Default::default())),
            Arc::new(crate::rate_limit::RateLimits::default()),
        )
        .unwrap()
        .with_loopback_endpoint(&endpoint)
        .unwrap();
        let github = Arc::new(GitHub::new(http).unwrap().with_app_sign_in(app));
        let read = tokio::spawn({
            let github = github.clone();
            async move { github.app_reach("github.com", "a/b").await }
        });
        request.await.unwrap();
        assert!(
            github.coverage.try_lock().is_ok(),
            "network request held the global cache lock"
        );
        release.send(()).unwrap();
        assert_eq!(
            read.await.unwrap().unwrap(),
            SourceControlAppAccessStatus::NotCovered
        );
        server.await.unwrap();
    }
}
