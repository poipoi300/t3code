use t3_contracts::{auth::EnvironmentAuthorizationError, base_schemas::DpopFailureReason};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CredentialFailure {
    #[error("Malformed session token.")]
    MalformedSessionTokenError,
    #[error("Invalid session token signature.")]
    InvalidSessionTokenSignatureError,
    #[error("Invalid session token payload.")]
    InvalidSessionTokenPayloadError,
    #[error("Session token expired.")]
    SessionTokenExpiredError,
    #[error("Unknown session token.")]
    UnknownSessionTokenError,
    #[error("Session token revoked.")]
    SessionTokenRevokedError,
    #[error("Invalid `exp` claim")]
    InvalidSessionExpirationClaimError,
    #[error("Malformed websocket token.")]
    MalformedWebSocketTokenError,
    #[error("Invalid websocket token signature.")]
    InvalidWebSocketTokenSignatureError,
    #[error("Invalid websocket token payload.")]
    InvalidWebSocketTokenPayloadError,
    #[error("Websocket token expired.")]
    WebSocketTokenExpiredError,
    #[error("Unknown websocket session.")]
    UnknownWebSocketSessionError,
    #[error("Websocket session expired.")]
    WebSocketSessionExpiredError,
    #[error("Websocket session revoked.")]
    WebSocketSessionRevokedError,
    #[error("Unknown bootstrap credential.")]
    UnknownBootstrapCredentialError,
    #[error("Bootstrap credential expired.")]
    ExpiredBootstrapCredentialError,
    #[error("Bootstrap credential proof key mismatch.")]
    BootstrapCredentialProofKeyMismatchError,
    #[error("Bootstrap credential is no longer available.")]
    UnavailableBootstrapCredentialError,
    #[error("The requested authentication scope was not granted.")]
    BootstrapCredentialScopeNotGrantedError,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Authentication operation task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Persistence(#[from] t3_persistence::Error),
    #[error("Failed to {operation} secret {name}.")]
    Secret {
        operation: &'static str,
        name: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Configured authentication key or secret cannot be empty.")]
    EmptySecret,
    #[error("Invalid HMAC signing key.")]
    InvalidSigningKey,
    #[error("Failed to generate random bytes: {0}")]
    Random(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Invalid date: {0}")]
    Date(String),
    #[error("{kind}")]
    Credential {
        kind: CredentialFailure,
        session_id: Option<String>,
        expires_at: Option<String>,
        observed_at: Option<String>,
        revoked_at: Option<String>,
    },
    #[error("Server authentication credential is missing.")]
    MissingCredential,
    #[error("Server authentication credential is invalid.")]
    InvalidCredential {
        diagnostic: String,
        dpop_failure_reason: Option<DpopFailureReason>,
    },
    #[error("The requested authentication scope is invalid.")]
    InvalidScope,
    #[error("The requested authentication scope was not granted.")]
    ScopeNotGranted,
    #[error("The current authentication session cannot revoke itself.")]
    SelfRevocation,
    #[error("{0}")]
    PolicyRefused(String),
    #[error("{0}")]
    UnknownRpc(String),
    #[error("{0:?}")]
    Authorization(EnvironmentAuthorizationError),
    #[error("{reason}")]
    McpApprovalCode { reason: McpApprovalReason },
    #[error("{0}")]
    ApprovalPage(ApprovalPage),
    #[error("The session could not be issued.")]
    OAuthSessionIssue {
        #[source]
        source: Box<Error>,
    },
    #[error("{description}")]
    OAuth {
        error: &'static str,
        description: String,
    },
    #[error("Invalid decoded RPC payload: {0}")]
    InvalidPayload(String),
    #[error("Authentication event stream closed")]
    EventStreamClosed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum McpApprovalReason {
    #[error(
        "That pairing code cannot grant this access. It has not been used, so choose an access level it grants, or create a code with the standard scopes."
    )]
    InsufficientScope,
    #[error("That is not a one-time pairing code.")]
    NotAPairingCode,
    #[error("That pairing code is unknown, expired, or already used.")]
    UnknownOrUsed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalPage {
    #[error("Linking needs a plain pairing code, not an app approval code.")]
    PeerLinkNeedsPairingCode,
    #[error("Your session cannot approve this request. Enter a pairing code instead.")]
    SessionCannotApprove,
}

impl Error {
    pub fn credential(kind: CredentialFailure, session_id: Option<&str>) -> Self {
        Self::Credential {
            kind,
            session_id: session_id.map(str::to_owned),
            expires_at: None,
            observed_at: None,
            revoked_at: None,
        }
    }
    pub fn invalid(diagnostic: impl Into<String>) -> Self {
        Self::InvalidCredential {
            diagnostic: diagnostic.into(),
            dpop_failure_reason: None,
        }
    }
    pub fn is_credential(&self) -> bool {
        matches!(
            self,
            Self::Credential { .. } | Self::InvalidCredential { .. } | Self::MissingCredential
        )
    }
    pub fn invalid_session(self) -> Self {
        if self.is_credential() {
            Self::invalid(self.to_string())
        } else {
            self
        }
    }
    pub fn is_already_exists(&self) -> bool {
        matches!(self, Self::Secret { source, .. } if source.kind() == std::io::ErrorKind::AlreadyExists)
    }
}
