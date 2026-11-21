//! Pairing links and codes as addresses and form input: reading a token out of a link, taking it
//! back out of the visible address, and splitting a pasted link into host and code.
//!
//! Ports `packages/shared/src/remote.ts` (`getPairingTokenFromUrl`, `stripPairingTokenFromUrl`,
//! `readHostedPairingRequest`, `readPairingAppLink`), `apps/web/src/components/settings/
//! ConnectionsSettings.logic.ts` (`parsePairingUrlFields`, `parseRemotePairingFields`) and
//! `apps/mobile/src/features/connection/pairing.ts` (`buildPairingUrl`, `parsePairingUrl`,
//! `extractPairingUrlFromQrPayload`). A token or code is a credential: it is carried as
//! [`Sensitive`] so it never reaches a log through `Debug`.

use url::{Host, Url};

use super::{ConnectionFailure, FailureKind, PairingInput, resolve_pairing_target};
use crate::authorization::Sensitive;

const PAIRING_TOKEN_PARAM: &str = "token";
const HOSTED_PAIRING_HOST_PARAM: &str = "host";
const HOSTED_PAIRING_LABEL_PARAM: &str = "label";
const PAIRING_APP_LINK_PARAM: &str = "pairingUrl";

fn hash_params(url: &Url) -> Vec<(String, String)> {
    url::form_urlencoded::parse(url.fragment().unwrap_or("").as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

fn query_param(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

/// `getPairingTokenFromUrl`: the fragment's `token`, else the query's; blank counts as none.
pub fn pairing_token_from_url(url: &Url) -> Option<Sensitive<String>> {
    let from_hash = hash_params(url)
        .into_iter()
        .find(|(key, _)| key == PAIRING_TOKEN_PARAM)
        .map(|(_, value)| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    from_hash
        .or_else(|| {
            query_param(url, PAIRING_TOKEN_PARAM)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .map(Sensitive)
}

/// `stripPairingTokenFromUrl`: the same address without `token` in its fragment or query.
pub fn strip_pairing_token(url: &Url) -> Url {
    let mut next = url.clone();
    let hash = hash_params(url);
    if hash.iter().any(|(key, _)| key == PAIRING_TOKEN_PARAM) {
        let kept: Vec<_> = hash
            .into_iter()
            .filter(|(key, _)| key != PAIRING_TOKEN_PARAM)
            .collect();
        if kept.is_empty() {
            next.set_fragment(None);
        } else {
            let encoded = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(kept)
                .finish();
            next.set_fragment(Some(&encoded));
        }
    }
    if url.query_pairs().any(|(key, _)| key == PAIRING_TOKEN_PARAM) {
        let kept: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(key, _)| key != PAIRING_TOKEN_PARAM)
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        if kept.is_empty() {
            next.set_query(None);
        } else {
            next.query_pairs_mut().clear().extend_pairs(kept);
        }
    }
    next
}

/// A hosted app's `/pair?host=…&label=…#token=…` link (`readHostedPairingRequest`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedPairingRequest {
    pub host: String,
    pub token: Sensitive<String>,
    pub label: String,
}

impl HostedPairingRequest {
    /// The pairing the request asks for, as `connectPairing({ host, pairingCode })` takes it.
    pub fn pairing_input(&self) -> PairingInput {
        PairingInput {
            host: Some(self.host.clone()),
            pairing_code: Some(self.token.expose().clone()),
            ..PairingInput::default()
        }
    }
}

pub fn read_hosted_pairing_request(url: &Url) -> Option<HostedPairingRequest> {
    let host = query_param(url, HOSTED_PAIRING_HOST_PARAM)
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();
    let token = pairing_token_from_url(url)?;
    let label = query_param(url, HOSTED_PAIRING_LABEL_PARAM)
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();
    (!host.is_empty()).then_some(HostedPairingRequest { host, token, label })
}

fn is_app_link_scheme(scheme: &str) -> bool {
    scheme == "t3code"
        || scheme
            .strip_prefix("t3code-")
            .is_some_and(|channel| !channel.is_empty() && channel.bytes().all(|b| b.is_ascii_lowercase()))
}

/// `readPairingAppLink`: the pairing link inside `t3code://pair?pairingUrl=…`.
pub fn read_pairing_app_link(value: &str) -> Option<Sensitive<String>> {
    let url = Url::parse(value).ok()?;
    if !is_app_link_scheme(url.scheme()) || url.host_str() != Some("pair") {
        return None;
    }
    if !matches!(url.path(), "" | "/") {
        return None;
    }
    query_param(&url, PAIRING_APP_LINK_PARAM)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .map(Sensitive)
}

/// What the pairing page does with the address it was opened at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingLaunch {
    /// No pairing credential in the address: the page shows its form.
    None,
    /// `/pair?host=…#token=…`: pair with the named backend at once and show its host while doing
    /// so (`HostedPairingRouteSurface`).
    Hosted(HostedPairingRequest),
    /// `/pair#token=…` on the environment's own page: pair with that origin at once
    /// (`PairingRouteSurface`'s URL token).
    Environment {
        host: String,
        token: Sensitive<String>,
    },
    /// `t3code://pair?pairingUrl=…`: any page can open one, so the user confirms the backend's
    /// origin before its one-time token is spent (`DesktopWebLinkCoordinator.pairFromLink`).
    AppLink { pairing_url: Sensitive<String> },
}

/// [`PairingLaunch`] for an address, and the address to show instead when it carried a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchAddress {
    pub launch: PairingLaunch,
    /// The address without its credential, to replace the visible one before pairing starts;
    /// `None` when there was nothing to take out.
    pub stripped: Option<String>,
}

pub fn read_pairing_launch(address: &str) -> LaunchAddress {
    let none = LaunchAddress {
        launch: PairingLaunch::None,
        stripped: None,
    };
    let Ok(url) = Url::parse(address) else {
        return none;
    };
    if let Some(pairing_url) = read_pairing_app_link(address) {
        let mut stripped = url.clone();
        let kept: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(key, _)| key != PAIRING_APP_LINK_PARAM)
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        if kept.is_empty() {
            stripped.set_query(None);
        } else {
            stripped.query_pairs_mut().clear().extend_pairs(kept);
        }
        return LaunchAddress {
            launch: PairingLaunch::AppLink { pairing_url },
            stripped: Some(stripped.into()),
        };
    }
    if !matches!(url.scheme(), "http" | "https") {
        return none;
    }
    let path = url.path().trim_end_matches('/');
    if path != "/pair" {
        return none;
    }
    let Some(token) = pairing_token_from_url(&url) else {
        return none;
    };
    let stripped = Some(String::from(strip_pairing_token(&url)));
    let launch = match read_hosted_pairing_request(&url) {
        Some(request) => PairingLaunch::Hosted(request),
        None => PairingLaunch::Environment {
            host: url.origin().ascii_serialization(),
            token,
        },
    };
    LaunchAddress { launch, stripped }
}

/// The origin a pairing link would pair with, shown before its token is spent.
pub fn pairing_link_origin(pairing_url: &str) -> Result<String, ConnectionFailure> {
    let target = resolve_pairing_target(&PairingInput {
        pairing_url: Some(pairing_url.to_owned()),
        ..PairingInput::default()
    })?;
    Url::parse(&target.http_base)
        .map(|url| url.origin().ascii_serialization())
        .map_err(|_| ConnectionFailure::new(FailureKind::Configuration, "Pairing URL is invalid."))
}

/// A backend host and pairing code as the Add environment form holds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingFields {
    pub host: String,
    pub pairing_code: Sensitive<String>,
}

