use crate::{
    discovery::{Driver, OpenCodeVersion, classify_opencode_version, probe_version},
    environment::{Environment, process_options},
    opencode_http::{OpenCodeError, OpenCodeHttp},
    sensitive::{SecretString, Sensitive, ServiceError, ServiceResult},
    transport::HttpTransport,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use std::{
    path::PathBuf,
    sync::{
        Arc, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use t3_process::CancellationToken;
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, oneshot},
};

pub struct OpenCodeLaunch {
    pub binary: String,
    pub directory: PathBuf,
    pub environment: Environment,
    pub server_url: String,
    pub server_password: Option<SecretString>,
}
impl std::fmt::Debug for OpenCodeLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OpenCodeLaunch([REDACTED])")
    }
}
struct ProcessControl {
    cancel: CancellationToken,
    exited: CancellationToken,
}
impl Drop for ProcessControl {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
struct Server {
    http: Arc<OpenCodeHttp>,
    version: OpenCodeVersion,
    control: Option<ProcessControl>,
    borrowers: AtomicUsize,
}
struct Cached {
    server: Arc<Server>,
    idle: CancellationToken,
}
struct Inner {
    launch: OpenCodeLaunch,
    http: Arc<dyn HttpTransport>,
    cache: Mutex<Option<Cached>>,
    ledger: Option<Arc<crate::opencode_ledger::OpenCodeLedger>>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(cached) = self.cache.get_mut().take() {
            cached.idle.cancel();
            if let Some(control) = cached.server.control.as_ref() {
                control.cancel.cancel();
            }
        }
    }
}
#[derive(Clone)]
pub struct OpenCodeRuntime(Arc<Inner>);
pub struct OpenCodeConnection {
    pub http: Arc<OpenCodeHttp>,
    pub version: OpenCodeVersion,
    pub external: bool,
    pub process_exit: Option<CancellationToken>,
    _borrow: Borrow,
}
impl std::fmt::Debug for OpenCodeConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OpenCodeConnection([REDACTED])")
    }
}
struct Borrow {
    owner: Weak<Inner>,
    server: Arc<Server>,
}
impl Drop for Borrow {
    fn drop(&mut self) {
        self.server.borrowers.fetch_sub(1, Ordering::AcqRel);
        let owner = self.owner.clone();
        let server = self.server.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
            let Some(inner)=owner.upgrade() else { return; };
            let mut cache=inner.cache.lock().await;
            let Some(cached)=cache.as_mut().filter(|cached|Arc::ptr_eq(&cached.server,&server)) else { return; };
            if server.borrowers.load(Ordering::Acquire)>0 || server.control.is_none() { return; }
            cached.idle.cancel(); cached.idle=CancellationToken::new();
            let idle=cached.idle.clone();
            drop(cache); drop(inner);
            tokio::select! { _=idle.cancelled()=>return,_=tokio::time::sleep(Duration::from_secs(30))=>{} }
            let Some(inner)=owner.upgrade() else { return; };
            let mut cache=inner.cache.lock().await;
            if cache.as_ref().is_some_and(|cached|Arc::ptr_eq(&cached.server,&server)&&cached.server.borrowers.load(Ordering::Acquire)==0&&!cached.idle.is_cancelled()) {
                if let Some(control)=server.control.as_ref() { control.cancel.cancel(); }
                cache.take();
            }
        });
        }
    }
}
impl OpenCodeRuntime {
    pub(crate) fn transport(&self) -> Arc<dyn HttpTransport> {
        self.0.http.clone()
    }
    pub fn new(launch: OpenCodeLaunch, http: Arc<dyn HttpTransport>) -> Self {
        Self::new_with_ledger(launch, http, None)
    }
    pub fn new_with_ledger(
        launch: OpenCodeLaunch,
        http: Arc<dyn HttpTransport>,
        ledger: Option<Arc<crate::opencode_ledger::OpenCodeLedger>>,
    ) -> Self {
        Self(Arc::new(Inner {
            launch,
            http,
            cache: Mutex::new(None),
            ledger,
        }))
    }
    pub async fn installed_version(&self) -> ServiceResult<Option<OpenCodeVersion>> {
        let launch = &self.0.launch;
        let probe = probe_version(
            Driver::OpenCode,
            Some(&launch.binary),
            &launch.environment,
            Some(&launch.directory),
        )
        .await
        .map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))?;
        Ok(probe.version.as_deref().and_then(classify_opencode_version))
    }
    pub fn external(&self) -> bool {
        !self.0.launch.server_url.trim().is_empty()
    }
    pub fn local_server(&self) -> bool {
        url::Url::parse(self.0.launch.server_url.trim())
            .ok()
            .is_some_and(|url| {
                matches!(url.scheme(), "http" | "https")
                    && match url.host() {
                        Some(url::Host::Domain("localhost")) => true,
                        Some(url::Host::Ipv4(address)) => address.is_loopback(),
                        Some(url::Host::Ipv6(address)) => address.is_loopback(),
                        _ => false,
                    }
            })
    }
    pub async fn probe(&self) -> Result<OpenCodeVersion, OpenCodeError> {
        if self.external() {
            let version = OpenCodeHttp::new(
                self.0.http.clone(),
                &self.0.launch.server_url,
                self.0.launch.server_password.clone(),
            )?
            .probe(&self.0.launch.environment)
            .await?;
            let mut cache = self.0.cache.lock().await;
            if cache.as_ref().is_some_and(|cached| {
                cached.server.control.is_none() && cached.server.version != version
            }) && let Some(cached) = cache.take()
            {
                cached.idle.cancel();
            }
            Ok(version)
        } else {
            self.installed_version()
                .await
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    OpenCodeError::Process,
                ))?
                .ok_or(OpenCodeError::Process)
        }
    }
    pub async fn acquire(&self) -> Result<OpenCodeConnection, OpenCodeError> {
        let mut cache = self.0.cache.lock().await;
        if cache.as_ref().is_some_and(|cached| {
            cached
                .server
                .control
                .as_ref()
                .is_some_and(|control| control.exited.is_cancelled())
        }) {
            cache.take();
        }
        if cache.is_none() {
            let launch = &self.0.launch;
            let server = if !launch.server_url.trim().is_empty() {
                let http = Arc::new(OpenCodeHttp::new(
                    self.0.http.clone(),
                    &launch.server_url,
                    launch.server_password.clone(),
                )?);
                let version = http.probe(&launch.environment).await?;
                if version.generation == 1
                    && crate::versions::compare_versions(&version.version, "1.14.19").is_lt()
                {
                    return Err(OpenCodeError::Process);
                }
                Arc::new(Server {
                    http,
                    version,
                    control: None,
                    borrowers: AtomicUsize::new(0),
                })
            } else {
                let installed = self
                    .installed_version()
                    .await
                    .map_err(crate::diagnostics::mapped(
                        concat!(module_path!(), "::error"),
                        OpenCodeError::Process,
                    ))?
                    .ok_or(OpenCodeError::Process)?;
                if installed.generation == 1
                    && crate::versions::compare_versions(&installed.version, "1.14.19").is_lt()
                {
                    return Err(OpenCodeError::Process);
                }
                start_owned(
                    launch,
                    self.0.http.clone(),
                    installed,
                    self.0.ledger.as_deref(),
                )
                .await?
            };
            *cache = Some(Cached {
                server,
                idle: CancellationToken::new(),
            });
        }
        let cached = cache.as_mut().ok_or(OpenCodeError::Process)?;
        cached.idle.cancel();
        cached.server.borrowers.fetch_add(1, Ordering::AcqRel);
        let server = cached.server.clone();
        Ok(OpenCodeConnection {
            http: server.http.clone(),
            version: server.version.clone(),
            external: server.control.is_none(),
            process_exit: server
                .control
                .as_ref()
                .map(|control| control.exited.clone()),
            _borrow: Borrow {
                owner: Arc::downgrade(&self.0),
                server,
            },
        })
    }
    pub async fn forget(&self) -> ServiceResult<()> {
        let mut cache = self.0.cache.lock().await;
        if cache.as_ref().is_some_and(|cached| {
            cached.server.borrowers.load(Ordering::Acquire) > 0 && cached.server.control.is_some()
        }) {
            return Err(ServiceError::Busy);
        }
        let cached = cache.take();
        drop(cache);
        if let Some(cached) = cached {
            cached.idle.cancel();
            if let Some(control) = cached.server.control.as_ref() {
                control.cancel.cancel();
                control.exited.cancelled().await;
            }
        }
        Ok(())
    }
    pub async fn close(&self) {
        let cached = self.0.cache.lock().await.take();
        if let Some(cached) = cached {
            cached.idle.cancel();
            if let Some(control) = cached.server.control.as_ref() {
                control.cancel.cancel();
                control.exited.cancelled().await;
            }
        }
    }
}
struct Drain(tokio::task::JoinHandle<()>);
impl Drop for Drain {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn start_owned(
    launch: &OpenCodeLaunch,
    transport: Arc<dyn HttpTransport>,
    installed: OpenCodeVersion,
    ledger: Option<&crate::opencode_ledger::OpenCodeLedger>,
) -> Result<Arc<Server>, OpenCodeError> {
    let socket =
        tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                OpenCodeError::Process,
            ))?;
    let port = socket
        .local_addr()
        .map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            OpenCodeError::Process,
        ))?
        .port();
    drop(socket);
    let mut environment = launch.environment.clone();
    let password = if installed.generation == 2 {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            OpenCodeError::Process,
        ))?;
        let password = Sensitive::new(URL_SAFE_NO_PAD.encode(bytes));
        environment.retain(|name, _| {
            !name.eq_ignore_ascii_case("OPENCODE_SERVER_PASSWORD")
                && !name.eq_ignore_ascii_case("OPENCODE_PASSWORD")
        });
        environment.insert("OPENCODE_PASSWORD".into(), password.expose().clone());
        Some(password)
    } else {
        launch.server_password.clone().or_else(|| {
            environment
                .get("OPENCODE_SERVER_PASSWORD")
                .cloned()
                .map(Sensitive::new)
        })
    };
    if installed.generation == 1
        && let Some(password) = password.as_ref()
    {
        environment.insert("OPENCODE_SERVER_PASSWORD".into(), password.expose().clone());
    }
    environment
        .entry("OPENCODE_CONFIG_CONTENT".into())
        .or_insert_with(|| "{}".into());
    let mut options = process_options(
        &launch.binary,
        &[
            "serve".into(),
            "--hostname=127.0.0.1".into(),
            format!("--port={port}"),
        ],
        &environment,
    );
    options.cwd = Some(launch.directory.clone());
    let mut process =
        t3_process::spawn_non_blocking(&options)
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                OpenCodeError::Process,
            ))?;
    let lease = if let (Some(ledger), Some(pid)) = (ledger, process.id()) {
        ledger
            .track(
                pid,
                port,
                vec![
                    "serve".into(),
                    "--hostname=127.0.0.1".into(),
                    format!("--port={port}"),
                ],
            )
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let mut stdout = process.stdout.take().ok_or(OpenCodeError::Process)?;
    let stderr = process.stderr.take();
    let control = ProcessControl {
        cancel: CancellationToken::new(),
        exited: CancellationToken::new(),
    };
    let cancel = control.cancel.clone();
    let exited = control.exited.clone();
    let (ready, started) = oneshot::channel();
    let reader = Drain(tokio::spawn(async move {
        let mut ready = Some(ready);
        let mut bytes = Vec::new();
        let mut chunk = [0; 8192];
        let pattern = regex::Regex::new(r"(?i)server listening on\s+(https?://[^\s]+)\s")
            .expect("OpenCode startup URL");
        while let Ok(count) = stdout.read(&mut chunk).await {
            if count == 0 {
                break;
            }
            if ready.is_none() {
                continue;
            }
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.len() > 64 * 1024 {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(OpenCodeError::Process));
                }
                continue;
            }
            if let Some(found) = pattern
                .captures(&String::from_utf8_lossy(&bytes))
                .and_then(|found| found.get(1).map(|found| found.as_str().to_owned()))
                && let Some(ready) = ready.take()
            {
                let _ = ready.send(Ok(found));
                bytes.clear();
            }
        }
    }));
    let errors = stderr.map(|mut stderr| {
        Drain(tokio::spawn(async move {
            let mut bytes = [0; 8192];
            while let Ok(count) = stderr.read(&mut bytes).await {
                if count == 0 {
                    break;
                }
            }
        }))
    });
    tokio::spawn(async move {
        let _reader = reader;
        let _errors = errors;
        tokio::select! {
            biased;
            _=cancel.cancelled()=>{let _=process.terminate(Duration::from_secs(1)).await;},
            _=process.wait()=>{},
        }
        if let Some(lease) = lease {
            let _ = lease.forget().await;
        }
        exited.cancel();
    });
    let timeout = Duration::from_millis(crate::versions::probe_timeout_ms(
        "opencode",
        30_000,
        &launch.environment,
    ));
    let work = async {
        let url = tokio::select! {biased;_=control.exited.cancelled()=>return Err(OpenCodeError::Process),result=started=>result.map_err(crate::diagnostics::mapped(concat!(module_path!(), "::error"), OpenCodeError::Process))??};
        let parsed = url::Url::parse(&url).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            OpenCodeError::Identity,
        ))?;
        if parsed.scheme() != "http"
            || parsed.host_str() != Some("127.0.0.1")
            || parsed.port_or_known_default() != Some(port)
        {
            return Err(OpenCodeError::Identity);
        }
        let http = Arc::new(OpenCodeHttp::new(transport, &url, password)?);
        let version = http.probe(&launch.environment).await?;
        if version.generation != installed.generation {
            return Err(OpenCodeError::Identity);
        }
        Ok((http, version))
    };
    let result = tokio::time::timeout(timeout, work)
        .await
        .map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            OpenCodeError::Timeout,
        ))
        .and_then(|result| result);
    let (http, version) = match result {
        Ok(result) => result,
        Err(error) => {
            control.cancel.cancel();
            control.exited.cancelled().await;
            return Err(error);
        }
    };
    Ok(Arc::new(Server {
        http,
        version,
        control: Some(control),
        borrowers: AtomicUsize::new(0),
    }))
}
