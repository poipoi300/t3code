use super::provider_event_ingestor_test::{decode, fixture};
use serde_json::{Value, json};
use t3_orchestration::{
    api::*,
    ports::runs_ingestion::{restart_background_note::*, restart_continuation::*},
    testing::TEST_TIME,
};
fn projection() -> Value {
    let mut p = fixture("OrchestrationV2ThreadProjection");
    let mut thread = fixture("OrchestrationV2AppThread");
    let mut run = fixture("OrchestrationV2Run");
    let mut provider = fixture("OrchestrationV2ProviderThread");
    let mut session = fixture("OrchestrationV2ProviderSession");
    let mut turn = fixture("OrchestrationV2ProviderTurn");
    thread["id"] = "thread:test".into();
    thread["providerInstanceId"] = "codex".into();
    thread["archivedAt"] = Value::Null;
    thread["deletedAt"] = Value::Null;
    thread["settledOverride"] = Value::Null;
    run["id"] = "run:test".into();
    run["activeAttemptId"] = "attempt:test".into();
    run["providerInstanceId"] = "codex".into();
    run["providerThreadId"] = "pt:test".into();
    run["status"] = "running".into();
    run["startedAt"] = TEST_TIME.into();
    run["completedAt"] = Value::Null;
    provider["id"] = "pt:test".into();
    provider["appThreadId"] = "thread:test".into();
    provider["ownerNodeId"] = Value::Null;
    provider["driver"] = "codex".into();
    provider["providerInstanceId"] = "codex".into();
    provider["providerSessionId"] = "session:test".into();
    provider["nativeThreadRef"] =
        json!({"driver":"codex","nativeId":"native:test","strength":"strong"});
    provider["status"] = "active".into();
    provider["pendingBackgroundTasks"] = json!([]);
    session["id"] = "session:test".into();
    session["driver"] = "codex".into();
    session["providerInstanceId"] = "codex".into();
    session["status"] = "running".into();
    turn["id"] = "turn:test".into();
    turn["providerThreadId"] = "pt:test".into();
    turn["runAttemptId"] = "attempt:test".into();
    turn["status"] = "running".into();
    p["thread"] = thread;
    p["runs"] = json!([run]);
    p["providerThreads"] = json!([provider]);
    p["providerSessions"] = json!([session]);
    p["providerTurns"] = json!([turn]);
    for key in ["runtimeRequests", "turnItems", "attempts"] {
        p[key] = json!([]);
    }
    p
}
fn eligible(p: Value) -> bool {
    restart_continuation_run(
        &decode(p),
        &t3_contracts::wire::WireDateTime(TEST_TIME.into()),
    )
    .unwrap()
    .is_some()
}
#[test]
fn recovery_requires_matching_saved_native_state_and_a_live_root_turn() {
    let p = projection();
    assert!(eligible(p.clone()));
    for (path, value) in [
        (vec!["thread", "archivedAt"], json!(TEST_TIME)),
        (vec!["thread", "deletedAt"], json!(TEST_TIME)),
        (vec!["thread", "providerInstanceId"], json!("other")),
        (vec!["providerThreads", "0", "nativeThreadRef"], Value::Null),
        (
            vec!["providerSessions", "0", "providerInstanceId"],
            json!("other"),
        ),
        (vec!["providerTurns"], json!([])),
    ] {
        let mut candidate = p.clone();
        let mut target = &mut candidate;
        for segment in path {
            target = if let Ok(index) = segment.parse::<usize>() {
                &mut target[index]
            } else {
                &mut target[segment]
            };
        }
        *target = value;
        assert!(!eligible(candidate));
    }
    for status in [
        "queued",
        "preparing",
        "starting",
        "waiting",
        "completed",
        "cancelled",
        "failed",
        "interrupted",
    ] {
        let mut candidate = p.clone();
        candidate["runs"][0]["status"] = status.into();
        assert!(!eligible(candidate), "{status}");
    }
    for status in [
        "starting", "ready", "running", "waiting", "stopped", "error",
    ] {
        let mut candidate = p.clone();
        candidate["providerSessions"][0]["status"] = status.into();
        assert_eq!(eligible(candidate), !matches!(status, "stopped" | "error"));
    }
    let mut prepared = p;
    prepared["runs"][0]["status"] = "starting".into();
    prepared["runs"][0]["startedAt"] = Value::Null;
    prepared["runs"][0]["restartContinuationOfRunId"] = "run:previous".into();
    prepared["providerSessions"][0]["status"] = "stopped".into();
    assert!(eligible(prepared));
}
#[test]
fn blocking_questions_and_terminal_provider_turns_cannot_restart_as_unfinished_work() {
    for status in ["completed", "failed", "interrupted"] {
        let mut p = projection();
        p["providerTurns"][0]["status"] = status.into();
        assert!(!eligible(p));
    }
    for (kind, status, capability, expected) in [
        ("user_input", "pending", "live", false),
        ("user_input", "pending", "message", true),
        ("dynamic_tool_call", "pending", "live", true),
        ("auth_refresh", "pending", "live", true),
        ("user_input", "cancelled", "not_resumable", false),
        ("user_input", "expired", "not_resumable", false),
        ("user_input", "cancelled", "live", true),
    ] {
        let mut p = projection();
        let mut request = fixture("OrchestrationV2RuntimeRequest");
        request["kind"] = kind.into();
        request["status"] = status.into();
        request["responseCapability"] =
            json!({"type":capability,"providerSessionId":"session:test","reason":"closed"});
        request["providerTurnId"] = "turn:test".into();
        p["runtimeRequests"] = json!([request]);
        assert_eq!(eligible(p), expected, "{kind}/{status}/{capability}");
    }
}
#[test]
fn continuation_start_failures_back_off_twice_and_permanent_failures_do_not_retry() {
    let p = projection();
    let mut undefined: OrchestrationV2Run = decode(p["runs"][0].clone());
    undefined.started_at = None;
    undefined.status = OrchestrationV2RunStatus::Failed;
    undefined.restart_continuation_of_run_id = Some(None);
    assert!(never_started(&undefined));
    assert!(!continuation_failed_before_prompt(&undefined, &[]));
    let mut first = p["runs"][0].clone();
    first["status"] = "failed".into();
    first["restartContinuationOfRunId"] = "source".into();
    let first: OrchestrationV2Run = decode(first);
    assert_eq!(
        continuation_start_retry_delay_ms(&first, std::slice::from_ref(&first), &[], &[]).unwrap(),
        Some(60_000)
    );
    let mut second = t3_contracts::stored::to_value(&first).unwrap();
    second["id"] = "second".into();
    second["restartContinuationOfRunId"] = "run:test".into();
    let second: OrchestrationV2Run = decode(second);
    assert_eq!(
        continuation_start_retry_delay_ms(&second, &[first.clone(), second.clone()], &[], &[])
            .unwrap(),
        Some(180_000)
    );
    let mut third = t3_contracts::stored::to_value(&second).unwrap();
    third["id"] = "third".into();
    third["restartContinuationOfRunId"] = "second".into();
    let third: OrchestrationV2Run = decode(third);
    assert_eq!(
        continuation_start_retry_delay_ms(&third, &[first, second, third.clone()], &[], &[])
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn continuation_dispatch_is_durable_idempotent_and_respects_opt_out_stop_and_maintenance()
-> t3_orchestration::error::Result<()> {
    use super::remote_delegated_task_test::live_dispatcher;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use t3_orchestration::{
        composition::StaticSettings,
        effects::TitleKind,
        error::{Error, Result},
        ports::thread_project::thread_management_service::{
            NoLegacyTranscript, ThreadManagementService, ThreadManagementServices,
        },
    };
    struct Operations(AtomicUsize);
    #[async_trait::async_trait]
    impl ThreadOperations for Operations {
        async fn start(&self, _thread: ThreadId, _run: RunId) -> Result<()> {
            Err(Error::Unsupported("unused start"))
        }
        async fn finalize(
            &self,
            _thread: ThreadId,
            _run: RunId,
            _scope: CheckpointScopeId,
        ) -> Result<()> {
            Err(Error::Unsupported("unused finalize"))
        }
        async fn continue_run(&self, _thread: ThreadId, _run: RunId) -> Result<()> {
            Err(Error::Unsupported("unused continuation host"))
        }
        async fn stop_delegated(&self, _thread: ThreadId, _reason: Option<String>) -> Result<()> {
            Err(Error::Unsupported("unused stop"))
        }
        async fn generate_title(&self, _thread: ThreadId, _kind: TitleKind) -> Result<()> {
            Err(Error::Unsupported("unused title"))
        }
        async fn recover_delegated_task(
            &self,
            thread: ThreadId,
            source: Option<RunId>,
            completed: Option<RunId>,
        ) -> Result<()> {
            assert_eq!(thread.0, "thread:test");
            assert_eq!(source.unwrap().0, "run:test");
            assert!(completed.is_none());
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
    for case in ["continue", "opt-out", "stop", "compact", "logout"] {
        let (h, dispatcher, _offers) = live_dispatcher().await?;
        let workspace = tempfile::tempdir()?;
        let mut thread = t3_contracts::stored::to_value(
            &h.reader
                .thread(ThreadId("thread:test".into()))
                .await?
                .unwrap(),
        )?;
        thread["worktreePath"] = json!(workspace.path());
        thread["settledOverride"] = Value::Null;
        thread["settledAt"] = Value::Null;
        h.write("thread.metadata-updated", thread).await?;
        let mut run = t3_contracts::stored::to_value(
            &h.reader
                .run(ThreadId("thread:test".into()), RunId("run:test".into()))
                .await?
                .unwrap(),
        )?;
        run["status"] = "cancelled".into();
        run["requestedAt"] = json!(TEST_TIME);
        run["startedAt"] = json!(TEST_TIME);
        run["completedAt"] = json!(TEST_TIME);
        run["modelSelection"] = json!({"instanceId":"codex","model":"m"});
        run["userMessageId"] = "message:original".into();
        h.write("run.updated", run).await?;
        let mut message = fixture("OrchestrationV2ConversationMessage");
        message["id"] = "message:original".into();
        message["threadId"] = "thread:test".into();
        message["runId"] = "run:test".into();
        message["role"] = "user".into();
        message["text"] = match case {
            "compact" => " /COMPACT ",
            "logout" => "/logout",
            _ => "Keep going",
        }
        .into();
        message["attachments"] = json!([]);
        h.write("message.updated", message).await?;
        if case == "stop" {
            let mut stop =
                super::provider_event_ingestor_test::item("run_interrupt_request", "stop");
            stop["message"] = "Stopped by user".into();
            h.write("turn-item.updated", stop).await?;
        }
        let settings = Arc::new(StaticSettings::new(
            t3_contracts::settings::ServerSettings {
                continue_threads_after_server_update: case != "opt-out",
                ..Default::default()
            },
        ));
        let threads = ThreadManagementService::new(ThreadManagementServices {
            reader: h.reader.clone(),
            dispatcher: Arc::new(dispatcher),
            committer: h.sink.clone(),
            database: h.database.stores.clone(),
            settings: settings.clone(),
            hydrator: Arc::new(NoLegacyTranscript),
            worktree_restore: Arc::new(t3_orchestration::composition::NoWorktreeRestore),
            signals: h.sink.signals(),
            clock: h.clock.clone(),
        });
        let operations = Arc::new(Operations(AtomicUsize::new(0)));
        let service = RestartContinuationService {
            threads: threads.clone(),
            settings,
            operations: operations.clone(),
            clock: h.clock.clone(),
            dispatch_policy: None,
        };
        for _ in 0..2 {
            service
                .continue_restarted_run(ThreadId("thread:test".into()), RunId("run:test".into()))
                .await?;
        }
        let projection = h
            .reader
            .projection(ThreadId("thread:test".into()))
            .await?
            .unwrap();
        let messages = projection
            .messages
            .iter()
            .filter(|message| {
                t3_contracts::stored::to_value(&message.id).unwrap()
                    == "message:restart-continuation:run:test"
            })
            .collect::<Vec<_>>();
        assert_eq!(messages.len(), usize::from(case == "continue"), "{case}");
        if case == "continue" {
            assert_eq!(messages[0].text, CONTINUE_PROMPT);
            let continuation = projection
                .runs
                .iter()
                .find(|run| run.user_message_id == messages[0].id)
                .unwrap();
            assert_eq!(
                t3_contracts::stored::to_value(continuation)?["restartContinuationOfRunId"],
                "run:test"
            );
        }
        assert_eq!(operations.0.load(Ordering::Relaxed), 2);
        threads.shutdown().await;
    }
    Ok(())
}
