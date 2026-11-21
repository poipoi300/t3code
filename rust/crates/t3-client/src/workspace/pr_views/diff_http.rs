use crate::{
    authorization::{
        AuthorizationError, AuthorizedEnvironment, HttpAuthorization, RemoteAuthorization,
        auth_headers,
    },
    platform::{Credentials, HttpRequest, Platform, PlatformError},
    relay::DpopSigner,
};
use futures::{
    FutureExt,
    future::{Either, select},
};
use t3_contracts::{
    environment::{ORCHESTRATION_PROTOCOL_HEADER, ORCHESTRATION_PROTOCOL_VERSION_TEXT},
    environment_http::{
        EnvironmentAuthInvalidError, EnvironmentAuthInvalidReason, EnvironmentHttpCommonError,
    },
    pull_request::{PullRequestDiffInput, PullRequestDiffResult},
};

pub const DIFF_TIMEOUT_MS: u64 = 60_000;
pub const DIFF_MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum DiffLoadError {
    #[error(
        "This environment session is no longer valid (invalid_credential). Refresh the page or quit and reopen T3 Code."
    )]
    CredentialRejected {
        repository: String,
        number: u64,
        trace_id: String,
        cause: EnvironmentAuthInvalidError,
    },
    #[error(transparent)]
    Authorization(#[from] AuthorizationError),
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error("pull request diff request timed out")]
    Timeout,
    #[error("pull request diff HTTP {status}")]
    Http {
        status: u16,
        details: Option<Box<EnvironmentHttpCommonError>>,
    },
    #[error("provider rate limit reached")]
    RateLimited,
    #[error("invalid pull request diff response")]
    InvalidResponse,
    #[error("Pull request diff requires orchestration:read")]
    PermissionDenied,
    #[error("pull request diff session changed")]
    Cancelled,
}

pub struct DiffLoader<'a> {
    pub platform: &'a dyn Platform,
    pub signer: Option<&'a dyn DpopSigner>,
    pub remote_authorization: Option<&'a RemoteAuthorization>,
}
impl DiffLoader<'_> {
    pub async fn load(
        &self,
        prepared: &AuthorizedEnvironment,
        input: &PullRequestDiffInput,
        relay_target: bool,
        current: impl Fn() -> bool,
        timeout_ms: Option<u64>,
    ) -> Result<PullRequestDiffResult, DiffLoadError> {
        let operation = async {
            let mut base = prepared.http_base_url.clone();
            let mut rejected = None;
            loop {
                if !current() {
                    return Err(DiffLoadError::Cancelled);
                }
                let mut authorization = prepared.authorization.clone();
                if matches!(authorization, HttpAuthorization::Dpop { .. }) {
                    let remote = self
                        .remote_authorization
                        .ok_or(AuthorizationError::SessionPending)?;
                    let (renewed_base, renewed) = remote
                        .authorize_dpop_http(&prepared.session, rejected.as_deref())
                        .await?;
                    if !current() {
                        return Err(DiffLoadError::Cancelled);
                    }
                    if relay_target {
                        base = renewed_base;
                    }
                    authorization = renewed;
                }
                let mut url = url::Url::parse(&base).map_err(|_| AuthorizationError::InvalidUrl)?;
                if !matches!(url.scheme(), "http" | "https")
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err(AuthorizationError::InvalidUrl.into());
                }
                url.set_path("/api/pull-requests/diff");
                url.set_query(None);
                url.set_fragment(None);
                let mut headers = auth_headers(&authorization, "POST", url.as_str(), self.signer)
                    .await?
                    .into_inner();
                if !current() {
                    return Err(DiffLoadError::Cancelled);
                }
                headers.insert(
                    ORCHESTRATION_PROTOCOL_HEADER.into(),
                    ORCHESTRATION_PROTOCOL_VERSION_TEXT.into(),
                );
                headers.insert("content-type".into(), "application/json".into());
                let response = self
                    .platform
                    .http(HttpRequest {
                        method: "POST".into(),
                        url: url.into(),
                        headers,
                        body: serde_json::to_vec(input)
                            .map_err(|_| DiffLoadError::InvalidResponse)?,
                        credentials: if matches!(authorization, HttpAuthorization::Cookie) {
                            Credentials::Include
                        } else {
                            Credentials::Omit
                        },
                        max_response_bytes: DIFF_MAX_BYTES,
                    })
                    .await?;
                if !current() {
                    return Err(DiffLoadError::Cancelled);
                }
                if response.body.len() > DIFF_MAX_BYTES {
                    return Err(PlatformError::TooLarge.into());
                }
                if (200..300).contains(&response.status) {
                    return serde_json::from_slice(&response.body)
                        .map_err(|_| DiffLoadError::InvalidResponse);
                }
                if let Ok(cause) =
                    serde_json::from_slice::<EnvironmentAuthInvalidError>(&response.body)
                    && cause.reason == EnvironmentAuthInvalidReason::InvalidCredential
                {
                    if matches!(authorization, HttpAuthorization::Dpop { .. }) && rejected.is_none()
                    {
                        rejected = authorization.access_token().map(str::to_owned);
                        continue;
                    }
                    return Err(DiffLoadError::CredentialRejected {
                        repository: input.repository.0.clone(),
                        number: input.number,
                        trace_id: cause.trace_id.0.clone(),
                        cause,
                    });
                }
                if quota_response(response.status, &response.body) {
                    return Err(DiffLoadError::RateLimited);
                }
                return Err(DiffLoadError::Http {
                    status: response.status,
                    details: serde_json::from_slice(&response.body).ok().map(Box::new),
                });
            }
        }
        .boxed_local();
        match select(
            operation,
            self.platform.sleep(timeout_ms.unwrap_or(DIFF_TIMEOUT_MS)),
        )
        .await
        {
            Either::Left((result, _)) => result,
            Either::Right(_) => Err(DiffLoadError::Timeout),
        }
    }
}

