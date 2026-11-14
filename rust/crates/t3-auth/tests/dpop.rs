#![allow(clippy::unwrap_used)]
mod common;
use common::*;
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use serde_json::json;
use t3_auth::{AuthConfig, Error, Request, clock::Clock, dpop::*, utils::*};
use t3_contracts::{auth::*, base_schemas::DpopFailureReason};

struct Key {
    signer: SigningKey,
    jwk: PublicJwk,
}
impl Key {
    fn new(byte: u8) -> Self {
        let signer = SigningKey::from_slice(&[byte; 32]).unwrap();
        let point = signer.verifying_key().to_encoded_point(false);
        let jwk = PublicJwk {
            kty: "EC".into(),
            crv: "P-256".into(),
            x: b64(point.x().unwrap()),
            y: b64(point.y().unwrap()),
        };
        Self { signer, jwk }
    }
    fn sign(&self, method: &str, url: &str, token: Option<&str>, iat: i64, jti: &str) -> String {
        let header = b64(serde_json::to_vec(
            &json!({"typ":"dpop+jwt","alg":"ES256","jwk":self.jwk}),
        )
        .unwrap());
        let mut payload =
            json!({"htm":method,"htu":normalize_htu(url).unwrap(),"iat":iat,"jti":jti});
        if let Some(token) = token {
            payload["ath"] = access_token_hash(token).into();
        }
        let body = b64(serde_json::to_vec(&payload).unwrap());
        let message = format!("{header}.{body}");
        let signature: Signature = self.signer.sign(message.as_bytes());
        let signature = signature.normalize_s().unwrap_or(signature);
        format!("{message}.{}", b64(signature.to_bytes()))
    }
}

