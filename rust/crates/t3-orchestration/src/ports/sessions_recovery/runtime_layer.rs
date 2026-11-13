use super::provider_runtime_recovery_service::ProviderRuntimeRecoveryService;
use crate::{
    api::*,
    error::{Error, Result},
    stores::receipts::CommandReceipt,
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Cold,
    Ready,
    Stopping,
    Stopped,
}

#[derive(Default)]
struct ShutdownProgress {
    prepared: bool,
    worker: bool,
    execution: bool,
    sessions: bool,
    reconciled: bool,
}

pub struct RuntimeLayer {
    dispatcher: Arc<dyn CommandDispatcher>,
    recovery: Arc<ProviderRuntimeRecoveryService>,
    worker: Arc<dyn EffectWorker>,
    sessions: Arc<dyn SessionDirectory>,
    execution:
        Option<Arc<crate::ports::runs_ingestion::run_execution_service::RunExecutionService>>,
    phase: RwLock<Phase>,
    lifecycle: Mutex<ShutdownProgress>,
}

impl RuntimeLayer {
    pub fn new(
        dispatcher: Arc<dyn CommandDispatcher>,
        recovery: Arc<ProviderRuntimeRecoveryService>,
        worker: Arc<dyn EffectWorker>,
        sessions: Arc<dyn SessionDirectory>,
    ) -> Self {
        Self {
            dispatcher,
            recovery,
            worker,
            sessions,
            execution: None,
            phase: RwLock::new(Phase::Cold),
            lifecycle: Mutex::new(ShutdownProgress::default()),
        }
    }
    pub fn with_run_execution(
        mut self,
        execution: Arc<crate::ports::runs_ingestion::run_execution_service::RunExecutionService>,
    ) -> Self {
        self.execution = Some(execution);
        self
    }
    pub async fn start(&self) -> Result<RecoverySummary> {
        let _lifecycle = self.lifecycle.lock().await;
        let phase = self.phase.write().await;
        if matches!(*phase, Phase::Stopping | Phase::Stopped) {
            return Err(Error::Cancelled);
        }
        if *phase == Phase::Ready {
            return Ok(RecoverySummary::default());
        }
        drop(phase);
        let summary = self.recovery.recover().await?;
        *self.phase.write().await = Phase::Ready;
        Ok(summary)
    }
    pub async fn close_admission(&self) {
        let _lifecycle = self.lifecycle.lock().await;
        let mut phase = self.phase.write().await;
        if *phase != Phase::Stopped {
            *phase = Phase::Stopping;
        }
    }
    async fn stop_worker(&self, progress: &mut ShutdownProgress) -> Result<()> {
        // Drain admitted dispatches, then let cleanup's callbacks observe closed admission.
        let mut phase = self.phase.write().await;
        if *phase == Phase::Stopped {
            return Ok(());
        }
        *phase = Phase::Stopping;
        drop(phase);
        let mut error = None;
        if !progress.prepared {
            match self.recovery.prepare_for_shutdown().await {
                Ok(()) => progress.prepared = true,
                Err(next) => {
                    error.get_or_insert(next);
                }
            }
        }
        if !progress.worker {
            match self.worker.shutdown().await {
                Ok(()) => progress.worker = true,
                Err(next) => {
                    error.get_or_insert(next);
                }
            }
        }
        error.map_or(Ok(()), Err)
    }

    pub async fn shutdown_worker(&self) -> Result<()> {
        let mut progress = self.lifecycle.lock().await;
        self.stop_worker(&mut progress).await
    }

    pub async fn shutdown_sessions(&self) -> Result<()> {
        let mut progress = self.lifecycle.lock().await;
        let mut error = self.stop_worker(&mut progress).await.err();
        if !progress.execution {
            let result = match &self.execution {
                Some(execution) => execution.shutdown().await,
                None => Ok(()),
            };
            match result {
                Ok(()) => progress.execution = true,
                Err(next) => {
                    error.get_or_insert(next);
                }
            }
        }
        if !progress.sessions {
            match self.sessions.shutdown().await {
                Ok(()) => progress.sessions = true,
                Err(next) => {
                    error.get_or_insert(next);
                }
            }
        }
        if !progress.reconciled {
            match self.recovery.reconcile_shutdown().await {
                Ok(_) => progress.reconciled = true,
                Err(next) => {
                    error.get_or_insert(next);
                }
            }
        }
        if error.is_none() {
            *self.phase.write().await = Phase::Stopped;
        }
        error.map_or(Ok(()), Err)
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown_sessions().await
    }
}

#[async_trait]
impl CommandDispatcher for RuntimeLayer {
    fn command_lanes(
        &self,
    ) -> Result<Arc<crate::ports::decisions::thread_command_executor::KeyedLanes>> {
        self.dispatcher.command_lanes()
    }
    async fn dispatch(&self, command: OrchestrationV2ServerCommand) -> Result<CommandReceipt> {
        let phase = self.phase.read().await;
        if *phase != Phase::Ready {
            return Err(Error::Service {
                operation: "runtime admission",
                detail: "provider recovery has not completed or the runtime is shutting down"
                    .into(),
            });
        }
        self.dispatcher.dispatch(command).await
    }
}
