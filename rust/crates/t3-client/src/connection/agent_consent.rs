//! An agent's MCP sign-in, approved on the environment's own page (`/connect-agent`).
//!
//! Ports `apps/web/src/components/auth/ConnectAgentSurface.logic.ts` (`resolveMcpAccess`,
//! `resolveApprovalDecision`, `agentRedirectUrl`) and the non-visual parts of
//! `ConnectAgentSurface.tsx` (`readAuthorizationRequest`, `redirectsToThisComputer`,
//! `ceilingReason`, the transport's answers and messages), with the helpers they use from
//! `packages/contracts` (`parseMcpApprovalCode`, `isRuntimeModeAbove`,
//! `managedRuntimeModeCeilingMessage`). The page talks to the environment that serves it, with
//! the browser's own session (`Credentials::SameOrigin`), as `PrimaryEnvironmentHttpClient` does.

use std::collections::BTreeMap;

use serde_json::Value;
use t3_contracts::auth::{
    AuthMcpApprovalBrowserSession, AuthMcpApprovalBrowserSessionTag, AuthMcpApprovalDecision,
    AuthMcpApprovalDecisionRequest, AuthMcpApprovalDeny, AuthMcpApprovalDenyTag,
    AuthMcpApprovalDetails, AuthMcpApprovalError, AuthMcpApprovalPairingCode,
    AuthMcpApprovalPairingCodeTag, AuthMcpApprovalRedirect, AuthMcpAuthorizationRequest,
    AuthMcpClientAccess,
};
use t3_contracts::provider_policy::RuntimeMode;
use url::{Host, Url};

use crate::platform::{Credentials, HttpRequest, Platform};

/// The page's message when the environment cannot be reached or answers nonsense.
pub const UNREACHABLE: &str = "Could not reach this environment. Try again.";
/// The page's message for a redirect it will not follow.
pub const UNSAFE_REDIRECT: &str = "The environment returned an address this page will not open.";
/// The page's message when a decision is answered without a way to continue.
pub const CANNOT_CONTINUE: &str = "The sign-in could not continue.";
/// Approve stays off this long after the page shows or regains focus (`ARM_DELAY_MS`).
pub const ARM_DELAY_MS: u64 = 700;

const PAIRING_CODE_ALPHABET: &str = "23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
const PAIRING_CODE_LENGTH: usize = 12;

/// The levels in the web's order (`AuthMcpClientAccess.literals`).
pub const ACCESS_LEVELS: [AuthMcpClientAccess; 5] = [
    AuthMcpClientAccess::ReadOnly,
    AuthMcpClientAccess::ApprovalRequired,
    AuthMcpClientAccess::AutoAcceptEdits,
    AuthMcpClientAccess::Auto,
    AuthMcpClientAccess::FullAccess,
];

fn code_prefix(access: AuthMcpClientAccess) -> &'static str {
    match access {
        AuthMcpClientAccess::ReadOnly => "READ",
        AuthMcpClientAccess::ApprovalRequired => "SUPERVISED",
        AuthMcpClientAccess::AutoAcceptEdits => "EDITS",
        AuthMcpClientAccess::Auto => "AUTO",
        AuthMcpClientAccess::FullAccess => "FULL",
    }
}

/// `parseMcpApprovalCode`: the level a code approves and the code as issued, or `None` for any
/// other code.
pub fn parse_mcp_approval_code(code: &str) -> Option<(AuthMcpClientAccess, String)> {
    let (prefix, token) = code.trim().split_once('-')?;
    let prefix = prefix.to_ascii_uppercase();
    let token = token.to_ascii_uppercase();
    let access = ACCESS_LEVELS
        .into_iter()
        .find(|level| code_prefix(*level) == prefix)?;
    (token.chars().count() == PAIRING_CODE_LENGTH
        && token.chars().all(|c| PAIRING_CODE_ALPHABET.contains(c)))
    .then(|| (access, format!("{prefix}-{token}")))
}

fn runtime_mode(access: AuthMcpClientAccess) -> Option<RuntimeMode> {
    match access {
        AuthMcpClientAccess::ReadOnly => None,
        AuthMcpClientAccess::ApprovalRequired => Some(RuntimeMode::ApprovalRequired),
        AuthMcpClientAccess::AutoAcceptEdits => Some(RuntimeMode::AutoAcceptEdits),
        AuthMcpClientAccess::Auto => Some(RuntimeMode::Auto),
        AuthMcpClientAccess::FullAccess => Some(RuntimeMode::FullAccess),
    }
}

