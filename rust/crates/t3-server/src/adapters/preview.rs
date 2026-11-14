//! One browser/discovery host per environment, with socket-owned profile reports.
use crate::{adapters::persistence::Database, services::DomainContext};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use t3_contracts::{
    auth::{AuthEnvironmentScope, EnvironmentAuthorizationError, EnvironmentAuthorizationErrorTag},
    preview::*,
};
use t3_preview::{
    PreviewService,
    discovery::{DiscoveryRetention, PortDiscovery},
    host::{HostInputs, PreviewHost, RuntimeInputs},
    profiles::ProfileReporter,
};

pub struct PreviewAdapter {
    pub browser: PreviewService,
    pub html: t3_preview::HtmlService,
    pub discovery: PortDiscovery,
    database: Database,
    reporters: Mutex<BTreeMap<u64, Arc<ProfileReporter>>>,
    retention: Mutex<Option<DiscoveryRetention>>,
    stop: tokio_util::sync::CancellationToken,
    terminal_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
impl PreviewAdapter {
    pub async fn start(context: &DomainContext<'_>) -> crate::Result<Option<Arc<Self>>> {
        let environment = &context.environment;
        let Some(node) = node(environment) else {
            tracing::info!("preview support is unavailable: no Node runtime configured or on PATH");
            return Ok(None);
        };
        let paths = &context.local.config.paths;
        let module = environment
            .get("T3_PREVIEW_PLAYWRIGHT_MODULE")
            .map(PathBuf::from);
        let root = environment
            .get("T3_PREVIEW_MODULE_ROOT")
            .map(PathBuf::from)
            .or_else(|| {
                module
                    .as_ref()
                    .and_then(|path| path.parent())
                    .map(PathBuf::from)
            })
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(PathBuf::from))
            });
        let Some(module_root) = root else {
            return Ok(None);
        };
        let host = match PreviewHost::new(HostInputs {
            runtime: RuntimeInputs {
                node: Some(node),
                module_root,
                playwright_module: module,
                electron_run_as_node: environment
                    .get("ELECTRON_RUN_AS_NODE")
                    .is_some_and(|value| value == "1"),
            },
            base_dir: paths.base_dir.clone(),
            state_dir: paths.state_dir.clone(),
            attachments_dir: paths.attachments(),
            secrets_dir: paths.secrets(),
            sandbox: environment
                .get("T3CODE_SERVER_BROWSER_SANDBOX")
                .is_none_or(|value| value != "0"),
            desktop: Default::default(),
            recovery: Arc::new(t3_preview::html::MemoryHtmlRecovery::default()),
        })
        .await
        {
            Ok(host) => host,
            Err(error) if error.tag() == "PreviewHostConfigurationError" => {
                tracing::info!(
                    "preview support is unavailable: Node or Playwright startup validation failed"
                );
                return Ok(None);
            }
            Err(_) => {
                return Err(crate::Error::Service(
                    "Preview runtime initialization failed.".into(),
                ));
            }
        };
        let retention = host.discovery.retain().await.map_err(|_| {
            crate::Error::Service("Preview discovery initialization failed.".into())
        })?;
        Ok(Some(Arc::new(Self {
            browser: host.preview,
            html: host.html,
            discovery: host.discovery,
            database: context.database.clone(),
            reporters: Mutex::default(),
            retention: Mutex::new(Some(retention)),
            stop: context.shutdown.child_token(),
            terminal_task: Mutex::new(None),
        })))
    }
    pub async fn require_thread(&self, thread: &str) -> Result<(), EnvironmentAuthorizationError> {
        let thread = thread.to_owned();
        let exists=self.database.reader(move|connection|connection.query_row("SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_threads WHERE thread_id=?1 AND deleted_at IS NULL)",[thread],|row|row.get::<_,bool>(0)).map_err(Into::into)).await.unwrap_or(false);
        if exists {
            Ok(())
        } else {
            Err(unavailable_thread())
        }
    }
    pub async fn require_tab(&self, thread: &str, tab: &str) -> t3_preview::Result<()> {
        let listed = self
            .browser
            .manager()
            .list(PreviewListInput {
                thread_id: thread.into(),
            })
            .await;
        if listed
            .sessions
            .iter()
            .any(|snapshot| snapshot.tab_id.as_str() == tab)
        {
            Ok(())
        } else {
            Err(t3_preview::Error::TabNotFound)
        }
    }
    pub fn reporter(&self, socket: u64) -> Arc<ProfileReporter> {
        lock(&self.reporters)
            .entry(socket)
            .or_insert_with(|| Arc::new(self.browser.profile_reporter()))
            .clone()
    }
    pub fn socket_closed(&self, socket: u64) {
        lock(&self.reporters).remove(&socket);
    }
    #[cfg(feature = "terminal")]
    pub fn observe_terminals(&self, terminal: Arc<crate::adapters::terminal::TerminalAdapter>) {
        let mut metadata = terminal.manager.subscribe_metadata();
        let discovery = self.discovery.clone();
        let stop = self.stop.clone();
        *lock(&self.terminal_task) = Some(tokio::spawn(async move {
            use t3_contracts::terminal::{
                TerminalMetadataStreamEvent as Event, TerminalSessionStatus,
            };
            let mut roots = BTreeMap::new();
            let mut registered = std::collections::BTreeSet::new();
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                tokio::select! {
                    _=stop.cancelled()=>break,
                    event=metadata.recv()=>match event {
                        Some(Event::TerminalMetadataSnapshotEvent(event))=>{roots.clear();for terminal in event.terminals {if terminal.status==TerminalSessionStatus::Running && let Some(pid)=terminal.pid{roots.insert((terminal.thread_id,terminal.terminal_id),pid);}}},
                        Some(Event::TerminalMetadataUpsertEvent(event))=>{let terminal=event.terminal;let key=(terminal.thread_id,terminal.terminal_id);if terminal.status==TerminalSessionStatus::Running && let Some(pid)=terminal.pid{roots.insert(key,pid);}else{roots.remove(&key);}},
                        Some(Event::TerminalMetadataRemoveEvent(event))=>{roots.remove(&(event.thread_id,event.terminal_id));},
                        None=>break,
                    },
                    _=tick.tick()=>{},
                }
                for key in registered.difference(&roots.keys().cloned().collect()) {
                    discovery.unregister_terminal(&key.0, &key.1);
                }
                registered = roots.keys().cloned().collect();
                if roots.is_empty() {
                    continue;
                }
                let snapshot = roots.clone();
                let collected =
                    tokio::task::spawn_blocking(move || terminal_descendants(snapshot)).await;
                if let Ok(Ok(trees)) = collected {
                    for ((thread, terminal), pids) in trees {
                        discovery.register_terminal_processes(&thread, &terminal, &pids);
                    }
                } else {
                    for (thread, terminal) in &registered {
                        discovery.unregister_terminal(thread, terminal);
                    }
                    registered.clear();
                }
            }
            for (thread, terminal) in registered {
                discovery.unregister_terminal(&thread, &terminal);
            }
        }));
    }
}
impl crate::services::DomainRuntime for PreviewAdapter {
    fn name(&self) -> &'static str {
        "preview"
    }
    fn shutdown(&self) -> futures::future::BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            self.stop.cancel();
            let task = lock(&self.terminal_task).take();
            if let Some(task) = task {
                let _ = task.await;
            }
            lock(&self.reporters).clear();
            drop(lock(&self.retention).take());
            self.discovery.shutdown().await;
            self.browser
                .shutdown()
                .await
                .map_err(|_| crate::Error::Service("Preview browser shutdown failed.".into()))
        })
    }
}
#[cfg(feature = "terminal")]
fn terminal_descendants(
    roots: BTreeMap<(String, String), u64>,
) -> Result<BTreeMap<(String, String), Vec<u64>>, ()> {
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    if system.processes().is_empty() {
        return Err(());
    }
    let mut children: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for process in system.processes().values() {
        if let Some(parent) = process.parent() {
            children
                .entry(u64::from(parent.as_u32()))
                .or_default()
                .push(u64::from(process.pid().as_u32()));
        }
    }
    Ok(roots
        .into_iter()
        .map(|(key, root)| {
            if u32::try_from(root)
                .ok()
                .and_then(|pid| system.process(sysinfo::Pid::from_u32(pid)))
                .is_none()
            {
                return (key, Vec::new());
            }
            let mut pending = vec![root];
            let mut seen = std::collections::BTreeSet::new();
            while let Some(pid) = pending.pop() {
                if seen.insert(pid)
                    && let Some(next) = children.get(&pid)
                {
                    pending.extend(next);
                }
            }
            (key, seen.into_iter().collect())
        })
        .collect())
}
pub fn unavailable_thread() -> EnvironmentAuthorizationError {
    EnvironmentAuthorizationError {
        _tag: EnvironmentAuthorizationErrorTag::EnvironmentAuthorizationError,
        message: "The requested thread is not available in this environment.".into(),
        required_scope: AuthEnvironmentScope::OrchestrationRead,
        required_permission: None,
    }
}
fn node(environment: &BTreeMap<String, String>) -> Option<PathBuf> {
    if let Some(path) = environment.get("T3_PREVIEW_NODE") {
        return Some(path.into());
    }
    let path = environment
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))?
        .1;
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    std::env::split_paths(path)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}
