//! Port of packages/client-runtime/src/linkRequest.ts (display, target name, default access,
//! machine list, hint ranking, target resolution, answers, failure copy), of the web's
//! `peerLinkEnvironment.answerRequest` command (`peerLinks.answerRequest`) with its permission,
//! and of `peerLinkEnvironment.list` for the machine step.
//!
//! Not ported: T3 Connect machines (relay discovery's connect-only entries and Connect
//! addresses), and minting a pairing code on a connected machine (`mintPairingCodeOn`): the client
//! has no authorized HTTP client per environment to mint with, so the dialog always asks for a
//! pasted code, the branch the web takes when it cannot mint.

use futures::{FutureExt, future::LocalBoxFuture};
use serde_json::Value;
use t3_contracts::auth::AuthMcpClientAccess;
use t3_contracts::base_schemas::TrimmedNonEmptyString;
use t3_contracts::base_schemas::{ThreadId, TurnItemId};
use t3_contracts::orchestration_v2::{
    OrchestrationV2LinkRequestStatus, ProjectedTurnItemVisibility, TurnItemLinkRequest,
};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_contracts::peer_link::{
    PeerLinkAnswer, PeerLinkAnswerDecline, PeerLinkAnswerDeclineType, PeerLinkAnswerLink,
    PeerLinkAnswerLinkType, PeerLinkAnswerUseExisting, PeerLinkAnswerUseExistingType,
    PeerLinkRequestAnswerInput, PeerLinkStatus, PeerLinkSummary,
};
use t3_contracts::rpc::{Empty, WsPeerLinksAnswerRequestRpc, WsPeerLinksListRpc};

use crate::composition::{ClientRuntime, EnvironmentEntry};
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

/// `linkRequestDefaultAccess`: the agent's suggestion, else supervised.
pub fn link_request_default_access(item: &TurnItemLinkRequest) -> AuthMcpClientAccess {
    item.requested_access
        .clone()
        .flatten()
        .unwrap_or(AuthMcpClientAccess::ApprovalRequired)
}

/// A machine the user could link the thread's environment to (`LinkableMachine`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkableMachine {
    pub environment_id: EnvironmentId,
    pub label: String,
    pub connected: bool,
    /// Found on the user's T3 Connect account rather than saved here (never set: not ported).
    pub connect_only: bool,
    /// The thread's environment already has a link to it.
    pub linked: bool,
    /// Where it answers, most preferred first: its reported addresses, then its saved routes.
    pub urls: Vec<String>,
}