fn access_of(mode: RuntimeMode) -> AuthMcpClientAccess {
    match mode {
        RuntimeMode::ApprovalRequired => AuthMcpClientAccess::ApprovalRequired,
        RuntimeMode::AutoAcceptEdits => AuthMcpClientAccess::AutoAcceptEdits,
        RuntimeMode::Auto => AuthMcpClientAccess::Auto,
        RuntimeMode::FullAccess => AuthMcpClientAccess::FullAccess,
    }
}

fn rank(mode: RuntimeMode) -> u8 {
    match mode {
        RuntimeMode::ApprovalRequired => 0,
        RuntimeMode::AutoAcceptEdits => 1,
        RuntimeMode::Auto => 2,
        RuntimeMode::FullAccess => 3,
    }
}

/// `isRuntimeModeAbove`, for an access level (read only is never above a ceiling).
fn above(access: AuthMcpClientAccess, ceiling: RuntimeMode) -> bool {
    runtime_mode(access).is_some_and(|mode| rank(mode) > rank(ceiling))
}

/// `RUNTIME_MODE_LABELS`.
pub fn runtime_mode_label(mode: RuntimeMode) -> &'static str {
    match mode {
        RuntimeMode::ApprovalRequired => "Supervised",
        RuntimeMode::AutoAcceptEdits => "Auto-accept edits",
        RuntimeMode::Auto => "Auto",
        RuntimeMode::FullAccess => "Full access",
    }
}

/// `resolveMcpAccess`: the picker's access, lowered to a managed ceiling: the user's choice, else
/// Auto for a typed code, else Read only.
pub fn resolve_mcp_access(
    requested: Option<AuthMcpClientAccess>,
    details: Option<&AuthMcpApprovalDetails>,
    has_code: bool,
) -> AuthMcpClientAccess {
    let access = requested.unwrap_or(if has_code {
        AuthMcpClientAccess::Auto
    } else {
        AuthMcpClientAccess::ReadOnly
    });
    match details.and_then(|details| details.runtime_mode_ceiling.as_ref()) {
        Some(ceiling) if above(access, ceiling.max_runtime_mode) => {
            access_of(ceiling.max_runtime_mode)
        }
        _ => access,
    }
}

/// `resolveApprovalDecision`: what Approve sends, or `None` when there is nothing to send. A typed
/// code is always checked as a code; a level-bound code sends its own level.
pub fn resolve_approval_decision(
    code: &str,
    access: AuthMcpClientAccess,
    details: &AuthMcpApprovalDetails,
) -> Option<AuthMcpApprovalDecision> {
    let code = code.trim();
    if !code.is_empty() {
        let access = parse_mcp_approval_code(code).map_or(access, |(level, _)| level);
        let code = t3_contracts::base_schemas::TrimmedNonEmptyString(code.to_owned());
        return Some(AuthMcpApprovalDecision::AuthMcpApprovalPairingCode(
            AuthMcpApprovalPairingCode {
                _tag: AuthMcpApprovalPairingCodeTag::PairingCode,
                access,
                code,
            },
        ));
    }
    let csrf_token = details.csrf_token.clone()?;
    details
        .one_click_access
        .as_ref()
        .is_some_and(|levels| levels.contains(&access))
        .then(|| {
            AuthMcpApprovalDecision::AuthMcpApprovalBrowserSession(AuthMcpApprovalBrowserSession {
                _tag: AuthMcpApprovalBrowserSessionTag::BrowserSession,
                access,
                csrf_token,
            })
        })
}

/// The decision Deny sends.
pub fn deny() -> AuthMcpApprovalDecision {
    AuthMcpApprovalDecision::AuthMcpApprovalDeny(AuthMcpApprovalDeny {
        _tag: AuthMcpApprovalDenyTag::Deny,
    })
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match Url::parse(&format!(
        "http://{}",
        if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_owned()
        }
    ))
    .ok()
    .and_then(|url| url.host().map(|host| host.to_owned()))
    {
        Some(Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost" || domain.ends_with(".localhost")
        }
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

/// `agentRedirectUrl`: the address to send the window to after a decision, or `None`. Only https,
/// or http on this computer, can carry a sign-in to an agent; `javascript:` and the rest are refused.
pub fn agent_redirect_url(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let safe = parsed.scheme() == "https"
        || (parsed.scheme() == "http" && parsed.host_str().is_some_and(is_loopback_host));
    safe.then(|| parsed.into())
}

/// `redirectsToThisComputer`: whether the agent's redirect host (a host with optional port) is
/// this computer.
pub fn redirects_to_this_computer(redirect_host: &str) -> bool {
    Url::parse(&format!("http://{redirect_host}"))
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| is_loopback_host(&host))
}

