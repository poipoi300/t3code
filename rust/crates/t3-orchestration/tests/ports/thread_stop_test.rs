#![allow(clippy::unwrap_used)]

use super::{
    orchestrator_receipt_race_test::Harness,
    thread_management_service_test::{emit, run, sample, service, settings},
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use t3_contracts::provider_instance::{ProviderDriverKind, ProviderInstanceId};
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    ports::{
        decisions::thread_command_executor::KeyedLanes,
        thread_project::thread_management_service::*,
    },
    provider::{OpenSession, SessionRuntime, ThreadSnapshot},
    testing::FakeProviderAdapter,
};
use tokio::sync::{Semaphore, mpsc};

struct BackgroundRuntime {
    session: OrchestrationV2ProviderSession,
    supported: bool,
    calls: mpsc::UnboundedSender<(ProviderThreadId, bool)>,
    workflow_calls: Mutex<Vec<(ProviderThreadId, String)>>,
}

use t3_contracts::model_selection::ModelSelection;
use t3_orchestration::provider::*;

fn unused<T>() -> ProviderResult<T> {
    Err(ProviderError::Unsupported(
        "unused background test operation",
    ))
}

#[async_trait]
impl SessionRuntime for BackgroundRuntime {
    fn session(&self) -> &OrchestrationV2ProviderSession {
        &self.session
    }
    fn can_resume_after_restart(&self) -> bool {
        false
    }
    fn background_tasks_end_with_process(&self) -> bool {
        true
    }
    async fn subscribe(&self) -> ProviderResult<EventSubscription> {
        unused()
    }
    async fn close(&self) -> ProviderResult<()> {
        unused()
    }
    async fn ensure_thread(
        &self,
        _: EnsureThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        unused()
    }
    async fn resume_thread(
        &self,
        _: ResumeThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        unused()
    }
    async fn start_turn(&self, _: TurnInput) -> ProviderResult<()> {
        unused()
    }
    async fn steer_turn(&self, _: SteerInput) -> ProviderResult<()> {
        unused()
    }
    async fn interrupt_turn(&self, _: InterruptInput) -> ProviderResult<()> {
        unused()
    }
    async fn respond(&self, _: RuntimeResponse) -> ProviderResult<()> {
        unused()
    }
    async fn snapshot(&self, _: OrchestrationV2ProviderThread) -> ProviderResult<ThreadSnapshot> {
        unused()
    }
    async fn rollback(&self, _: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        unused()
    }
    async fn fork(&self, _: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
        unused()
    }
    fn extensions(&self) -> Option<&dyn SessionExtensions> {
        self.supported.then_some(self)
    }
}

#[async_trait]
impl SessionExtensions for BackgroundRuntime {
    fn capabilities(&self) -> ExtensionCapabilities {
        ExtensionCapabilities {
            await_tasks: true,
            stop_task: true,
            ..Default::default()
        }
    }
    async fn pending_background_work(
        &self,
        _: Option<OrchestrationV2ProviderThread>,
        _: bool,
    ) -> ProviderResult<bool> {
        unused()
    }
    async fn model_context_window(
        &self,
        _: ModelSelection,
        _: Option<std::path::PathBuf>,
    ) -> ProviderResult<Option<u64>> {
        unused()
    }
    fn can_reuse_context_usage(&self, _: &ModelSelection, _: &ModelSelection) -> bool {
        false
    }
    fn normalize_thread_load_selection(&self, selection: ModelSelection) -> ModelSelection {
        selection
    }
    async fn inject_history(
        &self,
        _: OrchestrationV2ProviderThread,
        _: Vec<OrchestrationV2HistoricalMessage>,
        _: String,
    ) -> ProviderResult<bool> {
        unused()
    }
    async fn compact(&self, _: TurnInput) -> ProviderResult<()> {
        unused()
    }
    async fn apply_selection(
        &self,
        _: OrchestrationV2ProviderThread,
        _: ModelSelection,
    ) -> ProviderResult<()> {
        unused()
    }
    async fn stop_task(
        &self,
        thread: OrchestrationV2ProviderThread,
        task: String,
    ) -> ProviderResult<()> {
        self.workflow_calls.lock().unwrap().push((
            serde_json::from_value(serde_json::to_value(thread.id).unwrap()).unwrap(),
            task,
        ));
        Ok(())
    }
    async fn await_tasks(
        &self,
        thread: OrchestrationV2ProviderThread,
        task_ids: Vec<String>,
        awaited: bool,
    ) -> ProviderResult<Vec<String>> {
        self.calls
            .send((
                ProviderThreadId(
                    serde_json::to_value(thread.id)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .into(),
                ),
                awaited,
            ))
            .unwrap();
        Ok(task_ids.into_iter().filter(|id| id == "tests").collect())
    }
    async fn stop_subagent(
        &self,
        _: OrchestrationV2ProviderThread,
        _: String,
        _: Option<IsoDateTime>,
    ) -> ProviderResult<Option<SubagentOutcome>> {
        unused()
    }
    async fn unload(&self, _: OrchestrationV2ProviderThread, _: bool) -> ProviderResult<()> {
        unused()
    }
    async fn upload_feedback(
        &self,
        _: OrchestrationV2ProviderThread,
        _: Option<String>,
    ) -> ProviderResult<String> {
        unused()
    }
    async fn list_tools(
        &self,
        _: OrchestrationV2ProviderThread,
        _: String,
    ) -> ProviderResult<Vec<McpTool>> {
        unused()
    }
    async fn call_tool(
        &self,
        _: OrchestrationV2ProviderThread,
        _: String,
        _: String,
        _: serde_json::Value,
    ) -> ProviderResult<McpResult> {
        unused()
    }
    async fn read_resource(
        &self,
        _: OrchestrationV2ProviderThread,
        _: String,
        _: String,
    ) -> ProviderResult<Vec<serde_json::Value>> {
        unused()
    }
}

