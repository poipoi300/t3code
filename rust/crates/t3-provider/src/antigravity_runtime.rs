use crate::{
    acp_connection::{AcpConnection, SetupHandler},
    antigravity_callback,
    antigravity_installation::{
        AntigravityExecutable, AntigravityInstallation, AntigravityValidator, ExecutableLease,
    },
    antigravity_profile::{AUTH_PREFIX, AuthConfig, BROWSER_MARKER, Profile},
    environment::{Environment, process_options},
    sensitive::{Sensitive, ServiceError, ServiceResult},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use t3_process::CancellationToken;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::{Mutex, mpsc, watch},
};
#[derive(Debug, Clone)]
pub struct Started {
    pub initialize: Sensitive<Value>,
    pub setup: Sensitive<Value>,
}
#[derive(Default)]
pub struct Catalog {
    pub config_updates: Vec<Sensitive<Vec<Value>>>,
    pub commands: Vec<Value>,
}
#[async_trait]
pub trait AntigravityRuntime: Send + Sync {
    async fn initialize(&self) -> ServiceResult<Sensitive<Value>>;
    async fn start(&self) -> ServiceResult<Started>;
    async fn catalog(&self, _started: &Started) -> ServiceResult<Sensitive<Catalog>> {
        Ok(Sensitive::new(Catalog::default()))
    }
    async fn logout(&self) -> ServiceResult<()>;
    async fn close(&self);
}
#[async_trait]
pub trait AntigravityRuntimeFactory: Send + Sync {
    async fn health(&self) -> ServiceResult<Sensitive<Value>> {
        let runtime = self.create(None).await?;
        let result = runtime.initialize().await;
        runtime.close().await;
        result
    }

