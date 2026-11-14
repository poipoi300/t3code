//! How a saved environment's row describes it: its transport and its connection status.
//!
//! Ports `apps/web/src/components/settings/SavedEnvironmentRows.tsx` `savedBackendStatus`,
//! `EnvironmentRow.tsx` `environmentTransportLabel` and `packages/client-runtime/src/connection/
//! routes.ts` `connectionRouteLabel`. Status text with a reason is returned as a key and the
//! reason apart, so the UI translates the sentence and shows the server's reason as it is.

use url::Url;

use super::{ConnectionMode, ConnectionPresentation, PresentationPhase, Route, RouteKind, route_kind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    Muted,
    Error,
}

/// A status line: an owned English sentence, that sentence with the failure's reason
/// interpolated as `{reason}`, or text the host sent (a device-approval instruction).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusText {
    Key(&'static str),
    WithReason { key: &'static str, reason: String },
    Content(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedEnvironmentStatus {
    pub text: StatusText,
    pub tone: StatusTone,
}

/// `savedBackendStatus`. `enabled` is the saved switch; an unsupported client is described as
/// such even when switched off.
pub fn saved_environment_status(
    enabled: bool,
    connection: &ConnectionPresentation,
) -> SavedEnvironmentStatus {
    use StatusText::{Content, Key, WithReason};
    let muted = |text| SavedEnvironmentStatus {
        text,
        tone: StatusTone::Muted,
    };
    let error = |text| SavedEnvironmentStatus {
        text,
        tone: StatusTone::Error,
    };
    if !enabled && connection.phase != PresentationPhase::Unsupported {
        return muted(Key("Off"));
    }
    match connection.phase {
        PresentationPhase::Connected => muted(Key("Connected")),
        PresentationPhase::Connecting => muted(Key("Connecting")),
        PresentationPhase::Reconnecting if connection.awaiting_approval => muted(
            connection
                .error
                .clone()
                .map_or(Key("Waiting for approval"), Content),
        ),
        PresentationPhase::Reconnecting => error(connection.error.clone().map_or(
            Key("Reconnecting"),
            |reason| WithReason {
                key: "Reconnecting: {reason}",
                reason,
            },
        )),
        PresentationPhase::Unsupported => muted(Key("Client not supported")),
        PresentationPhase::Error => error(connection.error.clone().map_or(
            Key("Connection failed"),
            |reason| WithReason {
                key: "Connection failed: {reason}",
                reason,
            },
        )),
        PresentationPhase::Offline => muted(Key("Offline")),
        PresentationPhase::Available => muted(Key("Not connected")),
    }
}

/// A route's label: a key for the kinds the web names, the address's host name for a public one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteLabel {
    Key(&'static str),
    Address(String),
}

fn route_hostname(route: &Route) -> Option<String> {
    Url::parse(&route.http_base)
        .ok()?
        .host_str()
        .map(|host| host.trim_start_matches('[').trim_end_matches(']').to_owned())
}

/// `connectionRouteLabel`. The client keeps no SSH profile on the route, so an SSH route is "SSH".
pub fn connection_route_label(route: &Route) -> RouteLabel {
    match route_kind(route, false) {
        RouteKind::Relay => RouteLabel::Key("T3 Connect"),
        RouteKind::Loopback => RouteLabel::Key("This device"),
        RouteKind::Lan => RouteLabel::Key("LAN"),
        RouteKind::Tailnet => RouteLabel::Key("Tailscale"),
        RouteKind::Vpn => RouteLabel::Key("VPN"),
        RouteKind::Ssh => RouteLabel::Key("SSH"),
        RouteKind::Public => {
            route_hostname(route).map_or(RouteLabel::Key("Remote link"), RouteLabel::Address)
        }
    }
}

/// `environmentTransportLabel` without the connected route, which the client does not expose:
/// several routes name the preferred one; one relay route is T3 Connect; one direct route shows
/// its address.
pub fn environment_transport_label(routes: &[Route]) -> RouteLabel {
    match routes {
        [] => RouteLabel::Key("Remote link"),
        [route] => match route.mode {
            ConnectionMode::Relay => RouteLabel::Key("T3 Connect"),
            ConnectionMode::Ssh => RouteLabel::Key("SSH"),
            _ if route.http_base.trim().is_empty() => RouteLabel::Key("Remote link"),
            _ => RouteLabel::Address(route.http_base.trim_end_matches('/').to_owned()),
        },
        [first, ..] => connection_route_label(first),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EnvironmentId;

    fn presentation(phase: PresentationPhase, error: Option<&str>) -> ConnectionPresentation {
        ConnectionPresentation {
            phase,
            error: error.map(str::to_owned),
            trace_id: None,
            awaiting_approval: false,
        }
    }

    fn route(mode: ConnectionMode, base: &str) -> Route {
        Route {
            id: "r".into(),
            environment: EnvironmentId("e".into()),
            mode,
            http_base: base.into(),
            ws_base: String::new(),
        }
    }

    #[test]
    fn the_status_follows_saved_backend_status() {
        use PresentationPhase::*;
        let status = |enabled, phase, error| saved_environment_status(enabled, &presentation(phase, error));
        assert_eq!(status(false, Connected, None).text, StatusText::Key("Off"));
        assert_eq!(
            status(false, Unsupported, None).text,
            StatusText::Key("Client not supported"),
            "an unsupported client says so even when switched off"
        );
        assert_eq!(status(true, Available, None).text, StatusText::Key("Not connected"));
        let failed = status(true, Error, Some("Connection credential c is unavailable."));
        assert_eq!(failed.tone, StatusTone::Error);
        assert_eq!(
            failed.text,
            StatusText::WithReason {
                key: "Connection failed: {reason}",
                reason: "Connection credential c is unavailable.".into()
            }
        );
        assert_eq!(status(true, Reconnecting, None).tone, StatusTone::Error);
        let waiting = saved_environment_status(
            true,
            &ConnectionPresentation {
                awaiting_approval: true,
                ..presentation(Reconnecting, Some("Approve this device on box (code 1234)."))
            },
        );
        assert_eq!(waiting.tone, StatusTone::Muted, "waiting for approval is not a failure");
        assert_eq!(
            waiting.text,
            StatusText::Content("Approve this device on box (code 1234).".into())
        );
    }

    #[test]
    fn the_transport_names_the_route_the_way_the_web_does() {
        assert_eq!(
            environment_transport_label(&[route(ConnectionMode::Relay, "")]),
            RouteLabel::Key("T3 Connect")
        );
        assert_eq!(
            environment_transport_label(&[route(ConnectionMode::Remote, "https://box.example.com/")]),
            RouteLabel::Address("https://box.example.com".into())
        );
        assert_eq!(
            environment_transport_label(&[
                route(ConnectionMode::Remote, "http://192.168.1.4:3773/"),
                route(ConnectionMode::Relay, ""),
            ]),
            RouteLabel::Key("LAN")
        );
        assert_eq!(
            connection_route_label(&route(ConnectionMode::Remote, "https://box.example.com/")),
            RouteLabel::Address("box.example.com".into())
        );
    }
}
