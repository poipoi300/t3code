//! `shell.openInEditor`, `server.checkSecretSource`, `secrets.answerRequest` and `peerLinks.*`
//! (`adapters/host_leftovers.rs`). A part whose domains are absent leaves its methods unserved.
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "host")]
    let adapter = domains.get::<crate::adapters::host_leftovers::HostLeftoversAdapter>();
    #[cfg(not(feature = "host"))]
    let _ = domains;
    let mut editors = false;
    let mut checks = false;
    let mut requests = false;
    let mut peers = false;
    #[cfg(feature = "host")]
    if let Some(adapter) = adapter {
        use crate::registry::{Failure, ServerDefect};
        #[cfg(feature = "workspace")]
        if let Some(launcher) = adapter.editors.clone() {
            editors = true;
            registry.register::<WsShellOpenInEditorRpc, _, _>(move |_, input| {
                let launcher = launcher.clone();
                async move {
                    launcher.launch(&input).await.map_err(|error| {
                        Failure::Typed(WsShellOpenInEditorError::ExternalLauncherError(error))
                    })
                }
            })?;
        }
        #[cfg(feature = "providers")]
        if let Some(secrets) = adapter.secrets.clone() {
            checks = true;
            if let Some(answers) = secrets.requests.clone() {
                requests = true;
                registry.register::<WsSecretsAnswerRequestRpc, _, _>(move |_, input| {
                    let answers = answers.clone();
                    async move {
                        answers.answer(input).await.map_err(|error| {
                            Failure::Typed(WsSecretsAnswerRequestError::SecretRequestError(
                                Box::new(t3_contracts::secret_request::SecretRequestError {
                                    _tag: t3_contracts::secret_request::SecretRequestErrorTag::SecretRequestError,
                                    reason: error.0,
                                    provider: None,
                                    cause: None,
                                }),
                            ))
                        })
                    }
                })?;
            }
            // A read failure is a `Failed` result, never a transport failure.
            registry.register::<WsServerCheckSecretSourceRpc, _, _>(move |_, input| {
                let secrets = secrets.clone();
                async move {
                    secrets.check.check(&input.source).await.map_err(|_| {
                        Failure::Defect(ServerDefect::Internal {
                            message: "Could not check the secret source.".into(),
                        })
                    })
                }
            })?;
        }
        #[cfg(all(feature = "cloud", feature = "project"))]
        if let (Some(links), Some(cloud)) = (
            adapter.peers.clone(),
            domains.get::<crate::adapters::cloud::CloudAdapter>(),
        ) {
            peers = true;
            peer_links::register(registry, links, cloud)?;
        }
    }
    if !editors {
        registry.unimplemented::<WsShellOpenInEditorRpc>()?;
    }
    if !checks {
        registry.unimplemented::<WsServerCheckSecretSourceRpc>()?;
    }
    if !requests {
        registry.unimplemented::<WsSecretsAnswerRequestRpc>()?;
    }
    if !peers {
        registry.unimplemented::<WsPeerLinksListRpc>()?;
        registry.unimplemented::<WsPeerLinksLinkRpc>()?;
        registry.unimplemented::<WsPeerLinksUnlinkRpc>()?;
        registry.unimplemented::<WsPeerLinksAnswerRequestRpc>()?;
    }
    Ok(())
}

/// `peerLinks.*` (`ws.ts`): the RPC guard checks `access:read`/`access:write` first; the peer
/// services check the verified session's scopes again.
#[cfg(all(feature = "host", feature = "cloud", feature = "project"))]
mod peer_links {
    use crate::{
        adapters::{cloud::CloudAdapter, host_leftovers::peers::Peers},
        registry::{Failure, HandlerRegistry},
    };
    use std::sync::Arc;
    use t3_cloud::{Error, Rejection, http::Sensitive};
    use t3_contracts::{
        peer_link::{
            PeerLinkError, PeerLinkErrorReason as Reason, PeerLinkErrorTag, PeerLinkRequestError,
            PeerLinkRequestErrorTag, PeerLinkRequestFailureReason,
        },
        rpc::*,
    };

    const LINKED_ENVIRONMENTS_OFF: &str =
        "Linked environments is off on this machine. Turn it on in Settings → Labs.";

