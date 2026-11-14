#![allow(clippy::unwrap_used)]
mod support;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use support::*;
use t3_cloud::{
    Error, Rejection, Result, Secret,
    direct::*,
    http::{self, Sensitive},
    retry::*,
    tunnel::*,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn direct_origins_reject_credential_leaks_and_preserve_loopback_and_tailnet_rules() {
    for origin in [
        "http://host.ts.net:3773",
        "https://public.test/path?query#fragment",
        "http://127.0.0.8:3773",
        "http://[fd7a:115c:a1e0::1]:3773",
    ] {
        assert!(peer_origin(origin).is_ok(), "{origin}");
    }
    for origin in [
        "http://100.64.0.1:3773",
        "http://192.168.1.1:3773",
        "http://public.test",
        "file:///secret",
        "https://user:marker-secret@public.test",
    ] {
        let error = peer_origin(origin).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("marker-secret"));
    }
    for origin in [
        "ftp://127.0.0.1:3773",
        "http://user:password@127.0.0.1:3773",
        "http://127.0.0.1:3773/api",
        "http://127.0.0.1:3773?mode=test",
        "http://127.0.0.1:3773#fragment",
        "http://127.0.0.1:3773?",
        " http://127.0.0.1:3773",
    ] {
        assert!(parse_local_origin(origin).is_err());
    }
    let origin = parse_local_origin("http://127.0.0.1:3773").unwrap();
    assert!(allowed_link_origin(&origin, "http://127.0.0.1:3773/api", false).is_ok());
    assert!(allowed_link_origin(&origin, "http://remote.test:3773/api", false).is_err());
    assert!(allowed_link_origin(&origin, "http://remote.test:3773/api", true).is_ok());
    assert!(allowed_link_origin(&origin, "http://127.0.0.1:3774/api", false).is_err());
    assert_eq!(
        lan_endpoints(
            [
                "0.0.0.0".parse().unwrap(),
                "127.0.0.1".parse().unwrap(),
                "192.168.1.4".parse().unwrap(),
                "192.168.1.4".parse().unwrap(),
                "::1".parse().unwrap()
            ],
            3773
        )
        .unwrap()
        .len(),
        1
    );
}

