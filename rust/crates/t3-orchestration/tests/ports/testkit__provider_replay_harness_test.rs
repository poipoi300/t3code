use serde_json::json;
use t3_orchestration::testing::replay::*;
use tokio_util::sync::CancellationToken;

#[test]
fn replay_turns_filter_unsafe_requests_setup_and_background() {
    let input = [
        json!({"type":"expect_outbound","frame":{"kind":"request","method":"session/prompt"}}),
        json!({"type":"emit_inbound","frame":{"kind":"request","method":"terminal/create"}}),
        json!({"type":"emit_inbound","frame":{"kind":"request","method":"fs/write_text_file"}}),
        json!({"type":"emit_inbound","frame":{"kind":"request","method":"session/request_permission","params":{}}}),
        json!({"type":"emit_inbound","frame":{"kind":"notification","method":"session/update","params":{"update":{"sessionUpdate":"available_commands_update"}}}}),
        json!({"type":"emit_inbound","frame":{"kind":"notification","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"root"}}}},"afterMs":99}),
        json!({"type":"emit_inbound","frame":{"kind":"response","method":"session/prompt","result":{"stopReason":"refusal"}}}),
        json!({"type":"emit_inbound","frame":{"kind":"notification","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk"}}}}),
    ].iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
    let turns = parse_replay_turns(&input).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].frames.len(), 2);
    assert_eq!(turns[0].frames[1].after_ms, Some(99));
    assert_eq!(turns[0].stop_reason, "refusal");
    assert!(parse_replay_turns(&format!("\u{85}\n{input}")).is_err());
}

#[test]
fn all_dev_scenarios_have_text_and_no_unsafe_effects() {
    for (id, _, input) in DEV_SCENARIOS {
        let turns = parse_replay_turns(input).unwrap();
        assert!(!turns.is_empty(), "{id}");
        assert!(
            turns
                .iter()
                .flat_map(|turn| &turn.frames)
                .any(|entry| entry.frame["params"]["update"]["sessionUpdate"]
                    == "agent_message_chunk"),
            "{id}"
        );
        assert!(
            turns
                .iter()
                .flat_map(|turn| &turn.frames)
                .all(|entry| is_replayed_frame(&entry.frame)),
            "{id}"
        );
    }
    let turns = parse_replay_turns(DEV_SCENARIOS[0].2).unwrap();
    assert!(
        turns[0]
            .frames
            .iter()
            .any(|frame| frame.frame["method"] == "session/request_permission")
    );
    assert_eq!(turns[0].stop_reason, "end_turn");
}

#[test]
fn fixture_registry_decodes_every_driver_and_truncates_only_at_inbound_labels() {
    use t3_orchestration::ports::acp_replay::testkit::{
        fixtures::index::RECORDED_FIXTURES, provider_replay_fixtures::*,
    };
    let mut files = std::collections::BTreeSet::new();
    for driver in DRIVERS_WITH_REPLAY_FIXTURE_FILES {
        let fixtures = replay_fixtures(driver).unwrap();
        assert!(!fixtures.is_empty(), "{driver}");
        for fixture in fixtures {
            assert!(files.insert(fixture.recording.file));
            assert_eq!(
                read_replay_fixture_transcript(fixture.recording.file).unwrap(),
                fixture.transcript
            );
        }
    }
    assert_eq!(files.len(), RECORDED_FIXTURES.len());
    assert!(read_replay_fixture_transcript("../outside.ndjson").is_err());
    assert!(
        find_replay_fixture("not-a-fixture", "acpRegistry")
            .unwrap()
            .is_none()
    );
    let fixture = find_replay_fixture("simple", "acpRegistry")
        .unwrap()
        .unwrap();
    assert_eq!(
        transcript_entries_through_label(fixture.transcript.clone(), None).unwrap(),
        fixture.transcript
    );
    let truncated =
        transcript_entries_through_label(fixture.transcript.clone(), Some("initialized")).unwrap();
    assert_eq!(truncated.entries.len(), 2);
    assert!(
        transcript_entries_through_label(fixture.transcript.clone(), Some("initialize")).is_err()
    );
    assert!(transcript_entries_through_label(fixture.transcript, Some("missing")).is_err());
}

