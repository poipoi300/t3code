#![allow(clippy::unwrap_used)]

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use t3_contracts::{base_schemas::*, orchestration_v2::*, provider_instance::ProviderInstanceId};
use t3_orchestration::{
    api::*, decision::engine::PureDecisionEngine, ports::decisions::command_policy::*,
    runtime::dispatch::projection_from_records, testing::TEST_TIME,
};

pub(super) fn change<T: Serialize + DeserializeOwned>(value: &T, updates: Value) -> T {
    let mut value = serde_json::to_value(value).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .extend(updates.as_object().unwrap().clone());
    serde_json::from_value(value).unwrap()
}
pub(super) fn capabilities() -> OrchestrationV2ProviderCapabilities {
    let mut caps: OrchestrationV2ProviderCapabilities = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ProviderCapabilities.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    caps.turns.supports_interrupt = true;
    caps.turns.supports_active_steering = true;
    caps.turns.supports_steering_by_interrupt_restart = true;
    caps.turns.supports_queued_messages = true;
    caps.threads.can_fork_thread = true;
    caps.threads.can_fork_from_turn = true;
    caps.threads.can_rollback_thread = true;
    caps.identity.native_thread_ids = OrchestrationV2NativeRefStrength::Strong;
    caps.checkpointing.provider_can_rollback_conversation = true;
    caps.checkpointing.provider_rollback_returns_snapshot = true;
    caps.context.can_consume_handoff_summaries = true;
    caps.context.accepts_synthetic_user_context = true;
    caps.context.supports_full_thread_handoff = true;
    caps.context.supports_delta_handoff = true;
    caps
}

pub(super) fn context(projection: Option<OrchestrationV2ThreadProjection>) -> DecisionContext {
    DecisionContext {
        projection,
        now: t3_contracts::wire::WireDateTime(TEST_TIME.into()),
        ids: (0..200)
            .map(|index| (IdKind::Event, format!("event:{index}")))
            .chain([(IdKind::ProviderThread, "provider-thread:imported".into())])
            .collect(),
        capabilities: Some(capabilities()),
    }
}

pub(super) fn command(value: Value) -> OrchestrationV2ServerCommand {
    serde_json::from_value(value).unwrap()
}