/// `isLocalLoopbackHost` over a URL's host: `localhost`, `::1` or `127.0.0.0/8`.
fn is_loopback_url(url: &str) -> bool {
    match url::Url::parse(url)
        .ok()
        .and_then(|url| url.host().map(|host| host.to_owned()))
    {
        Some(url::Host::Domain(name)) => {
            name.trim_end_matches('.').eq_ignore_ascii_case("localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.octets()[0] == 127,
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

fn origin_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.origin().ascii_serialization())
}

/// `linkableMachines` over the saved catalog: every environment but the thread's own, in
/// catalog order. `reported` gives an environment's own direct addresses (its server config's
/// `directEndpoints`); saved routes follow, relay routes excepted; loopback addresses name this
/// device, not the machine, and are dropped. An expired link no longer counts as linked.
pub fn linkable_machines(
    environments: &[EnvironmentEntry],
    reported: impl Fn(&EnvironmentId) -> Vec<String>,
    thread_environment: &EnvironmentId,
    links: &[PeerLinkSummary],
) -> Vec<LinkableMachine> {
    environments
        .iter()
        .filter(|environment| &environment.id != thread_environment)
        .map(|environment| {
            let mut urls: Vec<String> = Vec::new();
            let routes = environment
                .routes
                .iter()
                .filter(|route| route.mode != crate::connection::ConnectionMode::Relay)
                .map(|route| route.http_base.clone());
            for url in reported(&environment.id).into_iter().chain(routes) {
                if !is_loopback_url(&url) && !urls.contains(&url) {
                    urls.push(url);
                }
            }
            LinkableMachine {
                environment_id: environment.id.clone(),
                label: environment.label.clone(),
                connected: environment.state.phase == crate::connection::Phase::Connected,
                connect_only: false,
                linked: links.iter().any(|link| {
                    link.environment_id == environment.id && link.status != PeerLinkStatus::Expired
                }),
                urls,
            }
        })
        .collect()
}

const NO_MATCH: u8 = 4;

/// Lower-cased words of `text` between single spaces, padded (`words` in linkRequest.ts).
fn words(text: &str) -> String {
    let joined = text
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    format!(" {joined} ")
}

/// How well a machine's name matches the agent's hint: exact, prefix, substring, then a hint
/// that names it among other words.
fn hint_rank(label: &str, hint: &str) -> u8 {
    let name = label.trim().to_lowercase();
    let mut wanted = hint.trim().to_lowercase();
    for article in ["my", "our", "the"] {
        if let Some(rest) = wanted.strip_prefix(article)
            && rest.starts_with(char::is_whitespace)
        {
            wanted = rest.trim_start().to_owned();
            break;
        }
    }
    if name == wanted {
        0
    } else if name.starts_with(&wanted) {
        1
    } else if name.contains(&wanted) {
        2
    } else if words(&wanted).contains(&words(&name)) {
        3
    } else {
        NO_MATCH
    }
}

/// `rankMachinesByHint`: best match for the hint first, each rank in catalog order.
pub fn rank_machines_by_hint(
    machines: Vec<LinkableMachine>,
    hint: Option<&str>,
) -> Vec<LinkableMachine> {
    let Some(hint) = hint.filter(|hint| !hint.trim().is_empty()) else {
        return machines;
    };
    let mut ranked: Vec<_> = machines
        .into_iter()
        .enumerate()
        .map(|(index, machine)| (hint_rank(&machine.label, hint), index, machine))
        .collect();
    ranked.sort_by_key(|(rank, index, _)| (*rank, *index));
    ranked.into_iter().map(|(_, _, machine)| machine).collect()
}

/// `linkRequestInitialMachine`: the machine the agent named, else the hint's best match.
pub fn link_request_initial_machine(
    item: &TurnItemLinkRequest,
    machines: &[LinkableMachine],
) -> Option<String> {
    if let Some(IdOrIndex::Id(id)) = item.environment_id.clone().flatten() {
        return Some(id.0);
    }
    let hint = item.hint.clone().flatten()?;
    let best = rank_machines_by_hint(machines.to_vec(), Some(&hint))
        .into_iter()
        .next()?;
    (hint_rank(&best.label, &hint) < NO_MATCH).then(|| best.environment_id.0)
}

/// The machine choice for linking by an address typed in.
pub const ANOTHER_ADDRESS: &str = "another-address";

/// What a link request links (`LinkRequestTarget`). `urls` is empty when the client knows no
/// address for the machine, so the user types one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkRequestTarget {
    None,
    Machine {
        environment_id: EnvironmentId,
        label: String,
        urls: Vec<String>,
        linked: bool,
    },
    Address,
}

/// `linkRequestTarget`: an address the agent gave is a machine only where exactly one of this
/// client's machines answers there; its pairing code goes to this client's own addresses alone.
pub fn link_request_target(
    item: &TurnItemLinkRequest,
    machines: &[LinkableMachine],
    choice: Option<&str>,
) -> LinkRequestTarget {
    let url = item.url.clone().flatten().map(|url| url.0);
    let chosen = match &url {
        None => match item.environment_id.clone().flatten() {
            Some(IdOrIndex::Id(id)) => Some(id.0),
            _ => choice.map(str::to_owned),
        },
        Some(url) => {
            let asked = origin_of(url);
            let reached: Vec<_> = machines
                .iter()
                .filter(|machine| {
                    asked.is_some()
                        && machine
                            .urls
                            .iter()
                            .any(|candidate| origin_of(candidate) == asked)
                })
                .collect();
            match reached.as_slice() {
                [only] => Some(only.environment_id.0.clone()),
                _ => Some(ANOTHER_ADDRESS.to_owned()),
            }
        }
    };
    let Some(chosen) = chosen else {
        return LinkRequestTarget::None;
    };
    if chosen == ANOTHER_ADDRESS {
        return LinkRequestTarget::Address;
    }
    match machines
        .iter()
        .find(|machine| machine.environment_id.0 == chosen)
    {
        Some(machine) => LinkRequestTarget::Machine {
            environment_id: machine.environment_id.clone(),
            label: machine.label.clone(),
            urls: machine.urls.clone(),
            linked: machine.linked,
        },
        None => LinkRequestTarget::Machine {
            environment_id: EnvironmentId(chosen),
            label: link_request_target_name(item).unwrap_or_else(|| "a machine".to_owned()),
            urls: Vec::new(),
            linked: false,
        },
    }
}

/// Where a `link` answer points: a known machine, or an address typed in its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkAnswerTarget {
    Machine {
        environment_id: EnvironmentId,
        label: String,
        urls: Vec<String>,
    },
    Address(String),
}

