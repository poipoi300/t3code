#![cfg(feature = "local-services")]
#![allow(clippy::unwrap_used)]
//! Environment auth routes over real HTTP: `apps/server/src/auth/http.ts`.
mod http_common;
use http_common::*;
use reqwest::Method;
use serde_json::{Value, json};

/// The `sid` claim of a signed session token (`base64url(JSON).base64url(MAC)`).
fn session_id(token: &str) -> String {
    let body = token.split('.').next().unwrap();
    let claims: Value = serde_json::from_slice(&t3_auth::utils::unb64(body).unwrap()).unwrap();
    claims["sid"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn descriptor_session_state_and_cors() {
    let f = fixture(&[]).await;

    let descriptor = f.get("/.well-known/t3/environment").send().await.unwrap();
    assert_eq!(descriptor.status(), 200);
    assert_eq!(descriptor.headers()["access-control-allow-origin"], "*");
    let body = json(descriptor).await;
    assert!(
        body["environmentId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert_eq!(body["orchestrationProtocolVersion"], 2);

    let anonymous = f.get("/api/auth/session").send().await.unwrap();
    assert_eq!(
        anonymous.status(),
        200,
        "no credential is a state, not a 401"
    );
    assert_no_store(&anonymous);
    assert_eq!(json(anonymous).await["authenticated"], false);

    let garbage = f
        .get("/api/auth/session")
        .header("cookie", "t3_session_x=not-a-token; other=1")
        .header("authorization", "Bearer not-a-token")
        .send()
        .await
        .unwrap();
    assert_eq!(garbage.status(), 200);
    assert_eq!(json(garbage).await["authenticated"], false);

    let cookie = f.admin_cookie().await;
    let session = json(
        f.get("/api/auth/session")
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(session["authenticated"], true);
    assert_eq!(session["sessionMethod"], "browser-session-cookie");
    assert!(
        session["permissions"]
            .as_array()
            .unwrap()
            .contains(&json!("access:write"))
    );

    // Cookie credentials are only honoured from the app's own origin.
    let foreign = f
        .get("/api/auth/session")
        .header("cookie", &cookie)
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(json(foreign).await["authenticated"], false);
    let own = f
        .get("/api/auth/session")
        .header("cookie", &cookie)
        .header("origin", f.base.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(json(own).await["authenticated"], true);

    // CORS answers every OPTIONS with 204, and decorates failures too.
    let preflight = f
        .request(Method::OPTIONS, "/api/auth/pairing-token")
        .header("origin", "https://app.example")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "authorization, dpop")
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), 204);
    let headers = preflight.headers();
    assert_eq!(headers["access-control-allow-origin"], "*");
    assert_eq!(
        headers["access-control-allow-methods"],
        "GET, POST, OPTIONS"
    );
    assert_eq!(headers["access-control-max-age"], "600");
    let allowed = headers["access-control-allow-headers"].to_str().unwrap();
    for name in [
        "authorization",
        "b3",
        "traceparent",
        "content-type",
        "dpop",
        "x-t3-orchestration-protocol",
    ] {
        assert!(allowed.split(',').any(|h| h == name), "{allowed}");
    }
    assert!(headers.get("access-control-allow-credentials").is_none());
    let failure = f
        .post("/api/auth/pairing-token")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(failure.headers()["access-control-allow-origin"], "*");
    assert_error(failure, 401, "EnvironmentAuthInvalidError", "auth_invalid").await;
    assert_eq!(
        f.get("/api/does-not-exist").send().await.unwrap().headers()["access-control-allow-origin"],
        "*"
    );
    f.stop().await;
}

#[tokio::test]
async fn browser_session_cookie_credential_reuse_and_bad_bodies() {
    let f = fixture(&[]).await;
    let (cookie, response) = f.browser_session(&f.startup_credential()).await;
    assert_eq!(response.status(), 200);
    assert_no_store(&response);
    let set_cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_owned();
    for attribute in ["HttpOnly", "Path=/", "SameSite=Lax", "Expires="] {
        assert!(set_cookie.contains(attribute), "{set_cookie}");
    }
    assert!(
        !set_cookie.contains("Secure"),
        "plain HTTP stays usable: {set_cookie}"
    );
    assert!(cookie.starts_with("t3_session_"), "{cookie}");
    let body = json(response).await;
    assert_eq!(body["authenticated"], true);
    assert_eq!(body["sessionMethod"], "browser-session-cookie");

    // The startup credential is single use.
    let (_, reuse) = f.browser_session(&f.startup_credential()).await;
    let error = assert_error(reuse, 401, "EnvironmentAuthInvalidError", "auth_invalid").await;
    assert_eq!(error["reason"], "invalid_credential");
    let (_, unknown) = f.browser_session("ABCDEFGHJKLM").await;
    assert_error(unknown, 401, "EnvironmentAuthInvalidError", "auth_invalid").await;

    // Behind a TLS-terminating proxy every cookie is Secure.
    let fresh = f.credential(&cookie, &["orchestration:read"]).await;
    let secure = f
        .post("/api/auth/browser-session")
        .header("x-forwarded-proto", "https")
        .json(&json!({ "credential": fresh }))
        .send()
        .await
        .unwrap();
    assert_eq!(secure.status(), 200);
    assert!(
        secure.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Secure")
    );

    for body in [
        "not json",
        "{}",
        "[]",
        r#"{"credential":5}"#,
        r#"{"credential":""}"#,
        "",
    ] {
        let response = f
            .post("/api/auth/browser-session")
            .header("content-type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{body:?}");
    }
    // Refused from the declared length, before any of the body is read.
    let oversized = f
        .post("/api/auth/browser-session")
        .header("content-type", "application/json")
        .body(format!(r#"{{"credential":"{}"}}"#, "A".repeat(70_000)))
        .send()
        .await
        .unwrap();
    assert_eq!(oversized.status(), 413);
    f.stop().await;
}

#[tokio::test]
async fn token_exchange_scopes_proofs_and_bearer_use() {
    let f = fixture(&[]).await;
    let admin = f.admin_cookie().await;
    let credential = f.credential(&admin, &["orchestration:read"]).await;

    // A scope the credential does not grant is refused without spending it.
    let not_granted = f
        .token_exchange(&credential, Some("orchestration:operate"))
        .send()
        .await
        .unwrap();
    assert_no_store(&not_granted);
    let body = assert_error(
        not_granted,
        400,
        "EnvironmentRequestInvalidError",
        "invalid_request",
    )
    .await;
    assert_eq!(body["reason"], "scope_not_granted");
    let unknown_scope = f
        .token_exchange(&credential, Some("bogus:scope"))
        .send()
        .await
        .unwrap();
    let body = assert_error(
        unknown_scope,
        400,
        "EnvironmentRequestInvalidError",
        "invalid_request",
    )
    .await;
    assert_eq!(body["reason"], "invalid_scope");

    let response = f
        .token_exchange(&credential, Some("orchestration:read"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_no_store(&response);
    let token = json(response).await;
    assert_eq!(token["token_type"], "Bearer");
    assert_eq!(token["scope"], "orchestration:read");
    assert_eq!(
        token["issued_token_type"],
        "urn:ietf:params:oauth:token-type:access_token"
    );
    let bearer = token["access_token"].as_str().unwrap().to_owned();
    let spent = f.token_exchange(&credential, None).send().await.unwrap();
    assert_error(spent, 401, "EnvironmentAuthInvalidError", "auth_invalid").await;

    // The bearer carries exactly the narrowed scopes, in any scheme case.
    for scheme in ["Bearer", "bearer", "BEARER"] {
        let session = json(
            f.get("/api/auth/session")
                .header("authorization", format!("{scheme} {bearer}"))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(session["authenticated"], true, "{scheme}");
        assert_eq!(session["permissions"], json!(["orchestration:read"]));
        assert_eq!(session["sessionMethod"], "bearer-access-token");
    }
    // Two Authorization headers are ambiguous and cannot authenticate, whatever their values.
    let duplicate = f
        .post("/api/auth/websocket-ticket")
        .header("authorization", format!("Bearer {bearer}"))
        .header("authorization", format!("Bearer {bearer}"))
        .send()
        .await
        .unwrap();
    assert_error(
        duplicate,
        401,
        "EnvironmentAuthInvalidError",
        "auth_invalid",
    )
    .await;

    // A duplicate that is not valid UTF-8 still counts: it cannot be dropped to hide the ambiguity.
    let opaque = f
        .post("/api/auth/websocket-ticket")
        .header("authorization", format!("Bearer {bearer}"))
        .header(
            "authorization",
            reqwest::header::HeaderValue::from_bytes(b"Bearer \xff\xfe").unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(opaque.status(), 401);

    // A rejected bootstrap credential is not a DPoP failure: no challenge.
    let wrong = f.token_exchange("ABCDEFGHJKLM", None).send().await.unwrap();
    assert!(wrong.headers().get("www-authenticate").is_none());
    assert_error(wrong, 401, "EnvironmentAuthInvalidError", "auth_invalid").await;
    // A DPoP header that is not a valid proof is, and says why.
    let fresh = f.credential(&admin, &["orchestration:read"]).await;
    let proof = f
        .token_exchange(&fresh, None)
        .header("dpop", "not.a.jwt")
        .send()
        .await
        .unwrap();
    assert_eq!(proof.headers()["www-authenticate"], "DPoP");
    let body = assert_error(proof, 401, "EnvironmentAuthInvalidError", "auth_invalid").await;
    assert!(body["dpopFailureReason"].is_string(), "{body}");
    // The refused proof did not burn the credential.
    assert_eq!(
        f.token_exchange(&fresh, None)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    // JSON is accepted as well as the contract's form encoding.
    let json_credential = f.credential(&admin, &["orchestration:read"]).await;
    let json_exchange = f
        .post("/oauth/token")
        .json(&json!({
            "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
            "subject_token": json_credential,
            "subject_token_type": "urn:t3:params:oauth:token-type:environment-bootstrap",
            "requested_token_type": "urn:ietf:params:oauth:token-type:access_token",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(json_exchange.status(), 200);

    // A repeated field is ambiguous even when every value is valid; the credential is not spent.
    let repeated = f.credential(&admin, &["orchestration:read"]).await;
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair(
        "grant_type",
        "urn:ietf:params:oauth:grant-type:token-exchange",
    )
    .append_pair("subject_token", &repeated)
    .append_pair(
        "subject_token_type",
        "urn:t3:params:oauth:token-type:environment-bootstrap",
    )
    .append_pair(
        "requested_token_type",
        "urn:ietf:params:oauth:token-type:access_token",
    )
    .append_pair("client_label", "one")
    .append_pair("client_label", "one");
    let response = f
        .post("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form.finish())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        f.token_exchange(&repeated, None)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    // Malformed: missing fields, wrong grant type, duplicated form fields, bad scope text.
    for body in [
        "",
        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange",
        "grant_type=password&subject_token=x&subject_token_type=y&requested_token_type=z",
        "grant_type=a&grant_type=b",
    ] {
        let response = f
            .post("/oauth/token")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body.to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{body:?}");
    }
    f.stop().await;
}

#[tokio::test]
async fn websocket_tickets() {
    let f = fixture(&[]).await;
    let anonymous = f.post("/api/auth/websocket-ticket").send().await.unwrap();
    let body = assert_error(
        anonymous,
        401,
        "EnvironmentAuthInvalidError",
        "auth_invalid",
    )
    .await;
    assert_eq!(body["reason"], "missing_credential");
    // Authentication precedes parameter checks: no probing routes without a session.
    let probe = f
        .post("/api/auth/websocket-ticket/banana")
        .send()
        .await
        .unwrap();
    assert_eq!(probe.status(), 401);
    let challenge = f
        .post("/api/auth/websocket-ticket")
        .header("authorization", "DPoP not-a-token")
        .send()
        .await
        .unwrap();
    assert_eq!(challenge.headers()["www-authenticate"], "DPoP");
    let plain = f
        .post("/api/auth/websocket-ticket")
        .header("authorization", "Bearer not-a-token")
        .send()
        .await
        .unwrap();
    assert!(plain.headers().get("www-authenticate").is_none());
    assert_eq!(plain.status(), 401);

    let cookie = f.admin_cookie().await;
    let foreign = f
        .post("/api/auth/websocket-ticket/rpc")
        .header("cookie", &cookie)
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), 401);
    for path in [
        "/api/auth/websocket-ticket",
        "/api/auth/websocket-ticket/rpc",
        "/api/auth/websocket-ticket/media",
    ] {
        let response = f.post(path).header("cookie", &cookie).send().await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert_no_store(&response);
        let body = json(response).await;
        assert!(body["ticket"].as_str().is_some_and(|t| t.contains('.')));
        assert!(body["expiresAt"].is_string());
    }
    let bad = f
        .post("/api/auth/websocket-ticket/banana")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
    // Any scope may mint a ticket: it only carries the session's own authority.
    let narrow = f.credential(&cookie, &["orchestration:read"]).await;
    let bearer = f.bearer(&narrow, None).await;
    let response = f
        .post("/api/auth/websocket-ticket/media")
        .header("authorization", format!("Bearer {bearer}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    f.stop().await;
}

#[tokio::test]
async fn pairing_credentials_and_delegation() {
    let f = fixture(&[]).await;
    let unauthenticated = f
        .post("/api/auth/pairing-token")
        .json(&json!({"scopes": ["orchestration:read"]}))
        .send()
        .await
        .unwrap();
    assert_error(
        unauthenticated,
        401,
        "EnvironmentAuthInvalidError",
        "auth_invalid",
    )
    .await;

    let admin = f.admin_cookie().await;
    let read_only = f.credential(&admin, &["orchestration:read"]).await;
    let read_only = f.bearer(&read_only, None).await;
    let wrong_scope = f
        .post("/api/auth/pairing-token")
        .header("authorization", format!("Bearer {read_only}"))
        .json(&json!({"scopes": ["orchestration:read"]}))
        .send()
        .await
        .unwrap();
    let body = assert_error(
        wrong_scope,
        403,
        "EnvironmentScopeRequiredError",
        "insufficient_scope",
    )
    .await;
    assert_eq!(body["requiredScope"], "access:write");
    assert_eq!(body["requiredPermission"], "access:write");
    // The scope check comes first: a caller without it is refused even with a malformed body.
    let wrong_scope_malformed = f
        .post("/api/auth/pairing-token")
        .header("authorization", format!("Bearer {read_only}"))
        .body("not json")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_scope_malformed.status(), 403);

    // The pairing request must state its grant; duplicates and empty grants are invalid.
    for body in [
        json!({}),
        json!({"scopes": []}),
        json!({"scopes": ["orchestration:read", "orchestration:read"]}),
    ] {
        let response = f
            .post("/api/auth/pairing-token")
            .header("cookie", &admin)
            .json(&body)
            .send()
            .await
            .unwrap();
        let error = assert_error(
            response,
            400,
            "EnvironmentRequestInvalidError",
            "invalid_request",
        )
        .await;
        assert_eq!(error["reason"], "invalid_scope", "{body}");
    }
    for body in [
        "not json",
        "[]",
        r#"{"scopes":"orchestration:read"}"#,
        r#"{"scopes":["nonsense"]}"#,
        r#"{"access":"root"}"#,
    ] {
        let response = f
            .post("/api/auth/pairing-token")
            .header("cookie", &admin)
            .header("content-type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{body}");
    }

    let issued = f
        .post("/api/auth/pairing-token")
        .header("cookie", &admin)
        .json(&json!({"scopes": ["orchestration:read"], "label": "Laptop"}))
        .send()
        .await
        .unwrap();
    assert_eq!(issued.status(), 200);
    assert_no_store(&issued);
    let issued = json(issued).await;
    assert_eq!(issued["label"], "Laptop");
    assert!(issued["credential"].as_str().is_some_and(|c| c.len() >= 8));
    assert!(issued["expiresAt"].is_string());
    let app = json(
        f.post("/api/auth/pairing-token")
            .header("cookie", &admin)
            .json(&json!({"access": "read-only"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(app["access"], "read-only");

    // A caller can only delegate what it holds.
    let delegate = f
        .credential(&admin, &["access:write", "orchestration:read"])
        .await;
    let delegate = f.bearer(&delegate, None).await;
    let as_delegate = |body: Value| {
        f.post("/api/auth/pairing-token")
            .header("authorization", format!("Bearer {delegate}"))
            .json(&body)
    };
    let missing = as_delegate(json!({"scopes": ["terminal:operate"]}))
        .send()
        .await
        .unwrap();
    let body = assert_error(
        missing,
        403,
        "EnvironmentScopeRequiredError",
        "insufficient_scope",
    )
    .await;
    assert_eq!(body["requiredPermission"], "terminal:operate");
    let escalation = as_delegate(json!({"access": "full-access"}))
        .send()
        .await
        .unwrap();
    let body = assert_error(
        escalation,
        403,
        "EnvironmentScopeRequiredError",
        "insufficient_scope",
    )
    .await;
    assert_eq!(body["requiredPermission"], "orchestration:operate");
    let narrower = as_delegate(json!({"scopes": ["orchestration:read"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(narrower.status(), 200);
    f.stop().await;
}

#[tokio::test]
async fn pairing_link_revocation() {
    let f = fixture(&[]).await;
    let admin = f.admin_cookie().await;
    let unauthenticated = f
        .post("/api/auth/pairing-links/revoke")
        .json(&json!({"id": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), 401);
    let read_only = f.credential(&admin, &["orchestration:read"]).await;
    let read_only = f.bearer(&read_only, None).await;
    let wrong_scope = f
        .post("/api/auth/pairing-links/revoke")
        .header("authorization", format!("Bearer {read_only}"))
        .json(&json!({"id": "x"}))
        .send()
        .await
        .unwrap();
    assert_error(
        wrong_scope,
        403,
        "EnvironmentScopeRequiredError",
        "insufficient_scope",
    )
    .await;

    let link = json(
        f.post("/api/auth/pairing-token")
            .header("cookie", &admin)
            .json(&json!({"scopes": ["orchestration:read"]}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let revoke = |id: &str| {
        f.post("/api/auth/pairing-links/revoke")
            .header("cookie", &admin)
            .json(&json!({"id": id}))
    };
    for body in ["not json", "{}", r#"{"id":7}"#] {
        let response = f
            .post("/api/auth/pairing-links/revoke")
            .header("cookie", &admin)
            .header("content-type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{body}");
    }
    let id = link["id"].as_str().unwrap();
    let revoked = revoke(id).send().await.unwrap();
    assert_eq!(revoked.status(), 200);
    assert_eq!(json(revoked).await, json!({"revoked": true}));
    assert_eq!(
        json(revoke(id).send().await.unwrap()).await,
        json!({"revoked": false})
    );
    assert_eq!(
        json(revoke("no-such-link").send().await.unwrap()).await,
        json!({"revoked": false})
    );
    // A revoked link cannot be redeemed.
    let (_, redeem) = f
        .browser_session(link["credential"].as_str().unwrap())
        .await;
    assert_eq!(redeem.status(), 401);
    f.stop().await;
}

#[tokio::test]
async fn client_session_revocation() {
    let f = fixture(&[]).await;
    let admin = f.admin_cookie().await;
    let admin_id = session_id(admin.split_once('=').unwrap().1);
    let scopes = ["access:write", "orchestration:read"];
    let mut clients = Vec::new();
    for _ in 0..3 {
        let credential = f.credential(&admin, &scopes).await;
        clients.push(f.bearer(&credential, None).await);
    }
    let server = &f;
    let authenticated = |token: String| async move {
        let response = server
            .get("/api/auth/session")
            .header("authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap();
        json(response).await["authenticated"] == true
    };

    let guarded = [
        ("/api/auth/clients/revoke", json!({"sessionId": "x"})),
        ("/api/auth/clients/revoke-others", json!({})),
        (
            "/api/auth/connect-devices/approve",
            json!({"pairingLinkId": "x"}),
        ),
        (
            "/api/auth/connect-devices/revoke",
            json!({"proofKeyThumbprint": "x"}),
        ),
    ];
    for (path, body) in &guarded {
        let unauthenticated = f.post(path).json(body).send().await.unwrap();
        assert_eq!(unauthenticated.status(), 401, "{path}");
    }
    let read_only = f.credential(&admin, &["orchestration:read"]).await;
    let read_only = f.bearer(&read_only, None).await;
    for (path, body) in guarded {
        let response = f
            .post(path)
            .header("authorization", format!("Bearer {read_only}"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_error(
            response,
            403,
            "EnvironmentScopeRequiredError",
            "insufficient_scope",
        )
        .await;
    }

    // Unknown Connect requests and keys are answered, not refused.
    for (path, body, answer) in [
        (
            "/api/auth/connect-devices/approve",
            json!({"pairingLinkId": "unknown"}),
            json!({"approved": false}),
        ),
        (
            "/api/auth/connect-devices/revoke",
            json!({"proofKeyThumbprint": "unknown"}),
            json!({"revoked": false}),
        ),
    ] {
        let response = f
            .post(path)
            .header("cookie", &admin)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert_no_store(&response);
        assert_eq!(json(response).await, answer, "{path}");
    }

    // A session cannot revoke itself.
    let own = f
        .post("/api/auth/clients/revoke")
        .header("cookie", &admin)
        .json(&json!({"sessionId": admin_id}))
        .send()
        .await
        .unwrap();
    assert_no_store(&own);
    let body = assert_error(
        own,
        403,
        "EnvironmentOperationForbiddenError",
        "operation_forbidden",
    )
    .await;
    assert_eq!(body["reason"], "current_session_revoke_not_allowed");
    assert!(authenticated(clients[0].clone()).await);

    for body in ["not json", "{}", r#"{"sessionId":7}"#] {
        let response = f
            .post("/api/auth/clients/revoke")
            .header("cookie", &admin)
            .header("content-type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{body}");
    }
    let target = session_id(&clients[0]);
    let revoke = |id: &str| {
        f.post("/api/auth/clients/revoke")
            .header("cookie", &admin)
            .json(&json!({"sessionId": id}))
    };
    assert_eq!(
        json(revoke(&target).send().await.unwrap()).await,
        json!({"revoked": true})
    );
    assert_eq!(
        json(revoke(&target).send().await.unwrap()).await,
        json!({"revoked": false})
    );
    assert!(
        !authenticated(clients[0].clone()).await,
        "revocation takes effect immediately"
    );
    assert!(authenticated(clients[1].clone()).await);

    // Revoke-others keeps the caller and ends everyone else, the read-only client included.
    let others = f
        .post("/api/auth/clients/revoke-others")
        .header("cookie", &admin)
        .send()
        .await
        .unwrap();
    assert_eq!(others.status(), 200);
    assert_no_store(&others);
    assert_eq!(
        json(others).await["revokedCount"],
        3,
        "two remaining clients and the read-only one"
    );
    assert!(!authenticated(clients[1].clone()).await);
    assert!(!authenticated(clients[2].clone()).await);
    assert!(!authenticated(read_only).await);
    let session = json(
        f.get("/api/auth/session")
            .header("cookie", &admin)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(session["authenticated"], true);
    f.stop().await;
}

#[cfg(feature = "pull-requests")]
#[tokio::test]
async fn pull_request_diff_requires_a_read_grant_for_bearer_sessions() {
    let f = fixture(&[]).await;
    let admin = f.admin_cookie().await;
    let grant = f.credential(&admin, &["access:read"]).await;
    let bearer = f.bearer(&grant, None).await;
    let denied = f
        .post("/api/pull-requests/diff")
        .bearer_auth(bearer)
        .json(&json!({"projectId":"bound","repository":"owner/repo","number":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    assert_eq!(json(denied).await["requiredScope"], "orchestration:read");
    f.stop().await;
}