pub(super) fn projection() -> OrchestrationV2ThreadProjection {
    let command = command(
        json!({"type":"thread.create","commandId":"create","threadId":"thread","projectId":"project","title":"Thread","modelSelection":{"instanceId":"codex","model":"a"},"runtimeMode":"approval-required","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}),
    );
    let decision = PureDecisionEngine.decide(&context(None), &command).unwrap();
    let event = serde_json::to_value(&decision.events[0]).unwrap();
    projection_from_records(ProjectionRecords {
        thread: serde_json::from_value(event["payload"].clone()).unwrap(),
        records: Vec::new(),
    })
}

pub(super) fn run(name: &str, ordinal: u64, status: &str) -> OrchestrationV2Run {
    let sample: OrchestrationV2Run = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2Run.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    change(
        &sample,
        json!({"id":name,"threadId":"thread","ordinal":ordinal,"status":status,"providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"a"},"providerThreadId":"provider-thread","userMessageId":format!("message:{name}"),"rootNodeId":"node","activeAttemptId":"attempt","requestedAt":TEST_TIME,"startedAt":null,"completedAt":null,"checkpointId":null,"contextHandoffId":null,"queuePosition":null,"queueHeld":false,"workspacePreparation":null,"delegatedCompletion":null}),
    )
}

pub(super) fn message(name: &str) -> OrchestrationV2ConversationMessage {
    let sample: OrchestrationV2ConversationMessage = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ConversationMessage.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let mut value = serde_json::to_value(change(&sample, json!({"id":format!("message:{name}"),"threadId":"thread","runId":name,"nodeId":"node","text":"first","attachments":[],"role":"user","createdBy":"user","creationSource":"web"}))).unwrap();
    for key in ["delegatedCompletion", "notification", "context"] {
        value.as_object_mut().unwrap().shift_remove(key);
    }
    serde_json::from_value(value).unwrap()
}

fn active_projection(caps: OrchestrationV2ProviderCapabilities) -> OrchestrationV2ThreadProjection {
    let mut projection = projection();
    projection.runs = vec![run("active", 1, "running")];
    projection.messages = vec![message("active")];
    let thread: OrchestrationV2ProviderThread = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ProviderThread.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    projection.provider_threads = vec![change(
        &thread,
        json!({"id":"provider-thread","providerSessionId":"session"}),
    )];
    let session: OrchestrationV2ProviderSession = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ProviderSession.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    projection.provider_sessions = vec![change(
        &session,
        json!({"id":"session","capabilities":caps}),
    )];
    let turn: OrchestrationV2ProviderTurn = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ProviderTurn.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    projection.provider_turns = vec![change(
        &turn,
        json!({"runAttemptId":"attempt","status":"running"}),
    )];
    projection
}

fn resolved(
    projection: &OrchestrationV2ThreadProjection,
    mode: Value,
    intent: Option<&str>,
) -> Value {
    serde_json::to_value(resolve_message_dispatch_intent(
        projection,
        serde_json::from_value(mode).unwrap(),
        intent,
    ))
    .unwrap()
}

#[test]
fn resolves_automatic_delivery_from_authoritative_capabilities() {
    let start = json!({"type":"start_immediately"});
    assert_eq!(resolved(&projection(), start.clone(), Some("auto")), start);
    let mut caps = capabilities();
    for (steer, queued, restart, expected) in [
        (true, true, true, "steer_active"),
        (false, true, true, "queue_after_active"),
        (false, false, true, "restart_active"),
    ] {
        caps.turns.supports_active_steering = steer;
        caps.turns.supports_queued_messages = queued;
        caps.turns.supports_steering_by_interrupt_restart = restart;
        let mode = resolved(
            &active_projection(caps.clone()),
            start.clone(),
            Some("auto"),
        );
        assert_eq!(mode["type"], expected);
        if expected != "queue_after_active" {
            assert_eq!(mode["targetRunId"], "active");
        }
    }
}

#[test]
fn automatic_delivery_queues_in_handoff_gaps_but_explicit_intents_remain_explicit() {
    for status in ["preparing", "starting"] {
        let mut projection = active_projection(capabilities());
        projection.runs[0] = change(&projection.runs[0], json!({"status":status}));
        assert_eq!(
            resolved(
                &projection,
                json!({"type":"start_immediately"}),
                Some("auto")
            ),
            json!({"type":"queue_after_active"})
        );
    }
    for (status, attempt) in [
        (None, "attempt"),
        (Some("pending"), "attempt"),
        (Some("completed"), "attempt"),
        (Some("running"), "old-attempt"),
    ] {
        let mut projection = active_projection(capabilities());
        if let Some(status) = status {
            projection.provider_turns[0] = change(
                &projection.provider_turns[0],
                json!({"status":status,"runAttemptId":attempt}),
            );
        } else {
            projection.provider_turns.clear();
        }
        for (intent, expected) in [
            ("auto", "queue_after_active"),
            ("steer", "steer_active"),
            ("restart", "restart_active"),
        ] {
            assert_eq!(
                resolved(
                    &projection,
                    json!({"type":"start_immediately"}),
                    Some(intent)
                )["type"],
                expected
            );
        }
    }
}

#[test]
fn providers_without_either_steering_path_queue_steers() {
    let mut caps = capabilities();
    caps.turns.supports_active_steering = false;
    caps.turns.supports_steering_by_interrupt_restart = false;
    let projection = active_projection(caps);
    for intent in [Some("auto"), Some("steer"), None] {
        assert_eq!(
            resolved(
                &projection,
                json!({"type":"steer_active","targetRunId":"active"}),
                intent
            ),
            json!({"type":"queue_after_active"})
        );
    }
    assert_eq!(
        resolved(
            &projection,
            json!({"type":"start_immediately"}),
            Some("restart")
        )["type"],
        "restart_active"
    );
    let missing = json!({"type":"steer_active","targetRunId":"missing"});
    assert_eq!(resolved(&projection, missing.clone(), None), missing);
}

#[test]
fn maintenance_queues_steers_but_not_restarts() {
    for text in ["/compact", " /LOGOUT ", "\u{feff}/compact\u{feff}"] {
        let mut projection = active_projection(capabilities());
        projection.messages[0] = change(&projection.messages[0], json!({"text":text}));
        for intent in [Some("auto"), Some("steer"), None] {
            assert_eq!(
                resolved(
                    &projection,
                    json!({"type":"steer_active","targetRunId":"active"}),
                    intent
                )["type"],
                "queue_after_active"
            );
        }
        assert_eq!(
            resolved(
                &projection,
                json!({"type":"start_immediately"}),
                Some("restart")
            )["type"],
            "restart_active"
        );
    }
    assert!(!is_native_maintenance_command("/compact", 1));
    assert!(!is_native_maintenance_command("\u{85}/compact\u{85}", 0));
}

#[test]
fn capability_checks_preserve_rejection_order_and_details() {
    let command = CommandId("command".into());
    let thread = ThreadId("thread".into());
    let instance = ProviderInstanceId("cursor".into());
    let check = |caps: &OrchestrationV2ProviderCapabilities, restart| {
        CapabilityCheck {
            command_id: &command,
            thread_id: &thread,
            provider_instance_id: &instance,
            capabilities: caps,
        }
        .steering(restart)
    };
    let mut caps = capabilities();
    assert_eq!(
        check(&caps, false).unwrap(),
        SteeringExecution::ActiveSteering
    );
    assert_eq!(
        check(&caps, true).unwrap(),
        SteeringExecution::InterruptRestart
    );
    caps.turns.supports_interrupt = false;
    caps.turns.supports_active_steering = false;
    assert_eq!(
        check(&caps, false).unwrap_err().capability,
        "active_steering"
    );
    assert_eq!(
        check(&caps, true).unwrap_err().detail,
        "providerInstanceId cannot satisfy a required interrupt-and-restart"
    );
}

#[test]
fn fork_policy_uses_portable_context_for_busy_or_unsuccessful_sources() {
    let command = CommandId("command".into());
    let thread = ThreadId("thread".into());
    let instance = ProviderInstanceId("codex".into());
    let mut caps = capabilities();
    for busy in [false, true] {
        let check = CapabilityCheck {
            command_id: &command,
            thread_id: &thread,
            provider_instance_id: &instance,
            capabilities: &caps,
        };
        assert_eq!(
            check
                .fork(ForkPolicy {
                    same_provider: true,
                    strong_native_source: true,
                    source_status: OrchestrationV2RunStatus::Completed,
                    from_specific_turn: true,
                    source_busy: busy
                })
                .unwrap(),
            if busy {
                ForkExecution::PortableContext
            } else {
                ForkExecution::NativeFork
            }
        );
    }
    caps.threads.can_fork_thread = false;
    let check = CapabilityCheck {
        command_id: &command,
        thread_id: &thread,
        provider_instance_id: &instance,
        capabilities: &caps,
    };
    assert_eq!(
        check
            .fork(ForkPolicy {
                same_provider: true,
                strong_native_source: true,
                source_status: OrchestrationV2RunStatus::Completed,
                from_specific_turn: true,
                source_busy: false
            })
            .unwrap(),
        ForkExecution::PortableContext
    );
}

#[test]
fn goal_parser_matches_whitespace_controls_and_attachment_exclusion() {
    for (text, count, expected) in [
        ("/goal", 0, Some("show")),
        (" /goal\u{feff}PAUSE ", 0, Some("pause")),
        ("/goal\nnew objective", 0, Some("set")),
        ("/goal resume", 0, Some("resume")),
        ("/goal clear", 0, Some("clear")),
        ("/goal edit", 0, Some("show")),
        ("/goalie", 0, None),
        ("/GOAL pause", 0, None),
        ("/goal pause", 1, None),
    ] {
        assert_eq!(goal_command_type(text, count), expected, "{text}");
    }
}