    async fn supports_text_generation(&self) -> bool {
        false
    }
    async fn create(
        &self,
        authorization: Option<mpsc::Sender<Sensitive<String>>>,
    ) -> ServiceResult<Arc<dyn AntigravityRuntime>>;
}
struct Task(tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct ProcessRuntime {
    connection: AcpConnection,
    initialized: Mutex<Option<Sensitive<Value>>>,
    failed: watch::Receiver<Option<ServiceError>>,
    cancel: CancellationToken,
    exited: CancellationToken,
    tasks: Vec<Task>,
    cwd: PathBuf,
    auth_method: String,
    version: Option<String>,
    session: Arc<std::sync::Mutex<Option<String>>>,
}
impl Drop for ProcessRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
async fn auth_url(
    url: &str,
    authorization: &Option<mpsc::Sender<Sensitive<String>>>,
) -> ServiceResult<()> {
    antigravity_callback::authorization(url)?;
    let sender = authorization.as_ref().ok_or(ServiceError::Identity)?;
    sender
        .send(Sensitive::new(url.into()))
        .await
        .map_err(|_| ServiceError::Cancelled)
}
async fn stdout(
    reader: impl AsyncRead + Unpin,
    mut writer: tokio::io::DuplexStream,
    authorization: Option<mpsc::Sender<Sensitive<String>>>,
) -> ServiceResult<()> {
    let mut reader = BufReader::new(reader);
    loop {
        let mut line = vec![];
        let count = (&mut reader)
            .take(16 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)
            .await
            .map_err(|_| ServiceError::Response)?;
        if count == 0 {
            break;
        }
        if count > 16 * 1024 * 1024 {
            return Err(ServiceError::Response);
        }
        if line.starts_with(AUTH_PREFIX.as_bytes()) {
            let message = std::str::from_utf8(&line)
                .map_err(|_| ServiceError::Callback)?
                .trim_end_matches(['\r', '\n']);
            auth_url(&message[AUTH_PREFIX.len()..], &authorization).await?;
        } else {
            writer
                .write_all(&line)
                .await
                .map_err(|_| ServiceError::Response)?;
        }
    }
    Ok(())
}
async fn stderr(
    reader: impl AsyncRead + Unpin,
    authorization: Option<mpsc::Sender<Sensitive<String>>>,
) -> ServiceResult<()> {
    let mut reader = BufReader::new(reader);
    let mut dropping = false;
    loop {
        let mut line = vec![];
        let count = (&mut reader)
            .take(16600)
            .read_until(b'\n', &mut line)
            .await
            .map_err(|_| ServiceError::Response)?;
        if count == 0 {
            break;
        }
        let complete = line.last() == Some(&b'\n');
        if dropping || count == 16600 && !complete {
            dropping = !complete;
            continue;
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\r', '\n']);
        let url = text
            .strip_prefix(AUTH_PREFIX)
            .map(str::to_owned)
            .or_else(|| {
                text.strip_prefix(BROWSER_MARKER)
                    .and_then(|value| serde_json::from_str::<String>(value).ok())
            });
        if let Some(url) = url
            && antigravity_callback::authorization(&url).is_ok()
        {
            auth_url(&url, &authorization).await?;
        }
    }
    Ok(())
}
impl ProcessRuntime {
    async fn spawn(
        executable: AntigravityExecutable,
        lease: Option<ExecutableLease>,
        profile: Arc<Profile>,
        auth: &AuthConfig,
        environment: &Environment,
        authorization: Option<mpsc::Sender<Sensitive<String>>>,
    ) -> ServiceResult<Arc<Self>> {
        let workspace = tempfile::Builder::new()
            .prefix("t3-antigravity-setup-")
            .tempdir()
            .map_err(|_| ServiceError::Storage)?;
        let cwd = workspace.path().to_path_buf();
        let runtime_temp = tempfile::Builder::new()
            .prefix("runtime-")
            .tempdir_in(&profile.temp_directory)
            .map_err(|_| ServiceError::Storage)?;
        let spawn = profile.spawn(
            &executable.executable,
            executable.harness.as_deref(),
            &cwd,
            environment,
            auth,
            Some(runtime_temp.path()),
        );
        let mut options =
            process_options(&spawn.executable, &spawn.args, spawn.environment.expose());
        options.cwd = Some(cwd.clone());
        options.timeout = None;
        let mut child = t3_process::spawn_non_blocking(&options)
            .await
            .map_err(|_| ServiceError::Unavailable)?;
        let input = child.stdin.take().ok_or(ServiceError::Response)?;
        let output = child.stdout.take().ok_or(ServiceError::Response)?;
        let errors = child.stderr.take();
        let (bridge, writer) = tokio::io::duplex(65536);
        let (failed, receiver) = watch::channel(None);
        let stdout_failure = failed.clone();
        let urls = authorization.clone();
        let mut tasks = vec![Task(tokio::spawn(async move {
            if let Err(error) = stdout(output, writer, urls).await {
                stdout_failure.send_replace(Some(error));
            }
        }))];
        if let Some(errors) = errors {
            tasks.push(Task(tokio::spawn(async move {
                if let Err(error) = stderr(errors, authorization).await {
                    failed.send_replace(Some(error));
                }
            })));
        }
        let cancel = CancellationToken::new();
        let exited = CancellationToken::new();
        let actor_cancel = cancel.clone();
        let actor_exited = exited.clone();
        let session = Arc::new(std::sync::Mutex::new(None::<String>));
        let owned_session = session.clone();
        let owned_profile = profile.clone();
        tokio::spawn(async move {
            actor_cancel.cancelled().await;
            let _ = child.terminate(Duration::from_secs(1)).await;
            let session = owned_session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            crate::antigravity_session_files::remove(
                &owned_profile.gemini_home,
                session.as_deref(),
                workspace.path(),
            );
            drop(runtime_temp);
            drop(lease);
            drop(workspace);
            actor_exited.cancel();
        });
        Ok(Arc::new(Self {
            connection: AcpConnection::from_pipes(
                bridge,
                input,
                Arc::new(SetupHandler { consent: None }),
            ),
            initialized: Mutex::new(None),
            failed: receiver,
            cancel,
            exited,
            tasks,
            cwd,
            auth_method: auth.method.clone(),
            version: executable.version,
            session,
        }))
    }
    async fn guarded<T>(
        &self,
        operation: impl std::future::Future<Output = crate::acp_service::AcpResult<T>>,
    ) -> ServiceResult<T> {
        let mut failed = self.failed.clone();
        if let Some(error) = *failed.borrow_and_update() {
            return Err(error);
        }
        tokio::select! {result=operation=>result.map_err(|_|ServiceError::Response),_=failed.changed()=>Err(failed.borrow().unwrap_or(ServiceError::Response))}
    }
}
#[async_trait]
impl AntigravityRuntime for ProcessRuntime {
    async fn initialize(&self) -> ServiceResult<Sensitive<Value>> {
        let mut initialized = self.initialized.lock().await;
        if let Some(value) = initialized.as_ref() {
            return Ok(value.clone());
        }
        let name = if self
            .version
            .as_deref()
            .is_some_and(|version| !crate::versions::compare_versions(version, "1.5.1").is_lt())
        {
            "t3-code"
        } else {
            "zed"
        };
        let value = self
            .guarded(self.connection.initialize(name, false))
            .await?;
        *initialized = Some(value.clone());
        Ok(value)
    }
    async fn start(&self) -> ServiceResult<Started> {
        let initialize = self.initialize().await?;
        self.guarded(self.connection.authenticate(&self.auth_method))
            .await?;
        let version = initialize.expose()["agentInfo"]["version"]
            .as_str()
            .unwrap_or_default();
        let meta = (crate::versions::compare_versions(version, "1.4.2").is_ge()
            && crate::versions::compare_versions(version, "1.4.5").is_le())
        .then(|| json!({"antigravity":{"disabledTools":["run_workflow"]}}));
        let setup = self
            .guarded(self.connection.new_session_with_meta(&self.cwd, meta))
            .await?;
        *self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            setup.expose()["sessionId"].as_str().map(str::to_owned);
        Ok(Started { initialize, setup })
    }
    async fn catalog(&self, started: &Started) -> ServiceResult<Sensitive<Catalog>> {
        let mut catalog = Catalog::default();
        let options = started.setup.expose()["configOptions"]
            .as_array()
            .map_or(&[][..], Vec::as_slice);
        let model = options.iter().find(|option| option["id"] == "model");
        if crate::antigravity_catalog::thinking(options).is_some()
            && let Some(current) = model.and_then(|option| option["currentValue"].as_str())
            && let Some(session) = started.setup.expose()["sessionId"].as_str()
        {
            let config_id = model
                .and_then(|option| option["id"].as_str())
                .unwrap_or("model");
            let mut models =
                crate::antigravity_catalog::models(started.setup.expose(), &Default::default())
                    .into_iter()
                    .map(|model| model.slug.0)
                    .collect::<Vec<_>>();
            models.push(current.into());
            for model in models {
                if let Ok(options) = self
                    .guarded(
                        self.connection
                            .select_catalog_model(session, config_id, &model),
                    )
                    .await
                {
                    catalog.config_updates.push(options);
                }
            }
        }
        catalog.commands = self.connection.catalog_commands();
        Ok(Sensitive::new(catalog))
    }
    async fn logout(&self) -> ServiceResult<()> {
        let initialize = self.initialize().await?;
        if !initialize.expose()["agentCapabilities"]["auth"]["logout"].is_object() {
            return Err(ServiceError::Unsupported);
        }
        self.guarded(self.connection.request(
            "logout",
            Sensitive::new(json!({})),
            t3_contracts::acp_registry::AcpRegistryOperationErrorReason::LogoutFailed,
        ))
        .await?;
        Ok(())
    }
    async fn close(&self) {
        self.cancel.cancel();
        self.exited.cancelled().await;
        self.connection.close().await;
        for task in &self.tasks {
            task.0.abort();
        }
    }
}
pub struct ProcessAntigravityFactory {
    pub installation: Arc<AntigravityInstallation>,
    pub binary: Option<String>,
    pub profile: Arc<Profile>,
    pub auth: AuthConfig,
    pub environment: Environment,
}
pub struct DeferredAntigravityFactory {
    pub installation: Arc<AntigravityInstallation>,
    pub binary: Option<String>,
    pub profile_directory: PathBuf,
    pub temp_directory: PathBuf,
    pub user_home: PathBuf,
    pub browser_helper: PathBuf,
    pub auth: AuthConfig,
    pub environment: Environment,
    pub platform: crate::environment::Platform,
    profile: Mutex<Option<Arc<Profile>>>,
}
pub struct DeferredProfileOptions {
    pub profile_directory: PathBuf,
    pub temp_directory: PathBuf,
    pub user_home: PathBuf,
    pub browser_helper: PathBuf,
    pub auth: AuthConfig,
    pub environment: Environment,
    pub platform: crate::environment::Platform,
}
impl DeferredAntigravityFactory {
    pub fn new(
        installation: Arc<AntigravityInstallation>,
        binary: Option<String>,
        options: DeferredProfileOptions,
    ) -> Self {
        Self {
            installation,
            binary,
            profile_directory: options.profile_directory,
            temp_directory: options.temp_directory,
            user_home: options.user_home,
            browser_helper: options.browser_helper,
            auth: options.auth,
            environment: options.environment,
            platform: options.platform,
            profile: Mutex::new(None),
        }
    }
    async fn profile(&self) -> ServiceResult<Arc<Profile>> {
        let mut cached = self.profile.lock().await;
        if let Some(profile) = cached.as_ref() {
            return Ok(profile.clone());
        }
        let paths = [
            self.temp_directory.clone(),
            self.profile_directory.join("antigravity-acp/tmp"),
        ];
        tokio::task::spawn_blocking(move || {
            for path in paths {
                let retired = crate::antigravity_session_files::retire(&path);
                tokio::task::spawn_blocking(move || retired.remove());
            }
        })
        .await
        .map_err(|_| ServiceError::Storage)?;
        let profile = Arc::new(
            crate::antigravity_profile::prepare(crate::antigravity_profile::PrepareProfile {
                directory: &self.profile_directory,
                temp_directory: Some(&self.temp_directory),
                user_home: &self.user_home,
                browser_helper: &self.browser_helper,
                environment: &self.environment,
                platform: self.platform,
                auth: &self.auth,
            })
            .await?,
        );
        *cached = Some(profile.clone());
        Ok(profile)
    }
}
fn installed_health(executable: AntigravityExecutable) -> Sensitive<Value> {
    Sensitive::new(
        json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":true,"audio":true,"embeddedContext":true},"sessionCapabilities":{"list":{},"resume":{}}},"authMethods":[{"id":"oauth-personal","name":"Log in with Google"}],"agentInfo":{"name":"antigravity-acp","title":"Google Antigravity","version":executable.version.as_deref().unwrap_or("unknown")}}),
    )
}
#[async_trait]
impl AntigravityRuntimeFactory for DeferredAntigravityFactory {
    async fn health(&self) -> ServiceResult<Sensitive<Value>> {
        if self.auth.issue().is_some() {
            return Err(ServiceError::Identity);
        }
        self.installation
            .resolve(self.binary.as_deref(), &self.environment)
            .await
            .map(installed_health)
    }

    async fn supports_text_generation(&self) -> bool {
        crate::antigravity_profile::text_generation_available(&self.profile_directory)
    }
    async fn create(
        &self,
        authorization: Option<mpsc::Sender<Sensitive<String>>>,
    ) -> ServiceResult<Arc<dyn AntigravityRuntime>> {
        let factory = ProcessAntigravityFactory {
            installation: self.installation.clone(),
            binary: self.binary.clone(),
            profile: self.profile().await?,
            auth: self.auth.clone(),
            environment: self.environment.clone(),
        };
        factory.create(authorization).await
    }
}
#[async_trait]
impl AntigravityRuntimeFactory for ProcessAntigravityFactory {
    async fn health(&self) -> ServiceResult<Sensitive<Value>> {
        if self.auth.issue().is_some() {
            return Err(ServiceError::Identity);
        }
        self.installation
            .resolve(self.binary.as_deref(), &self.environment)
            .await
            .map(installed_health)
    }

    async fn supports_text_generation(&self) -> bool {
        crate::antigravity_profile::text_generation_available(&self.profile.gemini_home)
    }
    async fn create(
        &self,
        authorization: Option<mpsc::Sender<Sensitive<String>>>,
    ) -> ServiceResult<Arc<dyn AntigravityRuntime>> {
        if self.auth.issue().is_some() {
            return Err(ServiceError::Identity);
        }
        let lease = self
            .installation
            .acquire(self.binary.as_deref(), &self.environment)
            .await?;
        ProcessRuntime::spawn(
            lease.executable.clone(),
            Some(lease),
            self.profile.clone(),
            &self.auth,
            &self.environment,
            authorization,
        )
        .await
        .map(|runtime| runtime as Arc<dyn AntigravityRuntime>)
    }
}
pub struct ProcessAntigravityValidator {
    pub profile: Arc<Profile>,
    pub environment: Environment,
    pub auth: AuthConfig,
}
#[async_trait]
impl AntigravityValidator for ProcessAntigravityValidator {
    async fn validate(
        &self,
        executable: &AntigravityExecutable,
        version: &str,
    ) -> ServiceResult<()> {
        let runtime = ProcessRuntime::spawn(
            executable.clone(),
            None,
            self.profile.clone(),
            &self.auth,
            &self.environment,
            None,
        )
        .await?;
        let result = runtime.initialize().await.and_then(|initialize| {
            if initialize.expose()["agentInfo"]["version"] == version {
                Ok(())
            } else {
                Err(ServiceError::Maintenance)
            }
        });
        runtime.close().await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn authorization_lines_are_consumed_before_json_rpc_and_stderr_markers_are_private() {
        let url = "https://accounts.google.com/o/oauth2/v2/auth?state=secret-state&response_type=code&redirect_uri=http%3A%2F%2F127.0.0.1%3A32123%2F";
        let (tx, mut rx) = mpsc::channel(2);
        let (input, mut source) = tokio::io::duplex(65536);
        let (writer, mut output) = tokio::io::duplex(65536);
        let text =
            format!("{AUTH_PREFIX}{url}\n{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{}}}}\n");
        let producer = tokio::spawn(async move {
            for chunk in text.as_bytes().chunks(7) {
                source.write_all(chunk).await.expect("fixture write");
            }
        });
        let reader = tokio::spawn(stdout(input, writer, Some(tx)));
        let captured = rx.recv().await.expect("captured link");
        assert_eq!(captured.expose(), url);
        assert!(!format!("{captured:?}").contains("secret-state"));
        let mut protocol = String::new();
        output
            .read_to_string(&mut protocol)
            .await
            .expect("protocol output");
        assert_eq!(protocol, "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n");
        producer.await.expect("producer");
        reader.await.expect("reader").expect("valid stdout");
        let (tx, mut rx) = mpsc::channel(2);
        let line = format!(
            "{BROWSER_MARKER}{}\n",
            serde_json::to_string(url).expect("fixture URL")
        );
        stderr(line.as_bytes(), Some(tx))
            .await
            .expect("valid marker");
        assert_eq!(rx.recv().await.expect("stderr URL").expose(), url);
        assert_eq!(
            stdout(
                format!("{AUTH_PREFIX}{url}\n").as_bytes(),
                tokio::io::duplex(65536).0,
                None
            )
            .await
            .err(),
            Some(ServiceError::Identity)
        );
    }
}
