use crate::model::SourceControlProviderKind;
use serde::{Deserialize, Serialize};
pub type Result<T> = std::result::Result<T, ForgeError>;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureReason {
    MissingTool,
    Unauthenticated,
    Forbidden,
    RateLimited,
    NotFound,
    Failed,
}
#[derive(Debug, Clone, thiserror::Error)]
pub enum ForgeError {
    #[error("Stack operation stopped at PR #{number} after {completed} layers: {detail}")]
    Stack {
        number: u64,
        completed: usize,
        detail: String,
    },
    #[error("GitHub sign-in {reason:?}: {detail}")]
    SignIn {
        host: String,
        reason: t3_contracts::source_control::SourceControlSignInFailure,
        detail: String,
    },
    #[error("{provider:?} failed in {operation}: {detail}")]
    Provider {
        provider: SourceControlProviderKind,
        operation: String,
        reason: FailureReason,
        detail: Box<str>,
        retry_at: Option<u64>,
        access_hint: Option<String>,
        status: Option<u16>,
        diagnostics: Box<[serde_json::Value]>,
    },
    #[error("{provider:?} does not implement {operation}")]
    Unsupported {
        provider: SourceControlProviderKind,
        operation: String,
    },
}
impl ForgeError {
    pub fn repository_hint(&self) -> Option<&'static str> {
        let Self::Provider { detail, .. } = self else {
            return None;
        };
        match detail.as_ref() {
            "Repositories are named workspace/repository." => {
                Some("Repositories are named workspace/repository.")
            }
            "A Bitbucket repository must be workspace/repository." => {
                Some("A Bitbucket repository must be workspace/repository.")
            }
            "Cursor Origin repositories are private. Choose private visibility." => {
                Some("Cursor Origin repositories are private. Choose private visibility.")
            }
            "Specify an Origin repository as owner/name or name." => {
                Some("Specify an Origin repository as owner/name or name.")
            }
            _ => None,
        }
    }
    pub fn provider(
        provider: &str,
        operation: &str,
        reason: FailureReason,
        detail: impl Into<String>,
    ) -> Self {
        Self::Provider {
            provider: SourceControlProviderKind(provider.into()),
            operation: operation.into(),
            reason,
            detail: detail.into().into_boxed_str(),
            retry_at: None,
            access_hint: None,
            status: None,
            diagnostics: Box::default(),
        }
    }
    pub fn unsupported(provider: &SourceControlProviderKind, operation: &str) -> Self {
        Self::Unsupported {
            provider: provider.clone(),
            operation: operation.into(),
        }
    }
    pub fn reason(&self) -> Option<FailureReason> {
        match self {
            Self::Provider { reason, .. } => Some(*reason),
            _ => None,
        }
    }
}
