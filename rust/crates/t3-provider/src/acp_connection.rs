use crate::{
    acp_catalog,
    acp_coordinator::{AcpRuntimeCoordinator, AvailableCommands},
    acp_service::{AcpError, AcpResult},
    environment::process_options,
    registry_service::ResolvedRegistryAgent,
    sensitive::Sensitive,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use t3_acp::{Client, ClientHandler, ClientRequest, RequestContext, RpcError};
use t3_contracts::{
    acp_registry::{AcpRegistryOperationErrorReason as Reason, AcpRegistryUrlAuthAction},
    provider_instance::ProviderInstanceId,
    wire::trim_js,
};
use t3_process::CancellationToken;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    sync::watch,
};
pub fn protocol_error(error: &t3_acp::Error, reason: Reason) -> AcpError {
    let authentication = matches!(error,t3_acp::Error::Rpc(error)if error.code== -32000)
        || regex::Regex::new(
            r"(?i)\b(?:authenticat(?:e|ed|es|ing|ion)|credentials?|log(?:[\s-]+)?in)\b",
        )
        .expect("ACP auth classification")
        .is_match(&error.to_string());
    if authentication {
        AcpError::new(
            Reason::AuthenticationFailed,
            "The ACP agent could not complete authentication.",
        )
    } else {
        AcpError::new(reason, "The ACP agent could not complete the request.")
    }
}
#[async_trait]
pub trait AcpUrlConsent: Send + Sync {
    async fn consent(&self, action: Sensitive<AcpRegistryUrlAuthAction>) -> bool;
}
pub struct CoordinatedConsent {
    pub coordinator: Arc<AcpRuntimeCoordinator>,
    pub instance_id: ProviderInstanceId,
}
#[async_trait]
impl AcpUrlConsent for CoordinatedConsent {
    async fn consent(&self, action: Sensitive<AcpRegistryUrlAuthAction>) -> bool {
        self.coordinator
            .request_url_auth(self.instance_id.clone(), action)
            .await
    }
}
pub struct SetupHandler {
    pub consent: Option<Arc<dyn AcpUrlConsent>>,
}
#[async_trait]
impl ClientHandler for SetupHandler {
    async fn request(&self, request: ClientRequest, _: RequestContext) -> Result<Value, RpcError> {
        if let ClientRequest::Elicitation(value) = request {
            let value = if value["request"].is_object() {
                &value["request"]
            } else {
                &value
            };
            let id = value["elicitationId"].as_str().filter(|id| {
                !id.is_empty() && trim_js(id) == *id && id.encode_utf16().count() <= 128
            });
            let url = value["url"].as_str().and_then(acp_catalog::web_url);
            let accepted = if value["mode"] == "url"
                && let (Some(id), Some(url), Some(consent)) = (id, url, self.consent.as_ref())
            {
                let message = String::from_utf16_lossy(
                    &trim_js(value["message"].as_str().unwrap_or_default())
                        .encode_utf16()
                        .take(1024)
                        .collect::<Vec<_>>(),
                );
                consent
                    .consent(Sensitive::new(AcpRegistryUrlAuthAction {
                        elicitation_id: id.into(),
                        url,
                        message,
                        created_at: None,
                        expires_at: None,
                    }))
                    .await
            } else {
                false
            };
            return Ok(json!({"action":if accepted{"accept"}else{"decline"}}));
        }
        Err(RpcError {
            code: -32601,
            message: "This request is unavailable during provider setup.".into(),
            data: None,
        })
    }
}
struct Task(tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct Child {
    cancel: CancellationToken,
    exited: CancellationToken,
}
impl Drop for Child {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
pub struct AcpConnection {
    client: Client,
    events: Task,
    child: Option<Child>,
    commands: watch::Receiver<Option<Arc<AvailableCommands>>>,
}
impl std::fmt::Debug for AcpConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AcpConnection([REDACTED])")
    }
}
impl AcpConnection {
    pub fn from_pipes(
        reader: impl AsyncRead + Unpin + Send + 'static,
        writer: impl AsyncWrite + Unpin + Send + 'static,
        handler: Arc<dyn ClientHandler>,
    ) -> Self {
        let (client, mut events) = Client::connect(reader, writer, handler, Default::default());
        let (send, commands) = watch::channel(None);
        let events = Task(tokio::spawn(async move {
            while let Some(Ok(event)) = events.next().await {
                if let t3_acp::Event::SessionUpdate(update) = event
                    && update.kind() == "available_commands_update"
                {
                    let commands = update.update["availableCommands"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    send.send_replace(Some(Arc::new(acp_catalog::commands(&commands))));
                }
                events.acknowledge();
            }
        }));
        Self {
            client,
            events,
            child: None,
            commands,
        }
    }
    pub async fn spawn(
        spawn: &ResolvedRegistryAgent,
        handler: Arc<dyn ClientHandler>,
    ) -> AcpResult<Self> {
        let mut options =
            process_options(&spawn.executable, &spawn.args, spawn.environment.expose());
        options.cwd = Some(spawn.cwd.clone());
        let mut process = tokio::task::spawn_blocking(move || t3_process::spawn(&options))
            .await
            .map_err(|_| AcpError::new(Reason::ProbeFailed, "Could not start the ACP agent."))?
            .map_err(|_| AcpError::new(Reason::ProbeFailed, "Could not start the ACP agent."))?;
        let stdin = process.stdin.take().ok_or_else(|| {
            AcpError::new(
                Reason::ProbeFailed,
                "The ACP agent did not provide an input pipe.",
            )
        })?;
        let stdout = process.stdout.take().ok_or_else(|| {
            AcpError::new(
                Reason::ProbeFailed,
                "The ACP agent did not provide an output pipe.",
            )
        })?;
        let stderr = process.stderr.take();
        let cancel = CancellationToken::new();
        let exited = CancellationToken::new();
        let actor_cancel = cancel.clone();
        let actor_exited = exited.clone();
        tokio::spawn(async move {
            let _errors = stderr.map(|mut stderr| {
                Task(tokio::spawn(async move {
                    let mut bytes = [0; 8192];
                    while let Ok(count) = stderr.read(&mut bytes).await {
                        if count == 0 {
                            break;
                        }
                    }
                }))
            });
            tokio::select! {biased;_=actor_cancel.cancelled()=>{let _=process.terminate(Duration::from_secs(1)).await;},_=process.wait()=>{}}
            actor_exited.cancel();
        });
        let mut connection = Self::from_pipes(stdout, stdin, handler);
        connection.child = Some(Child { cancel, exited });
        Ok(connection)
    }
    pub async fn initialize(&self, name: &str, terminal_auth: bool) -> AcpResult<Sensitive<Value>> {
        self.client.initialize(json!({"clientInfo":{"name":name,"version":"0.0.0"},"clientCapabilities":{"auth":{"terminal":terminal_auth},"elicitation":{"url":{}},"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}})).await.map(|result|Sensitive::new(result.response)).map_err(|error|protocol_error(&error,Reason::ProbeFailed))
    }
    pub async fn new_session(&self, cwd: &std::path::Path) -> AcpResult<Sensitive<Value>> {
        let mut setup = self
            .client
            .new_session(json!({"cwd":cwd,"mcpServers":[]}))
            .await
            .map_err(|error| protocol_error(&error, Reason::ProbeFailed))?;
        let id = setup["sessionId"].as_str().ok_or_else(|| {
            AcpError::new(
                Reason::ProbeFailed,
                "The ACP agent did not return a session identifier.",
            )
        })?;
        let state = self.client.session_state(id);
        for (key, value) in [
            ("configOptions", state.config_options),
            ("modes", state.modes),
            ("models", state.models),
        ] {
            if let Some(value) = value {
                setup[key] = value;
            }
        }
        Ok(Sensitive::new(setup))
    }
    pub fn commands(&self) -> Option<Arc<AvailableCommands>> {
        self.commands.borrow().clone()
    }
    pub async fn advertised_commands(&mut self) -> Option<Arc<AvailableCommands>> {
        if self.commands().is_none() {
            let _ = tokio::time::timeout(Duration::from_millis(500), self.commands.changed()).await;
        }
        self.commands()
    }
    pub(crate) async fn request(
        &self,
        method: &str,
        params: Sensitive<Value>,
        failure: Reason,
    ) -> AcpResult<Sensitive<Value>> {
        self.client
            .request(method, params.into_inner())
            .await
            .map(Sensitive::new)
            .map_err(|error| {
                if matches!(&error,t3_acp::Error::Rpc(error)if error.code== -32601) {
                    let reason = match failure {
                        Reason::SessionDeleteFailed => Reason::SessionDeleteUnsupported,
                        Reason::ProvidersListFailed | Reason::ProviderConfigurationFailed => {
                            Reason::ProvidersUnsupported
                        }
                        Reason::LogoutFailed => Reason::LogoutUnsupported,
                        _ => Reason::SessionListUnsupported,
                    };
                    AcpError::new(
                        reason,
                        "The ACP agent does not advertise this management operation.",
                    )
                } else {
                    protocol_error(&error, failure)
                }
            })
    }
    pub async fn authenticate(&self, method: &str) -> AcpResult<()> {
        self.client
            .authenticate(json!({"methodId":method}))
            .await
            .map(|_| ())
            .map_err(|error| protocol_error(&error, Reason::AuthenticationFailed))
    }
    pub async fn delete_probe_session(&self, id: &str) {
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            self.client.delete_session(json!({"sessionId":id})),
        )
        .await;
    }
    pub async fn close(&self) {
        self.client.shutdown().await;
        if let Some(child) = self.child.as_ref() {
            child.cancel.cancel();
            child.exited.cancelled().await;
        }
        self.events.0.abort();
    }
    pub fn generation(&self) -> t3_acp::Generation {
        self.client.generation()
    }
    pub fn process_exit(&self) -> Option<CancellationToken> {
        self.child.as_ref().map(|child| child.exited.clone())
    }
}
impl Drop for AcpConnection {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_ref() {
            child.cancel.cancel();
        }
    }
}
