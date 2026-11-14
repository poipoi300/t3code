use crate::{
    chatgpt::ChatGptAuth,
    sensitive::{SecretString, Sensitive, ServiceError, ServiceResult},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use t3_contracts::provider_instance::ProviderInstanceId;
use t3_process::CancellationToken;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{RwLock, Semaphore},
    task::JoinSet,
};

pub const REQUEST_BYTES: usize = 1024;
pub const RESPONSE_BYTES: usize = 64 * 1024;
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(80);
const SOCKET_IDLE: Duration = Duration::from_secs(5);
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TokenRequest {
    pub version: u8,
    pub instance_id: ProviderInstanceId,
    pub environment_id: String,
    pub profile_key: String,
}
impl std::fmt::Debug for TokenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenRequest([REDACTED])")
    }
}
impl TokenRequest {
    fn validate(&self) -> ServiceResult<()> {
        if self.version != 1
            || self.environment_id.is_empty()
            || self.profile_key.len() != 64
            || !self
                .profile_key
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(ServiceError::Response);
        }
        Ok(())
    }
}
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    max: usize,
    timeout: Duration,
) -> ServiceResult<Sensitive<Vec<u8>>> {
    tokio::time::timeout(timeout, async {
        let mut frame = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let read = reader
                .read(&mut chunk)
                .await
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ))?;
            if read == 0 || frame.len().saturating_add(read) > max {
                return Err(ServiceError::Response);
            }
            frame.extend_from_slice(&chunk[..read]);
            if let Some(end) = frame.iter().position(|c| *c == b'\n') {
                if end != frame.len() - 1 {
                    return Err(ServiceError::Response);
                }
                frame.pop();
                return Ok(Sensitive::new(frame));
            }
        }
    })
    .await
    .map_err(crate::diagnostics::mapped(
        concat!(module_path!(), "::error"),
        ServiceError::Response,
    ))?
}
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &[u8],
    max: usize,
) -> ServiceResult<()> {
    if frame.len().saturating_add(1) > max {
        return Err(ServiceError::Response);
    }
    tokio::time::timeout(SOCKET_IDLE, async {
        writer
            .write_all(frame)
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?;
        writer
            .write_all(b"\n")
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?;
        writer.shutdown().await.map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))
    })
    .await
    .map_err(crate::diagnostics::mapped(
        concat!(module_path!(), "::error"),
        ServiceError::Response,
    ))?
}
pub struct ManagedTokenBroker {
    environment_id: String,
    instances: RwLock<BTreeMap<ProviderInstanceId, Arc<ChatGptAuth>>>,
    jobs: Arc<Semaphore>,
    connections: Arc<Semaphore>,
}
impl ManagedTokenBroker {
    pub fn new(environment_id: String) -> Arc<Self> {
        Arc::new(Self {
            environment_id,
            instances: RwLock::new(BTreeMap::new()),
            jobs: Arc::new(Semaphore::new(16)),
            connections: Arc::new(Semaphore::new(32)),
        })
    }
    pub async fn register(&self, instance: ProviderInstanceId, auth: Arc<ChatGptAuth>) {
        self.instances.write().await.insert(instance, auth);
    }
    pub async fn unregister(&self, instance: &ProviderInstanceId) {
        self.instances.write().await.remove(instance);
    }
    pub async fn serve<S: AsyncRead + AsyncWrite + Unpin>(&self, stream: S) -> ServiceResult<()> {
        let connection =
            self.connections
                .clone()
                .try_acquire_owned()
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Busy,
                ))?;
        self.serve_admitted(stream, connection).await
    }
    async fn serve_admitted<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        stream: S,
        _connection: tokio::sync::OwnedSemaphorePermit,
    ) -> ServiceResult<()> {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let frame = read_frame(&mut reader, REQUEST_BYTES, SOCKET_IDLE).await?;
        let request: TokenRequest = serde_json::from_slice(frame.expose()).map_err(
            crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response),
        )?;
        request.validate()?;
        if request.environment_id != self.environment_id {
            return Err(ServiceError::Identity);
        }
        let _job = self
            .jobs
            .clone()
            .try_acquire_owned()
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Busy,
            ))?;
        let auth = self
            .instances
            .read()
            .await
            .get(&request.instance_id)
            .cloned()
            .ok_or(ServiceError::Unavailable)?;
        let credentials =
            tokio::time::timeout(COMMAND_TIMEOUT, auth.access(Some(request.profile_key)))
                .await
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Refresh,
                ))??;
        let frame = serde_json::to_vec(&serde_json::json!({"token":credentials.access_token}))
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?;
        write_frame(&mut writer, &frame, RESPONSE_BYTES).await
    }
    pub async fn listen(
        self: &Arc<Self>,
        parent: &Path,
        owner_uid: Option<u32>,
    ) -> ServiceResult<BrokerListener> {
        let cancel = CancellationToken::new();
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            let _ = (parent, owner_uid);
            let endpoint = format!("\\\\.\\pipe\\t3-token-{}", uuid::Uuid::new_v4());
            let initial = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(&endpoint)
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ))?;
            let broker = self.clone();
            let endpoint_task = endpoint.clone();
            let stop = cancel.clone();
            let task = tokio::spawn(async move {
                let mut server = initial;
                let mut tasks = JoinSet::new();
                loop {
                    tokio::select! {
                        _=stop.cancelled()=>break,
                        connected=server.connect()=>{
                            if connected.is_err(){break;}
                            let next=match ServerOptions::new().reject_remote_clients(true).create(&endpoint_task){Ok(next)=>next,Err(_)=>break};
                            let connected=std::mem::replace(&mut server,next);
                            let Ok(permit)=broker.connections.clone().try_acquire_owned() else{drop(connected);continue;};
                            let broker=broker.clone();
                            tasks.spawn(async move {let _=broker.serve_admitted(connected,permit).await;});
                        }
                        _=tasks.join_next(),if !tasks.is_empty()=>{}
                    }
                }
                drop(server);
                while tasks.join_next().await.is_some() {}
            });
            Ok(BrokerListener {
                endpoint,
                cancel,
                task: tokio::sync::Mutex::new(Some(task)),
            })
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let uid = owner_uid.ok_or(ServiceError::Identity)?;
            let parent = std::fs::canonicalize(parent).map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?;
            let metadata =
                std::fs::symlink_metadata(&parent).map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ))?;
            if !metadata.is_dir()
                || !((metadata.uid() == uid && metadata.mode() & 0o022 == 0)
                    || metadata.uid() == 0
                        && (metadata.mode() & 0o022 == 0 || metadata.mode() & 0o1000 != 0))
            {
                return Err(ServiceError::Identity);
            }
            let directory = tempfile::Builder::new()
                .prefix("t3-token-")
                .tempdir_in(parent)
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ))?;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ))?;
            let created =
                std::fs::symlink_metadata(directory.path()).map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ))?;
            if !private_directory(
                created.is_dir(),
                created.file_type().is_symlink(),
                created.uid(),
                uid,
                created.mode(),
            ) {
                return Err(ServiceError::Identity);
            }
            let directory = OwnedDirectory {
                path: directory.keep(),
                device: created.dev(),
                inode: created.ino(),
            };
            let path = directory.path.join("s");
            if path.as_os_str().as_encoded_bytes().len() > 100 {
                return Err(ServiceError::Response);
            }
            let listener =
                tokio::net::UnixListener::bind(&path).map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(
                crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ),
            )?;
            let socket_identity =
                std::fs::symlink_metadata(&path).map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ))?;
            let endpoint = path.to_string_lossy().into_owned();
            let stop = cancel.clone();
            let broker = self.clone();
            let task = tokio::spawn(async move {
                let mut tasks = JoinSet::new();
                loop {
                    tokio::select! {
                        _=stop.cancelled()=>break,
                        accepted=listener.accept()=>{let Ok((stream,_))=accepted else{break;};let Ok(permit)=broker.connections.clone().try_acquire_owned() else{drop(stream);continue;};let broker=broker.clone();tasks.spawn(async move {let _=broker.serve_admitted(stream,permit).await;});}
                        _=tasks.join_next(),if !tasks.is_empty()=>{}
                    }
                }
                drop(listener);
                while tasks.join_next().await.is_some() {}
                if std::fs::symlink_metadata(&path).is_ok_and(|m| {
                    m.dev() == socket_identity.dev() && m.ino() == socket_identity.ino()
                }) {
                    let _ = std::fs::remove_file(&path);
                }
                drop(directory);
            });
            Ok(BrokerListener {
                endpoint,
                cancel,
                task: tokio::sync::Mutex::new(Some(task)),
            })
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = (parent, owner_uid);
            Err(ServiceError::Unsupported)
        }
    }
}
pub struct BrokerListener {
    pub endpoint: String,
    cancel: CancellationToken,
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}
#[cfg(unix)]
struct OwnedDirectory {
    path: std::path::PathBuf,
    device: u64,
    inode: u64,
}
#[cfg(unix)]
impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        use std::os::unix::fs::MetadataExt;
        if std::fs::symlink_metadata(&self.path).is_ok_and(|m| {
            !m.file_type().is_symlink() && m.dev() == self.device && m.ino() == self.inode
        }) {
            let _ = std::fs::remove_dir(&self.path);
        }
    }
}
impl BrokerListener {
    pub async fn close(&self) {
        self.cancel.cancel();
        let mut running = self.task.lock().await;
        if let Some(task) = running.take() {
            let _ = task.await;
        }
    }
    pub async fn shutdown(self) {
        self.close().await;
    }
}
impl Drop for BrokerListener {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
pub async fn request_token<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    request: &TokenRequest,
) -> ServiceResult<SecretString> {
    request.validate()?;
    let (mut reader, mut writer) = tokio::io::split(stream);
    write_frame(
        &mut writer,
        &serde_json::to_vec(request).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))?,
        REQUEST_BYTES,
    )
    .await?;
    let response = read_frame(&mut reader, RESPONSE_BYTES, COMMAND_TIMEOUT).await?;
    #[derive(Deserialize)]
    struct Response {
        token: SecretString,
    }
    let response: Response = serde_json::from_slice(response.expose()).map_err(
        crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response),
    )?;
    if response.token.expose().is_empty() {
        return Err(ServiceError::Response);
    }
    Ok(response.token)
}
pub async fn request_endpoint_token(
    endpoint: &str,
    request: &TokenRequest,
) -> ServiceResult<SecretString> {
    #[cfg(windows)]
    {
        if !endpoint.starts_with("\\\\.\\pipe\\t3-token-") || endpoint.contains(['\r', '\n', '\0'])
        {
            return Err(ServiceError::Response);
        }
        let connect = async {
            loop {
                match tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint) {
                    Ok(stream) => return Ok(stream),
                    Err(error) if error.raw_os_error() == Some(231) => {
                        tokio::time::sleep(Duration::from_millis(25)).await
                    }
                    Err(_) => return Err(ServiceError::Response),
                }
            }
        };
        let stream = tokio::time::timeout(SOCKET_IDLE, connect).await.map_err(
            crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response),
        )??;
        request_token(stream, request).await
    }
    #[cfg(unix)]
    {
        let stream = tokio::time::timeout(SOCKET_IDLE, tokio::net::UnixStream::connect(endpoint))
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?;
        request_token(stream, request).await
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (endpoint, request);
        Err(ServiceError::Unsupported)
    }
}

#[cfg(any(unix, test))]
fn private_directory(directory: bool, symlink: bool, owner: u32, uid: u32, mode: u32) -> bool {
    directory && !symlink && owner == uid && mode & 0o777 == 0o700
}
#[cfg(test)]
mod directory_tests {
    #[test]
    fn only_a_non_symlink_directory_owned_by_the_expected_uid_with_0700_is_private() {
        assert!(super::private_directory(true, false, 7, 7, 0o40700));
        for fields in [
            (false, false, 7, 7, 0o100700),
            (true, true, 7, 7, 0o40700),
            (true, false, 8, 7, 0o40700),
            (true, false, 7, 7, 0o40750),
            (true, false, 7, 7, 0o40600),
        ] {
            assert!(!super::private_directory(
                fields.0, fields.1, fields.2, fields.3, fields.4
            ));
        }
    }
}