#[test]
fn tailscale_status_handles_down_older_daemons_exit_nodes_and_capabilities() {
    let status=parse_status(r#"{"BackendState":"Stopped","Self":{"DNSName":" box.tail.ts.net. ","TailscaleIPs":["100.64.1.2","100.128.1.2","192.168.1.1","fd7a:115c:a1e0::1",null],"CapMap":{}}}"#).unwrap();
    assert_eq!(status.running, Some(false));
    assert_eq!(status.magic_dns_name.as_deref(), Some("box.tail.ts.net"));
    assert_eq!(status.tailnet_ipv4_addresses, ["100.64.1.2"]);
    assert_eq!(status.https_serve_enabled, Some(false));
    let status = parse_status("{}").unwrap();
    assert_eq!(status.running, None);
    assert_eq!(status.https_serve_enabled, None);
    assert!(parse_ssh_enabled(r#"{"RunSSH":true}"#).unwrap());
    assert!(!parse_ssh_enabled(r#"{"RunSSH":true,"WantRunning":false}"#).unwrap());
    assert_eq!(parse_exit_node(r#"{"ExitNodeStatus":{"Online":true},"Peer":{"x":{"ExitNode":true,"DNSName":"renamed.tail.ts.net.","HostName":"old"}}}"#).unwrap(),Some(ExitNode{name:Some("renamed".into()),online:true}));
    assert_eq!(parse_exit_node("{}").unwrap(), None);
    assert!(parse_status(r#"{"Self":"marker-secret"}"#).is_err());
}

#[test]
fn serve_routes_target_only_our_port_and_remove_only_the_selected_handler() {
    let status = json!({"TCP":{"8443":{"TCPForward":"127.0.0.1:3773"},"9000":{"TCPForward":"unix:/socket"}},"Web":{"box.ts.net:443":{"Handlers":{"/":{"Proxy":"3773"},"/other":{"Proxy":"http://127.0.0.1:9999"},"/ambiguous":{"Proxy":"localhost:3773"}}}},"Services":{"svc:one":{"Web":{"service.ts.net:443":{"Handlers":{"/service":{"Proxy":"https+insecure://127.0.0.1:3773"}}}}}},"Foreground":{"session":{"Web":{"box.ts.net:444":{"Handlers":{"/":{"Proxy":"http://[::]:3773"}}}}}}});
    let routes = parse_serve_routes(&status.to_string(), "127.0.0.1", 3773).unwrap();
    assert_eq!(routes.len(), 5);
    let root = routes
        .iter()
        .find(|r| r.mount.as_deref() == Some("/") && r.owner.is_none())
        .unwrap();
    assert_eq!(
        serve_off_args(root).unwrap(),
        ["serve", "--https=443", "--set-path=/", "off"]
    );
    let service = routes
        .iter()
        .find(|r| r.owner.as_deref() == Some("svc:one"))
        .unwrap();
    assert_eq!(
        serve_off_args(service).unwrap(),
        [
            "serve",
            "--service=svc:one",
            "--https=443",
            "--set-path=/service",
            "off"
        ]
    );
    assert!(
        serve_off_args(
            routes
                .iter()
                .find(|r| r.owner.as_deref() == Some("foreground"))
                .unwrap()
        )
        .is_none()
    );
    assert!(
        routes
            .iter()
            .any(|r| r.ambiguous.as_deref() == Some("localhost"))
    );
    assert!(
        parse_serve_routes("null", "127.0.0.1", 3773)
            .unwrap()
            .is_empty()
    );
}
struct FakeCommands(Mutex<Vec<Vec<String>>>);
#[async_trait]
impl Commands for FakeCommands {
    async fn run(&self, args: &[String], _: Duration) -> Result<String> {
        self.0.lock().unwrap().push(args.to_vec());
        Ok(if args == ["status", "--json"] {
            r#"{"Self":{"CapMap":{"https":[]}}}"#.into()
        } else {
            String::new()
        })
    }
}
#[tokio::test]
async fn serve_command_uses_bg_and_path_specific_off_without_contacting_a_daemon() {
    let tailscale = Tailscale(FakeCommands(Mutex::new(Vec::new())));
    tailscale
        .ensure_serve(3773, 443, "127.0.0.1")
        .await
        .unwrap();
    tailscale
        .disable(&ServeRoute {
            port: 443,
            mount: Some("/".into()),
            host: None,
            ambiguous: None,
            owner: None,
        })
        .await
        .unwrap();
    assert_eq!(
        *tailscale.0.0.lock().unwrap(),
        vec![
            vec!["status", "--json"],
            vec!["serve", "--bg", "--https=443", "http://127.0.0.1:3773"],
            vec!["serve", "--https=443", "--set-path=/", "off"]
        ]
    );
}

#[test]
fn relay_error_classification_redacts_secrets_and_caps_retry_after() {
    for (status, rejection) in [
        (401, Rejection::Unauthorized),
        (403, Rejection::Forbidden),
        (409, Rejection::Rejected),
        (302, Rejection::Rejected),
        (408, Rejection::Unavailable),
        (429, Rejection::Unavailable),
        (503, Rejection::Unavailable),
    ] {
        let mut response = response(status, json!({"message":"marker-secret"}));
        response
            .headers
            .insert("retry-after".into(), "999999".into());
        let error = http::check(&response, NOW).unwrap_err();
        assert!(matches!(error,Error::Remote{rejection:r,..} if r==rejection));
        assert!(!format!("{error:?} {error}").contains("marker-secret"));
        if rejection == Rejection::Unavailable {
            assert!(matches!(
                error,
                Error::Remote {
                    retry_after_ms: Some(900_000),
                    ..
                }
            ));
        }
    }
    assert!(matches!(
        http::check(
            &response(
                401,
                json!({"_tag":"RelayEnvironmentLinkProofExpiredError","code":"environment_link_proof_expired","traceId":"test"})
            ),
            NOW
        ),
        Err(Error::Remote {
            rejection: Rejection::Unavailable,
            ..
        })
    ));
    assert_eq!(http::retry_after(Some(&"0".into()), NOW), None);
    assert_eq!(http::retry_after(Some(&"-5".into()), NOW), None);
    let date =
        httpdate::fmt_http_date(std::time::UNIX_EPOCH + Duration::from_millis(NOW as u64 + 60_000));
    assert_eq!(http::retry_after(Some(&date), NOW), Some(60_000));
}

#[test]
fn retry_windows_retry_after_recovery_cooldown_and_connector_crash_backoff() {
    let error = Error::Remote {
        rejection: Rejection::Unavailable,
        retry_after_ms: Some(900_000),
    };
    let mut registration = RetryPolicy::new(RetryMode::Registration, NOW);
    assert!(registration.next(&error, NOW, 0.5).is_none());
    let mut desired = RetryPolicy::new(RetryMode::DesiredLink, NOW);
    assert_eq!(
        desired.next(&error, NOW + 600_000, 0.5),
        Some(Duration::from_millis(900_000))
    );
    for rejection in [
        Rejection::Unauthorized,
        Rejection::Forbidden,
        Rejection::Rejected,
    ] {
        assert!(desired.next(&remote_error(rejection), NOW, 0.5).is_none());
    }
    let mut recovery = RetryPolicy::new(RetryMode::Recovery, NOW);
    assert_eq!(
        recovery.next(&remote_error(Rejection::Unavailable), NOW, 0.5),
        Some(Duration::from_secs(120))
    );
    let mut cooldown = RecoveryCooldown::default();
    assert_eq!(cooldown.wait_ms(NOW), 0);
    cooldown.finished(NOW);
    assert_eq!(cooldown.wait_ms(NOW), 240_000);
    cooldown.finished(NOW + 240_000);
    assert_eq!(cooldown.wait_ms(NOW + 240_000), 480_000);
    assert_eq!(cooldown.wait_ms(NOW + 2_000_000), 0);
    let mut state = ConnectorState::default();
    assert_eq!(state.exited(10), Duration::ZERO);
    assert_eq!(state.exited(10), Duration::from_secs(1));
    assert_eq!(state.exited(30_000), Duration::ZERO);
    assert_eq!(state.exited(10), Duration::ZERO);
}

#[test]
fn connector_protocol_is_token_in_environment_and_recovers_only_after_four_rejections() {
    let config = Sensitive::new(runtime());
    let options =
        connector_options(std::path::Path::new("fake-cloudflared"), &config, true).unwrap();
    assert_eq!(
        options
            .args
            .iter()
            .map(|s| s.to_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "tunnel",
            "--no-autoupdate",
            "--loglevel",
            "info",
            "--output",
            "default",
            "--metrics",
            "127.0.0.1:0",
            "--protocol",
            "http2",
            "run"
        ]
    );
    assert!(!format!("{:?}", options.args).contains("marker-connector-secret"));
    assert_eq!(
        options.env[&std::ffi::OsString::from("TUNNEL_TOKEN")]
            .as_ref()
            .unwrap(),
        "marker-connector-secret"
    );
    let mut state = ConnectorState::default();
    for _ in 0..3 {
        assert_eq!(
            state.observe(
                "ERR Register tunnel error from server side error=\"Unauthorized: marker-secret\"",
                NOW
            ),
            Some(ConnectorEvent::Warning)
        );
    }
    assert_eq!(
        state.observe(
            "ERR Register tunnel error from server side error=\"Unauthorized: marker-secret\"",
            NOW
        ),
        Some(ConnectorEvent::RecoveryRequired)
    );
    assert_eq!(
        state.observe("INF Registered tunnel connection protocol=quic", NOW),
        Some(ConnectorEvent::Connected)
    );
    for _ in 0..29 {
        assert_eq!(
            state.observe("WRN timeout: no recent network activity", NOW),
            Some(ConnectorEvent::Warning)
        );
    }
    assert_eq!(
        state.observe("WRN timeout: no recent network activity", NOW),
        Some(ConnectorEvent::SwitchHttp2)
    );
    state.observe(
        "INF Starting metrics server on 127.0.0.1:40401/metrics",
        NOW,
    );
    assert_eq!(state.metrics_port, Some(40401));
    assert!(!format!("{state:?}").contains("marker-secret"));
}

struct Auth;
#[async_trait]
impl ConnectionAuthenticator for Auth {
    async fn authenticate(&self, c: &UpgradeCredentials) -> Result<Principal> {
        if c.web_socket_ticket
            .as_ref()
            .is_some_and(|s| s.expose() == "accepted-ticket")
        {
            Ok(Principal {
                session_id: "session-one".into(),
                ..Default::default()
            })
        } else {
            Err(Error::Proof)
        }
    }
}
fn credentials(ticket: &str) -> UpgradeCredentials {
    UpgradeCredentials {
        bearer: None,
        dpop: None,
        web_socket_ticket: Some(Secret::new(ticket)),
        method: "GET".into(),
        absolute_url: Secret::new("https://env.test/ws"),
        cookies: Default::default(),
        origin: None,
    }
}
#[tokio::test]
async fn tunnel_ingress_only_yields_authenticated_application_streams() {
    let (ingress, mut accepted) = Ingress::channel(1, Arc::new(Auth)).unwrap();
    let (server, mut remote) = tokio::io::duplex(1024);
    assert!(matches!(
        ingress
            .accept(Box::new(server), credentials("marker-rejected-ticket"))
            .await,
        Err(Error::Proof)
    ));
    let (server, mut remote2) = tokio::io::duplex(1024);
    ingress
        .accept(Box::new(server), credentials("accepted-ticket"))
        .await
        .unwrap();
    let mut accepted = accepted.next().await.unwrap();
    assert_eq!(accepted.principal.session_id, "session-one");
    remote2.write_all(b"rpc-request").await.unwrap();
    let mut request = [0; 11];
    accepted.io.read_exact(&mut request).await.unwrap();
    assert_eq!(&request, b"rpc-request");
    accepted.io.write_all(b"rpc-response").await.unwrap();
    let mut response = [0; 12];
    remote2.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"rpc-response");
    let mut byte = [0; 1];
    assert_eq!(remote.read(&mut byte).await.unwrap(), 0);
    assert!(!format!("{:?}", credentials("marker-secret")).contains("marker-secret"));
}

#[tokio::test(start_paused = true)]
async fn retry_reruns_the_operation_after_virtual_time_advances() {
    let time = Arc::new(Time::default());
    let (sent, mut recv) = tokio::sync::mpsc::unbounded_channel();
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = count.clone();
    let task = tokio::spawn(retry(
        time.clone(),
        RetryPolicy::new(RetryMode::DesiredLink, NOW),
        move || {
            let n = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            sent.send(n).unwrap();
            async move {
                if n == 0 {
                    Err(remote_error(Rejection::Unavailable))
                } else {
                    Ok(())
                }
            }
        },
    ));
    assert_eq!(recv.recv().await, Some(0));
    time.advance(2000);
    tokio::time::advance(Duration::from_secs(2)).await;
    assert_eq!(recv.recv().await, Some(1));
    task.await.unwrap().unwrap();
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn all_service_error_and_debug_surfaces_hide_marker_credentials() {
    let secret = Secret::new("marker-secret");
    let request = http::Request {
        method: reqwest::Method::POST,
        url: url::Url::parse("https://test/marker-secret").unwrap(),
        headers: BTreeMap::from([("authorization".into(), secret.clone())]),
        body: b"marker-secret".to_vec(),
        timeout: Duration::from_secs(1),
    };
    let response = response(500, json!({"secret":"marker-secret"}));
    assert!(!format!("{secret:?} {request:?} {response:?}").contains("marker-secret"));
    let error = Error::Protocol;
    assert!(!format!("{error:?} {error}").contains("marker-secret"));
}

#[tokio::test]
async fn metrics_inspection_uses_only_the_connector_loopback_port_and_decodes_ready_and_config() {
    let relay = Relay::default();
    relay
        .add(|r| {
            assert_eq!(r.url.as_str(), "http://127.0.0.1:40401/ready");
            assert!(r.headers.is_empty());
            Ok(response(503, json!({"readyConnections":0})))
        })
        .await;
    relay
        .add(|r| {
            assert_eq!(r.url.as_str(), "http://127.0.0.1:40401/config");
            Ok(response(
                200,
                json!({"config":{"ingress":[{"hostname":"env.test"},{}]}}),
            ))
        })
        .await;
    assert_eq!(
        inspect_metrics(&relay, 40401).await.unwrap(),
        (0, Some("env.test".into()))
    );
    let mut state = ConnectorState::with_transport_choice(true);
    for _ in 0..35 {
        assert_ne!(
            state.observe("WRN timeout: no recent network activity", NOW),
            Some(ConnectorEvent::SwitchHttp2)
        );
    }
    relay.done().await;
}

#[tokio::test]
async fn reqwest_transport_preserves_http_bytes_and_never_follows_a_redirect() {
    use t3_cloud::http::Transport;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut received = Vec::new();
        let mut buffer = [0; 1024];
        loop {
            let n = socket.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            received.extend_from_slice(&buffer[..n]);
            if received.windows(4).any(|w| w == b"\r\n\r\n") && received.ends_with(b"marker-body") {
                break;
            }
        }
        let text = String::from_utf8(received).unwrap();
        assert!(text.starts_with("POST /v1/test HTTP/1.1"));
        assert!(
            text.to_lowercase()
                .contains("authorization: bearer marker-secret")
        );
        socket.write_all(format!("HTTP/1.1 302 Found\r\nLocation: http://{address}/must-not-follow\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").as_bytes()).await.unwrap();
    });
    let transport = http::ReqwestTransport::new().unwrap();
    let result = transport
        .send(http::Request {
            method: reqwest::Method::POST,
            url: url::Url::parse(&format!("http://{address}/v1/test")).unwrap(),
            headers: BTreeMap::from([(
                "authorization".into(),
                Secret::new("Bearer marker-secret"),
            )]),
            body: b"marker-body".to_vec(),
            timeout: Duration::from_secs(5),
        })
        .await
        .unwrap();
    assert_eq!(result.status, 302);
    server.await.unwrap();
}

#[tokio::test]
async fn connector_supervisor_observes_a_real_owned_child_and_joins_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    let executable = {
        let path = directory.path().join("connector.cmd");
        std::fs::write(&path, b"@echo off\r\nif not \"%TUNNEL_TOKEN%\"==\"marker-connector-secret\" exit /b 11\r\necho Starting metrics server on 127.0.0.1:40401/metrics\r\necho Registered tunnel connection protocol=http2\r\nset /p hold=\r\n").unwrap();
        path
    };
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.path().join("connector");
        std::fs::write(&path, b"#!/bin/sh\n[ \"$TUNNEL_TOKEN\" = \"marker-connector-secret\" ] || exit 11\nprintf '%s\\n' 'Starting metrics server on 127.0.0.1:40401/metrics' 'Registered tunnel connection protocol=http2'\nread hold\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    };
    let handle = start_connector(
        executable,
        Sensitive::new(runtime()),
        Arc::new(Time::default()),
        ConnectorServices {
            http: Arc::new(Relay::default()),
            secrets: Arc::new(MemorySecrets::default()),
        },
    )
    .unwrap();
    let mut events = handle.events.clone();
    let mut facts = handle.facts.clone();
    events
        .wait_for(|event| *event == Some(ConnectorEvent::Connected))
        .await
        .unwrap();
    let observed = facts.borrow().clone();
    assert!(observed.pid.is_some());
    assert_eq!(observed.metrics_port, Some(40401));
    assert!(observed.http2);
    handle.stop().await.unwrap();
    assert!(facts.changed().await.is_ok());
    assert!(facts.changed().await.is_err());
}

#[test]
fn reachability_classification_prioritizes_origin_and_dns_over_a_failed_tcp_probe() {
    use t3_cloud::reachability::{EndpointProbe, ReachabilityFacts, classify};
    use t3_contracts::environment_http::{
        EnvironmentConnectReachability, EnvironmentConnectReachabilityCause,
    };
    let mut facts = ReachabilityFacts {
        configured: true,
        connector: Some(ConnectorFacts {
            ready_connections: Some(0),
            ..Default::default()
        }),
        endpoint: Some(EndpointProbe::Status {
            status: 502,
            cloudflare_error: None,
        }),
        edge_reachable: Some(false),
        exit_node: None,
    };
    assert!(matches!(
        classify(&facts, "date".into()),
        EnvironmentConnectReachability::Unreachable {
            cause: EnvironmentConnectReachabilityCause::OriginUnreachable,
            ..
        }
    ));
    facts.connector.as_mut().unwrap().ready_connections = Some(1);
    facts.endpoint = Some(EndpointProbe::DnsFailed {
        exists_publicly: Some(true),
    });
    assert!(matches!(
        classify(&facts, "date".into()),
        EnvironmentConnectReachability::Unreachable {
            cause: EnvironmentConnectReachabilityCause::DnsFailed,
            ..
        }
    ));
    facts.endpoint = Some(EndpointProbe::Status {
        status: 530,
        cloudflare_error: Some("1016".into()),
    });
    assert!(matches!(
        classify(&facts, "date".into()),
        EnvironmentConnectReachability::Unreachable {
            cause: EnvironmentConnectReachabilityCause::UnexpectedResponse,
            ..
        }
    ));
}
