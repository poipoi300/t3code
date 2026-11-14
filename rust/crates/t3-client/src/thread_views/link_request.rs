//! Port of the parts of packages/client-runtime/src/linkRequest.ts that LinkRequestCard's
//! inline card uses (display, target name, the decline answer, failure copy), and of the web's
//! `peerLinkEnvironment.answerRequest` command (`peerLinks.answerRequest`) with its permission.
//!
//! Not ported yet: the review dialog's machine list (`linkableMachines`, `rankMachinesByHint`,
//! `linkRequestTarget`, `linkRequestInitialMachine`), pairing-code minting and the `link` and
//! `use-existing` answers.

use futures::{FutureExt, future::LocalBoxFuture};
use serde_json::Value;
use t3_contracts::auth::AuthMcpClientAccess;
use t3_contracts::base_schemas::{ThreadId, TurnItemId};
use t3_contracts::orchestration_v2::{
    OrchestrationV2LinkRequestStatus, ProjectedTurnItemVisibility, TurnItemLinkRequest,
};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_contracts::peer_link::{
    PeerLinkAnswer, PeerLinkAnswerDecline, PeerLinkAnswerDeclineType, PeerLinkRequestAnswerInput,
};
use t3_contracts::rpc::WsPeerLinksAnswerRequestRpc;

use crate::composition::ClientRuntime;
use crate::platform::Platform;
use crate::rpc::{RpcCallError, RpcEndpoint};
use crate::types::EnvironmentId;

pub const LINK_REQUEST_FAILURE: &str = "Could not answer the request. Try again.";

/// ConnectAgentSurface.tsx `accessConfig[access].label` (runtimeModeConfig.ts for the modes).
pub fn access_label(access: &AuthMcpClientAccess) -> &'static str {
    match access {
        AuthMcpClientAccess::ReadOnly => "Read only",
        AuthMcpClientAccess::ApprovalRequired => "Supervised",
        AuthMcpClientAccess::AutoAcceptEdits => "Auto-accept edits",
        AuthMcpClientAccess::Auto => "Auto",
        AuthMcpClientAccess::FullAccess => "Full access",
    }
}

/// `linkDestination`: the http(s) origin of an address, else the address trimmed.
pub fn link_destination(address: &str) -> String {
    url::Url::parse(address)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|| t3_contracts::wire::trim_js(address).to_owned())
}

/// `linkRequestTargetName`; `None` is the client sentence "a machine".
pub fn link_request_target_name(item: &TurnItemLinkRequest) -> Option<String> {
    item.url
        .clone()
        .flatten()
        .map(|url| link_destination(&url.0))
}

/// `picksMachine`: the agent left the machine for the user to pick.
pub fn link_request_picks_machine(item: &TurnItemLinkRequest) -> bool {
    item.environment_id.clone().flatten().is_none() && item.url.clone().flatten().is_none()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkRequestOutcome {
    Linked,
    Declined,
    Ended,
    Failed,
}

/// What the card shows. `Linked` and `Failed` carry content (names, the server's failure);
/// the others are client keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkRequestDisplay {
    Pending,
    PendingElsewhere,
    Linked {
        target: Option<String>,
        access: Option<AuthMcpClientAccess>,
    },
    Declined,
    Failed {
        failure: Option<String>,
    },
    Ended,
}

impl LinkRequestDisplay {
    pub fn outcome(&self) -> Option<LinkRequestOutcome> {
        match self {
            Self::Pending | Self::PendingElsewhere => None,
            Self::Linked { .. } => Some(LinkRequestOutcome::Linked),
            Self::Declined => Some(LinkRequestOutcome::Declined),
            Self::Failed { .. } => Some(LinkRequestOutcome::Failed),
            Self::Ended => Some(LinkRequestOutcome::Ended),
        }
    }
}

/// `linkRequestDisplay`. `visibility` is the projected row's: a request inherited from another
/// thread can only be answered where it was asked.
pub fn link_request_display(
    item: &TurnItemLinkRequest,
    visibility: &ProjectedTurnItemVisibility,
) -> LinkRequestDisplay {
    use OrchestrationV2LinkRequestStatus as S;
    match item.link_status {
        S::Pending if *visibility == ProjectedTurnItemVisibility::Local => {
            LinkRequestDisplay::Pending
        }
        S::Pending => LinkRequestDisplay::PendingElsewhere,
        S::Linked => LinkRequestDisplay::Linked {
            target: item
                .linked_label
                .clone()
                .flatten()
                .or_else(|| link_request_target_name(item)),
            access: item.linked_access.clone().flatten(),
        },
        S::Declined => LinkRequestDisplay::Declined,
        S::Failed => LinkRequestDisplay::Failed {
            failure: item.failure.clone().flatten(),
        },
        S::Cancelled => LinkRequestDisplay::Ended,
    }
}

/// `linkRequestAnswerInput(item, { type: "decline" })`.
pub fn link_request_decline_input(
    item: &TurnItemLinkRequest,
) -> Option<PeerLinkRequestAnswerInput> {
    let (IdOrIndex::Id(turn_item_id), IdOrIndex::Id(thread_id)) = (&item.id, &item.thread_id)
    else {
        return None;
    };
    Some(PeerLinkRequestAnswerInput {
        thread_id: ThreadId(thread_id.0.clone()),
        turn_item_id: TurnItemId(turn_item_id.0.clone()),
        answer: PeerLinkAnswer::PeerLinkAnswerDecline(PeerLinkAnswerDecline {
            r#type: PeerLinkAnswerDeclineType::Decline,
        }),
    })
}

