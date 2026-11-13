use super::super::{Registry, register};
use t3_contracts::orchestrator_mcp::*;

pub fn register_module(registry: &mut Registry) {
    register::<OrchestratorMcpCapabilitiesInput>(registry, "OrchestratorMcpCapabilitiesInput");
    register::<OrchestratorMcpCapabilitiesResult>(registry, "OrchestratorMcpCapabilitiesResult");
    register::<OrchestratorMcpClientRequestId>(registry, "OrchestratorMcpClientRequestId");
    register::<OrchestratorMcpCreateThreadRequest>(registry, "OrchestratorMcpCreateThreadRequest");
    register::<OrchestratorMcpCreateThreadsInput>(registry, "OrchestratorMcpCreateThreadsInput");
    register::<OrchestratorMcpCreateThreadsResult>(registry, "OrchestratorMcpCreateThreadsResult");
    register::<OrchestratorMcpCreatedThread>(registry, "OrchestratorMcpCreatedThread");
    register::<OrchestratorMcpCreatedThreadStatus>(registry, "OrchestratorMcpCreatedThreadStatus");
    register::<OrchestratorMcpDelegateTaskInput>(registry, "OrchestratorMcpDelegateTaskInput");
    register::<OrchestratorMcpDelegateTaskResult>(registry, "OrchestratorMcpDelegateTaskResult");
    register::<OrchestratorMcpDelegatedTaskStatus>(registry, "OrchestratorMcpDelegatedTaskStatus");
    register::<OrchestratorMcpDeleteScheduledTaskInput>(
        registry,
        "OrchestratorMcpDeleteScheduledTaskInput",
    );
    register::<OrchestratorMcpDeleteScheduledTaskResult>(
        registry,
        "OrchestratorMcpDeleteScheduledTaskResult",
    );
    register::<OrchestratorMcpEnvironmentLink>(registry, "OrchestratorMcpEnvironmentLink");
    register::<OrchestratorMcpEnvironmentLinkInput>(
        registry,
        "OrchestratorMcpEnvironmentLinkInput",
    );
    register::<OrchestratorMcpEnvironmentLinkResult>(
        registry,
        "OrchestratorMcpEnvironmentLinkResult",
    );
    register::<OrchestratorMcpEnvironmentLinksResult>(
        registry,
        "OrchestratorMcpEnvironmentLinksResult",
    );
    register::<OrchestratorMcpEnvironmentTarget>(registry, "OrchestratorMcpEnvironmentTarget");
    register::<OrchestratorMcpEnvironmentUnlinkInput>(
        registry,
        "OrchestratorMcpEnvironmentUnlinkInput",
    );
    register::<OrchestratorMcpEnvironmentUnlinkResult>(
        registry,
        "OrchestratorMcpEnvironmentUnlinkResult",
    );
    register::<OrchestratorMcpFailure>(registry, "OrchestratorMcpFailure");
    register::<OrchestratorMcpInteractionMode>(registry, "OrchestratorMcpInteractionMode");
    register::<OrchestratorMcpListScheduledTasksInput>(
        registry,
        "OrchestratorMcpListScheduledTasksInput",
    );
    register::<OrchestratorMcpListScheduledTasksResult>(
        registry,
        "OrchestratorMcpListScheduledTasksResult",
    );
    register::<OrchestratorMcpModelOption>(registry, "OrchestratorMcpModelOption");
    register::<OrchestratorMcpProviderCapability>(registry, "OrchestratorMcpProviderCapability");
    register::<OrchestratorMcpRequestSecretInput>(registry, "OrchestratorMcpRequestSecretInput");
    register::<OrchestratorMcpRequestSecretResult>(registry, "OrchestratorMcpRequestSecretResult");
    register::<OrchestratorMcpRuntimeMode>(registry, "OrchestratorMcpRuntimeMode");
    register::<OrchestratorMcpSaveScheduledTaskInput>(
        registry,
        "OrchestratorMcpSaveScheduledTaskInput",
    );
    register::<OrchestratorMcpScheduleTaskInput>(registry, "OrchestratorMcpScheduleTaskInput");
    register::<OrchestratorMcpScheduleTaskResult>(registry, "OrchestratorMcpScheduleTaskResult");
    register::<OrchestratorMcpScheduledTask>(registry, "OrchestratorMcpScheduledTask");
    register::<OrchestratorMcpTarget>(registry, "OrchestratorMcpTarget");
    register::<OrchestratorMcpTargetOptions>(registry, "OrchestratorMcpTargetOptions");
    register::<OrchestratorMcpTaskCancelInput>(registry, "OrchestratorMcpTaskCancelInput");
    register::<OrchestratorMcpTaskCancelResult>(registry, "OrchestratorMcpTaskCancelResult");
    register::<OrchestratorMcpTaskRole>(registry, "OrchestratorMcpTaskRole");
    register::<OrchestratorMcpTaskStatusInput>(registry, "OrchestratorMcpTaskStatusInput");
    register::<OrchestratorMcpTerminalDelegatedTaskStatus>(
        registry,
        "OrchestratorMcpTerminalDelegatedTaskStatus",
    );
    register::<OrchestratorMcpThreadBackgroundTask>(
        registry,
        "OrchestratorMcpThreadBackgroundTask",
    );
    register::<OrchestratorMcpThreadDetail>(registry, "OrchestratorMcpThreadDetail");
    register::<OrchestratorMcpThreadInterruptInput>(
        registry,
        "OrchestratorMcpThreadInterruptInput",
    );
    register::<OrchestratorMcpThreadInterruptResult>(
        registry,
        "OrchestratorMcpThreadInterruptResult",
    );
    register::<OrchestratorMcpThreadListInput>(registry, "OrchestratorMcpThreadListInput");
    register::<OrchestratorMcpThreadListItem>(registry, "OrchestratorMcpThreadListItem");
    register::<OrchestratorMcpThreadListResult>(registry, "OrchestratorMcpThreadListResult");
    register::<OrchestratorMcpThreadPullRequest>(registry, "OrchestratorMcpThreadPullRequest");
    register::<OrchestratorMcpThreadReadInput>(registry, "OrchestratorMcpThreadReadInput");
    register::<OrchestratorMcpThreadReadResult>(registry, "OrchestratorMcpThreadReadResult");
    register::<OrchestratorMcpThreadRun>(registry, "OrchestratorMcpThreadRun");
    register::<OrchestratorMcpThreadSendInput>(registry, "OrchestratorMcpThreadSendInput");
    register::<OrchestratorMcpThreadSendResult>(registry, "OrchestratorMcpThreadSendResult");
    register::<OrchestratorMcpThreadStatus>(registry, "OrchestratorMcpThreadStatus");
    register::<OrchestratorMcpThreadTimelineItem>(registry, "OrchestratorMcpThreadTimelineItem");
    register::<OrchestratorMcpThreadWaitInput>(registry, "OrchestratorMcpThreadWaitInput");
    register::<OrchestratorMcpThreadWaitResult>(registry, "OrchestratorMcpThreadWaitResult");
    register::<OrchestratorMcpUpdateScheduledTaskInput>(
        registry,
        "OrchestratorMcpUpdateScheduledTaskInput",
    );
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("orchestrator_mcp", register_module)
}

