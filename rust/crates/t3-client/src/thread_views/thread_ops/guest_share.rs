//! Owner side of guest sharing: create a link for a thread. Ending a share is the existing
//! `thread.guest-share.close` action, and the owner reads shares from the thread projection.

use futures::{FutureExt, future::LocalBoxFuture};
use t3_contracts::{
    guest_share::{GuestShareCreateErrorReason, GuestShareCreateInput},
    rpc::{WsGuestShareCreateError, WsGuestShareCreateRpc},
};

use super::super::thread_ops::ThreadOperations;
use crate::{platform::Platform, rpc::RpcCallError, types::ThreadKey};

/// A freshly created invite. The URL carries the one-use credential, so it is not printed.
pub struct GuestShareLink {
    url: String,
}

impl GuestShareLink {
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl std::fmt::Debug for GuestShareLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GuestShareLink(<redacted>)")
    }
}

fn reason_text(reason: &GuestShareCreateErrorReason) -> &'static str {
    match reason {
        GuestShareCreateErrorReason::Disabled => {
            "Guest links are off. Turn them on in Settings > Labs."
        }
        GuestShareCreateErrorReason::ConnectUnavailable => {
            "Guest links need a running T3 Connect tunnel."
        }
        GuestShareCreateErrorReason::ThreadUnavailable => "This thread cannot be shared.",
        GuestShareCreateErrorReason::ReadFailed => "Failed to read the shared thread.",
        GuestShareCreateErrorReason::Changed => {
            "The guest link changed while it was being created."
        }
    }
}

const FALLBACK: &str = "Could not create a guest link.";

impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    /// Create a link for `key`'s thread; a new link replaces the thread's current one.
    pub fn create_guest_share(
        &self,
        key: ThreadKey,
    ) -> LocalBoxFuture<'static, Result<GuestShareLink, String>> {
        let endpoint = self.endpoint(&key.environment);
        async move {
            let endpoint = endpoint.map_err(|_| FALLBACK.to_owned())?;
            match endpoint
                .call_typed::<WsGuestShareCreateRpc>(GuestShareCreateInput {
                    thread_id: key.thread,
                })
                .await
            {
                Ok(result) => Ok(GuestShareLink {
                    url: result.url.0.clone(),
                }),
                Err(RpcCallError::Rejected(errors)) => Err(errors
                    .iter()
                    .find_map(|error| match error {
                        WsGuestShareCreateError::GuestShareCreateError(error) => {
                            Some(reason_text(&error.reason))
                        }
                        WsGuestShareCreateError::EnvironmentAuthorizationError(_) => None,
                    })
                    .unwrap_or(FALLBACK)
                    .to_owned()),
                Err(_) => Err(FALLBACK.to_owned()),
            }
        }
        .boxed_local()
    }
}
