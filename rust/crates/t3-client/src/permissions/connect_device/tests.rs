use super::*;
use crate::{
    authorization::{AuthorizationError, AuthorizedEnvironment, HttpAuthorization, Sensitive},
    environment::SessionIdentity,
    permissions::EnvironmentHttpClient,
    testing::FakePlatform,
    types::{EnvironmentId, Generation},
};
use futures::{FutureExt, executor::block_on};
use serde_json::{Value, json};
use std::rc::Rc;

fn http_client(platform: &FakePlatform) -> EnvironmentHttpClient {
    EnvironmentHttpClient::new(
        Rc::new(platform.clone()),
        AuthorizedEnvironment {
            session: SessionIdentity {
                environment: EnvironmentId("env".into()),
                generation: Generation(1),
                account: None,
                dpop_thumbprint: None,
            },
            http_base_url: "https://host.test".into(),
            socket_url: Sensitive("wss://host.test/ws".into()),
            authorization: HttpAuthorization::Cookie,
            descriptor: None,
        },
        None,
        None,
        false,
    )
}

#[test]
fn approval_commands_use_access_timeout() {
    let platform = Rc::new(crate::authorization::tests::TestPlatform::default());
    let _response = platform.pending();
    let prepared = http_client(&platform.fake).prepared;
    let client = EnvironmentHttpClient::new(platform.clone(), prepared, None, None, false);
    let input = serde_json::from_value(json!({"pairingLinkId":"pending-link"})).unwrap();
    let mut request = client.approve_connect_device(&input).boxed_local();
    assert!(request.as_mut().now_or_never().is_none());
    platform.fake.advance(7_999);
    assert!(request.as_mut().now_or_never().is_none());
    platform.fake.advance(1);
    assert_eq!(block_on(request).unwrap_err(), AuthorizationError::Timeout);
}

#[test]
fn primary_pairing_and_access_snapshots_preserve_optional_connect_fields() {
    let waiting = link(
        "waiting",
        "2026-11-14T00:00:01.000Z",
        Some(json!({"verificationCode":"ABCD-EFGH"})),
    );
    assert_eq!(
        serde_json::to_value(&waiting).unwrap()["connectDeviceRequest"]["verificationCode"],
        "ABCD-EFGH"
    );
    let legacy: t3_contracts::auth::AuthAccessSnapshot =
        serde_json::from_value(json!({"pairingLinks":[],"clientSessions":[]})).unwrap();
    assert!(legacy.connect_devices.is_none());
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("connectDevices")
            .is_none()
    );
    let supported: t3_contracts::auth::AuthAccessSnapshot =
        serde_json::from_value(json!({"pairingLinks":[],"clientSessions":[],"connectDevices":[]}))
            .unwrap();
    assert_eq!(supported.connect_devices, Some(vec![]));
}

fn link(id: &str, created_at: &str, request: Option<Value>) -> AuthPairingLink {
    let mut value = json!({
        "id":id, "scopes":[], "subject":"cloud-connect",
        "createdAt":created_at, "expiresAt":"2026-11-14T00:10:00.000Z"
    });
    if let Some(request) = request {
        value["connectDeviceRequest"] = request;
    }
    serde_json::from_value(value).unwrap()
}

#[test]
fn prompts_once_per_waiting_device_oldest_first_and_skips_ordinary_pairing_links() {
    let links = [
        link(
            "newer",
            "2026-11-14T00:00:02.000Z",
            Some(json!({"verificationCode":"WXYZ-2345"})),
        ),
        link("plain", "2026-11-14T00:00:00.500Z", None),
        link(
            "older",
            "2026-11-14T00:00:01.000Z",
            Some(
                json!({"verificationCode":"ABCD-EFGH", "client":{"deviceType":"desktop", "browser":"Chrome", "os":"macOS"}}),
            ),
        ),
    ];
    let prompts =
        connect_device_approval_prompts("Mac mini", &links, &BTreeSet::from(["newer".into()]));
    assert_eq!(
        prompts
            .iter()
            .map(|p| p.pairing_link_id.as_str())
            .collect::<Vec<_>>(),
        ["older"]
    );
    assert_eq!(prompts[0].title, "Approve this device?");
    assert_eq!(
        prompts[0].message,
        "Chrome on macOS wants to connect to Mac mini through T3 Connect. It shows code ABCD-EFGH. Approve it only if that code matches."
    );
    let all = connect_device_approval_prompts("Mac mini", &links, &BTreeSet::new());
    assert_eq!(
        all.iter()
            .map(|p| p.pairing_link_id.as_str())
            .collect::<Vec<_>>(),
        ["older", "newer"]
    );
}

