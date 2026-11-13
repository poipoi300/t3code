use crate::checkpoint_service_test::support::*;
use serde_json::{Value, json};
use t3_orchestration::{api::*, error::Result, ports::checkpoints_forks::thread_fork_service::*};

fn input<'a>(
    source: &'a OrchestrationV2AppThread,
    run: Option<&'a OrchestrationV2Run>,
) -> ForkPlanInput<'a> {
    ForkPlanInput {
        source_thread: source,
        source_run: run,
        source_provider_thread: None,
        source_token_usage: None,
        canonical_source_point: serde_json::from_value(
            json!({"threadId":"thread:test","runId":"run:1"}),
        )
        .expect("source point"),
        transfer_id: ContextTransferId("transfer:test".into()),
        target_thread_id: ThreadId("thread:fork".into()),
        title: Some("Fork".into()),
        side_chat: false,
        created_by: OrchestrationV2Actor::User,
        creation_source: OrchestrationV2CreationSource::Mobile,
        created_at: time(),
    }
}

#[test]
fn fork_resets_thread_local_state_and_preserves_origin_settings_and_source() -> Result<()> {
    let mut value =
        serde_json::to_value(thread("thread:test", Some(std::path::Path::new("/source"))))?;
    for (key,entry) in json!({"snoozedUntil":"2027-01-01T00:00:00.000Z","snoozedAt":time(),"lastSnoozeWakeAt":time(),"pinnedAt":time(),"pinOrderKey":"a0","activeOrderKey":"a1","autoSettleDisabledAt":time(),"delegatedFrom":{"environmentId":"remote","threadId":"remote:parent","title":"Parent"},"historyOrigin":"v1_import","linkOrigin":{"sessionId":"link","label":"Laptop"},"tags":["private"],"titleRegeneration":{"requestId":"title:job","startedAt":time()},"archivedAt":time(),"settledAt":time(),"settledOverride":"active"}).as_object().expect("state"){value[key]=entry.clone();}
    let source: OrchestrationV2AppThread = serde_json::from_value(value.clone())?;
    let source_run = run(1, "completed");
    let planned = plan(input(&source, Some(&source_run)))?;
    let target = serde_json::to_value(planned.target_thread)?;
    for key in [
        "snoozedUntil",
        "snoozedAt",
        "lastSnoozeWakeAt",
        "pinnedAt",
        "pinOrderKey",
        "activeOrderKey",
        "autoSettleDisabledAt",
        "titleRegeneration",
        "activeProviderThreadId",
        "archivedAt",
        "settledAt",
        "settledOverride",
        "guestShare",
        "lastVisitedAt",
        "opens",
        "deletedAt",
    ] {
        assert_eq!(target[key], Value::Null, "{key}");
    }
    assert!(target.get("delegatedFrom").is_none());
    for key in [
        "historyOrigin",
        "linkOrigin",
        "tags",
        "projectId",
        "providerInstanceId",
        "modelSelection",
        "runtimeMode",
        "interactionMode",
        "branch",
        "worktreePath",
    ] {
        assert_eq!(target[key], value[key], "{key}");
    }
    assert_eq!(
        target["lineage"],
        json!({"parentThreadId":"thread:test","relationshipToParent":"fork","rootThreadId":"thread:test"})
    );
    assert_eq!(
        target["forkedFrom"],
        json!({"type":"run","threadId":"thread:test","runId":"run:1"})
    );
    assert_eq!(serde_json::to_value(source)?, value);
    Ok(())
}