impl PairingFields {
    pub fn pairing_input(&self) -> PairingInput {
        PairingInput {
            host: Some(self.host.clone()),
            pairing_code: Some(self.pairing_code.expose().clone()),
            ..PairingInput::default()
        }
    }
}

fn has_scheme_prefix(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once("://") else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// `parsePairingUrlFields`: a full pairing link pasted into the host field fills both fields.
/// `page_origin` resolves a scheme-relative `//host` link as the web resolves it against the page.
pub fn parse_pairing_url_fields(input: &str, page_origin: Option<&str>) -> Option<PairingFields> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let url = if has_scheme_prefix(trimmed) {
        Url::parse(trimmed).ok()?
    } else if trimmed.starts_with("//") {
        let base = Url::parse(page_origin.unwrap_or("https://localhost")).ok()?;
        base.join(trimmed).ok()?
    } else {
        Url::parse(&format!("https://{trimmed}")).ok()?
    };
    if let Some(request) = read_hosted_pairing_request(&url) {
        return Some(PairingFields {
            host: request.host,
            pairing_code: request.token,
        });
    }
    let pairing_code = pairing_token_from_url(&url)?;
    Some(PairingFields {
        host: url.origin().ascii_serialization(),
        pairing_code,
    })
}

/// `parseRemotePairingFields`: the fields to pair with, or the reason the form cannot submit.
pub fn parse_remote_pairing_fields(
    host: &str,
    pairing_code: &str,
    page_origin: Option<&str>,
) -> Result<PairingFields, ConnectionFailure> {
    if let Some(fields) = parse_pairing_url_fields(host, page_origin) {
        return Ok(fields);
    }
    let host = host.trim();
    let pairing_code = pairing_code.trim();
    if host.is_empty() {
        return Err(ConnectionFailure::new(
            FailureKind::Configuration,
            "Enter a backend host.",
        ));
    }
    if pairing_code.is_empty() {
        return Err(ConnectionFailure::new(
            FailureKind::Configuration,
            "Enter a pairing code.",
        ));
    }
    Ok(PairingFields {
        host: host.to_owned(),
        pairing_code: Sensitive(pairing_code.to_owned()),
    })
}

