use crate::auth::AuthEnvironmentScope;
use crate::git::{
    GitPreparePullRequestThreadInput, GitPreparePullRequestThreadMode, GitRunStackedActionInput,
    GitStackedAction,
};

macro_rules! guarded_methods {
    ($($name:ident => ($tag:literal, $scope:ident)),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub enum ClientGuardedRpcTag {
            $(#[serde(rename = $tag)] $name,)+
        }
        impl ClientGuardedRpcTag {
            pub const fn tag(self) -> &'static str {
                match self { $(Self::$name => $tag,)+ }
            }
            pub const fn scope(self) -> AuthEnvironmentScope {
                match self { $(Self::$name => AuthEnvironmentScope::$scope,)+ }
            }
        }
        pub const CLIENT_GUARDED_RPC_SCOPES: &[ClientGuardedRpcTag] = &[$(ClientGuardedRpcTag::$name,)+];
    };
}

guarded_methods! {
    ProviderAuthRemoveMethod => ("provider.auth.removeMethod", ProvidersManage),
    PreviewClose => ("preview.close", PreviewOperate),
    PreviewStopLocalServer => ("preview.stopLocalServer", TerminalOperate),
    ServerRunStorageCleanup => ("server.runStorageCleanup", SettingsWrite),
    ServerReclaimDatabaseSpace => ("server.reclaimDatabaseSpace", EnvironmentMaintain),
    ServerCheckOtlpEndpoint => ("server.checkOtlpEndpoint", SettingsWrite),
    ServerCheckSecretSource => ("server.checkSecretSource", SettingsWrite),
    ServerListTextGeneratorModels => ("server.listTextGeneratorModels", SettingsWrite),
    ServerRefreshProviders => ("server.refreshProviders", OrchestrationRead),
    PullRequestsRunAction => ("pullRequests.runAction", SourceControlWrite),
    PullRequestsUpdate => ("pullRequests.update", SourceControlWrite),
    PullRequestsComment => ("pullRequests.comment", SourceControlWrite),
    PullRequestsUpdateComment => ("pullRequests.updateComment", SourceControlWrite),
    PullRequestsSubmitReview => ("pullRequests.submitReview", SourceControlWrite),
    PullRequestsReplyToThread => ("pullRequests.replyToThread", SourceControlWrite),
    PullRequestsSetThreadResolution => ("pullRequests.setThreadResolution", SourceControlWrite),
    PullRequestsSetReaction => ("pullRequests.setReaction", SourceControlWrite),
    PullRequestsSetFilesViewed => ("pullRequests.setFilesViewed", SourceControlWrite),
    PullRequestsRequestReviewers => ("pullRequests.requestReviewers", SourceControlWrite),
    PullRequestsSetLabels => ("pullRequests.setLabels", SourceControlWrite),
    PullRequestsGenerateReviewGuide => ("pullRequests.generateReviewGuide", OrchestrationOperate),
    ProjectsCreateFromThread => ("projects.createFromThread", OrchestrationOperate),
    SourceControlCloneRepository => ("sourceControl.cloneRepository", SourceControlWrite),
    SourceControlPublishRepository => ("sourceControl.publishRepository", SourceControlWrite),
    SourceControlStar => ("sourceControl.star", SourceControlWrite),
    SourceControlSignInStart => ("sourceControl.signInStart", SettingsWrite),
    SourceControlSignInWait => ("sourceControl.signInWait", SettingsWrite),
    SourceControlSignInCancel => ("sourceControl.signInCancel", SettingsWrite),
    ProjectCloneStart => ("projectClone.start", SourceControlWrite),
    ProjectCloneCancel => ("projectClone.cancel", SourceControlWrite),
    ProjectCloneRetry => ("projectClone.retry", SourceControlWrite),
    VcsPull => ("vcs.pull", SourceControlWrite),
    GitRunStackedAction => ("git.runStackedAction", SourceControlWrite),
    GitPreparePullRequestThread => ("git.preparePullRequestThread", SourceControlWrite),
    VcsCreateWorktree => ("vcs.createWorktree", SourceControlWrite),
    VcsRemoveWorktree => ("vcs.removeWorktree", SourceControlWrite),
    VcsCreateRef => ("vcs.createRef", SourceControlWrite),
    VcsSwitchRef => ("vcs.switchRef", SourceControlWrite),
    VcsInit => ("vcs.init", SourceControlWrite),
    VcsTrustRepository => ("vcs.trustRepository", OrchestrationOperate),
    ServerPrepareTriage => ("server.prepareTriage", OrchestrationOperate),
    ScheduledTasksUpsert => ("scheduledTasks.upsert", OrchestrationOperate),
    ScheduledTasksSetEnabled => ("scheduledTasks.setEnabled", OrchestrationOperate),
    ScheduledTasksDelete => ("scheduledTasks.delete", OrchestrationOperate),
    ScheduledTasksRunNow => ("scheduledTasks.runNow", OrchestrationOperate),
    ScheduledTasksRotateWebhookToken => ("scheduledTasks.rotateWebhookToken", OrchestrationOperate),
    StopWorkflow => ("orchestration.stopWorkflow", OrchestrationOperate),
    ForceStopThread => ("orchestration.forceStopThread", OrchestrationOperate),
    GuestShareCreate => ("guestShare.create", AccessWrite),
    PeerLinksLink => ("peerLinks.link", AccessWrite),
    PeerLinksUnlink => ("peerLinks.unlink", AccessWrite),
    PeerLinksAnswerRequest => ("peerLinks.answerRequest", AccessWrite),
}

pub fn client_rpc_required_scope(method: &str) -> Option<AuthEnvironmentScope> {
    CLIENT_GUARDED_RPC_SCOPES
        .iter()
        .find(|entry| entry.tag() == method)
        .map(|entry| entry.scope())
}

pub fn client_rpc_required_scopes(
    method: &str,
    input: Option<&serde_json::Value>,
) -> Result<Vec<AuthEnvironmentScope>, serde_json::Error> {
    if let Some(input) = input {
        if method == ClientGuardedRpcTag::GitPreparePullRequestThread.tag() {
            let payload: GitPreparePullRequestThreadInput = serde_json::from_value(input.clone())?;
            if payload.mode == GitPreparePullRequestThreadMode::Worktree
                && payload.thread_id.is_some()
            {
                return Ok(vec![
                    AuthEnvironmentScope::SourceControlWrite,
                    AuthEnvironmentScope::OrchestrationOperate,
                ]);
            }
        }
        if method == ClientGuardedRpcTag::GitRunStackedAction.tag() {
            let payload: GitRunStackedActionInput = serde_json::from_value(input.clone())?;
            if payload.thread_id.is_some()
                && matches!(
                    payload.action,
                    GitStackedAction::CreatePr | GitStackedAction::CommitPushPr
                )
            {
                return Ok(vec![
                    AuthEnvironmentScope::SourceControlWrite,
                    AuthEnvironmentScope::OrchestrationRead,
                ]);
            }
        }
    }
    Ok(client_rpc_required_scope(method).into_iter().collect())
}