#[test]
fn proof_validation_matches_signature_claims_time_boundaries_and_diagnostics() {
    let key = Key::new(1);
    let url = "https://example.test/ws?token=x#y";
    let iat = NOW / 1000;
    let proof = key.sign("GET", url, Some("token"), iat, "jti");
    let verify = |proof: &str,
                  method: &str,
                  url: &str,
                  thumb: Option<&str>,
                  token: Option<&str>,
                  now: i64| {
        verify_proof(VerifyProof {
            proof: Some(proof),
            method,
            url,
            expected_thumbprint: thumb,
            expected_access_token: token,
            now_epoch_seconds: now,
            max_age_seconds: None,
        })
    };
    for delta in [-300, 0, 300] {
        assert!(
            verify(
                &proof,
                "get",
                url,
                Some(&thumbprint(&key.jwk)),
                Some("token"),
                iat + delta
            )
            .is_ok()
        );
    }
    for (path, value) in [
        ("alg", json!("none")),
        ("alg", json!("HS256")),
        ("typ", json!("JWT")),
        ("jwk/kty", json!("RSA")),
        ("jwk/crv", json!("P-384")),
        ("jwk/x", json!(b64([1u8; 31]))),
        ("jwk/y", json!(b64([1u8; 33]))),
    ] {
        let parts: Vec<_> = proof.split('.').collect();
        let mut header: serde_json::Value =
            serde_json::from_slice(&unb64(parts[0]).unwrap()).unwrap();
        *header.pointer_mut(&format!("/{path}")).unwrap() = value;
        let body = format!("{}.{}", b64(serde_json::to_vec(&header).unwrap()), parts[1]);
        let signature = if header["alg"] == "HS256" {
            use hmac::{Hmac, Mac};
            let public = key.signer.verifying_key().to_encoded_point(false);
            let mut mac = Hmac::<sha2::Sha256>::new_from_slice(public.as_bytes()).unwrap();
            mac.update(body.as_bytes());
            b64(mac.finalize().into_bytes())
        } else if header["alg"] == "none" {
            b64([0u8; 64])
        } else {
            let sig: Signature = key.signer.sign(body.as_bytes());
            b64(sig.to_bytes())
        };
        assert!(
            verify(
                &format!("{body}.{signature}"),
                "GET",
                url,
                None,
                Some("token"),
                iat
            )
            .is_err(),
            "{path}"
        );
    }
    assert_eq!(
        verify(&proof, "GET", url, Some(""), Some("token"), iat)
            .unwrap_err()
            .code,
        FailureCode::KeyMismatch
    );
    let no_signature = format!("{}.", proof.rsplit_once('.').unwrap().0);
    assert!(verify(&no_signature, "GET", url, None, Some("token"), iat).is_err());
    assert_eq!(
        verify(&proof, "GET", url, None, Some("token"), iat - 301)
            .unwrap_err()
            .code,
        FailureCode::TimeWindow
    );
    let parts: Vec<_> = proof.split('.').collect();
    let low = Signature::from_slice(&unb64(parts[2]).unwrap()).unwrap();
    let high = Signature::from_scalars(low.r().to_bytes(), (-*low.s()).to_bytes()).unwrap();
    let high_proof = format!("{}.{}.{}", parts[0], parts[1], b64(high.to_bytes()));
    assert_eq!(
        verify(&high_proof, "GET", url, None, Some("token"), iat)
            .unwrap_err()
            .code,
        FailureCode::InvalidSignature
    );
    for (p, method, url, thumb, token, now, code) in [
        (
            &proof[..],
            "POST",
            url,
            None,
            Some("token"),
            iat,
            FailureCode::MethodMismatch,
        ),
        (
            &proof[..],
            "GET",
            "https://other.test/ws",
            None,
            Some("token"),
            iat,
            FailureCode::UrlMismatch,
        ),
        (
            &proof[..],
            "GET",
            url,
            Some("bad"),
            Some("token"),
            iat,
            FailureCode::KeyMismatch,
        ),
        (
            &proof[..],
            "GET",
            url,
            None,
            Some("bad"),
            iat,
            FailureCode::AccessTokenHashMismatch,
        ),
        (
            &proof[..],
            "GET",
            url,
            None,
            Some("token"),
            iat + 301,
            FailureCode::TimeWindow,
        ),
        (
            "bad",
            "GET",
            url,
            None,
            None,
            iat,
            FailureCode::MalformedProof,
        ),
        ("  ", "GET", url, None, None, iat, FailureCode::MissingProof),
    ] {
        assert_eq!(
            verify(p, method, url, thumb, token, now).unwrap_err().code,
            code
        );
    }
    let parts: Vec<_> = proof.split('.').collect();
    let mut signature = unb64(parts[2]).unwrap();
    signature[0] ^= 1;
    let invalid = format!("{}.{}.{}", parts[0], parts[1], b64(signature));
    assert_eq!(
        verify(&invalid, "GET", url, None, Some("token"), iat)
            .unwrap_err()
            .code,
        FailureCode::InvalidSignature
    );
    let mut header: serde_json::Value = serde_json::from_slice(&unb64(parts[0]).unwrap()).unwrap();
    header["jwk"]["d"] = json!("private");
    let invalid = format!(
        "{}.{}.{}",
        b64(serde_json::to_vec(&header).unwrap()),
        parts[1],
        parts[2]
    );
    assert_eq!(
        verify(&invalid, "GET", url, None, None, iat)
            .unwrap_err()
            .code,
        FailureCode::MalformedProof
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_proofs_are_single_use_across_concurrent_verifiers_and_failures_are_classified() {
    let h = Harness::new().await;
    let key = Key::new(1);
    let mut request = token_request("token");
    let url = request.absolute_url().unwrap().to_string();
    let proof = key.sign(
        "GET",
        &url,
        Some("token"),
        h.clock.now_ms() / 1000,
        "replay",
    );
    request.headers.insert("dpop".into(), proof.clone());
    let mut malformed = request.clone();
    malformed.headers.insert("dpop".into(), "malformed".into());
    assert!(check_request(&malformed, None, None, Some("token"), h.clock.now_ms()).is_err());
    let checked = check_request(&request, None, None, Some("token"), h.clock.now_ms()).unwrap();
    assert!(checked.thumbprint() == thumbprint(&key.jwk));
    let checked_again =
        check_request(&request, None, None, Some("token"), h.clock.now_ms()).unwrap();
    assert!(checked_again.thumbprint() == checked.thumbprint());
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let store = h.auth.secrets.clone();
        let request = request.clone();
        let barrier = barrier.clone();
        let now = h.clock.now_ms();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            verify_request(&store, &request, None, None, Some("token"), now).await
        }));
    }
    let b = tasks.pop().unwrap().await.unwrap();
    let a = tasks.pop().unwrap().await.unwrap();
    assert_ne!(a.is_ok(), b.is_ok());
    let error = if let Err(e) = a { e } else { b.unwrap_err() };
    assert!(matches!(
        error,
        Error::InvalidCredential {
            dpop_failure_reason: Some(DpopFailureReason::Replay),
            ..
        }
    ));
    assert!(matches!(
        checked.consume(&h.auth.secrets).await,
        Err(Error::InvalidCredential {
            dpop_failure_reason: Some(DpopFailureReason::Replay),
            ..
        })
    ));
    let proof = key.sign(
        "GET",
        &url,
        Some("token"),
        h.clock.now_ms() / 1000,
        "disk-error",
    );
    let marker = format!(
        "dpop-proof-{}",
        b64(sha256(format!("{}:disk-error", thumbprint(&key.jwk))))
    );
    std::fs::create_dir(h.auth.secrets.path(&marker)).unwrap();
    // An existing final path is the source's replay conflict, even when it is a directory.
    assert!(matches!(
        verify_request(
            &h.auth.secrets,
            &request,
            Some(&proof),
            None,
            Some("token"),
            h.clock.now_ms()
        )
        .await,
        Err(Error::InvalidCredential {
            dpop_failure_reason: Some(DpopFailureReason::Replay),
            ..
        })
    ));
    let broken = ServerStoreFailure::new().await;
    assert!(matches!(
        verify_request(
            &broken.store,
            &request,
            Some(&key.sign("GET", &url, Some("token"), h.clock.now_ms() / 1000, "io")),
            None,
            Some("token"),
            h.clock.now_ms()
        )
        .await,
        Err(Error::Secret { .. })
    ));
}
struct ServerStoreFailure {
    store: t3_auth::ServerSecretStore,
    _temp: tempfile::TempDir,
}
impl ServerStoreFailure {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("secrets");
        let store = t3_auth::ServerSecretStore::open(&dir).await.unwrap();
        std::fs::remove_dir(&dir).unwrap();
        std::fs::write(&dir, b"file blocks directory").unwrap();
        Self { store, _temp: temp }
    }
}