fn is_ip_literal(host: &str) -> bool {
    Url::parse(&format!("http://{host}"))
        .ok()
        .and_then(|url| {
            url.host()
                .map(|host| matches!(host, Host::Ipv4(_) | Host::Ipv6(_)))
        })
        .unwrap_or(false)
}

/// Mobile `buildPairingUrl`: an address and code typed apart, as one pairing link.
pub fn build_pairing_url(host: &str, code: &str) -> Sensitive<String> {
    let host = host.trim();
    let code = code.trim();
    if host.is_empty() {
        return Sensitive(String::new());
    }
    if code.is_empty() {
        return Sensitive(host.to_owned());
    }
    let candidate = if host.contains("://") {
        host.to_owned()
    } else if is_ip_literal(host) {
        format!("http://{host}")
    } else {
        format!("https://{host}")
    };
    let fragment = url::form_urlencoded::Serializer::new(String::new())
        .append_pair(PAIRING_TOKEN_PARAM, code)
        .finish();
    match Url::parse(&candidate) {
        Ok(mut url) => {
            url.set_fragment(Some(&fragment));
            Sensitive(url.into())
        }
        Err(_) => Sensitive(format!("{host}#token={code}")),
    }
}

/// Mobile `parsePairingUrl`: a pairing link split back into address and code for the form.
pub fn parse_pairing_url(value: &str) -> (String, Sensitive<String>) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return (String::new(), Sensitive(String::new()));
    }
    let Ok(mut url) = Url::parse(trimmed) else {
        return (trimmed.to_owned(), Sensitive(String::new()));
    };
    if let Some(request) = read_hosted_pairing_request(&url) {
        return (
            request.host.trim_end_matches('/').to_owned(),
            request.token,
        );
    }
    let code = pairing_token_from_url(&url).unwrap_or_else(|| Sensitive(String::new()));
    url.set_fragment(None);
    url.set_query(None);
    url.set_path("/");
    let host: String = url.into();
    (host.trim_end_matches('/').to_owned(), code)
}

