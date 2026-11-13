use t3_contracts::pull_request::*;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureReason {
    MissingTool,
    Unauthenticated,
    Forbidden,
    RateLimited,
    NotFound,
    Failed,
    Unsupported,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{operation}: {detail}")]
pub struct Error {
    pub operation: String,
    pub detail: String,
    pub reason: FailureReason,
    pub transient: bool,
    pub provider: Option<Box<t3_contracts::source_control::SourceControlProviderKind>>,
    pub requirement: Option<Box<str>>,
    pub retry_at: Option<u64>,
    pub access_hint: Option<Box<str>>,
    pub list_result: Option<Box<PullRequestListResult>>,
}
impl Error {
    pub fn invalid(operation: &str, detail: &str) -> Self {
        Self {
            operation: operation.into(),
            detail: detail.into(),
            reason: FailureReason::Failed,
            transient: false,
            provider: None,
            requirement: None,
            retry_at: None,
            access_hint: None,
            list_result: None,
        }
    }
    pub fn unsupported(operation: &str) -> Self {
        Self {
            reason: FailureReason::Unsupported,
            ..Self::invalid(operation, "This host does not support this operation.")
        }
    }
    pub fn is_transient(&self) -> bool {
        self.reason == FailureReason::RateLimited
            || (self.reason == FailureReason::Failed && self.transient)
    }
    pub fn user_detail(&self) -> &str {
        self.requirement.as_deref().unwrap_or(&self.detail)
    }
    pub fn operation_error(&self) -> PullRequestOperationError {
        PullRequestOperationError {
            _tag: PullRequestOperationErrorTag::PullRequestOperationError,
            operation: self.operation.clone(),
            detail: self.detail.clone().into(),
            reason: (self.reason == FailureReason::NotFound)
                .then_some(Some(PullRequestOperationErrorReason::NotFound)),
            list_result: self.list_result.as_ref().map(|r| Some((**r).clone())),
            cause: None,
        }
    }
    pub fn unavailable_error(&self) -> Option<PullRequestUnavailableError> {
        let reason = match self.reason {
            FailureReason::Unsupported => PullRequestUnavailableReason::ProviderUnsupported,
            FailureReason::MissingTool => PullRequestUnavailableReason::CliMissing,
            FailureReason::Unauthenticated => PullRequestUnavailableReason::CliUnauthenticated,
            _ => return None,
        };
        Some(PullRequestUnavailableError {
            _tag: PullRequestUnavailableErrorTag::PullRequestUnavailableError,
            reason,
            provider: self
                .provider
                .as_ref()
                .map(|provider| Some((**provider).clone())),
            list_result: self.list_result.as_ref().map(|r| Some((**r).clone())),
            requirement: self
                .requirement
                .as_ref()
                .map(|value| Some(value.to_string().into())),
            cause: None,
        })
    }
}
