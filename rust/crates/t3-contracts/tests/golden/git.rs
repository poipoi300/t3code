use super::super::{Registry, register};
use t3_contracts::git::*;
pub fn register_module(registry: &mut Registry) {
    register::<GitActionProgressEvent>(registry, "GitActionProgressEvent");
    register::<GitActionProgressKind>(registry, "GitActionProgressKind");
    register::<GitActionProgressPhase>(registry, "GitActionProgressPhase");
    register::<GitActionProgressStream>(registry, "GitActionProgressStream");
    register::<GitCommandError>(registry, "GitCommandError");
    register::<GitCommandFailureReason>(registry, "GitCommandFailureReason");
    register::<GitManagerError>(registry, "GitManagerError");
    register::<GitManagerServiceError>(registry, "GitManagerServiceError");
    register::<GitPreparePullRequestThreadInput>(registry, "GitPreparePullRequestThreadInput");
    register::<GitPreparePullRequestThreadResult>(registry, "GitPreparePullRequestThreadResult");
    register::<GitPullRequestMaterializationError>(registry, "GitPullRequestMaterializationError");
    register::<GitPullRequestRefInput>(registry, "GitPullRequestRefInput");
    register::<GitResolvePullRequestResult>(registry, "GitResolvePullRequestResult");
    register::<GitRunStackedActionInput>(registry, "GitRunStackedActionInput");
    register::<GitRunStackedActionResult>(registry, "GitRunStackedActionResult");
    register::<GitRunStackedActionToastRunAction>(registry, "GitRunStackedActionToastRunAction");
    register::<GitStackedAction>(registry, "GitStackedAction");
    register::<TextGenerationError>(registry, "TextGenerationError");
    register::<VcsBranchStatus>(registry, "VcsBranchStatus");
    register::<VcsCreateRefInput>(registry, "VcsCreateRefInput");
    register::<VcsCreateRefResult>(registry, "VcsCreateRefResult");
    register::<VcsCreateWorktreeInput>(registry, "VcsCreateWorktreeInput");
    register::<VcsCreateWorktreeResult>(registry, "VcsCreateWorktreeResult");
    register::<VcsInitInput>(registry, "VcsInitInput");
    register::<VcsListRefsInput>(registry, "VcsListRefsInput");
    register::<VcsListRefsResult>(registry, "VcsListRefsResult");
    register::<VcsPullInput>(registry, "VcsPullInput");
    register::<VcsPullResult>(registry, "VcsPullResult");
    register::<VcsRef>(registry, "VcsRef");
    register::<VcsRemoveWorktreeInput>(registry, "VcsRemoveWorktreeInput");
    register::<VcsStatusInput>(registry, "VcsStatusInput");
    register::<VcsStatusLocalResult>(registry, "VcsStatusLocalResult");
    register::<VcsStatusRemoteResult>(registry, "VcsStatusRemoteResult");
    register::<VcsStatusResult>(registry, "VcsStatusResult");
    register::<VcsStatusStreamEvent>(registry, "VcsStatusStreamEvent");
    register::<VcsStatusSubscriptionInput>(registry, "VcsStatusSubscriptionInput");
    register::<VcsSwitchRefInput>(registry, "VcsSwitchRefInput");
    register::<VcsSwitchRefResult>(registry, "VcsSwitchRefResult");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("git", register_module)
}

#[test]
fn older_pull_request_results_and_future_failure_reasons_decode() -> Result<(), serde_json::Error> {
    use serde_json::json;
    let value = json!({
     "pullRequest": {"number": 1, "title": "Fix", "url": "https://example.com/1", "baseBranch": "main", "headBranch": "fix", "state": "open"},
     "branch": "fix", "worktreePath": null
    });
    let decoded: GitPreparePullRequestThreadResult = serde_json::from_value(value.clone())?;
    let mut expected = value;
    expected["isOnPullRequestHead"] = json!(true);
    assert_eq!(serde_json::to_value(decoded)?, expected);
    let value = json!({"_tag": "GitCommandError", "operation": "fetch", "command": "git", "cwd": "/repo", "reason": "new-reason", "detail": "failed"});
    let decoded: GitCommandError = serde_json::from_value(value.clone())?;
    let mut expected = value;
    expected.as_object_mut().expect("object").remove("reason");
    assert_eq!(serde_json::to_value(decoded)?, expected);
    Ok(())
}