/// `ceilingReason`: why a managed policy keeps `access` from being granted.
pub fn ceiling_reason(
    details: &AuthMcpApprovalDetails,
    access: AuthMcpClientAccess,
) -> Option<String> {
    let ceiling = details.runtime_mode_ceiling.as_ref()?;
    above(access, ceiling.max_runtime_mode).then(|| {
        format!(
            "{} limits threads to {}.",
            ceiling.organization,
            runtime_mode_label(ceiling.max_runtime_mode)
        )
    })
}

/// `readAuthorizationRequest`: the agent's OAuth request from the page's query; other parameters
/// are ignored, and a repeated one keeps its last value, as `Object.fromEntries` does.
pub fn read_authorization_request(address: &str) -> AuthMcpAuthorizationRequest {
    let pairs: BTreeMap<String, String> = Url::parse(address)
        .map(|url| {
            url.query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        })
        .unwrap_or_default();
    let field = |name: &str| pairs.get(name).cloned();
    AuthMcpAuthorizationRequest {
        response_type: field("response_type"),
        client_id: field("client_id"),
        redirect_uri: field("redirect_uri"),
        code_challenge: field("code_challenge"),
        code_challenge_method: field("code_challenge_method"),
        state: field("state"),
        resource: field("resource"),
    }
}

/// The origin of the page the app was opened at, when it is a web page: the environment the
/// consent page talks to. `None` for an app link or anything that is not http(s).
pub fn page_origin(address: &str) -> Option<String> {
    Url::parse(address)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.origin().ascii_serialization())
}

/// `McpApprovalAnswer`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpApprovalAnswer<A> {
    Ok(A),
    Redirect(String),
    Error(String),
}

fn answer<A: serde::de::DeserializeOwned>(status: u16, body: &[u8]) -> McpApprovalAnswer<A> {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return McpApprovalAnswer::Error(UNREACHABLE.into());
    };
    if let Ok(refused) = serde_json::from_value::<AuthMcpApprovalError>(value.clone()) {
        return McpApprovalAnswer::Error(refused.message);
    }
    if !(200..300).contains(&status) {
        return McpApprovalAnswer::Error(UNREACHABLE.into());
    }
    if let Ok(redirect) = serde_json::from_value::<AuthMcpApprovalRedirect>(value.clone()) {
        return McpApprovalAnswer::Redirect(redirect.redirect_to);
    }
    serde_json::from_value(value)
        .map(McpApprovalAnswer::Ok)
        .unwrap_or_else(|_| McpApprovalAnswer::Error(UNREACHABLE.into()))
}

/// The environment that serves the consent page: its `/oauth/mcp/approval` and
/// `/oauth/mcp/decision`, with this browser's session.
#[derive(Clone)]
pub struct AgentConsent<P> {
    platform: P,
    origin: String,
}

impl<P: Platform> AgentConsent<P> {
    /// `origin` is the page's own origin (`https://host:port`).
    pub fn new(platform: P, origin: impl Into<String>) -> Self {
        Self {
            platform,
            origin: origin.into(),
        }
    }