/// Mobile `extractPairingUrlFromQrPayload`: the pairing link a scanned code carries, whether it
/// is the link itself or a `t3code:` app link wrapping it.
pub fn extract_pairing_url_from_qr_payload(
    payload: &str,
) -> Result<Sensitive<String>, ConnectionFailure> {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return Err(ConnectionFailure::new(
            FailureKind::Configuration,
            "Scanned QR code did not contain a pairing URL.",
        ));
    }
    if let Ok(url) = Url::parse(trimmed)
        && url.scheme() == "t3code"
        && let Some(pairing_url) = query_param(&url, PAIRING_APP_LINK_PARAM)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    {
        return Ok(Sensitive(pairing_url));
    }
    Ok(Sensitive(trimmed.to_owned()))
}

/// Parses the manual fields in the desktop Connections dialog. Explicit fields
/// take precedence over the user and port embedded in the host.
pub fn manual_ssh_target(
    address: &str,
    user: &str,
    port: &str,
) -> Result<t3_contracts::ipc::DesktopSshEnvironmentTargetSchema, &'static str> {
    use t3_contracts::{ipc::DesktopSshEnvironmentTargetSchema, settings::WireNumber};
    let (inline_user, address) = address
        .trim()
        .rsplit_once('@')
        .filter(|(user, _)| !user.is_empty())
        .map_or(("", address.trim()), |(user, host)| {
            (user.trim(), host.trim())
        });
    let digits = |value: &str| !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit());
    let bracketed = address.strip_prefix('[').and_then(|s| s.split_once(']'));
    let (host, inline_port) = match bracketed {
        Some((host, "")) if !host.is_empty() => (host, ""),
        Some((host, suffix))
            if !host.is_empty() && suffix.strip_prefix(':').is_some_and(digits) =>
        {
            (host, &suffix[1..])
        }
        _ => address
            .split_once(':')
            .filter(|(_, suffix)| digits(suffix))
            .unwrap_or((address, "")),
    };
    let host = host.trim();
    if host.is_empty() {
        return Err("SSH host or alias is required.");
    }
    let user = if user.trim().is_empty() {
        inline_user
    } else {
        user.trim()
    };
    let port = if port.trim().is_empty() {
        inline_port
    } else {
        port.trim()
    };
    let port = if port.is_empty() {
        None
    } else {
        // The TS form uses Number.parseInt(rawPort, 10).
        let unsigned = port.strip_prefix('+').unwrap_or(port);
        let end = unsigned.bytes().take_while(u8::is_ascii_digit).count();
        let n = unsigned[..end]
            .parse::<u16>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("SSH port must be between 1 and 65535.")?;
        Some(WireNumber::Finite(n.into()))
    };
    Ok(DesktopSshEnvironmentTargetSchema {
        alias: host.to_owned(),
        hostname: host.to_owned(),
        username: (!user.is_empty()).then(|| user.to_owned()),
        port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        Url::parse(value).expect("valid test URL")
    }

    #[test]
    fn manual_ssh_fields_override_inline_credentials_and_preserve_ipv6() {
        let target =
            super::manual_ssh_target("first@[2001:db8::1]:2222", " build ", "2022").unwrap();
        assert_eq!(target.hostname, "2001:db8::1");
        assert_eq!(target.alias, target.hostname);
        assert_eq!(target.username.as_deref(), Some("build"));
        assert_eq!(serde_json::to_value(target.port).unwrap(), 2022);
        let target = super::manual_ssh_target("2001:db8::1", "", "").unwrap();
        assert_eq!(target.hostname, "2001:db8::1");
        assert!(target.username.is_none() && target.port.is_none());
        assert!(super::manual_ssh_target("builder@", "", "").is_err());
    }

    #[test]
    fn the_token_comes_from_the_fragment_first_then_the_query() {
        let both = url("https://h.test/pair?token=query#token=hash");
        assert_eq!(pairing_token_from_url(&both).map(|t| t.0), Some("hash".into()));
        let query = url("https://h.test/pair?token=%20query%20");
        assert_eq!(pairing_token_from_url(&query).map(|t| t.0), Some("query".into()));
        let blank = url("https://h.test/pair#token=%20%20");
        assert_eq!(pairing_token_from_url(&blank), None);
    }

    #[test]
    fn stripping_removes_only_the_token() {
        let stripped = strip_pairing_token(&url("https://h.test/pair?a=1&token=x#token=y&b=2"));
        assert_eq!(stripped.as_str(), "https://h.test/pair?a=1#b=2");
        let bare = strip_pairing_token(&url("https://h.test/pair#token=y"));
        assert_eq!(bare.as_str(), "https://h.test/pair");
        let untouched = url("https://h.test/pair?a=%20b");
        assert_eq!(strip_pairing_token(&untouched), untouched);
    }

    #[test]
    fn a_hosted_request_needs_both_host_and_token() {
        let request = read_hosted_pairing_request(&url(
            "https://app.t3.codes/pair?host=https%3A%2F%2Fbox.test&label=%20Box%20#token=abc",
        ))
        .expect("host and token");
        assert_eq!(request.host, "https://box.test");
        assert_eq!(request.label, "Box");
        assert_eq!(request.token.expose(), "abc");
        assert!(read_hosted_pairing_request(&url("https://app.t3.codes/pair#token=abc")).is_none());
        assert!(read_hosted_pairing_request(&url("https://app.t3.codes/pair?host=x")).is_none());
    }

    #[test]
    fn credentials_never_reach_debug_output() {
        let launch = read_pairing_launch("https://h.test/pair#token=secret-token");
        assert!(!format!("{launch:?}").contains("secret-token"));
        let fields = parse_pairing_url_fields("https://h.test/#token=secret-token", None);
        assert!(!format!("{fields:?}").contains("secret-token"));
    }

    #[test]
    fn the_launch_address_decides_how_the_page_pairs() {
        let hosted = read_pairing_launch("https://app.t3.codes/pair?host=box.test#token=abc");
        assert!(matches!(&hosted.launch, PairingLaunch::Hosted(r) if r.host == "box.test"));
        assert_eq!(hosted.stripped.as_deref(), Some("https://app.t3.codes/pair?host=box.test"));

        let own = read_pairing_launch("http://127.0.0.1:3773/pair/#token=abc");
        assert_eq!(
            own.launch,
            PairingLaunch::Environment {
                host: "http://127.0.0.1:3773".into(),
                token: Sensitive("abc".into()),
            }
        );
        assert_eq!(own.stripped.as_deref(), Some("http://127.0.0.1:3773/pair/"));

        let link = read_pairing_launch(
            "t3code://pair?pairingUrl=https%3A%2F%2Fbox.test%2Fpair%23token%3Dabc&backend=demo",
        );
        assert_eq!(
            link.launch,
            PairingLaunch::AppLink {
                pairing_url: Sensitive("https://box.test/pair#token=abc".into())
            }
        );
        assert_eq!(link.stripped.as_deref(), Some("t3code://pair?backend=demo"));

        for nothing in [
            "https://h.test/pair",
            "https://h.test/settings#token=abc",
            "t3code://thread?pairingUrl=x",
            "not a url",
        ] {
            let read = read_pairing_launch(nothing);
            assert_eq!(read.launch, PairingLaunch::None, "{nothing}");
            assert_eq!(read.stripped, None, "{nothing}");
        }
    }

    #[test]
    fn app_links_follow_the_shared_reader() {
        assert!(read_pairing_app_link("t3code-nightly://pair?pairingUrl=https://b.test").is_some());
        // Schemes parse lower-cased, as the web's `URL` does; a channel is letters only.
        assert!(read_pairing_app_link("t3code-Nightly://pair?pairingUrl=https://b.test").is_some());
        assert!(read_pairing_app_link("t3code-2://pair?pairingUrl=https://b.test").is_none());
        assert!(read_pairing_app_link("t3code://pair/x?pairingUrl=https://b.test").is_none());
        assert!(read_pairing_app_link("t3code://pair?pairingUrl=%20").is_none());
        assert!(read_pairing_app_link("https://pair?pairingUrl=https://b.test").is_none());
    }

    #[test]
    fn the_confirmation_names_the_origin_with_its_scheme() {
        assert_eq!(
            pairing_link_origin("http://box.test:3773/pair#token=abc").as_deref(),
            Ok("http://box.test:3773")
        );
        assert_eq!(
            pairing_link_origin("https://box.test/pair").map_err(|f| f.detail),
            Err("Pairing URL is missing its token.".to_owned())
        );
    }

    // ConnectionsSettings.logic.test.ts: parsePairingUrlFields / parseRemotePairingFields.
    #[test]
    fn a_pasted_pairing_link_fills_host_and_code() {
        let fields = parse_pairing_url_fields("https://remote.example.com/pair#token=pairing-token", None)
            .expect("a link");
        assert_eq!(fields.host, "https://remote.example.com");
        assert_eq!(fields.pairing_code.expose(), "pairing-token");
        let bare = parse_pairing_url_fields("remote.example.com:3773/pair?token=abc", None)
            .expect("no scheme");
        assert_eq!(bare.host, "https://remote.example.com:3773");
        let hosted = parse_pairing_url_fields(
            "https://app.t3.codes/pair?host=https%3A%2F%2Fbox.test#token=abc",
            None,
        )
        .expect("hosted");
        assert_eq!(hosted.host, "https://box.test");
        let relative = parse_pairing_url_fields("//box.test/pair#token=abc", Some("http://page.test"))
            .expect("scheme-relative");
        assert_eq!(relative.host, "http://box.test");
        assert_eq!(parse_pairing_url_fields("remote.example.com", None), None);
        assert_eq!(parse_pairing_url_fields("   ", None), None);
    }

    #[test]
    fn the_remote_fields_say_what_is_missing() {
        let message = |host, code| {
            parse_remote_pairing_fields(host, code, None)
                .map(|fields| (fields.host, fields.pairing_code.0))
                .map_err(|failure| failure.detail)
        };
        assert_eq!(message("  ", "abc"), Err("Enter a backend host.".into()));
        assert_eq!(message("box.test", " "), Err("Enter a pairing code.".into()));
        assert_eq!(
            message(" box.test ", " abc "),
            Ok(("box.test".into(), "abc".into()))
        );
        assert_eq!(
            message("https://box.test/pair#token=xyz", "ignored"),
            Ok(("https://box.test".into(), "xyz".into()))
        );
    }

    // apps/mobile/src/features/connection/pairing.test.ts
    #[test]
    fn mobile_pairing_links_round_trip_through_the_form() {
        assert_eq!(
            build_pairing_url("192.168.1.100:3773", "abc").0,
            "http://192.168.1.100:3773/#token=abc"
        );
        assert_eq!(build_pairing_url("box.test", "abc").0, "https://box.test/#token=abc");
        assert_eq!(build_pairing_url("box.test", " ").0, "box.test");
        assert_eq!(build_pairing_url(" ", "abc").0, "");
        let (host, code) = parse_pairing_url("https://box.test:3773/pair?x=1#token=abc");
        assert_eq!((host.as_str(), code.expose().as_str()), ("https://box.test:3773", "abc"));
        let (host, code) =
            parse_pairing_url("https://app.t3.codes/pair?host=https%3A%2F%2Fbox.test%2F#token=abc");
        assert_eq!((host.as_str(), code.expose().as_str()), ("https://box.test", "abc"));
        let (host, code) = parse_pairing_url("box.test");
        assert_eq!((host.as_str(), code.expose().as_str()), ("box.test", ""));
    }

    #[test]
    fn a_scanned_code_yields_its_pairing_link() {
        assert_eq!(
            extract_pairing_url_from_qr_payload(" https://box.test/pair#token=abc ")
                .map(|url| url.0),
            Ok("https://box.test/pair#token=abc".into())
        );
        assert_eq!(
            extract_pairing_url_from_qr_payload(
                "t3code://pair?pairingUrl=https%3A%2F%2Fbox.test%2Fpair%23token%3Dabc"
            )
            .map(|url| url.0),
            Ok("https://box.test/pair#token=abc".into())
        );
        assert_eq!(
            extract_pairing_url_from_qr_payload("  ").map_err(|failure| failure.detail),
            Err("Scanned QR code did not contain a pairing URL.".into())
        );
    }
}