#[tokio::test]
async fn background_wait_reports_matched_unmatched_unavailable_and_unsupported_work() -> Result<()>
{
    for state in [
        "live",
        "no-match",
        "no-match-clear",
        "no-session",
        "unsupported-runtime",
        "read-failed",
    ] {
        let harness = Harness::new(None, false);
        seed(&harness, false).await?;
        let mut thread = json!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
        );
        thread["activeProviderThreadId"] = json!("provider-thread");
        emit(
            &harness,
            "active-provider",
            "thread.metadata-updated",
            thread,
        )
        .await?;
        let (management, _, _) = service(&harness, settings(json!({})), false);
        let (sender, mut calls) = mpsc::unbounded_channel();
        let sessions = Arc::new(t3_orchestration::testing::FakeSessionDirectory::default());
        if state == "read-failed" {
            sessions.get.push(Err(Error::Service {
                operation: "test.get",
                detail: "provider is unavailable".into(),
            }));
        } else {
            sessions.get.push(Ok(if state == "no-session" {
                None
            } else {
                Some(Arc::new(BackgroundRuntime {
                    session: serde_json::from_value(sample("OrchestrationV2ProviderSession"))?,
                    supported: state != "unsupported-runtime",
                    calls: sender,
                    workflow_calls: Mutex::new(Vec::new()),
                }))
            }));
        }
        let stop = ThreadStopService::new(ThreadStopServices {
            management,
            sessions,
            agent_scope: Arc::new(t3_orchestration::testing::FakeAgentScope::default()),
            gate: harness.dispatcher.command_lanes()?,
        });
        let ids = if state.starts_with("no-match") {
            vec!["finished".into()]
        } else {
            vec!["tests".into(), "finished".into()]
        };
        let result = stop
            .await_background_tasks(ThreadId("thread".into()), ids, state != "no-match-clear")
            .await;
        match state {
            "live" => {
                assert_eq!(result?, (vec!["tests".into()], vec!["finished".into()]));
                assert_eq!(
                    calls.recv().await,
                    Some((ProviderThreadId("provider-thread".into()), true))
                );
            }
            "no-match-clear" => {
                assert_eq!(result?, (vec![], vec!["finished".into()]));
                assert_eq!(
                    calls.recv().await,
                    Some((ProviderThreadId("provider-thread".into()), false))
                );
            }
            _ => {
                let expected = match state {
                    "no-match" => "ThreadManagementBackgroundWaitError.unmatched",
                    "unsupported-runtime" => "ThreadManagementBackgroundWaitError.unsupported",
                    _ => "ThreadManagementBackgroundWaitError.unavailable",
                };
                assert!(
                    matches!(result, Err(Error::Service { operation, .. }) if operation == expected),
                    "{state}: {result:?}"
                );
            }
        }
        stop.shutdown().await;
    }
    Ok(())
}