#[test]
fn chunks_preserve_unicode_whitespace_and_empty_text() {
    for text in [
        "",
        " ",
        "  one two three four five\n\n```ts\nx\n```",
        "é 🦀 三 four five\t six",
    ] {
        assert_eq!(stream_chunks(text).concat(), text);
    }
    assert_eq!(
        stream_chunks("one two three four five"),
        ["one two three ", "four five"]
    );
    assert_eq!(
        stream_chunks("  one two three four"),
        ["  one two ", "three four"]
    );
}

#[test]
fn materialization_and_permission_rejection_preserve_recorded_semantics() {
    let recorded: ReplayTranscript = serde_json::from_value(json!({"provider":"codex","protocol":"codex.app-server","version":"1","scenario":"workspace","metadata":{"workspace":"<workspace>"},"entries":[
        {"type":"expect_outbound","frame":{"cwd":"<workspace>","prefix":"<workspace>/file","nested":["<workspace>",{"<workspace>":"<workspace>"}]}},
        {"type":"emit_inbound","frame":{"cwd":"<workspace>"}}
    ]})).unwrap();
    let rebound = materialize_transcript_workspace(recorded, "C:/isolated");
    let rebound = serde_json::to_value(rebound).unwrap();
    assert_eq!(
        rebound["entries"][0]["frame"],
        json!({"cwd":"C:/isolated","prefix":"<workspace>/file","nested":["C:/isolated",{"<workspace>":"C:/isolated"}]})
    );
    assert_eq!(rebound["entries"][1]["frame"]["cwd"], "<workspace>");
    assert_eq!(rebound["metadata"]["workspace"], "<workspace>");
    let request = json!({"method":"session/request_permission","params":{"sessionId":"old","toolCall":{"toolCallId":"edit","path":"<workspace>/x"},"options":[{"optionId":"no","kind":"reject_once"},{"optionId":"yes","kind":"allow_once"}]}});
    let request = materialize(&request, Some("live"), "C:/work", Some("turn-1"));
    assert_eq!(request["params"]["sessionId"], "live");
    assert_eq!(request["params"]["toolCall"]["path"], "C:/work/x");
    assert_eq!(
        permission_rejected_tool_call(
            &request,
            &json!({"outcome":{"outcome":"selected","optionId":"no"}})
        ),
        Some("turn-1-edit".into())
    );
    assert_eq!(
        permission_rejected_tool_call(&request, &json!({"outcome":{"outcome":"cancelled"}})),
        Some("turn-1-edit".into())
    );
    assert_eq!(
        permission_rejected_tool_call(
            &request,
            &json!({"outcome":{"outcome":"selected","optionId":"yes"}})
        ),
        None
    );
}