fn quota_response(status: u16, body: &[u8]) -> bool {
    status == 429
        || serde_json::from_slice::<t3_contracts::pull_request::PullRequestOperationError>(body)
            .is_ok_and(|error| quota_reason(&error.detail.0))
}

fn quota_reason(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    [
        "rate limit",
        "rate-limited",
        "rate_limited",
        "too many requests",
    ]
    .iter()
    .any(|phrase| {
        error.match_indices(phrase).any(|(at, found)| {
            !error[..at].chars().next_back().is_some_and(word)
                && !error[at + found.len()..].chars().next().is_some_and(word)
        })
    })
}
/// describePullRequestDiffError, also recognizing the typed HTTP quota answer.
pub fn describe_diff_error(error: &DiffLoadError, provider: &str, term: &str) -> (String, String) {
    if matches!(error, DiffLoadError::RateLimited)
        || matches!(error, DiffLoadError::Platform(PlatformError::Network(reason)) if quota_reason(reason))
    {
        (
            format!("{provider} rate limit reached"),
            format!(
                "{provider} is temporarily limiting requests, so this diff couldn't be loaded. Try again after the limit resets, or view the changes on {provider}."
            ),
        )
    } else {
        (
            "Couldn't load the changes".into(),
            format!(
                "The environment couldn't load this {term}'s diff. Try again, or view the changes on {provider}."
            ),
        )
    }
}
#[cfg(test)]
mod copy_tests {
    use super::*;
    #[test]
    fn provider_quota_and_ordinary_transport_errors() {
        for (why, quota) in [
            (
                "http://127.0.0.1/diff: requests are paused until the rate limit resets",
                true,
            ),
            ("GitHub API rate limit exceeded.", true),
            ("rate_limited", true),
            ("Too many requests", true),
            ("http://localhost/diff: Failed to fetch", false),
            ("separate limitations", false),
        ] {
            let (title, detail) = describe_diff_error(
                &DiffLoadError::Platform(PlatformError::Network(why.into())),
                "GitHub",
                "pull request",
            );
            assert_eq!(
                title,
                if quota {
                    "GitHub rate limit reached"
                } else {
                    "Couldn't load the changes"
                }
            );
            assert_eq!(detail.contains("after the limit resets"), quota);
            assert!(!detail.contains("http://"));
        }
    }
}