#[tokio::test]
async fn stop_workflow_targets_the_matching_native_task_and_rejects_ineligible_work() -> Result<()>
{
    use t3_contracts::provider_policy::{ProviderInteractionMode, RuntimeMode};
    use t3_orchestration::ports::decisions::dispatch_mode_limit::DispatchModes;
    for state in [
        "live",
        "ended",
        "non-native",
        "wrong-instance",
        "missing-task",
        "wrong-mode",
    ] {
        let harness = Harness::new(None, false);
        seed(&harness, false).await?;
        let mut provider = provider_thread();
        provider["driver"] = json!("claudeAgent");
        provider["providerInstanceId"] = json!("claudeAgent");
        emit(
            &harness,
            "workflow-provider",
            "provider-thread.updated",
            provider,
        )
        .await?;
        let mut task = sample("OrchestrationV2Subagent");
        for (key, value) in [
            ("id", json!("workflow")),
            ("threadId", json!("thread")),
            ("runId", json!("run")),
            ("parentNodeId", json!("node")),
            ("origin", json!("provider_native")),
            ("driver", json!("claudeAgent")),
            ("providerInstanceId", json!("claudeAgent")),
            ("providerThreadId", json!("provider-thread")),
            ("childThreadId", serde_json::Value::Null),
            (
                "nativeTaskRef",
                json!({"driver":"claudeAgent","nativeId":"native-workflow","strength":"strong"}),
            ),
            ("workflow", json!({"phases":[],"agents":[]})),
            ("status", json!("running")),
        ] {
            task[key] = value;
        }
        match state {
            "ended" => task["status"] = json!("completed"),
            "non-native" => task["origin"] = json!("app_owned"),
            "wrong-instance" => task["providerInstanceId"] = json!("other"),
            "missing-task" => task["nativeTaskRef"] = serde_json::Value::Null,
            _ => {}
        }
        emit(&harness, "workflow", "subagent.updated", task).await?;
        let (management, _, _) = service(&harness, settings(json!({})), false);
        let (sender, _) = mpsc::unbounded_channel();
        let runtime = Arc::new(BackgroundRuntime {
            session: serde_json::from_value(sample("OrchestrationV2ProviderSession"))?,
            supported: true,
            calls: sender,
            workflow_calls: Mutex::new(Vec::new()),
        });
        let sessions = Arc::new(t3_orchestration::testing::FakeSessionDirectory::default());
        sessions.get.push(Ok(Some(runtime.clone())));
        let stop = ThreadStopService::new(ThreadStopServices {
            management,
            sessions,
            agent_scope: Arc::new(t3_orchestration::testing::FakeAgentScope::default()),
            gate: harness.dispatcher.command_lanes()?,
        });
        let limit = (state == "wrong-mode").then_some(DispatchModes {
            runtime_mode: RuntimeMode::ApprovalRequired,
            interaction_mode: ProviderInteractionMode::Plan,
        });
        let result = stop
            .stop_workflow(
                serde_json::from_value(json!({"threadId":"thread","subagentId":"workflow"}))?,
                limit,
            )
            .await;
        if state == "live" {
            result?;
            assert_eq!(
                *runtime.workflow_calls.lock().unwrap(),
                [(
                    ProviderThreadId("provider-thread".into()),
                    "native-workflow".into()
                )]
            );
        } else {
            assert!(result.is_err(), "{state}");
            assert!(runtime.workflow_calls.lock().unwrap().is_empty());
        }
        stop.shutdown().await;
    }
    Ok(())
}

struct Gate(KeyedLanes);