#[test]
fn strict_recordings_validate_keys_wildcards_and_restart_cursor() {
    assert!(matches_expected(&json!(1), &json!(1.0), ""));
    assert!(!matches_expected(&json!(0), &json!(-0.0), ""));
    assert!(matches_expected(
        &json!(9_007_199_254_740_992_u64),
        &json!(9_007_199_254_740_993_u64),
        ""
    ));
    assert!(matches_expected(
        &json!("prefix<any>suffix"),
        &json!("xx prefix-middle-suffix yy"),
        ""
    ));
    assert!(!matches_expected(
        &json!({"a":1}),
        &json!({"a":1,"b":2}),
        ""
    ));
    let text = "{\"type\":\"transcript_start\",\"provider\":\"acpRegistry\",\"protocol\":\"acp.ndjson-jsonrpc\",\"version\":\"1\",\"scenario\":\"restart\"}\n{\"type\":\"expect_outbound\",\"frame\":{\"kind\":\"request\",\"method\":\"initialize\",\"params\":{}}}\n{\"type\":\"emit_inbound\",\"frame\":{\"kind\":\"response\",\"method\":\"initialize\",\"result\":{}}}\n{\"type\":\"runtime_exit\",\"status\":\"success\"}\n{\"type\":\"expect_outbound\",\"frame\":{\"kind\":\"request\",\"method\":\"initialize\"}}\n{\"type\":\"emit_inbound\",\"frame\":{\"kind\":\"response\",\"method\":\"initialize\",\"result\":{}}}\n{\"type\":\"runtime_exit\",\"status\":\"cancelled\"}";
    let transcript = decode_ndjson(text).unwrap();
    let decorated = text
        .lines()
        .map(|line| format!("\u{feff}\u{a0}{line}\u{2029}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(decode_ndjson(&decorated).unwrap(), transcript);
    assert!(decode_ndjson(&format!("\u{85}\n{text}")).is_err());
    let mut cursor = ReplayCursor::new(transcript.clone(), "".into(), None);
    cursor
        .accept(&json!({"id":"first","method":"initialize","params":{}}))
        .unwrap();
    let messages = cursor.flush_inbound().unwrap();
    assert_eq!(messages[0].1["id"], "first");
    assert_eq!(cursor.status.cursor, 3);
    cursor.close_input().unwrap();
    let mut restarted = ReplayCursor::new(transcript, "".into(), Some(&cursor.status));
    restarted
        .accept(&json!({"id":22,"method":"initialize"}))
        .unwrap();
    assert_eq!(restarted.flush_inbound().unwrap()[0].1["id"], 22);
    restarted.status.assert_complete().unwrap();
    assert!(decode_ndjson("\n").is_err());
    assert!(decode_ndjson("{}").is_err());
    assert!(decode_ndjson(&format!("{text}\n{{\"type\":\"transcript_start\"}}")).is_err());
}

#[tokio::test]
async fn gate_registration_release_cancellation_and_finish_are_lossless() {
    assert!(ReplayGate::new(["a".into(), "a".into()]).is_err());
    let gate = ReplayGate::new(["held".into()]).unwrap();
    let emit = {
        let gate = gate.clone();
        tokio::spawn(async move {
            gate.before_emit(Some("held"), &CancellationToken::new())
                .await
        })
    };
    assert!(gate.wait_for_reached("held").await);
    assert!(!emit.is_finished());
    assert!(gate.release("held"));
    assert!(!gate.release("held"));
    emit.await.unwrap();
    assert!(!gate.wait_for_reached("unknown").await);
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    gate.before_emit(Some("held"), &cancelled).await;
    gate.record_finish_armed(std::time::Duration::from_millis(50));
    assert_eq!(gate.finish_armed_count(), 1);
    assert_eq!(gate.wait_for_finish_armed(0).await.as_millis(), 50);
}

#[tokio::test(start_paused = true)]
async fn coalescing_keeps_insertion_order_snapshots_and_terminal_flush() {
    use t3_orchestration::ports::acp_replay::adapters::provider_text_delta_coalescer::ProviderTextDeltaCoalescer;
    let mut buffer = ProviderTextDeltaCoalescer::new(std::time::Duration::from_millis(50));
    buffer.append("turn", "z", "a");
    buffer.append("turn", "a", "b");
    buffer.append("turn", "z", "c");
    assert!(buffer.flush_due().is_empty());
    tokio::time::advance(std::time::Duration::from_millis(50)).await;
    let updates = buffer.flush_due();
    assert_eq!(
        updates.iter().map(|u| u.text.as_str()).collect::<Vec<_>>(),
        ["ac", "b"]
    );
    assert!(buffer.flush_due().is_empty());
    assert_eq!(
        buffer
            .complete("turn", "z", Some("final"), false)
            .unwrap()
            .text,
        "final"
    );
    let updates = buffer.flush_turn("turn");
    assert_eq!(updates.len(), 1);
    assert!(updates[0].completed);
    assert!(buffer.complete("turn", "empty", None, false).is_none());
}

fn replay_created_event(
    id: &str,
    thread: &str,
) -> t3_contracts::orchestration_v2::OrchestrationV2DomainEvent {
    use t3_orchestration::testing::TEST_TIME;
    serde_json::from_value(json!({"id":id,"threadId":thread,"type":"thread.created","occurredAt":TEST_TIME,"payload":{"id":thread,"projectId":"project:replay","title":"Replay","providerInstanceId":"replay","modelSelection":{"instanceId":"replay","model":"showcase"},"runtimeMode":"approval-required","interactionMode":"default","createdBy":"user","creationSource":"web","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":thread,"parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdAt":TEST_TIME,"updatedAt":TEST_TIME,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null}})).expect("test value")
}

#[tokio::test]
async fn projection_replay_restores_multi_page_log_and_turn_item_positions()
-> t3_orchestration::error::Result<()> {
    use std::sync::Arc;
    use t3_orchestration::{
        ports::acp_replay::testkit::projection_replay::ProjectionReplay,
        projections::write::SqliteProjectionWriter,
        stores::event_store::EventStore,
        testing::{ManualClock, TEST_TIME, TestDatabase, detached_event},
    };
    let database = TestDatabase::temporary()?;
    let replay = ProjectionReplay {
        database: database.stores.clone(),
        writer: Arc::new(SqliteProjectionWriter),
        clock: Arc::new(ManualClock::new(TEST_TIME)),
    };
    database.stores.transaction(|tx| {
        let mut events = vec![replay_created_event("created", "thread:replay")];
        for index in 0..501 { events.push(detached_event(&format!("detach:{index}"),"thread:replay")?); }
        events.push(serde_json::from_value(json!({"id":"item-event","threadId":"thread:replay","type":"turn-item.updated","occurredAt":TEST_TIME,"payload":{"id":"item:replay","threadId":"thread:replay","runId":null,"nodeId":"node:replay","providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":37,"status":"completed","title":null,"startedAt":TEST_TIME,"completedAt":TEST_TIME,"updatedAt":TEST_TIME,"type":"assistant_message","text":"replayed","streaming":false,"messageId":"message:replay"}}))?);
        EventStore::new().append(tx,None,&events)?;
        Ok(())
    }).await?;
    let before = replay.verify().await?;
    assert!(!before.valid);
    assert_eq!(
        before.missing_thread_ids,
        [t3_contracts::base_schemas::ThreadId("thread:replay".into())]
    );
    let rebuilt = replay.rebuild().await?;
    assert!(rebuilt.valid, "{rebuilt:?}");
    assert_eq!(rebuilt.expected_sequence, 503);
    database.stores.read(|db| {
        assert_eq!(db.query_row("SELECT ordinal FROM orchestration_v2_turn_item_positions WHERE turn_item_id='item:replay'",[],|row|row.get::<_,i64>(0))?,37);
        assert!(db.query_row("SELECT payload_json FROM orchestration_v2_projection_turn_items WHERE turn_item_id='item:replay'",[],|row|row.get::<_,String>(0))?.contains("replayed"));
        Ok(())
    }).await?;
    assert_eq!(replay.verify().await?, rebuilt);
    Ok(())
}

#[tokio::test]
async fn projection_replay_detects_unreadable_unexpected_threads_and_rolls_back_failed_rebuild()
-> t3_orchestration::error::Result<()> {
    use std::sync::Arc;
    use t3_orchestration::{
        api::ProjectionWriter,
        error::{Error, Result},
        ports::acp_replay::testkit::projection_replay::ProjectionReplay,
        projections::write::SqliteProjectionWriter,
        stores::event_store::EventStore,
        testing::{ManualClock, TEST_TIME, TestDatabase},
    };
    struct FailedWriter;
    impl ProjectionWriter for FailedWriter {
        fn apply(
            &self,
            _: &t3_persistence::rusqlite::Transaction<'_>,
            _: &t3_contracts::orchestration_v2::OrchestrationV2StoredEvent,
        ) -> Result<()> {
            Err(Error::Unsupported("injected replay failure"))
        }
    }
    let database = TestDatabase::memory()?;
    database
        .stores
        .transaction(|tx| {
            EventStore::new().append(
                tx,
                None,
                &[replay_created_event("created", "thread:replay")],
            )?;
            Ok(())
        })
        .await?;
    let mut replay = ProjectionReplay {
        database: database.stores.clone(),
        writer: Arc::new(SqliteProjectionWriter),
        clock: Arc::new(ManualClock::new(TEST_TIME)),
    };
    assert!(replay.rebuild().await?.valid);
    database.stores.transaction(|tx| {
        let stored = t3_contracts::orchestration_v2::OrchestrationV2StoredEvent { sequence: 1, command_id: None, event: replay_created_event("unexpected","thread:unexpected") };
        SqliteProjectionWriter.apply(tx,&stored)?;
        tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json='{}' WHERE thread_id='thread:replay'",[])?;
        Ok(())
    }).await?;
    let damaged = replay.verify().await?;
    assert!(!damaged.valid);
    assert_eq!(
        damaged.unexpected_thread_ids,
        [t3_contracts::base_schemas::ThreadId(
            "thread:unexpected".into()
        )]
    );
    assert_eq!(
        damaged.unreadable_thread_ids,
        [t3_contracts::base_schemas::ThreadId("thread:replay".into())]
    );
    replay.writer = Arc::new(FailedWriter);
    assert!(replay.rebuild().await.is_err());
    assert_eq!(replay.verify().await?, damaged);
    replay.writer = Arc::new(SqliteProjectionWriter);
    assert!(replay.rebuild().await?.valid);
    Ok(())
}

fn scenario_services(
    gate: ReplayGate,
) -> t3_orchestration::error::Result<(
    t3_orchestration::testing::TestDatabase,
    t3_orchestration::ports::acp_replay::testkit::orchestrator_scenario::ScenarioServices,
)> {
    use std::sync::Arc;
    use t3_orchestration::{
        ports::acp_replay::testkit::orchestrator_scenario::ScenarioServices,
        projections::{store::ProjectionStore, write::SqliteProjectionWriter},
        runtime::{
            commit::EventSink,
            worker::{EffectWorker, OutboxSignals, WorkerOptions},
        },
        testing::{
            FakeCommandDispatcher, FakeEffectExecutor, ManualClock, TEST_TIME, TestDatabase,
        },
    };
    let database = TestDatabase::memory()?;
    let clock = Arc::new(ManualClock::new(TEST_TIME));
    let signals = OutboxSignals::default();
    let services = ScenarioServices {
        database: database.stores.clone(),
        dispatcher: Arc::new(FakeCommandDispatcher::default()),
        committer: Arc::new(EventSink::new(
            database.stores.clone(),
            Arc::new(SqliteProjectionWriter),
            clock.clone(),
            signals.clone(),
        )),
        projections: Arc::new(ProjectionStore::new(database.stores.clone())),
        worker: Arc::new(EffectWorker::new(
            database.stores.clone(),
            Arc::new(FakeEffectExecutor::default()),
            clock.clone(),
            signals,
            WorkerOptions::default(),
        )),
        clock,
        gate,
    };
    Ok((database, services))
}

#[tokio::test(start_paused = true)]
async fn queued_turn_step_waits_for_the_background_dispatch_to_commit()
-> t3_orchestration::error::Result<()> {
    use std::sync::Arc;
    use t3_orchestration::{
        api::*,
        error::Error,
        ports::acp_replay::testkit::orchestrator_scenario::*,
        projections::write::SqliteProjectionWriter,
        stores::{event_store::EventStore, receipts::*},
        testing::TEST_TIME,
    };
    use tokio::sync::Semaphore;

    struct Intake {
        first_entered: Semaphore,
        allow_first_commit: Semaphore,
    }

    #[async_trait::async_trait]
    impl CommandDispatcher for Intake {
        async fn dispatch(
            &self,
            command: OrchestrationV2ServerCommand,
        ) -> Result<CommandReceipt, Error> {
            let command = serde_json::to_value(command)?;
            let first = command["commandId"] == "first";
            if first {
                self.first_entered.add_permits(1);
                self.allow_first_commit
                    .acquire()
                    .await
                    .expect("open gate")
                    .forget();
            } else if !self.allow_first_commit.is_closed() {
                panic!("queued input reached intake before the first dispatch committed");
            }
            if first {
                self.allow_first_commit.close();
            }
            Ok(CommandReceipt {
                command_id: serde_json::from_value(command["commandId"].clone())?,
                aggregate: AggregateId::Thread(ThreadId("thread:replay".into())),
                command_type: "message.dispatch".into(),
                accepted_at: TEST_TIME.into(),
                result_sequence: i64::from(!first) + 1,
                status: ReceiptStatus::Accepted,
                error: None,
            })
        }
    }

    let (database, mut services) = scenario_services(ReplayGate::default())?;
    database
        .stores
        .transaction(|tx| {
            for event in EventStore::new().append(
                tx,
                None,
                &[replay_created_event("created", "thread:replay")],
            )? {
                SqliteProjectionWriter.apply(tx, &event)?;
            }
            Ok(())
        })
        .await?;
    let intake = Arc::new(Intake {
        first_entered: Semaphore::new(0),
        allow_first_commit: Semaphore::new(0),
    });
    services.dispatcher = intake.clone();
    let dispatch = |id: &str, mode: &str, wait| -> Result<ScenarioStep, Error> {
        Ok(ScenarioStep::Dispatch {
            command: Box::new(serde_json::from_value(json!({
                "type":"message.dispatch","commandId":id,"threadId":"thread:replay",
                "messageId":id,"text":id,"attachments":[],"dispatchMode":{"type":mode},
                "createdBy":"user","creationSource":"web"
            }))?),
            wait,
            key: (!wait).then(|| id.to_owned()),
        })
    };
    let run = run_scenario(
        Scenario {
            name: "queued dispatch ordering".into(),
            commands: vec![],
            steps: Some(vec![
                dispatch("first", "start_immediately", false)?,
                dispatch("queued", "queue_after_active", true)?,
                ScenarioStep::Await("first".into()),
            ]),
            projection_thread_ids: vec![],
        },
        &services,
    );
    tokio::pin!(run);
    tokio::select! {
        biased;
        result = &mut run => panic!("scenario ended before the first dispatch committed: {:?}", result.err()),
        entered = intake.first_entered.acquire() => entered.expect("first dispatch reached intake").forget(),
    }
    intake.allow_first_commit.add_permits(1);
    run.await?;
    Ok(())
}

#[tokio::test]
async fn cancelled_scenario_releases_gates_before_teardown() -> t3_orchestration::error::Result<()>
{
    use t3_orchestration::ports::acp_replay::testkit::orchestrator_scenario::*;
    let gate = ReplayGate::new(["held".into()]).unwrap();
    let (_database, services) = scenario_services(gate.clone())?;
    let mut run = Box::pin(run_scenario(
        Scenario {
            name: "cancelled".into(),
            commands: vec![],
            steps: Some(vec![ScenarioStep::AwaitThreadIdle(
                t3_contracts::base_schemas::ThreadId("missing".into()),
            )]),
            projection_thread_ids: vec![],
        },
        &services,
    ));
    tokio::select! { biased; result = &mut run => panic!("scenario completed unexpectedly: {}",result.is_ok()), _ = std::future::ready(()) => {} }
    drop(run);
    assert!(
        !gate.release("held"),
        "scenario cancellation must release held replay frames"
    );
    Ok(())
}

#[tokio::test]
async fn scenario_collects_shell_threads_and_snapshots_after_draining()
-> t3_orchestration::error::Result<()> {
    use t3_orchestration::{
        api::ProjectionWriter, ports::acp_replay::testkit::orchestrator_scenario::*,
        projections::write::SqliteProjectionWriter, stores::event_store::EventStore,
    };
    let (database, services) = scenario_services(ReplayGate::default())?;
    database
        .stores
        .transaction(|tx| {
            for stored in EventStore::new().append(
                tx,
                None,
                &[
                    replay_created_event("created", "thread:replay"),
                    replay_created_event("child-created", "thread:child"),
                ],
            )? {
                SqliteProjectionWriter.apply(tx, &stored)?;
            }
            Ok(())
        })
        .await?;
    let result = run_scenario(
        Scenario {
            name: "snapshot".into(),
            commands: vec![],
            steps: Some(vec![
                ScenarioStep::CaptureShellSnapshot("before".into()),
                ScenarioStep::AwaitThreadIdle(t3_contracts::base_schemas::ThreadId(
                    "thread:replay".into(),
                )),
            ]),
            projection_thread_ids: vec![],
        },
        &services,
    )
    .await?;
    assert!(result.stored_events.is_empty());
    assert_eq!(result.projections.len(), 2);
    assert!(result.projections.contains_key("thread:child"));
    assert_eq!(
        result.captured_shell_snapshots["before"],
        result.shell_snapshot
    );
    Ok(())
}

#[tokio::test]
async fn command_output_recorder_keeps_failed_forwards_and_immutable_snapshots()
-> t3_orchestration::error::Result<()> {
    use std::sync::{Arc, Mutex};
    use t3_contracts::{
        base_schemas::TurnItemId, orchestration_v2::OrchestrationV2SubscribeCommandOutputInput,
    };
    use t3_orchestration::{error::Error, ports::acp_replay::testkit::command_output_recorder::*};
    struct FailingHub(Mutex<Vec<String>>);
    #[async_trait::async_trait]
    impl CommandOutputAppender for FailingHub {
        async fn append(
            &self,
            _target: OrchestrationV2SubscribeCommandOutputInput,
            chunk: String,
        ) -> t3_orchestration::error::Result<()> {
            self.0.lock().unwrap().push(chunk);
            Err(Error::Unsupported("test hub failure"))
        }
    }
    let recorder = CommandOutputRecorder::new(
        String::new(),
        Arc::new(|previous, chunk| format!("{previous}{chunk}")),
    );
    let hub = FailingHub(Mutex::new(Vec::new()));
    let target = OrchestrationV2SubscribeCommandOutputInput {
        thread_id: t3_contracts::base_schemas::ThreadId("thread:replay".into()),
        item_id: TurnItemId("item:stream".into()),
    };
    assert!(
        recorder
            .append(&hub, target.clone(), "before".into())
            .await
            .is_err()
    );
    let first = recorder.snapshot();
    assert!(
        recorder
            .append(&hub, target.clone(), "after".into())
            .await
            .is_err()
    );
    assert_eq!(first[&target.item_id].chunks, 1);
    assert_eq!(first[&target.item_id].output, "before");
    assert_eq!(recorder.snapshot()[&target.item_id].chunks, 2);
    assert_eq!(recorder.snapshot()[&target.item_id].output, "beforeafter");
    assert_eq!(*hub.0.lock().unwrap(), ["before", "after"]);
    use t3_orchestration::ports::transport::command_output_hub::{
        TerminalOutputFold, TerminalText,
    };
    struct RejectingFold;
    impl TerminalOutputFold for RejectingFold {
        fn empty(&self) -> serde_json::Value {
            json!([])
        }
        fn append(
            &self,
            state: &serde_json::Value,
            chunk: &str,
        ) -> t3_orchestration::error::Result<serde_json::Value> {
            if chunk == "reject" {
                return Err(Error::Unsupported("test fold failure"));
            }
            let mut chunks = state.as_array().unwrap().clone();
            chunks.push(json!(chunk));
            Ok(json!(chunks))
        }
        fn normalize(&self, _text: &str) -> t3_orchestration::error::Result<TerminalText> {
            Err(Error::Unsupported("unused fold view"))
        }
        fn resume(
            &self,
            _state: &serde_json::Value,
        ) -> t3_orchestration::error::Result<TerminalText> {
            Err(Error::Unsupported("unused fold view"))
        }
        fn settled(
            &self,
            _state: &serde_json::Value,
        ) -> t3_orchestration::error::Result<TerminalText> {
            Err(Error::Unsupported("unused fold view"))
        }
    }
    let recorder = CommandOutputRecorder::with_terminal_fold(Arc::new(RejectingFold));
    let hub = FailingHub(Mutex::new(Vec::new()));
    assert!(
        recorder
            .append(&hub, target.clone(), "ok".into())
            .await
            .is_err()
    );
    let before = recorder.snapshot();
    assert!(
        recorder
            .append(&hub, target.clone(), "reject".into())
            .await
            .is_err()
    );
    assert_eq!(*hub.0.lock().unwrap(), ["ok"]);
    assert_eq!(recorder.snapshot(), before);
    recorder.record(target.item_id.clone(), "tail")?;
    assert_eq!(before[&target.item_id].output, json!(["ok"]));
    assert_eq!(
        recorder.snapshot()[&target.item_id].output,
        json!(["ok", "tail"])
    );
    assert_eq!(recorder.snapshot()[&target.item_id].chunks, 2);
    Ok(())
}

#[tokio::test]
async fn base_projection_accepts_global_sequences_and_rejects_corrupt_event_lists()
-> t3_orchestration::error::Result<()> {
    use t3_contracts::orchestration_v2::OrchestrationV2StoredEvent;
    use t3_orchestration::{
        api::ProjectionWriter,
        ports::acp_replay::testkit::{
            fixtures::shared::assert_base_projection, orchestrator_scenario::*,
        },
        projections::write::SqliteProjectionWriter,
        stores::event_store::EventStore,
        testing::{TEST_TIME, detached_event},
    };
    let (database, services) = scenario_services(ReplayGate::default())?;
    database
        .stores
        .transaction(|tx| {
            for stored in EventStore::new().append(
                tx,
                None,
                &[replay_created_event("created", "thread:replay")],
            )? {
                SqliteProjectionWriter.apply(tx, &stored)?;
            }
            Ok(())
        })
        .await?;
    let mut result = run_scenario(
        Scenario {
            name: "global-sequences".into(),
            commands: vec![],
            steps: None,
            projection_thread_ids: vec![],
        },
        &services,
    )
    .await?;
    result
        .projections
        .get_mut("thread:replay")
        .unwrap()
        .provider_threads
        .push(serde_json::from_value(json!({
            "id":"provider:replay","driver":"replay","providerInstanceId":"replay",
            "providerSessionId":null,"appThreadId":"thread:replay","ownerNodeId":"node:replay",
            "nativeThreadRef":null,"nativeConversationHeadRef":null,"status":"not_loaded",
            "firstRunOrdinal":null,"lastRunOrdinal":1,"handoffIds":[],"forkedFrom":null,
            "pendingBackgroundTasks":[],"contextUsage":null,"nativeMetadata":null,"goal":null,
            "createdAt":TEST_TIME,"updatedAt":TEST_TIME
        }))?);
    result.domain_events = vec![
        detached_event("first", "thread:replay")?,
        detached_event("second", "thread:replay")?,
    ];
    result.stored_events = result
        .domain_events
        .iter()
        .zip([11, 14])
        .map(|(event, sequence)| OrchestrationV2StoredEvent {
            sequence,
            command_id: None,
            event: event.clone(),
        })
        .collect();
    let transcript = serde_json::from_value(
        json!({"provider":"replay","protocol":"acp.ndjson-jsonrpc","version":"1","scenario":"global-sequences","entries":[]}),
    )?;
    assert_base_projection(&result, &transcript, 0, Some(0), Some(&[]))?;
    for invalid in [0, 11, 10] {
        result.stored_events[1].sequence = invalid;
        assert!(assert_base_projection(&result, &transcript, 0, Some(0), None).is_err());
    }
    result.stored_events[1].sequence = 14;
    result.domain_events.swap(0, 1);
    assert!(assert_base_projection(&result, &transcript, 0, Some(0), None).is_err());
    result.domain_events.swap(0, 1);
    result.domain_events.pop();
    assert!(assert_base_projection(&result, &transcript, 0, Some(0), None).is_err());
    result.stored_events.clear();
    assert!(assert_base_projection(&result, &transcript, 0, Some(0), None).is_err());
    Ok(())
}
