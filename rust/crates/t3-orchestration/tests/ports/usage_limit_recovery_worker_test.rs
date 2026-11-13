use async_trait::async_trait;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use t3_contracts::{base_schemas::*, orchestration_v2::*, wire::WireDateTime};
use t3_orchestration::{
    api::*,
    error::Error,
    stores::{StoreDatabase, receipts::CommandReceipt},
    testing::{ManualClock, TestDatabase},
};
use t3_orchestration::{error::Result, ports::sessions_recovery::usage_limit_recovery_worker::*};

const NOW: i64 = 1_792_411_200_000;
fn candidate(reset: &str, failed: &str) -> LimitRecoveryCandidate {
    LimitRecoveryCandidate {
        id: ThreadId("thread:limited".into()),
        status: OrchestrationV2ShellThreadStatus::Failed,
        last_error_class: Some(OrchestrationV2ProviderFailureClass::UsageLimit),
        latest_run_id: Some(RunId("run:limited".into())),
        usage_limit_reset_at: Some(reset.into()),
        latest_run_completed_at: Some(WireDateTime(failed.into())),
        updated_at: WireDateTime(failed.into()),
        archived_at: None,
        settled_override: None,
        pending_runtime_request: None,
        limit_recovery: None,
        snoozed_until: None,
    }
}
#[test]
fn arming_honors_exact_overdue_window_and_rejects_old_reset() -> Result<()> {
    for (reset, failed, expected) in [
        (
            "2026-10-15T12:00:00.000Z",
            "2026-10-14T12:00:00.000Z",
            false,
        ),
        ("2026-10-19T11:00:00.000Z", "2026-10-19T06:00:00.000Z", true),
        ("2026-10-18T12:00:00.000Z", "2026-10-18T06:00:00.000Z", true),
        (
            "2026-10-18T11:59:59.999Z",
            "2026-10-18T06:00:00.000Z",
            false,
        ),
    ] {
        let candidate = candidate(reset, failed);
        for snooze in [false, true] {
            assert_eq!(
                limit_recovery_command(&candidate, true, NOW, snooze)?.is_some(),
                expected,
                "{reset}"
            );
        }
    }
    Ok(())
}
#[test]
fn arming_and_resuming_have_stable_ids_and_preserve_recovery_generation() -> Result<()> {
    let mut candidate = candidate("2026-10-19T13:00:00.000Z", "2026-10-19T06:00:00.000Z");
    assert!(limit_recovery_command(&candidate, false, NOW, false)?.is_none());
    let arm = serde_json::to_value(
        limit_recovery_command(&candidate, true, NOW, true)?.expect("test value"),
    )?;
    assert_eq!(arm["type"], "thread.metadata.update");
    assert_eq!(arm["limitRecovery"]["snooze"], true);
    candidate.limit_recovery = Some(serde_json::from_value(
        json!({"runId":"run:limited","resetAt":"2026-10-19T13:00:00.000Z","autoResume":true,"requestId":"request:2"}),
    )?);
    assert!(limit_recovery_command(&candidate, true, NOW, false)?.is_none());
    let resumed = serde_json::to_value(
        limit_recovery_command(&candidate, true, NOW + 3_600_000, false)?.expect("test value"),
    )?;
    assert_eq!(
        resumed["commandId"],
        "limit-resume:thread:limited:run:limited:1792414800000:request:2"
    );
    assert_eq!(resumed["messageId"], resumed["commandId"]);
    assert_eq!(resumed["usageLimitRecoveryRequestId"], "request:2");
    candidate.snoozed_until = Some(WireDateTime("2026-10-19T14:00:00.000Z".into()));
    assert!(limit_recovery_command(&candidate, true, NOW + 3_600_000, false)?.is_none());
    Ok(())
}
#[test]
fn invalid_or_already_expired_reset_cannot_start_a_retry_loop() -> Result<()> {
    for reset in [
        "invalid",
        "2026-10-19T05:00:00.000Z",
        "2026-10-19T06:00:00.000Z",
    ] {
        assert!(
            limit_recovery_command(
                &candidate(reset, "2026-10-19T06:00:00.000Z"),
                true,
                NOW,
                false
            )?
            .is_none()
        );
    }
    Ok(())
}
#[test]
fn idle_sweeps_only_wake_for_due_time_relevant_events_or_settings() -> Result<()> {
    let wake = UsageLimitRecoveryWake::default();
    let version = wake.begin_sweep(0).expect("test value");
    wake.record_idle(version, Some(60_000));
    for tick in 1..12 {
        assert_eq!(wake.begin_sweep(tick * 5_000), None);
    }
    assert!(wake.begin_sweep(60_000).is_some());
    wake.record_idle(version, None);
    let event = t3_orchestration::testing::detached_event("event", "thread")?;
    wake.on_event(&event);
    assert!(wake.begin_sweep(95_000).is_some());
    wake.record_idle(version, None);
    assert!(
        wake.begin_sweep(95_000).is_some(),
        "an invalidation during a read cannot be overwritten by its result"
    );
    let version = wake.begin_sweep(95_000).expect("test value");
    wake.record_idle(version, None);
    assert!(wake.begin_sweep(100_000).is_none());
    wake.invalidate();
    assert!(wake.begin_sweep(100_000).is_some());
    Ok(())
}