fn peer_link_reason_message(error: &Value) -> Option<String> {
    Some(
        match error.get("reason")?.as_str()? {
            "load_failed" => "Could not load the link request.",
            "not_found" => "This link request no longer exists.",
            "already_answered" => "This link request was already answered.",
            "record_failed" => "Could not update the link request.",
            _ => return None,
        }
        .to_owned(),
    )
}

/// `linkRequestFailureMessage`: known server errors pass their message through; anything else
/// gets the generic copy.
pub fn link_request_failure_message(error: &RpcCallError) -> String {
    let RpcCallError::Rejected(errors) = error else {
        return LINK_REQUEST_FAILURE.to_owned();
    };
    errors
        .first()
        .and_then(|error| match error.get("_tag").and_then(Value::as_str) {
            Some("PeerLinkRequestError") => peer_link_reason_message(error),
            Some("EnvironmentAuthorizationError" | "PeerPairingCodeError") => error
                .get("message")
                .and_then(Value::as_str)
                .filter(|message| !message.trim().is_empty())
                .map(str::to_owned),
            _ => None,
        })
        .unwrap_or_else(|| LINK_REQUEST_FAILURE.to_owned())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkAnswerError {
    Interrupted,
    Failed(String),
}

/// `peerLinkEnvironment.answerRequest` and its `permissionAtom`.
#[derive(Clone)]
pub struct LinkRequests<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
}

impl<P: Platform + Clone + 'static> LinkRequests<P> {
    pub fn new(runtime: ClientRuntime<P>) -> Self {
        Self { runtime }
    }

    /// Whether this session may answer (`peerLinks.answerRequest`'s guarded scope).
    pub fn can_answer(
        &self,
        environment: &EnvironmentId,
        input: &PeerLinkRequestAnswerInput,
    ) -> bool {
        self.runtime
            .client(environment)
            .and_then(|client| client.session())
            .is_some_and(|session| session.available::<WsPeerLinksAnswerRequestRpc>(input))
    }

    pub fn answer(
        &self,
        environment: EnvironmentId,
        input: PeerLinkRequestAnswerInput,
    ) -> LocalBoxFuture<'static, Result<(), LinkAnswerError>> {
        let session = self
            .runtime
            .client(&environment)
            .and_then(|client| client.session());
        async move {
            let Some(session) = session else {
                return Err(LinkAnswerError::Failed(LINK_REQUEST_FAILURE.to_owned()));
            };
            match session.command::<WsPeerLinksAnswerRequestRpc>(input).await {
                Ok(()) => Ok(()),
                Err(RpcCallError::Interrupted) => Err(LinkAnswerError::Interrupted),
                Err(error) => Err(LinkAnswerError::Failed(link_request_failure_message(
                    &error,
                ))),
            }
        }
        .boxed_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(extra: Value) -> TurnItemLinkRequest {
        let mut value = serde_json::json!({
            "id":"item-1","threadId":"thread-1","type":"link_request","runId":null,"nodeId":null,
            "providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
            "ordinal":0,"status":"waiting","title":null,"startedAt":null,"completedAt":null,
            "updatedAt":"2026-11-14T00:00:00.000Z","reason":"","linkStatus":"pending"
        });
        value
            .as_object_mut()
            .expect("item")
            .extend(extra.as_object().expect("extra").clone());
        serde_json::from_value(value).expect("link request item")
    }

    #[test]
    fn display_follows_status_and_the_original_thread() {
        let pending = item(serde_json::json!({}));
        assert_eq!(
            link_request_display(&pending, &ProjectedTurnItemVisibility::Local),
            LinkRequestDisplay::Pending
        );
        assert_eq!(
            link_request_display(&pending, &ProjectedTurnItemVisibility::Inherited),
            LinkRequestDisplay::PendingElsewhere
        );
        let linked = item(serde_json::json!({
            "linkStatus":"linked","url":"https://user@box.tail.ts.net:8443/path","linkedAccess":"read-only"
        }));
        assert_eq!(
            link_request_display(&linked, &ProjectedTurnItemVisibility::Local),
            LinkRequestDisplay::Linked {
                target: Some("https://box.tail.ts.net:8443".into()),
                access: Some(AuthMcpClientAccess::ReadOnly)
            }
        );
        let failed = item(serde_json::json!({"linkStatus":"failed"}));
        assert_eq!(
            link_request_display(&failed, &ProjectedTurnItemVisibility::Local),
            LinkRequestDisplay::Failed { failure: None }
        );
    }

    #[test]
    fn the_machine_is_picked_only_when_the_agent_named_none() {
        assert!(link_request_picks_machine(&item(serde_json::json!({}))));
        assert!(!link_request_picks_machine(&item(
            serde_json::json!({"url":"https://box.example"})
        )));
        assert_eq!(link_request_target_name(&item(serde_json::json!({}))), None);
    }

    #[test]
    fn failures_pass_known_server_errors_only() {
        let known = RpcCallError::Rejected(vec![serde_json::json!({
            "_tag":"PeerLinkRequestError","reason":"already_answered"
        })]);
        assert_eq!(
            link_request_failure_message(&known),
            "This link request was already answered."
        );
        let other = RpcCallError::Rejected(vec![serde_json::json!({
            "_tag":"SchemaError","message":"pairingCode: whsec"
        })]);
        assert_eq!(link_request_failure_message(&other), LINK_REQUEST_FAILURE);
        let declined = link_request_decline_input(&item(serde_json::json!({}))).expect("input");
        assert_eq!(
            serde_json::to_value(declined.answer).expect("answer"),
            serde_json::json!({"type":"decline"})
        );
    }
}
