use super::*;
use crate::rate_limit::RateLimits;
pub(super) struct Transport {
    pub config: CursorOriginConfig,
    cli: Arc<dyn ForgeCli>,
    base: url::Url,
    stop: t3_process::CancellationToken,
    session: Mutex<Option<(Instant, Result<String>)>>,
    clients: Mutex<HashMap<String, ApiHttp>>,
    limits: Arc<RateLimits>,
    revision: AtomicU64,
}
impl Transport {
    pub fn new(config: CursorOriginConfig, base: url::Url) -> Result<Self> {
        ApiHttp::new(
            "cursor-origin",
            base.clone(),
            ApiCredential::Anonymous,
            Arc::default(),
        )?;
        Ok(Self {
            config,
            base,
            cli: Arc::new(ProcessForgeCli),
            stop: Default::default(),
            session: Mutex::new(None),
            clients: Mutex::default(),
            limits: Arc::default(),
            revision: AtomicU64::new(0),
        })
    }
    pub fn with_cli(&self, cli: Arc<dyn ForgeCli>) -> Self {
        Self {
            config: self.config.clone(),
            base: self.base.clone(),
            cli,
            stop: Default::default(),
            session: Mutex::new(None),
            clients: Mutex::default(),
            limits: self.limits.clone(),
            revision: AtomicU64::new(self.revision() + 1),
        }
    }
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
    pub fn shutdown(&self) {
        self.stop.cancel();
    }
    pub async fn cancelled(&self) {
        self.stop.cancelled().await;
    }
    pub async fn execute(&self, cwd: &Path, op: &str, args: Vec<String>) -> Result<CliResponse> {
        let mut environment = self.config.environment.clone();
        environment.insert("NO_COLOR".into(), Some("1".into()));
        let result = self
            .cli
            .run(CliRequest {
                program: "origin",
                provider: "cursor-origin",
                operation: op.into(),
                cwd: cwd.into(),
                args,
                stdin: None,
                environment,
                max_bytes: 8_000_000,
                timeout: Duration::from_secs(30),
                cancellation: self.stop.child_token(),
            })
            .await?;
        if result.truncated {
            return Err(error(
                op,
                "Cursor Origin CLI exceeded the response byte limit.",
            ));
        }
        let auth_status = op == "getViewer"
            && serde_json::from_slice::<Value>(&result.stdout)
                .ok()
                .is_some_and(|v| v["authenticated"].is_boolean());
        if result.code != 0 && !auth_status {
            let output = format!(
                "{} {}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            )
            .to_ascii_lowercase();
            let reason = if [
                "not authenticated",
                "not logged",
                "not signed",
                "origin auth login",
                "unauthenticated",
                "unauthorized",
            ]
            .iter()
            .any(|s| output.contains(s))
            {
                FailureReason::Unauthenticated
            } else if output.contains("429") || output.contains("rate limit") {
                FailureReason::RateLimited
            } else if output.contains("not found") {
                FailureReason::NotFound
            } else {
                FailureReason::Failed
            };
            return Err(ForgeError::provider(
                "cursor-origin",
                op,
                reason,
                "Cursor Origin CLI could not complete the operation. Check origin auth status.",
            ));
        }
        Ok(result)
    }
    async fn token(&self, _cwd: &Path) -> Result<String> {
        let mut session = self.session.lock().await;
        if let Some((until, result)) = &*session
            && *until > Instant::now()
        {
            return result.clone();
        }
        let result = self
            .execute(
                &self.config.authentication_cwd,
                "authToken",
                vec!["auth".into(), "token".into()],
            )
            .await
            .and_then(|r| {
                String::from_utf8(r.stdout)
                    .map_err(|_| error("authToken", "Invalid session token response."))
            })
            .and_then(|s| {
                if s.trim().is_empty() {
                    Err(ForgeError::provider(
                        "cursor-origin",
                        "authToken",
                        FailureReason::Unauthenticated,
                        "Sign in using origin auth login.",
                    ))
                } else {
                    Ok(s.trim().into())
                }
            });
        self.revision.fetch_add(1, Ordering::AcqRel);
        *session = Some((
            Instant::now() + Duration::from_secs(if result.is_ok() { 300 } else { 10 }),
            result.clone(),
        ));
        result
    }
    async fn client(&self, token: &str) -> Result<ApiHttp> {
        use sha2::{Digest, Sha256};
        let key = format!("{:x}", Sha256::digest(token.as_bytes()));
        let mut clients = self.clients.lock().await;
        if let Some(client) = clients.get(&key) {
            return Ok(client.clone());
        }
        let client = ApiHttp::new(
            "cursor-origin",
            self.base.clone(),
            ApiCredential::Bearer(token.into()),
            self.limits.clone(),
        )?
        .with_cancellation(self.stop.child_token())
        .with_redirect_policy(ApiRedirectPolicy {
            path_prefix: "/v1/origin/".into(),
            statuses: &[],
            write_statuses: &[],
            max_hops: 0,
        });
        if clients.len() >= 4 {
            clients.clear();
        }
        clients.insert(key, client.clone());
        Ok(client)
    }
    pub async fn api(
        &self,
        cwd: &Path,
        op: &str,
        path: &str,
        method: Method,
        body: Option<Value>,
    ) -> Result<Value> {
        if !path.starts_with("v1/origin/")
            || path
                .split('?')
                .next()
                .is_some_and(|p| p.split('/').any(|s| matches!(s, "." | "..")))
        {
            return Err(error(op, "Invalid Cursor Origin API path."));
        }
        let mut token = self.token(cwd).await?;
        for attempt in 0..2 {
            let result = self
                .client(&token)
                .await?
                .request(op, method.clone(), path, body.clone(), 8_000_000)
                .await;
            match result {
                Err(e) if e.reason() == Some(FailureReason::Unauthenticated) => {
                    let mut session = self.session.lock().await;
                    if session
                        .as_ref()
                        .is_some_and(|(_, r)| r.as_ref().is_ok_and(|current| current == &token))
                    {
                        *session = None;
                        self.revision.fetch_add(1, Ordering::AcqRel);
                    }
                    drop(session);
                    if attempt == 1 {
                        return Err(e);
                    }
                    token = self.token(cwd).await?;
                }
                result => return result?.json("cursor-origin", op),
            }
        }
        unreachable!()
    }
}
