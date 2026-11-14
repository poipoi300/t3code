use crate::{Forge, forge::*, registry::ForgeHandle};
use async_trait::async_trait;
use std::{path::Path, sync::Arc};
use t3_contracts::source_control::{
    ChangeRequest, ChangeRequestState, SourceControlRepositoryCloneUrls,
};
use t3_git::GitError;

#[async_trait]
pub trait ReviewSetup: Send + Sync {
    async fn setup(
        &self,
        cwd: &Path,
        thread_id: &str,
        worktree: &Path,
    ) -> std::result::Result<(), GitError>;
}

/// Bind after workspace authorization; project setup remains an explicit application dependency.
pub struct ForgeReviewHost {
    forge: Arc<dyn Forge>,
    context: Option<CheckoutContext>,
    setup: Arc<dyn ReviewSetup>,
}
impl ForgeReviewHost {
    pub async fn prepare_pull_request_thread(
        self: &Arc<Self>,
        workflow: t3_git::GitWorkflowService,
        input: &t3_contracts::git::GitPreparePullRequestThreadInput,
    ) -> std::result::Result<t3_contracts::git::GitPreparePullRequestThreadResult, GitError> {
        let cwd = Path::new(&input.cwd.0);
        self.forge
            .validate_checkout_reference(&self.checkout(cwd), &input.reference.0)
            .map_err(|e| self.error("preparePullRequestThread", cwd, e))?;
        workflow
            .with_review_host(self.clone())
            .prepare_pull_request_thread(input)
            .await
    }
    pub fn new(handle: ForgeHandle, setup: Arc<dyn ReviewSetup>) -> Self {
        Self {
            forge: handle.forge,
            context: handle.context,
            setup,
        }
    }
    fn checkout(&self, cwd: &Path) -> CheckoutInput {
        CheckoutInput {
            cwd: cwd.into(),
            context: self.context.clone(),
        }
    }
    fn error(&self, operation: &str, cwd: &Path, error: crate::ForgeError) -> GitError {
        let reason = match error.reason() {
            Some(crate::FailureReason::Unauthenticated | crate::FailureReason::Forbidden) => {
                Some(t3_contracts::git::GitCommandFailureReason::AuthenticationFailed)
            }
            Some(crate::FailureReason::NotFound) => {
                Some(t3_contracts::git::GitCommandFailureReason::RefNotFound)
            }
            _ => None,
        };
        t3_git::CommandError {
            operation: operation.into(),
            command: "forge".into(),
            cwd: cwd.to_string_lossy().into(),
            argument_count: 0,
            exit_code: None,
            reason,
            detail: "The source control provider could not complete the operation.".into(),
            stdout_length: 0,
            stderr_length: 0,
        }
        .into()
    }
}
#[async_trait]
impl t3_git::ReviewHost for ForgeReviewHost {
    async fn list(
        &self,
        cwd: &Path,
        head: &str,
        state: Option<ChangeRequestState>,
        limit: usize,
    ) -> std::result::Result<Vec<ChangeRequest>, GitError> {
        self.forge
            .checkout_list_change_requests(CheckoutListInput {
                checkout: self.checkout(cwd),
                source: None,
                head_selector: head.into(),
                state,
                limit: Some(limit),
            })
            .await
            .map_err(|e| self.error("listPullRequests", cwd, e))
    }
    async fn get(
        &self,
        cwd: &Path,
        reference: &str,
    ) -> std::result::Result<ChangeRequest, GitError> {
        self.forge
            .checkout_get_change_request(CheckoutGetInput {
                checkout: self.checkout(cwd),
                reference: reference.into(),
                force: false,
            })
            .await
            .map_err(|e| self.error("resolvePullRequest", cwd, e))
    }
    async fn checkout(&self, cwd: &Path, reference: &str) -> std::result::Result<(), GitError> {
        self.forge
            .checkout_change_request(CheckoutGetInput {
                checkout: self.checkout(cwd),
                reference: reference.into(),
                force: false,
            })
            .await
            .map_err(|e| self.error("checkoutPullRequest", cwd, e))
    }
    async fn create(
        &self,
        cwd: &Path,
        base: &str,
        head: &str,
        title: &str,
        body_file: &Path,
    ) -> std::result::Result<(), GitError> {
        self.forge
            .create_change_request(CreateChangeRequestInput {
                checkout: self.checkout(cwd),
                source: None,
                target: None,
                base_ref_name: base.into(),
                head_selector: head.into(),
                title: title.into(),
                body_file: body_file.into(),
            })
            .await
            .map_err(|e| self.error("createPullRequest", cwd, e))
    }
    async fn clone_urls(
        &self,
        cwd: &Path,
        repository: &str,
        request_url: &str,
    ) -> std::result::Result<SourceControlRepositoryCloneUrls, GitError> {
        let host = crate::remote::parse_pull_request_url(request_url).map(|p| p.authority);
        self.forge
            .get_repository_clone_urls(CloneUrlsInput {
                checkout: self.checkout(cwd),
                repository: repository.into(),
                host,
            })
            .await
            .map_err(|e| self.error("getRepositoryCloneUrls", cwd, e))
    }
    async fn default_branch(&self, cwd: &Path) -> std::result::Result<Option<String>, GitError> {
        self.forge
            .get_default_branch(self.checkout(cwd))
            .await
            .map_err(|e| self.error("getDefaultBranch", cwd, e))
    }
    fn head_ref(&self, number: u64) -> Option<String> {
        (self.forge.kind().0 == "github").then(|| format!("refs/pull/{number}/head"))
    }
    fn head_branch_probe(
        &self,
        selectors: &[String],
        _state: Option<ChangeRequestState>,
        default: Option<&str>,
    ) -> (Vec<String>, usize) {
        let probe = self.forge.head_branch_probe(selectors, default);
        (probe.head_selectors, probe.limit)
    }
    async fn targets_remote(
        &self,
        cwd: &Path,
        remote: &str,
        repository: Option<&str>,
    ) -> std::result::Result<bool, GitError> {
        if self
            .context
            .as_ref()
            .is_some_and(|c| c.remote_name != remote)
        {
            return Ok(false);
        }
        self.forge
            .creates_change_requests_in(CreatesInInput {
                checkout: self.checkout(cwd),
                repository: repository.map(str::to_string),
            })
            .await
            .map_err(|e| self.error("createsPullRequestsIn", cwd, e))
    }
    async fn setup(
        &self,
        cwd: &Path,
        thread_id: &str,
        worktree: &Path,
    ) -> std::result::Result<(), GitError> {
        self.setup.setup(cwd, thread_id, worktree).await
    }
}
