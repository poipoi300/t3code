//! PTY integration and the per-call output window used by the WebSocket transport.
#[cfg(feature = "terminal")]
mod enabled {
    use crate::registry::{Failure, ServerDefect};
    use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
    use std::sync::{Arc, Mutex};
    use t3_contracts::{rpc::WsTerminalOpenError, terminal::*};
    pub use t3_terminal::{AttachStream, ManagerOptions, MetadataStream, TerminalManager};
    use t3_terminal::{OutputWindow, TerminalError as ServiceError};
    use tokio::sync::Notify;

    /// One instance per streaming RPC. The transport calls `ack` for each real client ACK.
    #[derive(Default)]
    pub struct TerminalOutputWindow {
        inner: Mutex<OutputWindow>,
        ready: Notify,
    }
    impl TerminalOutputWindow {
        pub fn is_full(&self) -> bool {
            self.inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_full()
        }
        pub fn sent(&self, events: &[TerminalAttachStreamEvent]) {
            self.inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .sent(events);
        }
        pub fn ack(&self) {
            if self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .ack()
            {
                self.ready.notify_one();
            }
        }
        pub async fn capacity(&self) {
            loop {
                let ready = self.ready.notified();
                if !self.is_full() {
                    return;
                }
                ready.await;
            }
        }
    }

