//! Owner side of guest sharing: create a link for a thread. Ending a share is the existing
//! `thread.guest-share.close` action, and the owner reads shares from the thread projection.

use futures::{FutureExt, future::LocalBoxFuture};
use t3_contracts::{
    guest_share::{GuestShareCreateErrorReason, GuestShareCreateInput},
    rpc::{WsGuestShareCreateError, WsGuestShareCreateRpc},
};

use super::{super::thread_ops::ThreadOperations, actions::Action};
use crate::{
    ClientError, composition::ClientRuntime, platform::Platform, rpc::RpcCallError,
    types::ThreadKey,
};
use t3_contracts::base_schemas::GuestMessageId;

/// What the owner decides for a guest's pending message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestMessageDecision {
    /// Run it as a normal user turn, queued after the active run.
    Send,
    /// Decline it; the guest sees "Not sent".
    Dismiss,
}

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

/// Whether this environment offers "Share with guest" for `key`: the session may create links,
/// the Guest links beta is on (Settings > Labs) and a managed policy has not turned T3 Connect off
/// (guests arrive through it). The thread menu and the command palette share this answer.
pub fn can_share_with_guest<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
    key: &ThreadKey,
) -> bool {
    let snapshot = runtime.store().snapshot();
    let Some(config) = snapshot
        .environments
        .get(&key.environment)
        .and_then(|environment| environment.config.as_ref())
    else {
        return false;
    };
    config.settings.guest_sharing_enabled
        && config
            .managed_policy
            .as_ref()
            .and_then(|policy| policy.limits.as_ref())
            .and_then(|limits| limits.connect)
            != Some(false)
        && runtime
            .client(&key.environment)
            .and_then(|client| client.session())
            .is_some_and(|endpoint| {
                endpoint.available::<WsGuestShareCreateRpc>(&GuestShareCreateInput {
                    thread_id: key.thread.clone(),
                })
            })
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

    /// Send or dismiss one pending guest message (the TS client's `resolveGuestMessage`).
    pub fn resolve_guest_message(
        &self,
        key: ThreadKey,
        message: GuestMessageId,
        decision: GuestMessageDecision,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.perform(
            key,
            match decision {
                GuestMessageDecision::Send => Action::SendGuestMessage(message),
                GuestMessageDecision::Dismiss => Action::DismissGuestMessage(message),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ClientStore,
        composition::Spawner,
        testing::environment::{FakeEnvironment, shell},
    };
    use futures::{executor::LocalPool, task::LocalSpawnExt};
    use serde_json::json;
    use std::rc::Rc;
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

    fn menu_offers_share(enabled: bool, scopes: &[&str]) -> bool {
        let mut pool = LocalPool::new();
        let spawner = pool.spawner();
        let spawn: Spawner =
            Rc::new(move |task| spawner.spawn_local(task).expect("spawn test task"));
        let fake = FakeEnvironment::new("share-tests", spawn.clone());
        fake.set_shell(shell(&[("alpha", "Alpha")]));
        fake.set_permissions(scopes);
        fake.register("guestShare.create", t3_rpc::CallKind::Unary);
        fake.edit_config(|config| config["settings"]["guestSharingEnabled"] = json!(enabled));
        let client = ClientRuntime::new(
            fake.platform(),
            ClientStore::default(),
            OrchestrationV2CreationSource::Web,
            spawn,
        );
        pool.run_until(client.pair(fake.pairing()))
            .expect("pair test server");
        pool.run_until_stalled();
        let key = ThreadKey {
            environment: crate::types::EnvironmentId("share-tests".into()),
            thread: crate::types::ThreadId("alpha".into()),
        };
        client
            .thread_actions()
            .thread_action_menu(&key)
            .items
            .iter()
            .any(|item| item.id == "share-guest")
    }

    const OWNER: [&str; 3] = [
        "orchestration:read",
        "orchestration:operate",
        "access:write",
    ];

    #[test]
    fn the_thread_menu_offers_sharing_only_where_guest_links_work() {
        assert!(menu_offers_share(true, &OWNER));
        assert!(!menu_offers_share(false, &OWNER), "the beta is off");
        assert!(
            !menu_offers_share(true, &["orchestration:read", "orchestration:operate"]),
            "no access:write, no links"
        );
    }
}