/// The answers `linkRequestAnswerInput` builds.
#[derive(Clone, PartialEq, Eq)]
pub enum LinkRequestReply {
    Link {
        target: LinkAnswerTarget,
        access: AuthMcpClientAccess,
        pairing_code: String,
    },
    UseExisting(EnvironmentId),
    Decline,
}

/// `linkRequestAnswerInput`: exactly the target, access and code the user chose; none when the
/// code or the address is empty, or the machine has no address.
pub fn link_request_answer_input(
    item: &TurnItemLinkRequest,
    reply: LinkRequestReply,
) -> Option<PeerLinkRequestAnswerInput> {
    let (IdOrIndex::Id(turn_item_id), IdOrIndex::Id(thread_id)) = (&item.id, &item.thread_id)
    else {
        return None;
    };
    let answer = match reply {
        LinkRequestReply::Decline => PeerLinkAnswer::PeerLinkAnswerDecline(PeerLinkAnswerDecline {
            r#type: PeerLinkAnswerDeclineType::Decline,
        }),
        LinkRequestReply::UseExisting(environment_id) => {
            PeerLinkAnswer::PeerLinkAnswerUseExisting(PeerLinkAnswerUseExisting {
                r#type: PeerLinkAnswerUseExistingType::UseExisting,
                environment_id,
            })
        }
        LinkRequestReply::Link {
            target,
            access,
            pairing_code,
        } => {
            let pairing_code = t3_contracts::wire::trim_js(&pairing_code);
            if pairing_code.is_empty() {
                return None;
            }
            let (environment_id, label, urls) = match target {
                LinkAnswerTarget::Address(url) => {
                    let url = t3_contracts::wire::trim_js(&url);
                    if url.is_empty() {
                        return None;
                    }
                    (None, None, vec![url.to_owned()])
                }
                LinkAnswerTarget::Machine {
                    environment_id,
                    label,
                    urls,
                } => {
                    if urls.is_empty() {
                        return None;
                    }
                    let label = t3_contracts::wire::trim_js(&label);
                    (
                        Some(environment_id),
                        (!label.is_empty()).then(|| TrimmedNonEmptyString(label.to_owned())),
                        urls,
                    )
                }
            };
            PeerLinkAnswer::PeerLinkAnswerLink(PeerLinkAnswerLink {
                r#type: PeerLinkAnswerLinkType::Link,
                environment_id,
                label,
                urls: urls.into_iter().map(TrimmedNonEmptyString).collect(),
                access,
                pairing_code: TrimmedNonEmptyString(pairing_code.to_owned()),
            })
        }
    };
    Some(PeerLinkRequestAnswerInput {
        thread_id: ThreadId(thread_id.0.clone()),
        turn_item_id: TurnItemId(turn_item_id.0.clone()),
        answer,
    })
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
    link_request_answer_input(item, LinkRequestReply::Decline)
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

    /// `useLinkableMachines`: the thread environment's links (`peerLinks.list`), then the saved
    /// catalog's other machines. Links that cannot be read mark no machine linked.
    pub fn machines(
        &self,
        thread_environment: EnvironmentId,
    ) -> LocalBoxFuture<'static, Vec<LinkableMachine>> {
        let runtime = self.runtime.clone();
        let session = runtime
            .client(&thread_environment)
            .and_then(|client| client.session());
        async move {
            let links = match session {
                Some(session) => session
                    .call::<WsPeerLinksListRpc>(Empty {})
                    .await
                    .map(|listed| listed.links)
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            let snapshot = runtime.store().snapshot();
            let reported = |id: &EnvironmentId| {
                snapshot
                    .environments
                    .get(id)
                    .and_then(|environment| environment.config.as_ref())
                    .and_then(|config| config.direct_endpoints.as_ref())
                    .map(|endpoints| {
                        endpoints
                            .0
                            .iter()
                            .map(|endpoint| endpoint.http_base_url.0.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            };
            linkable_machines(
                &runtime.environments(),
                reported,
                &thread_environment,
                &links,
            )
        }
        .boxed_local()
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

    fn machine(id: &str, label: &str, urls: &[&str]) -> LinkableMachine {
        LinkableMachine {
            environment_id: EnvironmentId(id.into()),
            label: label.into(),
            connected: true,
            connect_only: false,
            linked: false,
            urls: urls.iter().map(|url| (*url).to_owned()).collect(),
        }
    }

    #[test]
    fn hints_rank_exact_prefix_substring_then_words_in_catalog_order() {
        let machines = vec![
            machine("a", "Office desktop", &[]),
            machine("b", "Box", &[]),
            machine("c", "Hetzner box", &[]),
            machine("d", "Boxer", &[]),
        ];
        let ranked = rank_machines_by_hint(machines.clone(), Some("my box"));
        let order: Vec<_> = ranked.iter().map(|m| m.environment_id.0.as_str()).collect();
        assert_eq!(order, ["b", "d", "c", "a"]);
        assert_eq!(
            rank_machines_by_hint(machines.clone(), Some("  ")),
            machines
        );
        let named = item(serde_json::json!({"hint":"the office desktop"}));
        assert_eq!(
            link_request_initial_machine(&named, &machines).as_deref(),
            Some("a")
        );
        let among_words = item(serde_json::json!({"hint":"use the office desktop please"}));
        assert_eq!(
            link_request_initial_machine(&among_words, &machines).as_deref(),
            Some("a")
        );
        let unknown = item(serde_json::json!({"hint":"laptop"}));
        assert_eq!(link_request_initial_machine(&unknown, &machines), None);
        let explicit = item(serde_json::json!({"environmentId":"c","hint":"box"}));
        assert_eq!(
            link_request_initial_machine(&explicit, &machines).as_deref(),
            Some("c")
        );
    }

    #[test]
    fn an_agents_address_is_a_machine_only_where_exactly_one_machine_answers_there() {
        let machines = vec![
            machine("a", "Workstation", &["https://work.tail.ts.net"]),
            machine("b", "Laptop", &["http://192.168.1.4:3773"]),
            machine("c", "Other LAN box", &["http://192.168.1.4:3773"]),
        ];
        let known = item(serde_json::json!({"url":"https://work.tail.ts.net/pair?x=1"}));
        assert!(matches!(
            link_request_target(&known, &machines, None),
            LinkRequestTarget::Machine { ref label, .. } if label == "Workstation"
        ));
        let shared = item(serde_json::json!({"url":"http://192.168.1.4:3773"}));
        assert_eq!(
            link_request_target(&shared, &machines, None),
            LinkRequestTarget::Address
        );
        let picks = item(serde_json::json!({}));
        assert_eq!(
            link_request_target(&picks, &machines, None),
            LinkRequestTarget::None
        );
        assert_eq!(
            link_request_target(&picks, &machines, Some(ANOTHER_ADDRESS)),
            LinkRequestTarget::Address
        );
    }

    #[test]
    fn answers_carry_exactly_the_chosen_access_and_refuse_empty_codes() {
        let request = item(serde_json::json!({"requestedAccess":"read-only"}));
        assert_eq!(
            link_request_default_access(&request),
            AuthMcpClientAccess::ReadOnly
        );
        assert_eq!(
            link_request_default_access(&item(serde_json::json!({}))),
            AuthMcpClientAccess::ApprovalRequired
        );
        let link = |code: &str| LinkRequestReply::Link {
            target: LinkAnswerTarget::Machine {
                environment_id: EnvironmentId("a".into()),
                label: "Workstation".into(),
                urls: vec!["https://work.tail.ts.net".into()],
            },
            access: AuthMcpClientAccess::ReadOnly,
            pairing_code: code.into(),
        };
        assert!(link_request_answer_input(&request, link("  ")).is_none());
        let input = link_request_answer_input(&request, link(" ABC-123 ")).expect("link");
        assert_eq!(
            serde_json::to_value(&input.answer).expect("answer"),
            serde_json::json!({"type":"link","environmentId":"a","label":"Workstation",
                "urls":["https://work.tail.ts.net"],"access":"read-only","pairingCode":"ABC-123"})
        );
        let address = LinkRequestReply::Link {
            target: LinkAnswerTarget::Address(" https://box.example ".into()),
            access: AuthMcpClientAccess::FullAccess,
            pairing_code: "code".into(),
        };
        assert_eq!(
            serde_json::to_value(
                link_request_answer_input(&request, address)
                    .expect("address")
                    .answer
            )
            .expect("answer"),
            serde_json::json!({"type":"link","urls":["https://box.example"],"access":"full-access","pairingCode":"code"})
        );
        let existing = link_request_answer_input(
            &request,
            LinkRequestReply::UseExisting(EnvironmentId("a".into())),
        )
        .expect("existing");
        assert_eq!(
            serde_json::to_value(existing.answer).expect("answer"),
            serde_json::json!({"type":"use-existing","environmentId":"a"})
        );
    }

    #[test]
    fn loopback_addresses_are_never_offered() {
        for (url, loopback) in [
            ("http://localhost:3773", true),
            ("http://127.0.0.2", true),
            ("http://[::1]:80", true),
            ("https://box.example", false),
            ("http://192.168.1.2", false),
        ] {
            assert_eq!(is_loopback_url(url), loopback, "{url}");
        }
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
