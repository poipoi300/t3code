use super::super::Registry;
use serde_json::json;
use t3_contracts::{
    auth::AuthEnvironmentScope::{OrchestrationOperate, OrchestrationRead, SourceControlWrite},
    client_rpc_permissions::client_rpc_required_scopes,
};

pub fn register_module(_registry: &mut Registry) {}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("client_rpc_permissions", register_module)
}

#[test]
fn scopes_depend_on_pull_request_action_and_thread() -> Result<(), serde_json::Error> {
    let prepare = "git.preparePullRequestThread";
    let stacked = "git.runStackedAction";
    for (method, input, expected) in [
        ("future.method", None, vec![]),
        (prepare, None, vec![SourceControlWrite]),
        (stacked, None, vec![SourceControlWrite]),
        (
            prepare,
            Some(json!({"cwd": "repo", "reference": "1", "mode": "worktree"})),
            vec![SourceControlWrite],
        ),
        (
            prepare,
            Some(json!({"cwd": "repo", "reference": "1", "mode": "local", "threadId": "thread"})),
            vec![SourceControlWrite],
        ),
        (
            prepare,
            Some(
                json!({"cwd": "repo", "reference": "1", "mode": "worktree", "threadId": "thread"}),
            ),
            vec![SourceControlWrite, OrchestrationOperate],
        ),
        (
            stacked,
            Some(json!({"actionId": "action", "cwd": "repo", "action": "create_pr"})),
            vec![SourceControlWrite],
        ),
        (
            stacked,
            Some(
                json!({"actionId": "action", "cwd": "repo", "action": "commit_push", "threadId": "thread"}),
            ),
            vec![SourceControlWrite],
        ),
        (
            stacked,
            Some(
                json!({"actionId": "action", "cwd": "repo", "action": "create_pr", "threadId": "thread"}),
            ),
            vec![SourceControlWrite, OrchestrationRead],
        ),
        (
            stacked,
            Some(
                json!({"actionId": "action", "cwd": "repo", "action": "commit_push_pr", "threadId": "thread"}),
            ),
            vec![SourceControlWrite, OrchestrationRead],
        ),
    ] {
        assert_eq!(
            client_rpc_required_scopes(method, input.as_ref())?,
            expected,
            "{method}: {input:?}"
        );
    }
    assert!(client_rpc_required_scopes(prepare, Some(&json!({"mode": "worktree"}))).is_err());
    assert!(client_rpc_required_scopes(stacked, Some(&json!({"action": "unknown"}))).is_err());
    Ok(())
}
