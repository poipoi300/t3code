#![cfg(feature = "local-services")]
#![allow(clippy::unwrap_used)]
//! MCP OAuth over real HTTP: `apps/server/src/auth/mcpOAuthHttp.ts` and `McpOAuth.ts`.
mod http_common;
use http_common::*;
use serde_json::{Value, json};

const REDIRECT: &str = "http://127.0.0.1:8765/callback";

struct Client {
    id: String,
    verifier: String,
    challenge: String,
    resource: String,
}
impl Client {
    fn authorization(&self) -> Value {
        json!({
            "response_type": "code",
            "client_id": self.id,
            "redirect_uri": REDIRECT,
            "code_challenge": self.challenge,
            "code_challenge_method": "S256",
            "state": "xyz",
            "resource": self.resource,
        })
    }
    fn query(&self) -> String {
        query_of(&self.authorization())
    }
}

fn query_of(request: &Value) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(
            request
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str().unwrap())),
        )
        .finish()
}

async fn register(f: &Fixture) -> Client {
    let response = f
        .post("/oauth/mcp/register")
        .json(&json!({
            "client_name": "Test Agent",
            "redirect_uris": [REDIRECT],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_no_store(&response);
    let body = json(response).await;
    assert_eq!(body["client_name"], "Test Agent");
    assert_eq!(body["grant_types"], json!(["authorization_code"]));
    assert_eq!(body["response_types"], json!(["code"]));
    assert_eq!(body["token_endpoint_auth_method"], "none");
    let verifier = "v".repeat(60);
    Client {
        id: body["client_id"].as_str().unwrap().to_owned(),
        challenge: t3_auth::utils::b64(t3_auth::utils::sha256(&verifier)),
        verifier,
        resource: format!("{}/mcp", f.base),
    }
}

fn code_of(redirect_to: &str) -> String {
    let url = url::Url::parse(redirect_to).unwrap();
    url.query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap_or_else(|| panic!("no code in {redirect_to}"))
        .1
        .into_owned()
}

async fn decide(
    f: &Fixture,
    cookie: Option<&str>,
    client: &Client,
    decision: Value,
) -> reqwest::Response {
    let mut request = f
        .post("/oauth/mcp/decision")
        .json(&json!({"authorization": client.authorization(), "decision": decision}));
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    request.send().await.unwrap()
}

async fn csrf(f: &Fixture, cookie: &str, client: &Client) -> String {
    let details = json(
        f.post("/oauth/mcp/approval")
            .header("cookie", cookie)
            .json(&client.authorization())
            .send()
            .await
            .unwrap(),
    )
    .await;
    details["csrfToken"].as_str().unwrap().to_owned()
}

fn token_request(f: &Fixture, client: &Client, fields: &[(&str, &str)]) -> reqwest::RequestBuilder {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in fields {
        form.append_pair(k, v);
    }
    let _ = client;
    f.post("/oauth/mcp/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form.finish())
}

fn code_fields<'a>(client: &'a Client, code: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", REDIRECT),
        ("client_id", &client.id),
        ("code_verifier", &client.verifier),
        ("resource", &client.resource),
    ]
}

#[tokio::test]
async fn discovery_documents_follow_the_request_origin() {
    let f = fixture(&[]).await;
    let server = json(
        f.get("/.well-known/oauth-authorization-server")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(server["issuer"], f.base);
    assert_eq!(
        server["authorization_endpoint"],
        format!("{}/oauth/mcp/authorize", f.base)
    );
    assert_eq!(
        server["token_endpoint"],
        format!("{}/oauth/mcp/token", f.base)
    );
    assert_eq!(
        server["registration_endpoint"],
        format!("{}/oauth/mcp/register", f.base)
    );
    assert_eq!(server["code_challenge_methods_supported"], json!(["S256"]));
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
    ] {
        let resource = json(f.get(path).send().await.unwrap()).await;
        assert_eq!(resource["resource"], format!("{}/mcp", f.base), "{path}");
        assert_eq!(resource["authorization_servers"], json!([f.base]), "{path}");
    }
    let proxied = json(
        f.get("/.well-known/oauth-authorization-server")
            .header("x-forwarded-proto", "https")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(proxied["issuer"], f.base.replacen("http://", "https://", 1));
    f.stop().await;
}

#[tokio::test]
async fn registration_validates_redirects() {
    let f = fixture(&[]).await;
    let register = |body: Value| f.post("/oauth/mcp/register").json(&body).send();
    for (body, code) in [
        (json!({"client_name": "x"}), "invalid_redirect_uri"),
        (json!({"redirect_uris": []}), "invalid_redirect_uri"),
        (
            json!({"redirect_uris": ["http://evil.example/cb"]}),
            "invalid_redirect_uri",
        ),
        (
            json!({"redirect_uris": ["https://ok.example/cb#frag"]}),
            "invalid_redirect_uri",
        ),
        (
            json!({"redirect_uris": ["https://user@ok.example/cb"]}),
            "invalid_redirect_uri",
        ),
        (
            json!({"redirect_uris": ["https://a.example/1", "https://a.example/2", "https://a.example/3", "https://a.example/4", "https://a.example/5", "https://a.example/6"]}),
            "invalid_redirect_uri",
        ),
        (
            json!({"redirect_uris": [REDIRECT], "software_id": "t3code-peer-link"}),
            "invalid_client_metadata",
        ),
    ] {
        let response = register(body.clone()).await.unwrap();
        assert_eq!(response.status(), 400, "{body}");
        let error = json(response).await;
        assert_eq!(error["error"], code, "{body}");
        assert!(
            error["error_description"]
                .as_str()
                .is_some_and(|d| !d.is_empty())
        );
    }
    let hosted = register(json!({"redirect_uris": ["https://app.example/cb"]}))
        .await
        .unwrap();
    assert_eq!(hosted.status(), 200);
    assert_eq!(json(hosted).await["client_name"], "MCP client");
    for body in ["not json", "[]", r#"{"redirect_uris":"x"}"#] {
        let response = f
            .post("/oauth/mcp/register")
            .header("content-type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{body}");
    }
    f.stop().await;
}

#[tokio::test]
async fn authorize_reports_unverified_requests_and_redirects_the_rest() {
    let f = fixture(&[]).await;
    let client = register(&f).await;
    let valid = f
        .get(&format!("/oauth/mcp/authorize?{}", client.query()))
        .send()
        .await
        .unwrap();
    assert_eq!(valid.status(), 302);
    assert_eq!(valid.headers()["cache-control"], "no-store");
    assert_eq!(valid.headers()["referrer-policy"], "no-referrer");
    assert_eq!(
        valid.headers()["location"].to_str().unwrap(),
        format!("/connect-agent?{}", client.query())
    );

    // An unknown client or unregistered redirect is shown, never redirected to.
    for query in [
        client.query().replace(&client.id, "bogus"),
        client.query().replace("127.0.0.1%3A8765", "evil.example"),
        String::new(),
    ] {
        let response = f
            .get(&format!("/oauth/mcp/authorize?{query}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{query}");
        let headers = response.headers().clone();
        assert!(
            headers["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        assert_eq!(
            headers["content-security-policy"],
            "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'"
        );
        assert_eq!(headers["x-frame-options"], "DENY");
        assert_eq!(headers["cache-control"], "no-store");
        assert!(headers.get("location").is_none());
        assert!(
            response
                .text()
                .await
                .unwrap()
                .contains("This sign-in cannot continue")
        );
    }

    // A verified client gets other problems back through its own redirect URI.
    for (field, value, error) in [
        ("response_type", "token", "unsupported_response_type"),
        ("code_challenge_method", "plain", "invalid_request"),
        ("code_challenge", "short", "invalid_request"),
        (
            "resource",
            "https://elsewhere.example/mcp",
            "invalid_target",
        ),
    ] {
        let mut request = client.authorization();
        request[field] = json!(value);
        let response = f
            .get(&format!("/oauth/mcp/authorize?{}", query_of(&request)))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 302, "{field}");
        let location = url::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
        assert_eq!(
            location.origin().ascii_serialization(),
            "http://127.0.0.1:8765"
        );
        assert_eq!(location.path(), "/callback");
        let pairs: std::collections::HashMap<_, _> = location.query_pairs().into_owned().collect();
        assert_eq!(pairs["error"], error, "{field}");
        assert_eq!(pairs["state"], "xyz");
        assert_eq!(pairs["iss"], f.base);
    }
    f.stop().await;
}

#[tokio::test]
async fn approval_details_and_decisions_are_bound_to_the_approving_session() {
    let f = fixture(&[]).await;
    let client = register(&f).await;
    let admin = f.admin_cookie().await;

    let anonymous = f
        .post("/oauth/mcp/approval")
        .json(&client.authorization())
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 200);
    assert_no_store(&anonymous);
    let details = json(anonymous).await;
    assert_eq!(details["clientName"], "Test Agent");
    assert_eq!(details["redirectHost"], "127.0.0.1:8765");
    assert_eq!(
        details["environmentHost"],
        f.base.trim_start_matches("http://")
    );
    assert!(details.get("csrfToken").is_none() && details.get("oneClickAccess").is_none());
    let details = json(
        f.post("/oauth/mcp/approval")
            .header("cookie", &admin)
            .json(&client.authorization())
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(details["csrfToken"].as_str().is_some_and(|t| !t.is_empty()));
    assert_eq!(
        details["oneClickAccess"],
        json!([
            "read-only",
            "approval-required",
            "auto-accept-edits",
            "auto",
            "full-access"
        ])
    );
    // Cookies from another origin do not count as a session here.
    let foreign = json(
        f.post("/oauth/mcp/approval")
            .header("cookie", &admin)
            .header("origin", "https://evil.example")
            .json(&client.authorization())
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(foreign.get("csrfToken").is_none());

    // Problems after the client is verified travel back through the redirect.
    let mut bad = client.authorization();
    bad["response_type"] = json!("token");
    let redirected = json(
        f.post("/oauth/mcp/approval")
            .json(&bad)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(
        redirected["redirectTo"]
            .as_str()
            .unwrap()
            .contains("error=unsupported_response_type")
    );
    let mut unknown = client.authorization();
    unknown["client_id"] = json!("bogus");
    let response = f
        .post("/oauth/mcp/approval")
        .json(&unknown)
        .send()
        .await
        .unwrap();
    let error = assert_status_body(response, 400).await;
    assert_eq!(error["_tag"], "AuthMcpApprovalError");
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|m| m.contains("unknown app"))
    );
    for body in ["not json", "[]"] {
        let response = f
            .post("/oauth/mcp/approval")
            .header("content-type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
    }

    let denied = json(decide(&f, None, &client, json!({"_tag": "deny"})).await).await;
    let denied = url::Url::parse(denied["redirectTo"].as_str().unwrap()).unwrap();
    let pairs: std::collections::HashMap<_, _> = denied.query_pairs().into_owned().collect();
    assert_eq!(
        (pairs["error"].as_str(), pairs["state"].as_str()),
        ("access_denied", "xyz")
    );

    // The browser-session decision needs the CSRF token minted for this exact request.
    let token = csrf(&f, &admin, &client).await;
    let approve =
        |token: &str| json!({"_tag": "browser-session", "access": "read-only", "csrfToken": token});
    for (cookie, decision) in [
        (Some(admin.as_str()), approve("wrong")),
        (Some(admin.as_str()), approve("")),
        (None, approve(&token)),
    ] {
        let response = decide(&f, cookie, &client, decision).await;
        let error = assert_status_body(response, 400).await;
        assert_eq!(error["_tag"], "AuthMcpApprovalError");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("cannot approve")
        );
    }
    let other_request = Client {
        id: client.id.clone(),
        verifier: client.verifier.clone(),
        challenge: t3_auth::utils::b64(t3_auth::utils::sha256(
            "another-verifier-another-verifier-another",
        )),
        resource: client.resource.clone(),
    };
    let stolen = decide(&f, Some(&admin), &other_request, approve(&token)).await;
    assert_eq!(
        stolen.status(),
        400,
        "a token for one challenge does not approve another"
    );
    let foreign = f
        .post("/oauth/mcp/decision")
        .header("cookie", &admin)
        .header("origin", "https://evil.example")
        .json(&json!({"authorization": client.authorization(), "decision": approve(&token)}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        foreign.status(),
        400,
        "cross-origin approval with the user's cookie"
    );

    // A session that only holds read scopes cannot grant more.
    let credential = f
        .credential(&admin, &["access:write", "orchestration:read"])
        .await;
    let narrow = f.bearer(&credential, None).await;
    let narrow_details = json(
        f.post("/oauth/mcp/approval")
            .header("authorization", format!("Bearer {narrow}"))
            .json(&client.authorization())
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(narrow_details["oneClickAccess"], json!(["read-only"]));
    let escalate = f
        .post("/oauth/mcp/decision")
        .header("authorization", format!("Bearer {narrow}"))
        .json(&json!({"authorization": client.authorization(), "decision": {"_tag": "browser-session", "access": "full-access", "csrfToken": narrow_details["csrfToken"]}}))
        .send()
        .await
        .unwrap();
    assert_eq!(escalate.status(), 400);

    // A pairing code approves once, for the access it was created for.
    let code = json(
        f.post("/api/auth/pairing-token")
            .header("cookie", &admin)
            .json(&json!({"access": "approval-required"}))
            .send()
            .await
            .unwrap(),
    )
    .await["credential"]
        .as_str()
        .unwrap()
        .to_owned();
    let by_code = json!({"_tag": "pairing-code", "access": "approval-required", "code": code});
    let approved = decide(&f, None, &client, by_code.clone()).await;
    assert_eq!(approved.status(), 200);
    assert_no_store(&approved);
    let redirect_to = json(approved).await["redirectTo"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(redirect_to.starts_with(REDIRECT) && redirect_to.contains("state=xyz"));
    let _ = code_of(&redirect_to);
    let reused = decide(&f, None, &client, by_code).await;
    let error = assert_status_body(reused, 400).await;
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("unknown, expired, or already used")
    );
    f.stop().await;
}

async fn assert_status_body(response: reqwest::Response, status: u16) -> Value {
    assert_eq!(response.status(), status);
    json(response).await
}

#[tokio::test]
async fn token_endpoint_pkce_single_use_codes_and_mcp_only_tokens() {
    let f = fixture(&[]).await;
    let client = register(&f).await;
    let admin = f.admin_cookie().await;
    let approve = async |access: &str| {
        let token = csrf(&f, &admin, &client).await;
        let response = decide(
            &f,
            Some(&admin),
            &client,
            json!({"_tag": "browser-session", "access": access, "csrfToken": token}),
        )
        .await;
        assert_eq!(response.status(), 200);
        code_of(json(response).await["redirectTo"].as_str().unwrap())
    };

    let code = approve("approval-required").await;
    let response = token_request(&f, &client, &code_fields(&client, &code))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_no_store(&response);
    let token = json(response).await;
    assert_eq!(token["token_type"], "Bearer");
    assert_eq!(token["scope"], "orchestration:read orchestration:operate");
    assert!(token["expires_in"].as_u64().unwrap() > 0);
    let access = token["access_token"].as_str().unwrap().to_owned();
    let reuse = token_request(&f, &client, &code_fields(&client, &code))
        .send()
        .await
        .unwrap();
    assert_eq!(oauth_error(reuse).await, "invalid_grant");

    // An MCP token only works on the MCP endpoint, never on the environment API.
    let session = json(
        f.get("/api/auth/session")
            .header("authorization", format!("Bearer {access}"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(session["authenticated"], false);
    let ticket = f
        .post("/api/auth/websocket-ticket")
        .header("authorization", format!("Bearer {access}"))
        .send()
        .await
        .unwrap();
    assert_eq!(ticket.status(), 401);

    // Every failed check spends the code; the right verifier cannot rescue it afterwards.
    let code = approve("read-only").await;
    let wrong_verifier = "w".repeat(60);
    let mut wrong = code_fields(&client, &code);
    wrong[4] = ("code_verifier", &wrong_verifier);
    let failed = token_request(&f, &client, &wrong).send().await.unwrap();
    assert_eq!(oauth_error(failed).await, "invalid_grant");
    let retry = token_request(&f, &client, &code_fields(&client, &code))
        .send()
        .await
        .unwrap();
    assert_eq!(oauth_error(retry).await, "invalid_grant");

    for (index, value) in [
        (2, "http://127.0.0.1:8765/other"),
        (5, "https://elsewhere.example/mcp"),
        (4, "too-short"),
    ] {
        let code = approve("read-only").await;
        let mut fields = code_fields(&client, &code);
        fields[index].1 = value;
        let response = token_request(&f, &client, &fields).send().await.unwrap();
        assert_eq!(oauth_error(response).await, "invalid_grant", "{value}");
    }

    let code = approve("read-only").await;
    let json_exchange = f
        .post("/oauth/mcp/token")
        .json(&json!({"grant_type": "authorization_code", "code": code, "redirect_uri": REDIRECT, "client_id": client.id, "code_verifier": client.verifier}))
        .send()
        .await
        .unwrap();
    assert_eq!(json(json_exchange).await["scope"], "orchestration:read");

    let code = approve("read-only").await;
    for (fields, error) in [
        (
            vec![("grant_type", "password"), ("code", code.as_str())],
            "unsupported_grant_type",
        ),
        (
            vec![("grant_type", "authorization_code")],
            "invalid_request",
        ),
        (
            vec![
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", REDIRECT),
                ("client_id", "bogus"),
                ("code_verifier", &client.verifier),
            ],
            "invalid_client",
        ),
    ] {
        let response = token_request(&f, &client, &fields).send().await.unwrap();
        assert_eq!(oauth_error(response).await, error);
    }
    // Rejected before reading the code, so the earlier checks did not spend it.
    let still_valid = token_request(&f, &client, &code_fields(&client, &code))
        .send()
        .await
        .unwrap();
    assert_eq!(still_valid.status(), 200);

    let response = f
        .post("/oauth/mcp/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body("code=a&code=b".to_owned())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        f.post("/mcp").send().await.unwrap().status(),
        404,
        "the MCP endpoint is not ported"
    );
    f.stop().await;
}

async fn oauth_error(response: reqwest::Response) -> String {
    assert_eq!(response.status(), 400);
    assert_no_store(&response);
    let body = json(response).await;
    assert!(
        body["error_description"]
            .as_str()
            .is_some_and(|d| !d.is_empty())
    );
    body["error"].as_str().unwrap().to_owned()
}
