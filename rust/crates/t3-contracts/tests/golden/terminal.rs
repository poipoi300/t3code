use super::super::{Registry, register};
use t3_contracts::terminal::*;

pub fn register_module(registry: &mut Registry) {
    register::<TerminalAttachInput>(registry, "TerminalAttachInput");
    register::<TerminalAttachStreamEvent>(registry, "TerminalAttachStreamEvent");
    register::<TerminalClearInput>(registry, "TerminalClearInput");
    register::<TerminalCloseInput>(registry, "TerminalCloseInput");
    register::<TerminalCwdError>(registry, "TerminalCwdError");
    register::<TerminalCwdNotDirectoryError>(registry, "TerminalCwdNotDirectoryError");
    register::<TerminalCwdNotFoundError>(registry, "TerminalCwdNotFoundError");
    register::<TerminalCwdStatError>(registry, "TerminalCwdStatError");
    register::<TerminalError>(registry, "TerminalError");
    register::<TerminalEvent>(registry, "TerminalEvent");
    register::<TerminalHistoryError>(registry, "TerminalHistoryError");
    register::<TerminalMetadataStreamEvent>(registry, "TerminalMetadataStreamEvent");
    register::<TerminalNotRunningError>(registry, "TerminalNotRunningError");
    register::<TerminalObserveInput>(registry, "TerminalObserveInput");
    register::<TerminalOpenInput>(registry, "TerminalOpenInput");
    register::<TerminalProviderEnvironmentError>(registry, "TerminalProviderEnvironmentError");
    register::<TerminalProviderInstanceNotFoundError>(
        registry,
        "TerminalProviderInstanceNotFoundError",
    );
    register::<TerminalResizeError>(registry, "TerminalResizeError");
    register::<TerminalResizeInput>(registry, "TerminalResizeInput");
    register::<TerminalSessionLookupError>(registry, "TerminalSessionLookupError");
    register::<TerminalSessionSnapshot>(registry, "TerminalSessionSnapshot");
    register::<TerminalSessionStatus>(registry, "TerminalSessionStatus");
    register::<TerminalSummary>(registry, "TerminalSummary");
    register::<TerminalThreadInput>(registry, "TerminalThreadInput");
    register::<TerminalWriteError>(registry, "TerminalWriteError");
    register::<TerminalWriteInput>(registry, "TerminalWriteInput");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("terminal", register_module)
}

#[test]
fn optional_inputs_keep_null_distinct_and_snapshots_require_nullable_keys()
-> Result<(), serde_json::Error> {
    use serde_json::json;
    for value in [
        json!({"threadId": "thread", "terminalId": "term"}),
        json!({"threadId": "thread", "terminalId": "term", "cols": null, "providerInstanceId": null}),
        json!({"threadId": "thread", "terminalId": "term", "cols": 80, "worktreePath": "/repo"}),
    ] {
        let input: TerminalAttachInput = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(input)?, value);
    }
    let snapshot = json!({
        "threadId": "thread", "terminalId": "term", "cwd": "/repo", "worktreePath": null,
        "status": "running", "pid": null, "history": "", "exitCode": null, "exitSignal": null,
        "label": "shell", "updatedAt": "", "sequence": 9_007_199_254_740_991_u64
    });
    let decoded: TerminalSessionSnapshot = serde_json::from_value(snapshot.clone())?;
    assert_eq!(serde_json::to_value(decoded)?, snapshot);
    for key in ["worktreePath", "pid", "exitCode", "exitSignal"] {
        let mut invalid = snapshot.clone();
        invalid
            .as_object_mut()
            .expect("snapshot is an object")
            .remove(key);
        assert!(
            serde_json::from_value::<TerminalSessionSnapshot>(invalid).is_err(),
            "missing {key}"
        );
    }
    Ok(())
}