    async fn post<A: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: Value,
    ) -> McpApprovalAnswer<A> {
        let Ok(body) = serde_json::to_vec(&body) else {
            return McpApprovalAnswer::Error(UNREACHABLE.into());
        };
        let request = HttpRequest {
            method: "POST".into(),
            url: format!("{}{path}", self.origin.trim_end_matches('/')),
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            body,
            credentials: Credentials::SameOrigin,
            max_response_bytes: 64 * 1024,
        };
        match self.platform.http(request).await {
            Ok(response) => answer(response.status, &response.body),
            Err(_) => McpApprovalAnswer::Error(UNREACHABLE.into()),
        }
    }

    /// The sign-in request's details, a redirect, or why it cannot continue.
    pub async fn load(
        &self,
        authorization: &AuthMcpAuthorizationRequest,
    ) -> McpApprovalAnswer<AuthMcpApprovalDetails> {
        match serde_json::to_value(authorization) {
            Ok(body) => self.post("/oauth/mcp/approval", body).await,
            Err(_) => McpApprovalAnswer::Error(UNREACHABLE.into()),
        }
    }

    /// Sends a decision; a success is a redirect back to the agent.
    pub async fn decide(
        &self,
        authorization: &AuthMcpAuthorizationRequest,
        decision: AuthMcpApprovalDecision,
    ) -> McpApprovalAnswer<Value> {
        let request = AuthMcpApprovalDecisionRequest {
            authorization: authorization.clone(),
            decision,
        };
        match serde_json::to_value(request) {
            Ok(body) => self.post("/oauth/mcp/decision", body).await,
            Err(_) => McpApprovalAnswer::Error(UNREACHABLE.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use t3_contracts::auth::AuthRuntimeModeCeiling;

    use crate::platform::HttpResponse;
    use crate::testing::FakePlatform;

    fn details(csrf: Option<&str>, one_click: &[AuthMcpClientAccess]) -> AuthMcpApprovalDetails {
        AuthMcpApprovalDetails {
            client_name: "Claude Code".into(),
            redirect_host: "127.0.0.1:43123".into(),
            environment_host: "box.test".into(),
            csrf_token: csrf.map(str::to_owned),
            one_click_access: Some(one_click.to_vec()),
            runtime_mode_ceiling: None,
        }
    }

    fn ceiling(mode: RuntimeMode) -> AuthMcpApprovalDetails {
        AuthMcpApprovalDetails {
            runtime_mode_ceiling: Some(AuthRuntimeModeCeiling {
                organization: "Example Corp".into(),
                max_runtime_mode: mode,
            }),
            ..details(None, &[])
        }
    }

    // ConnectAgentSurface.logic.test.ts: "keeps MCP access within the %s policy ceiling".
    #[test]
    fn access_is_kept_within_every_policy_ceiling() {
        use AuthMcpClientAccess::*;
        for mode in [
            RuntimeMode::ApprovalRequired,
            RuntimeMode::AutoAcceptEdits,
            RuntimeMode::Auto,
            RuntimeMode::FullAccess,
        ] {
            let capped = ceiling(mode);
            let expected = if mode == RuntimeMode::FullAccess {
                Auto
            } else {
                access_of(mode)
            };
            assert_eq!(
                resolve_mcp_access(Some(Auto), Some(&capped), false),
                expected,
                "{mode:?}"
            );
            assert_eq!(
                resolve_mcp_access(Some(ReadOnly), Some(&capped), false),
                ReadOnly
            );
            assert_eq!(
                resolve_mcp_access(Some(ApprovalRequired), Some(&capped), false),
                ApprovalRequired
            );
        }
    }

    // "checks a typed code even in a signed-in browser, and approves without one at Read only".
    #[test]
    fn a_typed_code_is_checked_even_in_a_signed_in_browser() {
        use AuthMcpClientAccess::*;
        let signed_out = AuthMcpApprovalDetails {
            client_name: "Poke".into(),
            redirect_host: "poke.com".into(),
            environment_host: "prod-a1.t3coderelay.com".into(),
            csrf_token: None,
            one_click_access: None,
            runtime_mode_ceiling: None,
        };
        let signed_in = AuthMcpApprovalDetails {
            csrf_token: Some("csrf".into()),
            one_click_access: Some(ACCESS_LEVELS.to_vec()),
            ..signed_out.clone()
        };
        let access = resolve_mcp_access(None, None, false);
        let code = |decision: Option<AuthMcpApprovalDecision>| match decision {
            Some(AuthMcpApprovalDecision::AuthMcpApprovalPairingCode(code)) => {
                Some((code.access, code.code.0))
            }
            _ => None,
        };
        assert_eq!(
            code(resolve_approval_decision(
                " 7K2P9QXR4MZT ",
                resolve_mcp_access(None, None, true),
                &signed_in
            )),
            Some((Auto, "7K2P9QXR4MZT".into()))
        );
        assert_eq!(
            code(resolve_approval_decision(
                "auto-q8rz3nwd5ktc",
                access,
                &signed_in
            )),
            Some((Auto, "auto-q8rz3nwd5ktc".into()))
        );
        assert!(matches!(
            resolve_approval_decision("", access, &signed_in),
            Some(AuthMcpApprovalDecision::AuthMcpApprovalBrowserSession(ref session))
                if session.access == ReadOnly && session.csrf_token == "csrf"
        ));
        assert_eq!(resolve_approval_decision("", access, &signed_out), None);
        assert_eq!(
            resolve_approval_decision("", Auto, &details(Some("csrf"), &[ReadOnly])),
            None,
            "a level the session may not approve in one click needs a code"
        );
    }

    // "follows only https or loopback http redirects to an agent".
    #[test]
    fn only_https_or_loopback_http_redirects_are_followed() {
        assert_eq!(
            agent_redirect_url("https://www.cursor.com/cb?code=x").as_deref(),
            Some("https://www.cursor.com/cb?code=x")
        );
        assert_eq!(
            agent_redirect_url("http://localhost:51234/callback?code=x").as_deref(),
            Some("http://localhost:51234/callback?code=x")
        );
        assert!(agent_redirect_url("http://127.0.0.1:43123/cb").is_some());
        assert!(agent_redirect_url("http://[::1]:43123/cb").is_some());
        for url in [
            "javascript:alert(document.domain)",
            "data:text/html,hi",
            "http://evil.example/cb",
            "not a url",
        ] {
            assert_eq!(agent_redirect_url(url), None, "{url}");
        }
    }

    #[test]
    fn codes_name_their_level_and_ceilings_explain_themselves() {
        assert_eq!(
            parse_mcp_approval_code(" auto-q8rz3nwd5ktc "),
            Some((AuthMcpClientAccess::Auto, "AUTO-Q8RZ3NWD5KTC".into()))
        );
        assert_eq!(
            parse_mcp_approval_code("AUTO-Q8RZ3NWD5KT0"),
            None,
            "0 is not in the alphabet"
        );
        assert_eq!(parse_mcp_approval_code("NOPE-Q8RZ3NWD5KTC"), None);
        let capped = ceiling(RuntimeMode::Auto);
        assert_eq!(
            ceiling_reason(&capped, AuthMcpClientAccess::FullAccess).as_deref(),
            Some("Example Corp limits threads to Auto.")
        );
        assert_eq!(ceiling_reason(&capped, AuthMcpClientAccess::Auto), None);
        assert!(redirects_to_this_computer("127.0.0.1:43123"));
        assert!(!redirects_to_this_computer("agent.example"));
    }

    #[test]
    fn the_request_is_read_from_the_page_query() {
        let request = read_authorization_request(
            "https://box.test/connect-agent?client_id=c&redirect_uri=http%3A%2F%2F127.0.0.1%3A1%2Fcb&state=s&other=x",
        );
        assert_eq!(request.client_id.as_deref(), Some("c"));
        assert_eq!(
            request.redirect_uri.as_deref(),
            Some("http://127.0.0.1:1/cb")
        );
        assert_eq!(request.state.as_deref(), Some("s"));
        assert_eq!(request.resource, None);
        assert_eq!(
            page_origin("https://box.test:3773/connect-agent?client_id=c").as_deref(),
            Some("https://box.test:3773")
        );
        assert_eq!(page_origin("t3code://connect-agent?client_id=c"), None);
    }

    fn respond(platform: &FakePlatform, status: u16, body: &str) {
        platform.push_http(Ok(HttpResponse {
            status,
            headers: BTreeMap::new(),
            body: body.as_bytes().to_vec(),
        }));
    }

    #[test]
    fn the_transport_posts_to_the_page_origin_and_maps_answers() {
        let platform = FakePlatform::default();
        let consent = AgentConsent::new(platform.clone(), "https://box.test/");
        let authorization =
            read_authorization_request("https://box.test/connect-agent?client_id=c");
        respond(
            &platform,
            200,
            r#"{"clientName":"Claude Code","redirectHost":"127.0.0.1:1","environmentHost":"box.test"}"#,
        );
        let loaded = block_on(consent.load(&authorization));
        assert!(matches!(loaded, McpApprovalAnswer::Ok(ref d) if d.client_name == "Claude Code"));
        let sent = platform.http_requests();
        assert_eq!(sent[0].url, "https://box.test/oauth/mcp/approval");
        assert_eq!(sent[0].credentials, Credentials::SameOrigin);

        respond(
            &platform,
            403,
            r#"{"_tag":"AuthMcpApprovalError","message":"That code has expired."}"#,
        );
        assert_eq!(
            block_on(consent.decide(&authorization, deny())),
            McpApprovalAnswer::Error("That code has expired.".into())
        );
        respond(
            &platform,
            200,
            r#"{"redirectTo":"http://127.0.0.1:1/cb?code=x"}"#,
        );
        assert_eq!(
            block_on(consent.decide(&authorization, deny())),
            McpApprovalAnswer::Redirect("http://127.0.0.1:1/cb?code=x".into())
        );
        assert_eq!(
            platform.http_requests()[2].url,
            "https://box.test/oauth/mcp/decision"
        );
        respond(&platform, 502, "<html>");
        assert_eq!(
            block_on(consent.load(&authorization)),
            McpApprovalAnswer::Error(UNREACHABLE.into())
        );
    }
}
