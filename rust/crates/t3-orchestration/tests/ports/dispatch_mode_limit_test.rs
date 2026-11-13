use super::{
    command_policy_test::{change, command, projection, run},
    orchestrator_receipt_race_test::Harness,
};
use serde_json::json;
use std::collections::HashMap;
use t3_contracts::{
    base_schemas::ThreadId,
    orchestration_v2::OrchestrationV2DomainEvent,
    provider_policy::{ProviderInteractionMode::*, RuntimeMode::*},
};
use t3_orchestration::{api::*, ports::decisions::dispatch_mode_limit::*, testing::TEST_TIME};

fn supervised() -> DispatchModes {
    DispatchModes {
        runtime_mode: ApprovalRequired,
        interaction_mode: Default,
    }
}
fn ceiling() -> RuntimeModeCeiling {
    RuntimeModeCeiling {
        organization: "Example Corp".into(),
        max_runtime_mode: AutoAcceptEdits,
    }
}

#[tokio::test]
async fn caller_limit_is_rechecked_inside_lane_and_refusal_does_not_poison_retry() {
    let harness = Harness::new(None, false);
    harness.create("thread").await.unwrap();
    harness.dispatch(json!({"type":"thread.runtime-mode.set","commandId":"raise","threadId":"thread","runtimeMode":"full-access"})).await.unwrap();
    let rename = command(
        json!({"type":"thread.metadata.update","commandId":"rename","threadId":"thread","title":"After"}),
    );
    assert!(
        harness
            .dispatcher
            .dispatch_with_limit(rename.clone(), Some(supervised()))
            .await
            .is_err()
    );
    assert_eq!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap()
            .title
            .0,
        "Before"
    );
    assert!(
        harness
            .database
            .stores
            .read(|db| t3_orchestration::stores::receipts::get(db, &CommandId("rename".into())))
            .await
            .unwrap()
            .is_none()
    );
    harness.dispatch(json!({"type":"thread.runtime-mode.set","commandId":"lower","threadId":"thread","runtimeMode":"approval-required"})).await.unwrap();
    harness
        .dispatcher
        .dispatch_with_limit(rename, Some(supervised()))
        .await
        .unwrap();
    assert_eq!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap()
            .title
            .0,
        "After"
    );
}

#[tokio::test]
async fn managed_ceiling_refuses_creation_and_mode_raises_without_recording_receipts() {
    let harness = Harness::new(Some(ceiling()), false);
    let create = json!({"type":"thread.create","commandId":"full","threadId":"thread","projectId":"project","title":"Full","modelSelection":{"instanceId":"codex","model":"a"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"});
    assert!(
        harness
            .dispatch(create)
            .await
            .unwrap_err()
            .to_string()
            .ends_with("Example Corp limits threads to Auto-accept edits.")
    );
    assert!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await
            .unwrap()
            .is_none()
    );
    harness.create("thread").await.unwrap();
    let raise = json!({"type":"thread.runtime-mode.set","commandId":"raise","threadId":"thread","runtimeMode":"auto"});
    assert!(harness.dispatch(raise).await.is_err());
    harness.dispatch(json!({"type":"thread.runtime-mode.set","commandId":"allowed","threadId":"thread","runtimeMode":"auto-accept-edits"})).await.unwrap();
    assert_eq!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap()
            .runtime_mode,
        AutoAcceptEdits
    );
}

#[tokio::test]
async fn housekeeping_above_ceiling_remains_open_and_full_access_callers_are_narrowed() {
    let harness = Harness::new(Some(ceiling()), false);
    harness.create("thread").await.unwrap();
    let thread = change(
        &harness
            .reader
            .thread(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap(),
        json!({"runtimeMode":"full-access"}),
    );
    let event:OrchestrationV2DomainEvent=serde_json::from_value(json!({"id":"raised-behind-policy","type":"thread.runtime-mode-updated","threadId":"thread","occurredAt":TEST_TIME,"payload":thread})).unwrap();
    harness
        .sink
        .commit(CommitRequest {
            receipt: None,
            decision: Decision {
                events: vec![event],
                ..Decision::default()
            },
            guard: CommitGuard::None,
        })
        .await
        .unwrap();
    let rename = command(
        json!({"type":"thread.metadata.update","commandId":"rename","threadId":"thread","title":"After"}),
    );
    assert!(
        harness
            .dispatcher
            .dispatch_with_limit(
                rename.clone(),
                Some(DispatchModes {
                    runtime_mode: FullAccess,
                    interaction_mode: Default
                })
            )
            .await
            .is_err()
    );
    harness.dispatcher.dispatch(rename).await.unwrap();
    harness.dispatch(json!({"type":"thread.runtime-mode.set","commandId":"lower","threadId":"thread","runtimeMode":"approval-required"})).await.unwrap();
    assert_eq!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap()
            .title
            .0,
        "After"
    );
}

#[test]
fn mode_rank_and_ceiling_follow_events_in_order_and_include_run_attempts() {
    assert_eq!(
        exceeded_dispatch_mode_limit(
            supervised(),
            DispatchModes {
                runtime_mode: FullAccess,
                interaction_mode: Plan
            }
        ),
        Some(ModeLimit::Runtime)
    );
    assert_eq!(
        exceeded_dispatch_mode_limit(
            DispatchModes {
                runtime_mode: FullAccess,
                interaction_mode: Plan
            },
            supervised()
        ),
        Some(ModeLimit::Interaction)
    );
    assert_eq!(
        narrow_to_ceiling(
            Some(DispatchModes {
                runtime_mode: FullAccess,
                interaction_mode: Plan
            }),
            Some(&ceiling())
        )
        .unwrap()
        .runtime_mode,
        AutoAcceptEdits
    );
    assert!(narrow_to_ceiling(None, Some(&ceiling())).is_none());
    let mut modes = HashMap::new();
    modes.insert(ThreadId("thread".into()), FullAccess);
    let run = run("run", 1, "starting");
    let started:OrchestrationV2DomainEvent=serde_json::from_value(json!({"id":"run-created","type":"run.created","threadId":"thread","occurredAt":TEST_TIME,"payload":run})).unwrap();
    assert_eq!(
        runtime_mode_ceiling_refusal(AutoAcceptEdits, std::slice::from_ref(&started), &modes)
            .unwrap()
            .refused,
        CeilingRefused::Turn
    );
    assert!(
        threads_starting_work(std::slice::from_ref(&started)).contains(&ThreadId("thread".into()))
    );
    let created:OrchestrationV2DomainEvent=serde_json::from_value(json!({"id":"created","type":"thread.created","threadId":"thread","occurredAt":TEST_TIME,"payload":projection().thread})).unwrap();
    assert!(
        runtime_mode_ceiling_refusal(AutoAcceptEdits, &[created.clone(), started.clone()], &modes)
            .is_none()
    );
    assert!(threads_starting_work(&[created, started]).is_empty());
}
