use serde::{Deserialize, Serialize};
use std::fmt;

/// Explicit access is required even when the underlying wire type derives Debug.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sensitive<T>(pub(crate) T);

impl<T> Sensitive<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &T {
        &self.0
    }
    pub fn into_inner(self) -> T {
        self.0
    }
}
impl<T> fmt::Debug for Sensitive<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl<T> fmt::Display for Sensitive<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}
pub type SecretString = Sensitive<String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    #[error("This provider instance is no longer available.")]
    Unavailable,
    #[error("This provider does not support this operation.")]
    Unsupported,
    #[error("Provider setup is already in progress.")]
    Busy,
    #[error("This sign-in is no longer active in this client.")]
    Inactive,
    #[error("This sign-in interaction is no longer available.")]
    Interaction,
    #[error("A sign-in response is already in progress.")]
    ResponseBusy,
    #[error("The provider did not advertise this sign-in method.")]
    Method,
    #[error("Sign-in expired. Start again.")]
    Expired,
    #[error("Sign-in was cancelled.")]
    Cancelled,
    #[error("Could not stop all sessions for this provider. Try again.")]
    StopSessions,
    #[error("Could not read or save provider credentials.")]
    Storage,
    #[error("Provider sign-in could not be verified. Start again.")]
    Identity,
    #[error("This redirect URL does not belong to the active sign-in.")]
    Callback,
    #[error("Provider sign-in is temporarily unavailable. Try again.")]
    Network,
    #[error("The provider returned an invalid response.")]
    Response,
    #[error("The saved connection expired or was disconnected. Sign in again.")]
    Disconnected,
    #[error("Token sharing is disabled. Sign in with ChatGPT and enable token sharing.")]
    Sharing,
    #[error("The connection changed. Start a new session.")]
    ProfileChanged,
    #[error("The provider cannot renew this connection yet. Try again shortly.")]
    RefreshEarly,
    #[error("The provider returned an invalid renewal. Sign in again.")]
    Refresh,
    #[error("This sign-in code expired. Reconnect the saved profile.")]
    CodeExpired,
    #[error("The provider rejected this app's client registration.")]
    Client,
    #[error("Only incomplete connections can be removed.")]
    RemoveMethod,
    #[error(
        "Another provider instance is changing this shared sign-in. Finish or cancel it first."
    )]
    SharedBinding,
    #[error("This provider is disabled.")]
    Disabled,
    #[error("Provider maintenance could not be completed.")]
    Maintenance,
}

pub type ServiceResult<T> = Result<T, ServiceError>;