#[tokio::test]
async fn stop_and_queue_holding_interrupt_end_preparing_runs_hold_followups_and_clear_watches()
-> Result<()> {
    for kind in ["thread.stop", "run.interrupt"] {
        let harness = Harness::new(None, false);
        harness.create("thread").await?;
        let mut preparing = run("active", 1, "preparing");
        preparing["startedAt"] = serde_json::Value::Null;
        preparing["completedAt"] = serde_json::Value::Null;
        preparing["workspacePreparation"] = json!({"type":"worktree","baseRef":"main"});
        emit(&harness, "preparing", "run.created", preparing).await?;
        let mut provider = provider_thread();
        provider["providerSessionId"] = serde_json::Value::Null;
        emit(
            &harness,
            "prepared-provider",
            "provider-thread.updated",
            provider,
        )
        .await?;
        emit(&harness, "prepared-root", "node.updated", json!({"id":"node","threadId":"thread","runId":"active","parentNodeId":null,"rootNodeId":"node","kind":"root_turn","status":"pending","countsForRun":true,"providerThreadId":"provider-thread","providerTurnId":null,"nativeItemRef":null,"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":null,"completedAt":null})).await?;
        emit(&harness, "prepared-attempt", "run-attempt.created", json!({"id":"attempt","runId":"active","attemptOrdinal":1,"rootNodeId":"node","providerInstanceId":"codex","providerThreadId":"provider-thread","providerTurnId":null,"reason":"initial","status":"pending","startedAt":null,"completedAt":null})).await?;
        emit(
            &harness,
            "queued",
            "run.created",
            run("queued", 2, "queued"),
        )
        .await?;
        let mut thread = json!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
        );
        thread["pullRequests"] = json!([{"host":"github.com","repository":"owner/repo","number":1,"url":"https://github.com/owner/repo/pull/1","source":"agent","linkedAt":t3_orchestration::testing::TEST_TIME,"snapshot":null,"stack":null,"watch":sample("ThreadPullRequestWatch")}]);
        emit(&harness, "watch", "thread.metadata-updated", thread).await?;
        let command = if kind == "thread.stop" {
            json!({"type":kind,"commandId":"stop","threadId":"thread"})
        } else {
            json!({"type":kind,"commandId":"stop","threadId":"thread","runId":"active","holdQueue":true})
        };
        harness.dispatch(command.clone()).await?;
        let projection = harness
            .reader
            .projection(ThreadId("thread".into()))
            .await?
            .unwrap();
        let active = projection
            .runs
            .iter()
            .find(|run| json!(run.id) == "active")
            .unwrap();
        assert_eq!(json!(active.status), "interrupted", "{kind}");
        assert_eq!(
            json!(projection.attempts[0].status),
            "interrupted",
            "{kind}"
        );
        assert_eq!(json!(projection.nodes[0].status), "interrupted", "{kind}");
        let queued = json!(
            projection
                .runs
                .iter()
                .find(|run| json!(run.id) == "queued")
                .unwrap()
        );
        assert_eq!(queued["status"], "queued");
        assert_eq!(queued["queueHeld"], true);
        assert!(
            json!(projection.thread)["pullRequests"][0]
                .get("watch")
                .is_none()
        );
        let before = harness.count_events().await;
        harness.dispatch(command).await?;
        assert_eq!(harness.count_events().await, before);
    }
    Ok(())
}
#[async_trait]
impl ThreadControlGate for Gate {
    async fn lock(&self, thread_id: ThreadId) -> Result<tokio::sync::OwnedMutexGuard<()>> {
        Ok(self.0.lane(&thread_id.0).lock_owned().await)
    }
}
struct Scope {
    killed: bool,
    calls: AtomicUsize,
}
#[async_trait]
impl AgentScope for Scope {
    async fn kill(&self, _: ThreadId) -> Result<bool> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.killed)
    }
}
struct Sessions {
    runtime: Arc<dyn SessionRuntime>,
    others: Vec<ThreadId>,
    release_result: bool,
    calls: mpsc::UnboundedSender<&'static str>,
    release_gate: Option<Arc<Semaphore>>,
    retained: Mutex<Vec<bool>>,
}
#[async_trait]
impl SessionDirectory for Sessions {
    async fn get(&self, _: ProviderSessionId) -> Result<Option<Arc<dyn SessionRuntime>>> {
        Ok(Some(self.runtime.clone()))
    }
    async fn attach(
        &self,
        _: ProviderInstanceId,
        _: OpenSession,
    ) -> Result<Arc<dyn SessionRuntime>> {
        Err(Error::Unsupported("test attach"))
    }
    async fn detach(&self, _: ProviderSessionId, _: bool) -> Result<()> {
        panic!("force stop must use thread-specific detach")
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
    async fn other_working_threads(
        &self,
        _: ProviderSessionId,
        _: ThreadId,
    ) -> Result<Vec<ThreadId>> {
        Ok(self.others.clone())
    }
    async fn detach_thread(&self, input: DetachSession) -> Result<()> {
        assert_eq!(input.thread_id.0, "thread");
        assert!(!input.revoke_credential);
        self.calls.send("detach").unwrap();
        Ok(())
    }
    async fn release_unless_busy(&self, input: ReleaseSession) -> Result<bool> {
        assert_eq!(input.reason, "manual_shutdown");
        assert_eq!(input.through_run_ordinal, 1);
        assert_eq!(
            input.own_provider_thread_ids,
            [ProviderThreadId("provider-thread".into())]
        );
        self.calls.send("release").unwrap();
        if let Some(gate) = &self.release_gate {
            gate.acquire().await.unwrap().forget();
        }
        let retained = input.keep_if.unwrap().keep().await?;
        self.retained.lock().unwrap().push(retained);
        Ok(self.release_result && !retained)
    }
}

fn provider_thread() -> serde_json::Value {
    let mut value = sample("OrchestrationV2ProviderThread");
    value["id"] = json!("provider-thread");
    value["appThreadId"] = json!("thread");
    value["providerSessionId"] = json!("session");
    value["providerInstanceId"] = json!("codex");
    value["driver"] = json!("codex");
    value["handoffIds"] = json!([]);
    value
}
fn runtime() -> Arc<dyn SessionRuntime> {
    let mut session = sample("OrchestrationV2ProviderSession");
    session["id"] = json!("session");
    let (adapter, _calls) = FakeProviderAdapter::new(
        ProviderInstanceId("codex".into()),
        ProviderDriverKind("codex".into()),
        serde_json::from_value(sample("OrchestrationV2ProviderCapabilities")).unwrap(),
        serde_json::from_value(session).unwrap(),
        ThreadSnapshot {
            provider_thread: serde_json::from_value(provider_thread()).unwrap(),
            provider_turns: vec![],
            messages: vec![],
            reasoning_message_ids: None,
            runtime_requests: vec![],
            provider_payload: None,
        },
    );
    adapter.runtime
}
async fn seed(harness: &Harness, completed: bool) -> Result<()> {
    harness.create("thread").await?;
    emit(
        harness,
        "run",
        "run.created",
        run("run", 1, if completed { "completed" } else { "running" }),
    )
    .await?;
    emit(harness, "queued", "run.created", run("queued", 2, "queued")).await?;
    emit(
        harness,
        "provider-thread",
        "provider-thread.updated",
        provider_thread(),
    )
    .await?;
    let mut turn = sample("OrchestrationV2ProviderTurn");
    turn["id"] = json!("provider-turn");
    turn["providerThreadId"] = json!("provider-thread");
    turn["runAttemptId"] = json!("attempt");
    turn["status"] = json!(if completed { "completed" } else { "running" });
    emit(harness, "provider-turn", "provider-turn.updated", turn).await
}

#[tokio::test]
async fn force_stop_ends_own_turn_preserving_shared_processes_and_busy_release_races() -> Result<()>
{
    for (others, killed, release, completed, expected, teardown) in [
        (vec![], true, true, false, "killed", vec!["release"]),
        (
            vec![ThreadId("other".into())],
            false,
            true,
            false,
            "shared",
            vec!["detach"],
        ),
        (
            vec![ThreadId("other".into())],
            false,
            true,
            true,
            "shared",
            vec!["detach"],
        ),
        (
            vec![],
            false,
            false,
            false,
            "shared",
            vec!["release", "detach"],
        ),
        (vec![], false, true, false, "released", vec!["release"]),
    ] {
        let harness = Harness::new(None, false);
        seed(&harness, completed).await?;
        let (management, recorder, _) = service(&harness, settings(json!({})), false);
        let scope = Arc::new(Scope {
            killed,
            calls: AtomicUsize::new(0),
        });
        let (sender, mut calls) = mpsc::unbounded_channel();
        let sessions = Arc::new(Sessions {
            runtime: runtime(),
            others: others.clone(),
            release_result: release,
            calls: sender,
            release_gate: None,
            retained: Mutex::new(Vec::new()),
        });
        let stop = ThreadStopService::new(ThreadStopServices {
            management,
            sessions,
            agent_scope: scope.clone(),
            gate: Arc::new(Gate(KeyedLanes::default())),
        });
        let result = stop
            .force_stop_thread(OrchestrationV2ForceStopThreadInput {
                thread_id: ThreadId("thread".into()),
                run_id: Some(RunId("run".into())),
            })
            .await?;
        assert_eq!(json!(result.outcome), expected);
        assert_eq!(result.shared_with_thread_ids, others);
        assert_eq!(scope.calls.load(Ordering::SeqCst), 1);
        let mut actual = Vec::new();
        for _ in &teardown {
            actual.push(calls.recv().await.unwrap());
        }
        stop.shutdown().await;
        assert!(calls.try_recv().is_err());
        assert_eq!(actual, teardown);
        let commands = recorder.commands.lock().unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0]["type"], "thread.background-work.settle");
        assert_eq!(commands[0]["providerThreadId"], "provider-thread");
        assert_eq!(commands[0]["providerTurnId"], "provider-turn");
        assert_eq!(commands[0]["reason"], "Force stopped by user");
    }
    Ok(())
}
#[tokio::test(start_paused = true)]
async fn force_stop_timeout_keeps_release_owned_until_shutdown() -> Result<()> {
    let harness = Harness::new(None, false);
    seed(&harness, false).await?;
    let (management, _, _) = service(&harness, settings(json!({})), false);
    let (sender, mut calls) = mpsc::unbounded_channel();
    let gate = Arc::new(Semaphore::new(0));
    let stop = Arc::new(ThreadStopService::new(ThreadStopServices {
        management,
        sessions: Arc::new(Sessions {
            runtime: runtime(),
            others: vec![],
            release_result: true,
            calls: sender,
            release_gate: Some(gate.clone()),
            retained: Mutex::new(Vec::new()),
        }),
        agent_scope: Arc::new(Scope {
            killed: false,
            calls: AtomicUsize::new(0),
        }),
        gate: Arc::new(Gate(KeyedLanes::default())),
    }));
    let task = tokio::spawn({
        let stop = stop.clone();
        async move {
            stop.force_stop_thread(OrchestrationV2ForceStopThreadInput {
                thread_id: ThreadId("thread".into()),
                run_id: None,
            })
            .await
        }
    });
    assert_eq!(calls.recv().await, Some("release"));
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    assert_eq!(json!(task.await.unwrap()?.outcome), "releasing");
    gate.add_permits(1);
    stop.shutdown().await;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stop_shutdown_cancels_and_joins_timed_out_release_without_waiting_for_backend_gate()
-> Result<()> {
    let harness = Harness::new(None, false);
    seed(&harness, false).await?;
    let (management, _, _) = service(&harness, settings(json!({})), false);
    let (sender, mut calls) = mpsc::unbounded_channel();
    let sessions = Arc::new(Sessions {
        runtime: runtime(),
        others: vec![],
        release_result: true,
        calls: sender,
        release_gate: Some(Arc::new(Semaphore::new(0))),
        retained: Mutex::new(Vec::new()),
    });
    let stop = Arc::new(ThreadStopService::new(ThreadStopServices {
        management,
        sessions: sessions.clone(),
        agent_scope: Arc::new(Scope {
            killed: false,
            calls: AtomicUsize::new(0),
        }),
        gate: harness.dispatcher.command_lanes()?,
    }));
    let stopping = tokio::spawn({
        let stop = stop.clone();
        async move {
            stop.force_stop_thread(OrchestrationV2ForceStopThreadInput {
                thread_id: ThreadId("thread".into()),
                run_id: None,
            })
            .await
        }
    });
    assert_eq!(calls.recv().await, Some("release"));
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    assert_eq!(json!(stopping.await.unwrap()?.outcome), "releasing");
    stop.shutdown().await;
    assert!(sessions.retained.lock().unwrap().is_empty());
    assert!(matches!(
        stop.force_stop_thread(OrchestrationV2ForceStopThreadInput {
            thread_id: ThreadId("thread".into()),
            run_id: None
        })
        .await,
        Err(Error::Cancelled)
    ));
    stop.shutdown().await;
    Ok(())
}
#[tokio::test]
async fn force_stop_refuses_superseded_target_before_killing_or_dispatching() -> Result<()> {
    let harness = Harness::new(None, false);
    seed(&harness, false).await?;
    emit(&harness, "newer", "run.created", run("newer", 3, "running")).await?;
    let (management, recorder, _) = service(&harness, settings(json!({})), false);
    let scope = Arc::new(Scope {
        killed: true,
        calls: AtomicUsize::new(0),
    });
    let (sender, mut calls) = mpsc::unbounded_channel();
    let stop = ThreadStopService::new(ThreadStopServices {
        management,
        sessions: Arc::new(Sessions {
            runtime: runtime(),
            others: vec![],
            release_result: true,
            calls: sender,
            release_gate: None,
            retained: Mutex::new(Vec::new()),
        }),
        agent_scope: scope.clone(),
        gate: Arc::new(Gate(KeyedLanes::default())),
    });
    assert!(matches!(
        stop.force_stop_thread(OrchestrationV2ForceStopThreadInput {
            thread_id: ThreadId("thread".into()),
            run_id: Some(RunId("run".into()))
        })
        .await,
        Err(Error::Service {
            operation: "ForceStop.superseded",
            ..
        })
    ));
    assert_eq!(scope.calls.load(Ordering::SeqCst), 0);
    assert!(recorder.commands.lock().unwrap().is_empty());
    assert!(calls.try_recv().is_err());
    Ok(())
}
#[tokio::test]
async fn interrupt_checks_run_membership_terminal_status_and_active_target() -> Result<()> {
    let harness = Harness::new(None, false);
    seed(&harness, false).await?;
    emit(
        &harness,
        "finished",
        "run.created",
        run("finished", 3, "completed"),
    )
    .await?;
    let (service, recorder, _) = service(&harness, settings(json!({})), false);
    for (id, expected) in [
        ("finished", "terminal"),
        ("missing", "missing"),
        ("queued", "not-interruptible"),
        ("run", "requested"),
    ] {
        let result = service
            .interrupt_thread(
                ProjectId("project".into()),
                ThreadId("thread".into()),
                CommandId(format!("interrupt:{id}")),
                Some(RunId(id.into())),
                Some("User stopped".into()),
            )
            .await;
        match expected {
            "terminal" => assert!(matches!(result, Ok(InterruptResult::AlreadyTerminal(_)))),
            "requested" => assert!(matches!(result, Ok(InterruptResult::Requested { .. }))),
            _ => assert!(result.is_err()),
        }
    }
    let commands = recorder.commands.lock().unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0]["type"], "run.interrupt");
    assert_eq!(commands[0]["runId"], "run");
    assert_eq!(commands[0]["reason"], "User stopped");
    Ok(())
}

