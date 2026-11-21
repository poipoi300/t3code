#![allow(dead_code, clippy::unwrap_used)]
//! Real server on an ephemeral loopback port against a temp home, driven over real HTTP.
use reqwest::{Client, Method, RequestBuilder, Response, redirect::Policy};
use serde_json::{Value, json};
use t3_server::{
    adapters::config::{ConfigInputs, ServerFlags},
    runtime::{RunningServer, start},
};

pub struct Fixture {
    pub server: Option<RunningServer>,
    pub base: String,
    pub http: Client,
    pub temp: tempfile::TempDir,
    startup: String,
}

pub fn inputs(temp: &std::path::Path) -> ConfigInputs {
    ConfigInputs {
        flags: ServerFlags {
            base_dir: Some(temp.to_string_lossy().into_owned()),
            host: Some("127.0.0.1".into()),
            port: Some(0),
            ..Default::default()
        },
        env: Default::default(),
        home: temp.into(),
        process_cwd: temp.into(),
        bootstrap: None,
        headless: true,
        force_auto_bootstrap: Some(false),
        service_managed: false,
        cli_log_level: None,
        automatic_port: None,
        persisted_observability: Default::default(),
        static_dir: Some(temp.join("web")),
    }
}

/// Starts a server whose static directory holds `files` (paths relative to it).
pub async fn fixture(files: &[(&str, &str)]) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let web = temp.path().join("web");
    std::fs::create_dir(&web).unwrap();
    std::fs::write(web.join("index.html"), "<html>t3 client</html>").unwrap();
    for (path, content) in files {
        let target = web.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, content).unwrap();
    }
    let server = start(inputs(temp.path())).await.unwrap();
    Fixture {
        base: format!("http://{}", server.address),
        startup: server.pairing_credential.clone(),
        server: Some(server),
        http: Client::builder().redirect(Policy::none()).build().unwrap(),
        temp,
    }
}

impl Fixture {
    pub async fn stop(mut self) {
        self.server.take().unwrap().shutdown().await.unwrap();
    }
    pub fn startup_credential(&self) -> String {
        self.startup.clone()
    }
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
    pub fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http.request(method, self.url(path))
    }
    pub fn get(&self, path: &str) -> RequestBuilder {
        self.request(Method::GET, path)
    }
    pub fn post(&self, path: &str) -> RequestBuilder {
        self.request(Method::POST, path)
    }
    /// The first browser session, from the single-use startup credential: all standard scopes.
    pub async fn admin_cookie(&self) -> String {
        self.browser_session(&self.startup).await.0
    }
    /// `name=value` of the session cookie, and the full response.
    pub async fn browser_session(&self, credential: &str) -> (String, Response) {
        let response = self
            .post("/api/auth/browser-session")
            .json(&json!({ "credential": credential }))
            .send()
            .await
            .unwrap();
        let cookie = response
            .headers()
            .get("set-cookie")
            .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_owned())
            .unwrap_or_default();
        (cookie, response)
    }
    /// A one-time pairing credential carrying exactly `scopes`.
    pub async fn credential(&self, cookie: &str, scopes: &[&str]) -> String {
        let response = self
            .post("/api/auth/pairing-token")
            .header("cookie", cookie)
            .json(&json!({ "scopes": scopes }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{scopes:?}");
        response.json::<Value>().await.unwrap()["credential"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    /// Exchanges a credential at `/oauth/token` as a form post and returns the bearer token.
    pub async fn bearer(&self, credential: &str, scope: Option<&str>) -> String {
        let response = self.token_exchange(credential, scope).send().await.unwrap();
        assert_eq!(response.status(), 200);
        response.json::<Value>().await.unwrap()["access_token"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    pub fn token_exchange(&self, credential: &str, scope: Option<&str>) -> RequestBuilder {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair(
            "grant_type",
            "urn:ietf:params:oauth:grant-type:token-exchange",
        )
        .append_pair("subject_token", credential)
        .append_pair(
            "subject_token_type",
            "urn:t3:params:oauth:token-type:environment-bootstrap",
        )
        .append_pair(
            "requested_token_type",
            "urn:ietf:params:oauth:token-type:access_token",
        );
        if let Some(scope) = scope {
            form.append_pair("scope", scope);
        }
        self.post("/oauth/token")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form.finish())
    }
}

pub async fn json(response: Response) -> Value {
    response.json().await.unwrap()
}

/// Asserts the status and the `_tag`/`code`/`reason` of an environment error body.
pub async fn assert_error(response: Response, status: u16, tag: &str, code: &str) -> Value {
    assert_eq!(response.status(), status);
    let body = json(response).await;
    assert_eq!(body["_tag"], tag, "{body}");
    assert_eq!(body["code"], code, "{body}");
    assert!(
        body["traceId"].as_str().is_some_and(|t| !t.is_empty()),
        "{body}"
    );
    body
}

pub fn assert_no_store(response: &Response) {
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["pragma"], "no-cache");
}

/// A single approved unthreaded OAuth client for MCP behavior tests.
#[cfg(all(feature = "mcp", feature = "workspace"))]
pub async fn approved_mcp_access(http: &Client, origin: &str, admin: &str) -> String {
    let redirect_uri = "http://127.0.0.1:30999/callback";
    let registered = json(
        http.post(format!("{origin}/oauth/mcp/register"))
            .json(&json!({
                "client_name": "Attachment test agent",
                "redirect_uris": [redirect_uri],
                "token_endpoint_auth_method": "none",
            }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap(),
    )
    .await;
    let verifier = "v".repeat(60);
    let resource = format!("{origin}/mcp");
    let client_id = registered["client_id"].as_str().unwrap();
    let authorization = json!({
        "response_type": "code",
        "client_id": client_id,
        "redirect_uri": redirect_uri,
        "code_challenge": t3_auth::utils::b64(t3_auth::utils::sha256(&verifier)),
        "code_challenge_method": "S256",
        "state": "attachment-test",
        "resource": resource,
    });
    let approval = json(
        http.post(format!("{origin}/oauth/mcp/approval"))
            .header("cookie", admin)
            .json(&authorization)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap(),
    )
    .await;
    let accepted = json(http
        .post(format!("{origin}/oauth/mcp/decision"))
        .header("cookie", admin)
        .json(&json!({
            "authorization": authorization,
            "decision": {"_tag":"browser-session","access":"approval-required","csrfToken":approval["csrfToken"]},
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()).await;
    let redirect = reqwest::Url::parse(accepted["redirectTo"].as_str().unwrap()).unwrap();
    let code = redirect
        .query_pairs()
        .find(|(name, _)| name == "code")
        .unwrap()
        .1;
    let token = json(
        http.post(format!("{origin}/oauth/mcp/token"))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code.as_ref()),
                ("redirect_uri", redirect_uri),
                ("client_id", client_id),
                ("code_verifier", verifier.as_str()),
                ("resource", resource.as_str()),
            ])
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap(),
    )
    .await;
    token["access_token"].as_str().unwrap().to_owned()
}