    /// `PeerLinks.ts` reasons. t3-cloud's errors are coarser than TypeScript's, so a message
    /// names the address only where the caller supplied it.
    fn link_error(error: Error, address: Option<&str>) -> PeerLinkError {
        let at = |text: &str| match address {
            Some(address) => format!("{address} {text}"),
            None => format!("The other environment {text}"),
        };
        let (reason, message) = match error {
            Error::Disabled => (Reason::Disabled, LINKED_ENVIRONMENTS_OFF.to_owned()),
            Error::Configuration => (
                Reason::Unreachable,
                at("is not an https address, this machine or a Tailscale address, so the link's credentials would travel unencrypted. Use its https address, or its Tailscale address."),
            ),
            Error::Identity => (
                Reason::Incompatible,
                at("is this environment, already linked at another address, or runs a T3 Code version that cannot keep a linked agent's limits."),
            ),
            Error::Proof
            | Error::Remote {
                rejection: Rejection::Unauthorized | Rejection::Forbidden | Rejection::Rejected,
                ..
            } => (Reason::PairingRejected, at("refused the sign-in.")),
            Error::Protocol => (
                Reason::Incompatible,
                at("answered in a shape this version does not understand. Update both environments."),
            ),
            Error::Expired => (
                Reason::Expired,
                "The link expired. Link it again with a new pairing code.".to_owned(),
            ),
            Error::NotLinked => (
                Reason::UnknownLink,
                "This environment is not linked there. t3_environment_links lists the linked ones."
                    .to_owned(),
            ),
            Error::Storage => {
                tracing::warn!("peer link storage failed");
                (
                    Reason::Storage,
                    "Linked environments could not be read or saved here. The server log has details."
                        .to_owned(),
                )
            }
            _ => (Reason::Unreachable, at("did not answer.")),
        };
        PeerLinkError {
            _tag: PeerLinkErrorTag::PeerLinkError,
            reason,
            message,
            cause: None,
        }
    }

    /// `PeerLinkRequests.ts`: a missing card is `not_found`, a settled one `already_answered`;
    /// a projection that cannot be read or a record the decider refuses fails closed.
    fn request_error(error: Error) -> PeerLinkRequestError {
        let reason = match error {
            Error::NotLinked => PeerLinkRequestFailureReason::NotFound,
            Error::Superseded => PeerLinkRequestFailureReason::AlreadyAnswered,
            Error::Storage => PeerLinkRequestFailureReason::LoadFailed,
            _ => PeerLinkRequestFailureReason::RecordFailed,
        };
        PeerLinkRequestError {
            _tag: PeerLinkRequestErrorTag::PeerLinkRequestError,
            reason,
            cause: None,
        }
    }

    pub fn register(
        registry: &mut HandlerRegistry,
        peers: Arc<Peers>,
        cloud: Arc<CloudAdapter>,
    ) -> crate::Result<()> {
        let (a, c) = (peers.clone(), cloud.clone());
        registry.register::<WsPeerLinksListRpc, _, _>(move |ctx, _| {
            let (a, c) = (a.clone(), c.clone());
            async move {
                let context = c.request_context(&ctx.session_id, &ctx.scopes);
                let list = a.service.list(&context).await.map_err(|error| {
                    Failure::Typed(WsPeerLinksListError::PeerLinkError(Box::new(link_error(
                        error, None,
                    ))))
                })?;
                serde_json::from_value(list).map_err(|_| {
                    Failure::Typed(WsPeerLinksListError::PeerLinkError(Box::new(link_error(
                        Error::Storage,
                        None,
                    ))))
                })
            }
        })?;
        let (a, c) = (peers.clone(), cloud.clone());
        registry.register::<WsPeerLinksLinkRpc, _, _>(move |ctx, input| {
            let (a, c) = (a.clone(), c.clone());
            async move {
                let context = c.request_context(&ctx.session_id, &ctx.scopes);
                let address = input.url.0.clone();
                a
                    .service
                    .link(&context, Sensitive::new(input))
                    .await
                    .map_err(|error| {
                        Failure::Typed(WsPeerLinksListError::PeerLinkError(Box::new(link_error(
                            error,
                            Some(&address),
                        ))))
                    })
            }
        })?;
        let (a, c) = (peers.clone(), cloud.clone());
        registry.register::<WsPeerLinksUnlinkRpc, _, _>(move |ctx, input| {
            let (a, c) = (a.clone(), c.clone());
            async move {
                let context = c.request_context(&ctx.session_id, &ctx.scopes);
                a
                    .service
                    .unlink(&context, input)
                    .await
                    .map_err(|error| {
                        Failure::Typed(WsPeerLinksListError::PeerLinkError(Box::new(link_error(
                            error, None,
                        ))))
                    })
            }
        })?;
        let (a, c) = (peers, cloud);
        registry.register::<WsPeerLinksAnswerRequestRpc, _, _>(move |ctx, input| {
            let (a, c) = (a.clone(), c.clone());
            async move {
                let context = c.request_context(&ctx.session_id, &ctx.scopes);
                a
                    .service
                    .answer_request(&context, Sensitive::new(input))
                    .await
                    .map_err(|error| {
                        Failure::Typed(WsPeerLinksAnswerRequestError::PeerLinkRequestError(
                            Box::new(request_error(error)),
                        ))
                    })
            }
        })?;
        Ok(())
    }
}