#[tokio::test]
async fn force_stop_keeps_a_promoted_run_on_the_same_session_during_teardown() -> Result<()> {
    for same_session in [true, false] {
        let harness = Harness::new(None, false);
        seed(&harness, false).await?;
        let (management, _, _) = service(&harness, settings(json!({})), false);
        let (sender, mut calls) = mpsc::unbounded_channel();
        let gate = Arc::new(Semaphore::new(0));
        let sessions = Arc::new(Sessions {
            runtime: runtime(),
            others: vec![],
            release_result: true,
            calls: sender,
            release_gate: Some(gate.clone()),
            retained: Mutex::new(Vec::new()),
        });
        let stop = Arc::new(ThreadStopService::new(ThreadStopServices {
            management,
            sessions: sessions.clone(),
            agent_scope: Arc::new(Scope {
                killed: false,
                calls: AtomicUsize::new(0),
            }),
            gate: harness.dispatcher.command_lanes()?,
        }));
        let stopping = tokio::spawn({
            let stop = stop.clone();
            async move {
                stop.force_stop_thread(OrchestrationV2ForceStopThreadInput {
                    thread_id: ThreadId("thread".into()),
                    run_id: Some(RunId("run".into())),
                })
                .await
            }
        });
        assert_eq!(calls.recv().await, Some("release"));
        let mut newer = run("promoted", 3, "starting");
        if !same_session {
            let mut provider = provider_thread();
            provider["id"] = json!("new-provider-thread");
            provider["providerSessionId"] = json!("other-session");
            emit(
                &harness,
                "other-session",
                "provider-thread.updated",
                provider,
            )
            .await?;
            newer["providerThreadId"] = json!("new-provider-thread");
        }
        emit(&harness, "promoted", "run.created", newer).await?;
        gate.add_permits(1);
        assert_eq!(json!(stopping.await.unwrap()?.outcome), "released");
        stop.shutdown().await;
        assert_eq!(*sessions.retained.lock().unwrap(), [same_session]);
        assert!(calls.try_recv().is_err());
    }
    Ok(())
}