#[derive(Debug)]
struct Prompt {
    pairing_link_id: &'static str,
    host: &'static str,
}
impl ApprovalPrompt for Prompt {
    fn pairing_link_id(&self) -> &str {
        self.pairing_link_id
    }
}
fn prompt(id: &'static str, host: &'static str) -> Prompt {
    Prompt {
        pairing_link_id: id,
        host,
    }
}
fn answer(queue: &mut ApprovalPromptQueue<Prompt>, outcome: ApprovalOutcome) -> PresentationId {
    let id = queue.showing().unwrap().0;
    queue.answer(id, outcome);
    id
}
fn showing(queue: &ApprovalPromptQueue<Prompt>) -> Option<&str> {
    queue.showing().map(|(_, p)| p.pairing_link_id)
}

#[test]
fn shows_one_prompt_at_a_time_and_offers_a_failed_one_again_before_the_rest() {
    let mut queue = ApprovalPromptQueue::default();
    queue.offer([prompt("a", "mini"), prompt("b", "mini")]);
    queue.offer([prompt("a", "mini")]);
    assert_eq!(showing(&queue), Some("a"));
    let previous = answer(&mut queue, ApprovalOutcome::Retry);
    assert_eq!(showing(&queue), Some("a"));
    queue.answer(previous, ApprovalOutcome::Done);
    assert_eq!(showing(&queue), Some("a"));
    answer(&mut queue, ApprovalOutcome::Done);
    assert_eq!(showing(&queue), Some("b"));
}

#[test]
fn waits_for_the_next_offer_when_another_dialog_replaced_the_prompt() {
    let mut queue = ApprovalPromptQueue::default();
    queue.offer([prompt("a", "mini")]);
    answer(&mut queue, ApprovalOutcome::Later);
    assert_eq!(showing(&queue), None);
    queue.offer([]);
    assert_eq!(showing(&queue), Some("a"));
}

#[test]
fn offers_a_dropped_hosts_on_screen_prompt_again_once_the_host_is_back() {
    let mut queue = ApprovalPromptQueue::default();
    queue.offer([prompt("a", "mini")]);
    queue.drop_matching(|p| p.host == "mini");
    answer(&mut queue, ApprovalOutcome::Retry);
    assert_eq!(showing(&queue), None);
    queue.offer([prompt("a", "mini")]);
    assert_eq!(showing(&queue), Some("a"));
}

#[test]
fn drops_a_hosts_waiting_prompts_so_they_cannot_hold_up_other_hosts() {
    let mut queue = ApprovalPromptQueue::default();
    queue.offer([
        prompt("a", "mini"),
        prompt("b", "mini"),
        prompt("c", "laptop"),
    ]);
    queue.drop_matching(|p| p.host == "mini");
    answer(&mut queue, ApprovalOutcome::Done);
    assert_eq!(showing(&queue), Some("c"));
    assert!(!queue.known().contains("b"));
}

#[test]
fn device_descriptions_keep_surface_specific_fallbacks_and_deduplicate_details() {
    for (client, short, detailed, mobile) in [
        (None, "this device", "Unknown device", "A new device"),
        (
            Some(
                json!({"deviceType":"desktop","label":"Chrome","os":"macOS","browser":"Chrome","ipAddress":"127.0.0.1"}),
            ),
            "Chrome",
            "Chrome · macOS · 127.0.0.1",
            "Chrome",
        ),
        (
            Some(json!({"deviceType":"desktop","os":"macOS","browser":"Chrome"})),
            "macOS",
            "macOS · Chrome",
            "Chrome on macOS",
        ),
        (
            Some(json!({"deviceType":"desktop","browser":"Chrome"})),
            "Chrome",
            "Chrome",
            "Chrome",
        ),
    ] {
        let client: Option<AuthClientMetadata> =
            client.map(|value| serde_json::from_value(value).unwrap());
        assert_eq!(connect_device_name(client.as_ref()), short);
        assert_eq!(describe_connect_device_client(client.as_ref()), detailed);
        assert_eq!(describe_connect_device(client.as_ref()), mobile);
    }
}