#[tokio::test]
async fn dpop_bound_http_tokens_and_socket_audiences_enforce_binding_and_replay() {
    let h = Harness::config(AuthConfig {
        desktop_bootstrap_token: Some("desktop".into()),
        ..Default::default()
    })
    .await;
    let key = Key::new(1);
    let other = Key::new(2);
    let token = h
        .auth
        .exchange_bootstrap(
            &bootstrap_request(Some(thumbprint(&key.jwk))),
            "desktop",
            Some(vec![AuthEnvironmentScope::OrchestrationRead]),
            metadata(),
        )
        .await
        .unwrap();
    let token = &token.access_token.0;
    for scheme in ["DPoP", "dpop", "dPoP"] {
        let mut request = token_request(token);
        request
            .headers
            .insert("authorization".into(), format!("{scheme} {token}"));
        let url = request.absolute_url().unwrap().to_string();
        request.headers.insert(
            "dpop".into(),
            key.sign("GET", &url, Some(token), h.clock.now_ms() / 1000, scheme),
        );
        assert!(h.auth.authenticate_http(&request).await.is_ok());
    }
    assert!(
        h.auth
            .authenticate_http(&token_request(token))
            .await
            .is_err()
    );
    let session = h.auth.sessions.verify(token).await.unwrap();
    let rpc = h
        .auth
        .issue_websocket_ticket(&session, Some(AuthWebSocketTicketUse::Rpc))
        .await
        .unwrap();
    let upgrade = |ticket: &str, proof: Option<String>| {
        let mut url = url::Url::parse("http://192.168.1.50:3773/ws").unwrap();
        url.query_pairs_mut().append_pair("wsTicket", ticket);
        if let Some(proof) = proof {
            url.query_pairs_mut().append_pair("dpop", &proof);
        }
        Request {
            method: "GET".into(),
            url: url.to_string(),
            headers: [("host".into(), "192.168.1.50:3773".into())].into(),
            ..Default::default()
        }
    };
    assert!(
        h.auth
            .authenticate_websocket(&upgrade(&rpc.ticket.0, None), AuthWebSocketTicketUse::Rpc)
            .await
            .is_err()
    );
    let proof = key.sign(
        "GET",
        "http://192.168.1.50:3773/ws",
        Some(&rpc.ticket.0),
        h.clock.now_ms() / 1000,
        "rpc",
    );
    let request = upgrade(&rpc.ticket.0, Some(proof));
    assert!(
        h.auth
            .authenticate_websocket(&request, AuthWebSocketTicketUse::Rpc)
            .await
            .is_ok()
    );
    assert!(matches!(
        h.auth
            .authenticate_websocket(&request, AuthWebSocketTicketUse::Rpc)
            .await,
        Err(Error::InvalidCredential {
            dpop_failure_reason: Some(DpopFailureReason::Replay),
            ..
        })
    ));
    let proof = other.sign(
        "GET",
        "http://192.168.1.50:3773/ws",
        Some(&rpc.ticket.0),
        h.clock.now_ms() / 1000,
        "other",
    );
    assert!(
        h.auth
            .authenticate_websocket(
                &upgrade(&rpc.ticket.0, Some(proof)),
                AuthWebSocketTicketUse::Rpc
            )
            .await
            .is_err()
    );
    assert!(
        h.auth
            .authenticate_websocket(&request, AuthWebSocketTicketUse::Media)
            .await
            .is_err()
    );
    let media = h
        .auth
        .issue_websocket_ticket(&session, Some(AuthWebSocketTicketUse::Media))
        .await
        .unwrap();
    assert!(
        h.auth
            .authenticate_websocket(
                &upgrade(&media.ticket.0, None),
                AuthWebSocketTicketUse::Media
            )
            .await
            .is_ok()
    );
    assert!(
        h.auth
            .authenticate_websocket(&upgrade(&media.ticket.0, None), AuthWebSocketTicketUse::Rpc)
            .await
            .is_err()
    );
    let legacy = h.auth.issue_websocket_ticket(&session, None).await.unwrap();
    assert!(
        h.auth
            .authenticate_websocket(
                &upgrade(&legacy.ticket.0, None),
                AuthWebSocketTicketUse::Rpc
            )
            .await
            .is_ok()
    );
    let unbound = h.auth.issue_session(None, None, None, None).await.unwrap();
    let mut request = token_request(&unbound.token);
    request
        .headers
        .insert("authorization".into(), format!("DPoP {}", unbound.token));
    assert!(h.auth.authenticate_http(&request).await.is_err());
}