struct EmptyState {
    stores: StoreDatabase,
    queries: AtomicUsize,
}
#[async_trait]
impl LimitRecoveryState for EmptyState {
    async fn preferences(&self) -> Result<LimitRecoveryPreferences> {
        Ok(LimitRecoveryPreferences {
            auto_resume: true,
            snooze: false,
        })
    }
    async fn candidates(
        &self,
        _: IsoDateTime,
        _: LimitRecoveryPreferences,
    ) -> Result<Vec<LimitRecoveryCandidate>> {
        self.stores
            .read(|db| {
                let count: i64 = db.query_row(
                    "SELECT COUNT(*) FROM orchestration_v2_projection_threads",
                    [],
                    |r| r.get(0),
                )?;
                assert_eq!(count, 0);
                Ok(())
            })
            .await?;
        self.queries.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }
    async fn wake_at(&self, now: IsoDateTime) -> Result<Option<i64>> {
        Ok((now.0.as_str() < "1970-01-01T00:01:00.000Z").then_some(60_000))
    }
    async fn auto_arm(&self, _: CommandThreadMetadataUpdate) -> Result<()> {
        Err(Error::Unsupported("no candidates"))
    }
}
struct NoDispatch;

struct Armer(t3_orchestration::testing::Script<CommandThreadMetadataUpdate, ()>);

struct PausedArmerCommitter {
    sink: Arc<t3_orchestration::runtime::commit::EventSink>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait]
impl EventCommitter for PausedArmerCommitter {
    async fn commit(&self, input: CommitRequest) -> Result<CommitResult> {
        self.sink.commit(input).await
    }
    async fn commit_prepared(
        &self,
        input: CommitRequest,
        options: CommitOptions,
        prepare: CommitPreparation,
    ) -> Result<CommitResult> {
        self.entered.notify_one();
        self.release.notified().await;
        self.sink.commit_prepared(input, options, prepare).await
    }
    async fn commit_project(
        &self,
        event: t3_contracts::application_event::ApplicationProjectEvent,
        receipt: CommandReceipt,
    ) -> Result<CommandReceipt> {
        self.sink.commit_project(event, receipt).await
    }
    async fn events_by_command(&self, id: CommandId) -> Result<Vec<OrchestrationV2StoredEvent>> {
        self.sink.events_by_command(id).await
    }
    fn subscribe(
        &self,
    ) -> tokio::sync::broadcast::Receiver<t3_contracts::application_event::ApplicationStoredEvent>
    {
        self.sink.subscribe()
    }
    async fn latest_sequence(&self, id: Option<ThreadId>) -> Result<u64> {
        self.sink.latest_sequence(id).await
    }
}