    pub type TerminalResult<T> = std::result::Result<T, Failure<WsTerminalOpenError>>;
    /// The application owns cwd/worktree recovery and provider launch preparation.
    pub trait TerminalLaunchPreparation: Send + Sync {
        fn open(
            &self,
            input: TerminalOpenInput,
        ) -> BoxFuture<'_, TerminalResult<TerminalOpenInput>>;
        fn attach(
            &self,
            input: TerminalAttachInput,
        ) -> BoxFuture<'_, TerminalResult<TerminalAttachInput>>;
    }
    pub struct TerminalAdapter {
        /// Starting a process through it directly bypasses `gate`; setup scripts go through
        /// `ProjectSetupAdapter`, which enters it.
        pub manager: TerminalManager,
        pub launch: Arc<dyn TerminalLaunchPreparation>,
        /// The environment's one writer gate (`crate::writers`).
        pub gate: Arc<crate::writers::FolderGate>,
        #[cfg(all(
            feature = "workspace",
            feature = "orchestration",
            feature = "local-services"
        ))]
        recovery: std::sync::OnceLock<
            std::sync::Weak<t3_orchestration::composition::OrchestrationRuntime>,
        >,
    }
    impl TerminalAdapter {
        pub fn new(
            mut options: ManagerOptions,
            launch: Arc<dyn TerminalLaunchPreparation>,
        ) -> crate::Result<Self> {
            options.max_queued_output = options.max_queued_output.min(64 * 1024);
            let manager =
                TerminalManager::new(options).map_err(|e| crate::Error::Service(e.to_string()))?;
            Ok(Self {
                manager,
                launch,
                gate: Arc::default(),
                #[cfg(all(
                    feature = "workspace",
                    feature = "orchestration",
                    feature = "local-services"
                ))]
                recovery: Default::default(),
            })
        }
        #[cfg(all(
            feature = "workspace",
            feature = "orchestration",
            feature = "local-services"
        ))]
        pub(crate) fn bind_worktree_recovery(
            &self,
            runtime: &Arc<t3_orchestration::composition::OrchestrationRuntime>,
        ) {
            let _ = self.recovery.set(Arc::downgrade(runtime));
        }

        #[cfg(all(
            feature = "workspace",
            feature = "orchestration",
            feature = "local-services"
        ))]
        async fn prepare_worktree(
            &self,
            thread_id: &str,
            cwd: &str,
        ) -> TerminalResult<
            Option<(
                t3_contracts::base_schemas::TrimmedNonEmptyString,
                t3_orchestration::composition::TerminalWorktreeLease,
            )>,
        > {
            use t3_orchestration::api::ProjectionReader;
            use t3_workspace::paths;
            let Some(runtime) = self.recovery.get().and_then(std::sync::Weak::upgrade) else {
                return Ok(None);
            };
            let Ok(Some(thread)) = runtime
                .projections
                .thread(t3_orchestration::api::ThreadId(thread_id.into()))
                .await
            else {
                return Ok(None);
            };
            let Some(worktree) = thread.worktree_path.clone() else {
                return Ok(None);
            };
            let root = std::path::Path::new(&worktree.0);
            let cwd = std::path::Path::new(cwd);
            if !root.is_absolute()
                || !cwd.is_absolute()
                || paths::validate_host_path(root).is_err()
                || paths::validate_host_path(cwd).is_err()
                || !paths::absolute(cwd).is_ok_and(|path| paths::within(&path, root))
            {
                return Ok(None);
            }
            match runtime.lease_terminal_worktree(thread).await {
                Ok(lease) => Ok(lease.map(|guard| (worktree, guard))),
                Err(error) => {
                    tracing::warn!(%error, "terminal worktree recovery failed");
                    Err(failure(
                        ServiceError::CwdNotFound {
                            cwd: cwd.to_string_lossy().into_owned(),
                        },
                        thread_id,
                        "",
                    ))
                }
            }
        }
        async fn launch_with_recovery<T, F>(
            &self,
            thread: &str,
            terminal: &str,
            cwd: Option<&str>,
            launch: impl Fn(Option<t3_contracts::base_schemas::TrimmedNonEmptyString>) -> F,
        ) -> TerminalResult<T>
        where
            F: Future<Output = Result<T, ServiceError>>,
        {
            #[cfg(all(
                feature = "workspace",
                feature = "orchestration",
                feature = "local-services"
            ))]
            if let Some(cwd) = cwd
                && let Some((path, lease)) = self.prepare_worktree(thread, cwd).await?
            {
                let result = launch(Some(path.clone())).await;
                if matches!(&result, Err(ServiceError::CwdNotFound { .. })) {
                    match lease.recover_missing_cwd().await {
                        Ok(()) => {
                            return launch(Some(path))
                                .await
                                .map_err(|e| failure(e, thread, terminal));
                        }
                        Err(error) => tracing::warn!(%error, "terminal worktree recovery failed"),
                    }
                }
                return result.map_err(|e| failure(e, thread, terminal));
            }
            let _ = cwd;
            launch(None).await.map_err(|e| failure(e, thread, terminal))
        }
        pub async fn open(
            &self,
            input: TerminalOpenInput,
        ) -> TerminalResult<TerminalSessionSnapshot> {
            let _admitted = self
                .gate
                .enter_folders(
                    format!("terminal `{}`", input.terminal_id.0),
                    Some(&input.thread_id.0),
                    &std::iter::once(std::path::Path::new(&input.cwd.0))
                        .chain(
                            input
                                .worktree_path
                                .iter()
                                .flatten()
                                .map(|p| std::path::Path::new(&p.0)),
                        )
                        .collect::<Vec<_>>(),
                )
                .await;
            let input = self.launch.open(input).await?;
            self.launch_with_recovery(
                &input.thread_id.0,
                &input.terminal_id.0,
                Some(&input.cwd.0),
                |path| {
                    let mut input = input.clone();
                    if let Some(path) = path {
                        input.worktree_path = Some(Some(path));
                    }
                    self.manager.open(input)
                },
            )
            .await
        }
        pub async fn attach(
            &self,
            input: TerminalAttachInput,
            viewer_id: String,
        ) -> TerminalResult<AttachStream> {
            // An attach may restart an exited terminal.
            let _admitted = self
                .gate
                .enter_folders(
                    format!("terminal `{}`", input.terminal_id.0),
                    Some(&input.thread_id.0),
                    &input
                        .cwd
                        .iter()
                        .flatten()
                        .chain(input.worktree_path.iter().flatten())
                        .map(|p| std::path::Path::new(&p.0))
                        .collect::<Vec<_>>(),
                )
                .await;
            let input = self.launch.attach(input).await?;
            // An attach without a cwd can only observe an existing session. Let the
            // manager take that path first. Only an exited session needs a requested restart.
            if input.cwd.as_ref().is_some_and(Option::is_some) {
                let mut existing = input.clone();
                existing.cwd = None;
                match self.manager.attach(existing, Some(viewer_id.clone())).await {
                    Ok(stream) => {
                        let restart = input.restart_if_not_running.flatten() == Some(true)
                            && input.resume.flatten() != Some(true);
                        if !restart
                            || self
                                .manager
                                .get_shell(TerminalObserveInput {
                                    thread_id: input.thread_id.clone(),
                                    terminal_id: input.terminal_id.clone(),
                                })
                                .await
                                .is_ok()
                        {
                            return Ok(stream);
                        }
                        drop(stream);
                    }
                    Err(ServiceError::SessionLookup { .. }) => {}
                    Err(error) => {
                        return Err(failure(error, &input.thread_id.0, &input.terminal_id.0));
                    }
                }
            }
            let cwd = input
                .cwd
                .as_ref()
                .and_then(Option::as_ref)
                .map(|v| v.0.as_str());
            self.launch_with_recovery(&input.thread_id.0, &input.terminal_id.0, cwd, |path| {
                let mut input = input.clone();
                if let Some(path) = path {
                    input.worktree_path = Some(Some(path));
                }
                self.manager.attach(input, Some(viewer_id.clone()))
            })
            .await
        }
        pub async fn observe(&self, input: TerminalObserveInput) -> TerminalResult<AttachStream> {
            self.manager
                .observe(input.clone())
                .await
                .map_err(|e| failure(e, &input.thread_id.0, &input.terminal_id.0))
        }
        /// Not gated: input reaches only a running shell, and a hold refuses while any shell in
        /// its folder or thread runs.
        pub async fn write(&self, input: TerminalWriteInput) -> TerminalResult<()> {
            self.manager
                .write(input.clone())
                .await
                .map_err(|e| failure(e, &input.thread_id.0, &input.terminal_id.0))
        }
        pub async fn resize(
            &self,
            input: TerminalResizeInput,
            viewer_id: String,
        ) -> TerminalResult<()> {
            self.manager
                .resize(input.clone(), Some(viewer_id))
                .await
                .map_err(|e| failure(e, &input.thread_id.0, &input.terminal_id.0))
        }
        pub async fn clear(&self, input: TerminalClearInput) -> TerminalResult<()> {
            self.manager
                .clear(input.clone())
                .await
                .map_err(|e| failure(e, &input.thread_id.0, &input.terminal_id.0))
        }
        pub async fn close(&self, input: TerminalCloseInput) -> TerminalResult<()> {
            self.manager.close(input.clone()).await.map_err(|e| {
                failure(
                    e,
                    &input.thread_id.0,
                    input
                        .terminal_id
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map_or("", |id| &id.0),
                )
            })
        }
        pub fn metadata(
            &self,
        ) -> BoxStream<
            'static,
            std::result::Result<
                TerminalMetadataStreamEvent,
                Failure<t3_contracts::auth::EnvironmentAuthorizationError>,
            >,
        > {
            let mut stream = self.manager.subscribe_metadata();
            async_stream::stream! { while let Some(event) = stream.recv().await { yield Ok(event); } }.boxed()
        }
    }
    /// Opens terminals exactly where asked. TypeScript first recovers a thread's worktree and
    /// prepares provider launch environments; that needs orchestration and providers.
    #[cfg(feature = "local-services")]
    struct PassThroughLaunch;
    #[cfg(feature = "local-services")]
    impl TerminalLaunchPreparation for PassThroughLaunch {
        fn open(
            &self,
            input: TerminalOpenInput,
        ) -> BoxFuture<'_, TerminalResult<TerminalOpenInput>> {
            Box::pin(async move { Ok(input) })
        }
        fn attach(
            &self,
            input: TerminalAttachInput,
        ) -> BoxFuture<'_, TerminalResult<TerminalAttachInput>> {
            Box::pin(async move { Ok(input) })
        }
    }
    #[cfg(feature = "local-services")]
    impl TerminalAdapter {
        /// Composition-root hook (package A): `<state>/logs/terminals`, the captured account
        /// home and environment.
        pub fn start(
            context: &crate::services::DomainContext<'_>,
            gate: Arc<crate::writers::FolderGate>,
        ) -> crate::Result<Arc<Self>> {
            let mut options = ManagerOptions::from_state_dir(&context.local.config.paths.state_dir);
            options.home = context.account_home.clone();
            options.env = context.environment.clone();
            options.app_version = env!("CARGO_PKG_VERSION").into();
            let mut adapter = Self::new(options, Arc::new(PassThroughLaunch))?;
            adapter.gate = gate;
            Ok(Arc::new(adapter))
        }
    }
    /// The restore's writer port (t3-git H-2). Terminals are the host's only processes that
    /// write in a thread's folder: setup and settle scripts run in them and preview servers
    /// are discovered from them. A live shell cannot be paused, so each one is named and the
    /// restore refuses; new ones wait for the hold. TypeScript restores under live writers.
    #[cfg(feature = "orchestration")]
    pub struct TerminalWriters(pub Arc<TerminalAdapter>);
    #[cfg(feature = "orchestration")]
    #[async_trait::async_trait]
    impl t3_orchestration::api::ThreadWriters for TerminalWriters {
        async fn hold(
            &self,
            thread_id: t3_contracts::base_schemas::ThreadId,
            cwd: std::path::PathBuf,
        ) -> t3_orchestration::error::Result<t3_orchestration::api::WriterHold> {
            let (guard, mut running) = self.0.gate.hold(&thread_id.0, &cwd);
            let mut metadata = self.0.manager.subscribe_metadata();
            let terminals = match metadata.recv().await {
                Some(TerminalMetadataStreamEvent::TerminalMetadataSnapshotEvent(snapshot)) => {
                    snapshot.terminals
                }
                // The manager stopped: no terminal can be running.
                _ => Vec::new(),
            };
            let live = terminals
                .into_iter()
                .filter(|terminal| {
                    matches!(
                        terminal.status,
                        TerminalSessionStatus::Starting | TerminalSessionStatus::Running
                    ) && (terminal.thread_id == thread_id.0
                        || crate::writers::overlaps(std::path::Path::new(&terminal.cwd), &cwd)
                        || terminal.worktree_path.as_ref().is_some_and(|p| {
                            crate::writers::overlaps(std::path::Path::new(&p.0), &cwd)
                        }))
                })
                .map(|terminal| {
                    let name = if terminal.label.trim().is_empty() {
                        terminal.terminal_id
                    } else {
                        terminal.label
                    };
                    format!("terminal `{name}`")
                });
            for name in live {
                if !running.contains(&name) {
                    running.push(name);
                }
            }
            Ok(t3_orchestration::api::WriterHold {
                running,
                guard: Box::new(guard),
            })
        }
    }
    impl crate::services::DomainRuntime for TerminalAdapter {
        fn name(&self) -> &'static str {
            "terminal"
        }
        fn shutdown(&self) -> BoxFuture<'_, crate::Result<()>> {
            Box::pin(async {
                self.manager
                    .shutdown()
                    .await
                    .map_err(|e| crate::Error::Service(e.to_string()))
            })
        }
    }
    pub fn output_stream(
        mut stream: AttachStream,
        window: Arc<TerminalOutputWindow>,
    ) -> BoxStream<'static, TerminalResult<TerminalAttachStreamEvent>> {
        async_stream::stream! {
            loop {
                window.capacity().await;
                match stream.recv().await {
                    Ok(Some(event)) => {
                        window.sent(std::slice::from_ref(&event));
                        yield Ok(event);
                    }
                    Ok(None) => return,
                    Err(error) => { yield Err(failure(error, "", "")); return; }
                }
            }
        }
        .boxed()
    }
    fn failure(error: ServiceError, thread: &str, terminal: &str) -> Failure<WsTerminalOpenError> {
        use serde_json::json;
        let cause = error.to_string();
        let value = match error {
            ServiceError::CwdNotFound { cwd } => {
                json!({"_tag":"TerminalCwdNotFoundError","cwd":cwd})
            }
            ServiceError::CwdNotDirectory { cwd } => {
                json!({"_tag":"TerminalCwdNotDirectoryError","cwd":cwd})
            }
            ServiceError::CwdStat { cwd, .. } => {
                json!({"_tag":"TerminalCwdStatError","cwd":cwd,"cause":cause})
            }
            ServiceError::SessionLookup {
                thread_id,
                terminal_id,
            } => {
                json!({"_tag":"TerminalSessionLookupError","threadId":thread_id,"terminalId":terminal_id})
            }
            ServiceError::NotRunning {
                thread_id,
                terminal_id,
            } => {
                json!({"_tag":"TerminalNotRunningError","threadId":thread_id,"terminalId":terminal_id})
            }
            ServiceError::ProviderInstanceNotFound(id) => {
                json!({"_tag":"TerminalProviderInstanceNotFoundError","providerInstanceId":id})
            }
            ServiceError::ProviderEnvironment(id, _) => {
                json!({"_tag":"TerminalProviderEnvironmentError","providerInstanceId":id,"cause":cause})
            }
            ServiceError::History {
                operation,
                thread_id,
                terminal_id,
                ..
            } => {
                json!({"_tag":"TerminalHistoryError","operation":operation,"threadId":thread_id,"terminalId":terminal_id,"cause":cause})
            }
            ServiceError::Pty {
                operation: "write",
                pid: Some(pid),
                ..
            } => {
                json!({"_tag":"TerminalWriteError","threadId":thread,"terminalId":terminal,"terminalPid":pid,"cause":cause})
            }
            ServiceError::Pty {
                operation: "resize",
                pid: Some(pid),
                cols: Some(cols),
                rows: Some(rows),
                ..
            } => {
                json!({"_tag":"TerminalResizeError","threadId":thread,"terminalId":terminal,"terminalPid":pid,"cols":cols,"rows":rows,"cause":cause})
            }
            _ => return Failure::Defect(ServerDefect::Internal { message: cause }),
        };
        match serde_json::from_value(value) {
            Ok(error) => Failure::Typed(error),
            Err(error) => Failure::Defect(ServerDefect::Internal {
                message: format!("terminal error encoding failed: {error}"),
            }),
        }
    }
}
#[cfg(feature = "terminal")]
pub use enabled::*;