#[tokio::test]
async fn oauth_csrf_survives_renewal_only_for_the_same_browser_proof_key() {
    let h = Harness::new().await;
    let oauth = t3_auth::oauth::McpOAuth::new(h.auth.clone(), tokio::sync::watch::channel(true).1)
        .await
        .unwrap();
    let key = Key::new(1);
    let other = Key::new(2);
    let client = oauth
        .register(
            serde_json::from_value(json!({"redirect_uris":["https://example.test/callback"]}))
                .unwrap(),
        )
        .unwrap();
    let urls = t3_auth::oauth::OAuthUrls {
        issuer: "https://example.test".into(),
        resource: "https://example.test/mcp".into(),
    };
    let authorization=oauth.validate_authorization(&urls,serde_json::from_value(json!({"client_id":client.client_id,"redirect_uri":"https://example.test/callback","response_type":"code","code_challenge":b64(sha256("a".repeat(64))),"code_challenge_method":"S256"})).unwrap()).unwrap();
    let session = |key: &Key| {
        let auth = h.auth.clone();
        let thumb = thumbprint(&key.jwk);
        async move {
            auth.sessions
                .issue(t3_auth::session::IssueSession {
                    proof_key_thumbprint: Some(thumb),
                    method: Some(ServerAuthSessionMethod::DpopAccessToken),
                    ..Default::default()
                })
                .await
                .unwrap()
        }
    };
    let request = |key: &Key, token: &str, path: &str, jti: &str| {
        let url = format!("https://example.test{path}");
        Request {
            method: "POST".into(),
            url: url.clone(),
            headers: [
                ("host".into(), "example.test".into()),
                ("authorization".into(), format!("DPoP {token}")),
                (
                    "dpop".into(),
                    key.sign("POST", &url, Some(token), h.clock.now_ms() / 1000, jti),
                ),
            ]
            .into(),
            ..Default::default()
        }
    };
    let first = session(&key).await;
    let details = oauth
        .approval_details(
            &request(&key, &first.token, "/oauth/mcp/approval", "load"),
            &authorization,
        )
        .await
        .unwrap();
    assert_eq!(details.one_click_access.unwrap().len(), 5);
    let csrf = details.csrf_token.unwrap();
    let renewed = session(&key).await;
    let decision =
        serde_json::from_value(json!({"_tag":"browser-session","access":"auto","csrfToken":csrf}))
            .unwrap();
    assert!(
        oauth
            .approve(
                &request(&key, &renewed.token, "/oauth/mcp/decision", "same"),
                &authorization,
                decision
            )
            .await
            .is_ok()
    );
    let elsewhere = session(&other).await;
    let decision =
        serde_json::from_value(json!({"_tag":"browser-session","access":"auto","csrfToken":csrf}))
            .unwrap();
    assert!(
        oauth
            .approve(
                &request(&other, &elsewhere.token, "/oauth/mcp/decision", "other"),
                &authorization,
                decision
            )
            .await
            .is_err()
    );
}