#[tokio::test]
async fn arming_rechecks_an_explicit_choice_committed_while_waiting_for_the_writer() -> Result<()> {
    use super::provider_runtime_recovery_service_test::{
        RECOVERED_AT, item, projection, seed, sink,
    };
    use t3_orchestration::{
        decision::engine::PureDecisionEngine, projections::store::ProjectionStore,
        testing::SequenceIds,
    };
    let database = TestDatabase::memory()?;
    let sink = Arc::new(sink(database.stores.clone()));
    let mut projection = projection()?;
    projection.runs[0].status = OrchestrationV2RunStatus::Failed;
    projection.runs[0].completed_at = Some(WireDateTime(RECOVERED_AT.into()));
    let reset = "2026-09-04T00:01:00.000Z";
    let mut failure = item("limit:failure", "error");
    failure["status"] = json!("failed");
    failure["failure"] = json!({"class":"usage_limit","message":"limit","code":null,"retryable":false,"resetAt":reset});
    projection.turn_items = vec![serde_json::from_value(failure)?];
    seed(&sink, &projection).await?;
    let committer = Arc::new(PausedArmerCommitter {
        sink: sink.clone(),
        entered: Default::default(),
        release: Default::default(),
    });
    let armer = GuardedLimitRecoveryArmer::new(
        committer.clone(),
        Arc::new(PureDecisionEngine),
        Arc::new(ManualClock::new(RECOVERED_AT)),
        Arc::new(SequenceIds::new("race-arm")),
    );
    let command = serde_json::from_value(
        json!({"commandId":"race:arm","threadId":"thread:test","limitRecovery":{"runId":"run:test","resetAt":reset,"autoResume":true,"snooze":true}}),
    )?;
    let arm = tokio::spawn(async move { armer.auto_arm(command).await });
    committer.entered.notified().await;
    projection.thread.limit_recovery = Some(Some(serde_json::from_value(
        json!({"runId":"run:test","resetAt":reset,"autoResume":false,"snooze":false,"requestId":"explicit:choice"}),
    )?));
    sink.commit(CommitRequest { receipt: None, guard: CommitGuard::None, decision: Decision {
        events: vec![serde_json::from_value(json!({"id":"choice:committed","threadId":"thread:test","type":"thread.metadata-updated","occurredAt":RECOVERED_AT,"payload":t3_contracts::stored::to_value(&projection.thread)?}))?], ..Default::default()
    }}).await?;
    let chosen_at = sink.latest_sequence(None).await?;
    committer.release.notify_one();
    arm.await.unwrap()?;
    assert_eq!(sink.latest_sequence(None).await?, chosen_at);
    let thread = ProjectionStore::new(database.stores)
        .thread(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    let recovery = thread.limit_recovery.flatten().unwrap();
    assert!(!recovery.auto_resume);
    assert_eq!(
        recovery.request_id.flatten(),
        Some(CommandId("explicit:choice".into()))
    );
    Ok(())
}

#[tokio::test]
async fn guarded_arming_preserves_explicit_choices_and_disarms_a_stop_that_precedes_limit_failure()
-> Result<()> {
    use super::provider_runtime_recovery_service_test::{
        RECOVERED_AT, item, projection, seed, sink,
    };
    use t3_orchestration::{
        api::*, decision::engine::PureDecisionEngine, projections::store::ProjectionStore,
        testing::SequenceIds,
    };
    for state in ["unarmed", "explicit", "stopped", "newer-run"] {
        let database = TestDatabase::memory()?;
        let sink = Arc::new(sink(database.stores.clone()));
        let mut projection = projection()?;
        projection.runs[0].status = OrchestrationV2RunStatus::Failed;
        projection.runs[0].completed_at = Some(WireDateTime(RECOVERED_AT.into()));
        let reset = "2026-09-04T00:01:00.000Z";
        let mut failure = item("limit:failure", "error");
        failure["status"] = json!("failed");
        failure["failure"] = json!({"class":"usage_limit","message":"limit","code":null,"retryable":false,"resetAt":reset});
        projection.turn_items = vec![serde_json::from_value(failure)?];
        if state == "explicit" {
            projection.thread.limit_recovery = Some(Some(serde_json::from_value(
                json!({"runId":"run:test","resetAt":reset,"autoResume":false,"snooze":false,"requestId":"explicit:choice"}),
            )?));
        } else if state == "stopped" {
            let mut stop = item("stop:request", "run_interrupt_request");
            stop["message"] = json!("Stop");
            stop["status"] = json!("completed");
            stop["ordinal"] = json!(2);
            projection.turn_items.push(serde_json::from_value(stop)?);
        } else if state == "newer-run" {
            let mut next = projection.runs[0].clone();
            next.id = serde_json::from_value(json!("run:newer"))?;
            next.ordinal = 2;
            projection.runs.push(next);
        }
        seed(&sink, &projection).await?;
        let before = sink.latest_sequence(None).await?;
        let armer = GuardedLimitRecoveryArmer::new(
            sink.clone(),
            Arc::new(PureDecisionEngine),
            Arc::new(ManualClock::new(RECOVERED_AT)),
            Arc::new(SequenceIds::new("arm")),
        );
        let command = serde_json::from_value(
            json!({"commandId":format!("arm:{state}"),"threadId":"thread:test","limitRecovery":{"runId":"run:test","resetAt":reset,"autoResume":true,"snooze":true}}),
        )?;
        let result = armer.auto_arm(command).await;
        if state == "newer-run" {
            assert!(matches!(result, Err(Error::Rejected { .. })));
            assert_eq!(sink.latest_sequence(None).await?, before);
            continue;
        }
        result?;
        let thread = ProjectionStore::new(database.stores.clone())
            .thread(ThreadId("thread:test".into()))
            .await?
            .unwrap();
        let recovery = thread.limit_recovery.flatten().expect("recovery metadata");
        assert_eq!(recovery.auto_resume, state == "unarmed");
        assert_eq!(recovery.snooze, Some(Some(state == "unarmed")));
        assert_eq!(
            recovery.request_id.flatten().unwrap().0,
            if state == "explicit" {
                "explicit:choice".into()
            } else {
                format!("arm:{state}")
            }
        );
        if state == "explicit" {
            assert_eq!(sink.latest_sequence(None).await?, before);
        } else {
            assert!(sink.latest_sequence(None).await? > before);
            assert_eq!(
                thread.snoozed_until.flatten(),
                (state == "unarmed").then(|| WireDateTime(reset.into()))
            );
            database
                .stores
                .read(move |db| {
                    let receipt = t3_orchestration::stores::receipts::get(
                        db,
                        &CommandId(format!("arm:{state}")),
                    )?
                    .unwrap();
                    assert_eq!(
                        receipt.status,
                        t3_orchestration::stores::receipts::ReceiptStatus::Accepted
                    );
                    Ok(())
                })
                .await?;
        }
    }
    Ok(())
}

#[async_trait]
impl LimitRecoveryArmer for Armer {
    async fn auto_arm(&self, command: CommandThreadMetadataUpdate) -> Result<()> {
        self.0.call(command)
    }
}

#[tokio::test]
async fn projection_state_reads_settings_and_failed_usage_rows_then_uses_guarded_arming()
-> Result<()> {
    use super::provider_runtime_recovery_service_test::{
        RECOVERED_AT, item, projection, seed, sink,
    };
    use t3_orchestration::{
        projections::store::ProjectionStore,
        testing::{FakeSettingsService, Script},
    };
    let database = TestDatabase::memory()?;
    let mut projection = projection()?;
    projection.runs[0].status = OrchestrationV2RunStatus::Failed;
    projection.runs[0].completed_at = Some(WireDateTime(RECOVERED_AT.into()));
    projection.attempts.clear();
    projection.nodes.clear();
    projection.provider_sessions[0].last_error = Some("Usage limit hit".into());
    projection.provider_sessions[0].status = ProviderSessionStatus::Error;
    let mut failure = item("limit:failure", "error");
    failure["status"] = json!("failed");
    failure["updatedAt"] = json!(RECOVERED_AT);
    failure["completedAt"] = json!(RECOVERED_AT);
    failure["failure"] = json!({"class":"usage_limit","message":"Usage limit hit","code":null,"retryable":false,"resetAt":"2026-09-04T00:01:00.000Z"});
    projection.turn_items = vec![serde_json::from_value(failure)?];
    seed(&sink(database.stores.clone()), &projection).await?;
    let settings = Arc::new(FakeSettingsService::default());
    let preferences = t3_contracts::settings::ServerSettings {
        auto_resume_limited_threads: true,
        snooze_limited_threads: true,
        ..Default::default()
    };
    settings.get.push(Ok(preferences));
    let armer = Arc::new(Armer(Script::default()));
    armer.0.push(Ok(()));
    let state = Arc::new(ProjectionLimitRecoveryState::new(
        ProjectionStore::new(database.stores.clone()),
        settings,
        armer.clone(),
    ));
    let worker = UsageLimitRecoveryWorker::new(
        state.clone(),
        Arc::new(NoDispatch),
        Arc::new(ManualClock::new(RECOVERED_AT)),
    );
    let report = worker.sweep().await?;
    assert_eq!(report.acted, 1);
    assert!(report.failures.is_empty());
    let command = t3_contracts::stored::to_value(&armer.0.next_call().await.expect("guarded arm"))?;
    assert_eq!(command["threadId"], "thread:test");
    assert_eq!(
        command["limitRecovery"],
        json!({"runId":"run:test","resetAt":"2026-09-04T00:01:00.000Z","autoResume":true,"snooze":true})
    );
    assert_eq!(
        state.wake_at(WireDateTime(RECOVERED_AT.into())).await?,
        None
    );
    let recovery = serde_json::to_string(&command["limitRecovery"])?;
    database.stores.transaction(move|tx| {
        tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,'$.limitRecovery',json(?1)) WHERE thread_id='thread:test'",[recovery])?; Ok(())
    }).await?;
    assert_eq!(
        state.wake_at(WireDateTime(RECOVERED_AT.into())).await?,
        Some(1_788_480_060_000)
    );
    Ok(())
}
#[async_trait]
impl CommandDispatcher for NoDispatch {
    async fn dispatch(&self, _: OrchestrationV2ServerCommand) -> Result<CommandReceipt> {
        Err(Error::Unsupported("no candidates"))
    }
}
#[tokio::test]
async fn idle_worker_skips_database_until_due_time_or_relevant_invalidation() -> Result<()> {
    let database = TestDatabase::memory()?;
    let clock = Arc::new(ManualClock::new("1970-01-01T00:00:00.000Z"));
    let state = Arc::new(EmptyState {
        stores: database.stores,
        queries: AtomicUsize::new(0),
    });
    let worker = UsageLimitRecoveryWorker::new(state.clone(), Arc::new(NoDispatch), clock.clone());
    worker.sweep().await?;
    for seconds in (5..60).step_by(5) {
        clock.set(&format!("1970-01-01T00:00:{seconds:02}.000Z"));
        worker.sweep().await?;
    }
    assert_eq!(state.queries.load(Ordering::SeqCst), 1);
    clock.set("1970-01-01T00:01:00.000Z");
    worker.sweep().await?;
    assert_eq!(state.queries.load(Ordering::SeqCst), 2);
    let mut item =
        super::provider_runtime_recovery_service_test::item("streaming", "assistant_message");
    item["messageId"] = json!("message");
    item["text"] = json!("chunk");
    item["streaming"] = json!(true);
    let event = serde_json::from_value(
        json!({"id":"event","threadId":"thread:test","type":"turn-item.updated","occurredAt":"1970-01-01T00:01:30.000Z","payload":item}),
    )?;
    worker.wake.on_event(&event);
    clock.set("1970-01-01T00:01:30.000Z");
    worker.sweep().await?;
    assert_eq!(state.queries.load(Ordering::SeqCst), 2);
    worker
        .wake
        .on_event(&t3_orchestration::testing::detached_event(
            "event:relevant",
            "thread:test",
        )?);
    clock.set("1970-01-01T00:01:35.000Z");
    worker.sweep().await?;
    assert_eq!(state.queries.load(Ordering::SeqCst), 3);
    Ok(())
}