#[test]
fn clamps_thread_read_pagination() -> Result<(), serde_json::Error> {
    let decoded: OrchestratorMcpThreadReadInput = serde_json::from_value(serde_json::json!({
        "threadId": "thread", "limit": 101, "runLimit": 51, "maxCharsPerItem": 20001,
    }))?;
    assert_eq!(
        serde_json::to_value(decoded)?,
        serde_json::json!({
            "threadId": "thread", "limit": 100, "runLimit": 50, "maxCharsPerItem": 20000,
        })
    );
    for field in ["limit", "runLimit", "maxCharsPerItem"] {
        let mut value = serde_json::json!({"threadId": "thread"});
        value[field] = serde_json::json!(0);
        assert!(serde_json::from_value::<OrchestratorMcpThreadReadInput>(value).is_err());
    }
    Ok(())
}

#[test]
fn bounds_retry_keys_by_utf16_length() {
    for value in ["a".repeat(257), "😀".repeat(129)] {
        assert!(
            serde_json::from_value::<OrchestratorMcpClientRequestId>(serde_json::json!(value))
                .is_err()
        );
    }
    assert!(
        serde_json::from_value::<OrchestratorMcpClientRequestId>(serde_json::json!(
            "😀".repeat(128)
        ))
        .is_ok()
    );
}

#[test]
fn accepts_alternate_mcp_wire_encodings() -> Result<(), serde_json::Error> {
    let options = serde_json::json!({"effort": "max", "fast": true});
    let decoded: OrchestratorMcpTargetOptions = serde_json::from_value(options.clone())?;
    assert_eq!(serde_json::to_value(decoded)?, options);
    assert!(
        serde_json::from_value::<OrchestratorMcpTargetOptions>(serde_json::json!({"effort": 1}))
            .is_err()
    );
    for schedule in [
        serde_json::json!({"type": "interval", "everyMs": 60000}),
        serde_json::json!(r#"{ "type": "interval", "everyMs": 60000 }"#),
    ] {
        let input = serde_json::json!({"prompt": "task", "schedule": schedule});
        let decoded: OrchestratorMcpScheduleTaskInput = serde_json::from_value(input.clone())?;
        assert_eq!(serde_json::to_value(decoded)?, input);
    }
    assert!(
        serde_json::from_value::<OrchestratorMcpScheduleTaskInput>(
            serde_json::json!({"prompt": "task", "schedule": r#"{"type":"interval"}"#})
        )
        .is_err()
    );
    Ok(())
}
