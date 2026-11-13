#![cfg(feature = "acp-adapter")]

use serde_json::json;
use std::sync::{Arc, Mutex};
use t3_contracts::{
    base_schemas::*,
    model_selection::ModelSelection,
    orchestration_v2::*,
    provider_instance::ProviderInstanceId,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};
use t3_orchestration::{
    adapters::acp::{DEV_REPLAY_INSTANCE_ID, dev_replay_registration},
    api::{Clock, ProviderRegistry},
    ports::{
        acp_replay::adapters::acp_adapter_v2_testkit::make_turn_input,
        sessions_recovery::provider_adapter_registry::AdapterRegistry,
    },
    provider::*,
    testing::{ManualClock, SequenceIds, TEST_TIME},
};
use tokio_util::sync::CancellationToken;

fn policy() -> RuntimePolicy {
    RuntimePolicy {
        runtime_mode: RuntimeMode::ApprovalRequired,
        interaction_mode: ProviderInteractionMode::Default,
        cwd: Some(".".into()),
        approval_policy: None,
        sandbox_policy: None,
        max_runtime_mode: None,
    }
}
fn selection(model: &str) -> ModelSelection {
    serde_json::from_value(json!({"instanceId":DEV_REPLAY_INSTANCE_ID,"model":model})).unwrap()
}
fn make_registry(
    prefix: &str,
    observed: Arc<Mutex<Vec<ProviderEvent>>>,
) -> Arc<dyn ProviderRegistry> {
    let mut registration = dev_replay_registration(
        Arc::new(ManualClock::new(TEST_TIME)),
        Arc::new(SequenceIds::new(prefix)),
        false,
    );
    registration.adapter = t3_orchestration::ports::acp_replay::testkit::provider_replay_harness::event_tap::observe_adapter(registration.adapter, Arc::new(move |event| observed.lock().unwrap().push(event.clone())));
    Arc::new(AdapterRegistry::new(vec![registration]))
}
async fn open(
    registry: &dyn ProviderRegistry,
    previous: Option<(
        OrchestrationV2ProviderSession,
        OrchestrationV2ProviderThread,
    )>,
    model: &str,
    session_id: &str,
) -> (
    Arc<dyn SessionRuntime>,
    EventSubscription,
    OrchestrationV2ProviderThread,
) {
    let adapter = registry
        .adapter(ProviderInstanceId(DEV_REPLAY_INSTANCE_ID.into()))
        .await
        .unwrap();
    let runtime = adapter
        .open_session(
            OpenSession {
                thread_id: ThreadId("thread:registry-replay".into()),
                provider_session_id: ProviderSessionId(session_id.into()),
                model_selection: selection(model),
                runtime_policy: policy(),
                prompt_suggestions: None,
                resume_from_session: previous.as_ref().map(|(session, _)| session.clone()),
                initial_native_thread_id: previous.as_ref().and_then(|(_, thread)| {
                    thread
                        .native_thread_ref
                        .as_ref()?
                        .native_id
                        .as_ref()
                        .map(|id| id.0.clone())
                }),
                initial_provider_item_identity_version: previous.as_ref().map(|_| 2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let events = runtime.subscribe().await.unwrap();
    let thread = match previous {
        Some((_, thread)) => runtime
            .resume_thread(ResumeThread {
                provider_thread: thread,
                thread_id: None,
                model_selection: Some(selection(model)),
                runtime_policy: Some(policy()),
            })
            .await
            .unwrap(),
        None => runtime
            .ensure_thread(EnsureThread {
                thread_id: ThreadId("thread:registry-replay".into()),
                title: None,
                model_selection: selection(model),
                runtime_policy: policy(),
                provider_session_id: None,
                existing_provider_thread: None,
                force_new_native_thread: false,
            })
            .await
            .unwrap(),
    };
    (runtime, events, thread)
}

async fn turn(
    runtime: &Arc<dyn SessionRuntime>,
    events: &mut EventSubscription,
    thread: OrchestrationV2ProviderThread,
    ordinal: u64,
    interrupt: bool,
) -> OrchestrationV2ProviderThread {
    let model = if interrupt { "showcase" } else { "multi-turn" };
    runtime
        .start_turn(
            make_turn_input(
                thread.clone(),
                selection(model),
                policy(),
                ManualClock::new(TEST_TIME).now(),
                ordinal,
                "Replay registry lifecycle",
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let mut current_thread = thread.clone();
    let mut running_turn = None;
    let mut interrupt_task = None;
    let mut streamed = false;
    let mut completed = false;
    loop {
        match events
            .events
            .recv()
            .await
            .expect("event stream ended")
            .unwrap()
            .kind
        {
            ProviderEventKind::Turn { provider_turn, .. }
                if provider_turn.status == ProviderTurnStatus::Running =>
            {
                running_turn = Some(
                    serde_json::from_value::<ProviderTurnId>(
                        serde_json::to_value(provider_turn.id).unwrap(),
                    )
                    .unwrap(),
                );
            }
            ProviderEventKind::Message(message) => {
                streamed |= message.streaming;
                completed |= !message.streaming && !message.text.is_empty();
            }
            ProviderEventKind::Thread {
                provider_thread, ..
            } => current_thread = *provider_thread,
            ProviderEventKind::RuntimeRequest { request, .. }
                if request.status == RuntimeRequestStatus::Pending && interrupt =>
            {
                let runtime = runtime.clone();
                let input = InterruptInput {
                    provider_thread: thread.clone(),
                    provider_turn_id: running_turn.clone().expect("turn started before request"),
                    request_runtime_restart: false,
                };
                interrupt_task = Some(tokio::spawn(
                    async move { runtime.interrupt_turn(input).await },
                ));
            }
            ProviderEventKind::Terminal { outcome, .. } => {
                assert_eq!(
                    matches!(outcome, TerminalOutcome::Interrupted { .. }),
                    interrupt,
                    "ordinal={ordinal}, outcome={outcome:?}"
                );
                assert!(interrupt || matches!(outcome, TerminalOutcome::Completed { .. }));
                break;
            }
            _ => {}
        }
    }
    if let Some(task) = interrupt_task {
        task.await.unwrap().unwrap();
    }
    assert!(streamed);
    assert!(completed);
    current_thread
}

#[tokio::test(start_paused = true)]
async fn registered_replay_completes_interrupts_and_resumes_restored_native_thread() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let registry = make_registry("before-restart", observed.clone());
    assert_eq!(
        registry.list().await.unwrap(),
        [ProviderInstanceId(DEV_REPLAY_INSTANCE_ID.into())]
    );
    let metadata = registry
        .metadata(ProviderInstanceId(DEV_REPLAY_INSTANCE_ID.into()))
        .await
        .unwrap();
    assert_eq!(metadata.driver.0, "acpRegistry");
    assert!(metadata.enabled);
    let (runtime, mut events, thread) =
        open(registry.as_ref(), None, "multi-turn", "session:before").await;
    assert!(runtime.can_resume_after_restart());
    let thread = turn(&runtime, &mut events, thread, 1, false).await;
    let thread = turn(&runtime, &mut events, thread, 2, true).await;
    let state = serde_json::to_vec(&(runtime.session(), &thread)).unwrap();
    runtime.close().await.unwrap();
    events.close();
    drop(runtime);
    drop(registry);

    let restored = serde_json::from_slice(&state).unwrap();
    let restarted = make_registry("after-restart", observed.clone());
    let (runtime, mut events, resumed) = open(
        restarted.as_ref(),
        Some(restored),
        "multi-turn",
        "session:after",
    )
    .await;
    assert_eq!(resumed.id, thread.id);
    assert_eq!(resumed.native_thread_ref, thread.native_thread_ref);
    let thread = turn(&runtime, &mut events, resumed, 3, false).await;
    assert_eq!(thread.last_run_ordinal, Some(3));
    runtime.close().await.unwrap();
    let outcomes = observed
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match &event.kind {
            ProviderEventKind::Terminal { outcome, .. } => {
                Some(matches!(outcome, TerminalOutcome::Interrupted { .. }))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(outcomes, [false, true, false]);
}