#[test]
fn fork_preserves_pull_request_context_without_inheriting_watches() -> Result<()> {
    let mut source = thread("thread:test", None);
    let links:Vec<t3_contracts::thread_pull_request::ThreadPullRequestLink>=["agent","stack","manual","stack-dismissed"].into_iter().enumerate().map(|(index,kind)|serde_json::from_value(json!({"host":"github.com","repository":"example/repo","number":index+1,"url":format!("https://github.com/example/repo/pull/{}",index+1),"source":kind,"linkedAt":time(),"snapshot":null,"stack":null,"watch":{"startedAt":time(),"headSha":"head","failedChecks":[],"passed":false,"passedChecks":[],"remarksThrough":time(),"remarkIds":[],"conflicting":false,"wakes":0}})).expect("link")).collect();
    source.pull_requests = Some(Some(links.clone()));
    let source_run = run(1, "completed");
    let fork = plan(input(&source, Some(&source_run)))?;
    let target = fork.target_thread.pull_requests.flatten().expect("links");
    assert_eq!(target.len(), 4);
    for (mut original, link) in links.into_iter().zip(target) {
        assert!(link.watch.is_none());
        original.watch = None;
        assert_eq!(link, original);
    }
    assert!(
        source
            .pull_requests
            .as_ref()
            .expect("present")
            .as_ref()
            .expect("links")[0]
            .watch
            .is_some()
    );
    Ok(())
}

#[test]
fn fork_only_accepts_terminal_or_waiting_source_runs() -> Result<()> {
    let source = thread("thread:test", None);
    for status in [
        "preparing",
        "queued",
        "starting",
        "running",
        "rolled_back",
        "waiting",
        "completed",
        "failed",
        "interrupted",
        "cancelled",
    ] {
        let run = run(1, status);
        assert_eq!(
            plan(input(&source, Some(&run))).is_ok(),
            matches!(
                status,
                "waiting" | "completed" | "failed" | "interrupted" | "cancelled"
            ),
            "{status}"
        );
    }
    let fork = plan(input(&source, None))?;
    assert!(fork.target_thread.forked_from.is_none());
    Ok(())
}

#[test]
fn side_chat_inherits_permissions_and_rejects_nested_or_delegated_parents() -> Result<()> {
    let run = run(1, "completed");
    for mode in [
        "approval-required",
        "auto-accept-edits",
        "auto",
        "full-access",
    ] {
        let mut source = thread("thread:test", None);
        source.runtime_mode = serde_json::from_value(json!(mode))?;
        let mut request = input(&source, Some(&run));
        request.side_chat = true;
        let fork = plan(request)?;
        assert_eq!(fork.target_thread.runtime_mode, source.runtime_mode);
        assert_eq!(fork.target_thread.lineage.side_chat, Some(Some(true)));
    }
    for state in ["side", "subagent", "delegated"] {
        let mut value = serde_json::to_value(thread("thread:test", None))?;
        match state {
            "side" => value["lineage"]["sideChat"] = true.into(),
            "subagent" => value["lineage"]["relationshipToParent"] = "subagent".into(),
            _ => {
                value["delegatedFrom"] =
                    json!({"environmentId":"remote","threadId":"parent","title":"Parent"})
            }
        };
        let source = serde_json::from_value(value)?;
        let mut request = input(&source, Some(&run));
        request.side_chat = true;
        assert!(plan(request).is_err());
        let fork = plan(input(&source, Some(&run)))?;
        assert!(fork.target_thread.lineage.side_chat.is_none());
    }
    Ok(())
}

#[test]
fn only_successful_strong_native_forks_keep_source_token_usage() -> Result<()> {
    let source = thread("thread:test", None);
    let mut provider = provider();
    let usage: OrchestrationV2ProviderTurnTokenUsage = serde_json::from_value(
        json!({"usedTokens":44,"inputTokens":42,"outputTokens":2,"updatedAt":"now"}),
    )?;
    for status in ["completed", "waiting", "failed", "interrupted", "cancelled"] {
        for strength in [
            OrchestrationV2NativeRefStrength::Strong,
            OrchestrationV2NativeRefStrength::Weak,
        ] {
            provider
                .native_thread_ref
                .as_mut()
                .expect("native ref")
                .strength = strength.clone();
            let run = run(1, status);
            let mut request = input(&source, Some(&run));
            request.source_provider_thread = Some(&provider);
            request.source_token_usage = Some(usage.clone());
            let fork = plan(request)?;
            assert_eq!(
                fork.transfer.source_token_usage.is_some(),
                matches!(status, "completed" | "waiting")
                    && strength == OrchestrationV2NativeRefStrength::Strong
            );
            assert_eq!(
                fork.transfer.error.is_none(),
                strength == OrchestrationV2NativeRefStrength::Strong
            );
        }
    }
    Ok(())
}
