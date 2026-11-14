use super::{
    provider_event_ingestor_test::Harness,
    run_finalization_service_test::{service, target},
};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use t3_orchestration::{
    api::*,
    effects::*,
    error::{Error, Result},
    ports::runs_ingestion::run_finalization_service::RunFinalizationService,
    runtime::worker::{EffectWorker as Worker, WorkerOptions},
    stores::outbox,
};
use tokio_util::sync::CancellationToken;

struct Executor {
    service: RunFinalizationService,
    calls: Arc<Mutex<Vec<&'static str>>>,
}
#[async_trait]
impl EffectExecutor for Executor {
    async fn execute(&self, effect: EffectRow, cancellation: CancellationToken) -> Result<()> {
        match effect.request {
            EffectRequest::Capture { .. } => self
                .service
                .finalize(target(), cancellation)
                .await
                .map_err(|error| Error::Service {
                    operation: "finalize",
                    detail: error.to_string(),
                }),
            EffectRequest::Start { .. } => {
                self.calls.lock().expect("test value").push("start");
                Ok(())
            }
            _ => Err(Error::Unsupported("unused test effect")),
        }
    }
    async fn failed(&self, _effect: EffectRow, _error: &Error, _will_retry: bool) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn capture_failure_holds_the_next_run_but_display_refresh_failure_releases_it() -> Result<()>
{
    for fail in [true, false] {
        let h = Harness::new().await?;
        let (service, calls) = service(&h, fail).await?;
        let effects = vec![
            PendingEffect {
                id: "capture".into(),
                command_id: CommandId("capture".into()),
                thread_id: ThreadId("thread:test".into()),
                request: EffectRequest::Capture {
                    run_id: RunId("run:test".into()),
                    scope_id: CheckpointScopeId("scope:test".into()),
                },
                available_at: None,
            },
            PendingEffect {
                id: "start".into(),
                command_id: CommandId("start".into()),
                thread_id: ThreadId("thread:test".into()),
                request: EffectRequest::Start {
                    run_id: RunId("run:next".into()),
                },
                available_at: None,
            },
        ];
        h.sink
            .commit(CommitRequest {
                receipt: None,
                guard: CommitGuard::None,
                decision: Decision {
                    effects,
                    ..Default::default()
                },
            })
            .await?;
        let worker = Worker::new(
            h.database.stores.clone(),
            Arc::new(Executor {
                service,
                calls: calls.clone(),
            }),
            h.clock.clone(),
            h.sink.signals(),
            WorkerOptions::default(),
        );
        assert_eq!(
            worker.drain(if fail { 1 } else { 2 }).await?,
            if fail { 1 } else { 2 }
        );
        let (capture, start) = h
            .database
            .stores
            .read(|db| {
                Ok((
                    outbox::get(db, "capture")?.unwrap(),
                    outbox::get(db, "start")?.unwrap(),
                ))
            })
            .await?;
        assert_eq!(
            capture.status,
            if fail {
                EffectStatus::Pending
            } else {
                EffectStatus::Succeeded
            }
        );
        assert_eq!(
            start.status,
            if fail {
                EffectStatus::Pending
            } else {
                EffectStatus::Succeeded
            }
        );
        assert_eq!(capture.attempt_count, 1);
        assert_eq!(start.attempt_count, if fail { 0 } else { 1 });
        if fail {
            assert!(!worker.run_once(ClaimOptions::default()).await?);
            assert_eq!(*calls.lock().unwrap(), vec!["capture"]);
            assert!(
                capture
                    .last_error
                    .unwrap()
                    .contains("checkpoint storage unavailable")
            );
        } else {
            assert_eq!(*calls.lock().unwrap(), vec!["capture", "refresh", "start"]);
            assert!(capture.last_error.is_none());
        }
        worker.shutdown().await?;
    }
    Ok(())
}
