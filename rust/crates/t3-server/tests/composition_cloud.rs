//! T3 Connect through the server's own HTTP routes (`reviews/t3-auth-connect-review.md`): with
//! device approval on, a relay-signed mint for an untrusted key yields a pending request whose
//! credential cannot bootstrap a session, while the relay still receives a signed answer.
#![cfg(all(feature = "local-services", feature = "cloud"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use p256::ecdsa::signature::Signer;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_server::adapters::config::{ConfigInputs, ServerFlags};

/// The relay is never contacted while minting; any request is a test failure.
struct NoNetwork;
#[async_trait::async_trait]
impl t3_cloud::http::Transport for NoNetwork {
    async fn send(&self, _: t3_cloud::http::Request) -> t3_cloud::Result<t3_cloud::http::Response> {
        panic!("unexpected network request")
    }
}

fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A real P-256 proof-of-possession key: its RFC 7638 thumbprint and a DPoP proof for a URL.
struct DeviceKey(p256::ecdsa::SigningKey);
impl DeviceKey {
    fn jwk(&self) -> t3_auth::dpop::PublicJwk {
        let point = self.0.verifying_key().to_encoded_point(false);
        t3_auth::dpop::PublicJwk {
            kty: "EC".into(),
            crv: "P-256".into(),
            x: t3_auth::utils::b64(point.x().unwrap()),
            y: t3_auth::utils::b64(point.y().unwrap()),
        }
    }
    fn thumbprint(&self) -> String {
        t3_auth::dpop::thumbprint(&self.jwk())
    }
    fn proof(&self, url: &str) -> String {
        let header = t3_auth::utils::b64(
            serde_json::to_vec(&json!({"typ":"dpop+jwt","alg":"ES256","jwk":self.jwk()})).unwrap(),
        );
        let claims = t3_auth::utils::b64(
            serde_json::to_vec(&json!({"htm":"POST","htu":url,"iat":now_s(),"jti":uuid::Uuid::new_v4().to_string()}))
                .unwrap(),
        );
        let input = format!("{header}.{claims}");
        let signature: p256::ecdsa::Signature = self.0.sign(input.as_bytes());
        let signature = signature.normalize_s().unwrap_or(signature);
        format!("{input}.{}", t3_auth::utils::b64(signature.to_bytes()))
    }
}

#[tokio::test]
async fn with_approval_on_an_untrusted_mint_is_pending_and_its_credential_unusable() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    std::fs::create_dir_all(home.join("userdata")).unwrap();
    std::fs::write(
        home.join("userdata/settings.json"),
        r#"{"requireConnectDeviceApproval":true}"#,
    )
    .unwrap();
    let server = t3_server::runtime::start(ConfigInputs {
        flags: ServerFlags {
            base_dir: Some(home.to_string_lossy().into_owned()),
            host: Some("127.0.0.1".into()),
            port: Some(0),
            ..Default::default()
        },
        env: Default::default(),
        home: home.into(),
        process_cwd: home.into(),
        bootstrap: None,
        headless: true,
        force_auto_bootstrap: Some(false),
        service_managed: false,
        cli_log_level: None,
        automatic_port: None,
        persisted_observability: Default::default(),
        static_dir: None,
    })
    .await
    .unwrap();
    let origin = format!("http://{}", server.address);
    let http = reqwest::Client::new();
    let descriptor: t3_contracts::environment::ExecutionEnvironmentDescriptor = http
        .get(format!("{origin}/.well-known/t3/environment"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let environment = descriptor.environment_id.0.clone();

    // Link this environment to a relay whose mint key the test holds (what `relay-config`
    // stores), through the same secret store the server reads on every mint.
    let secrets: t3_cloud::store::SecretStore = Arc::new(
        t3_auth::ServerSecretStore::open(home.join("userdata/secrets"))
            .await
            .unwrap(),
    );
    let relay_key = t3_cloud::crypto::EnvironmentKey::generate().unwrap();
    let link = t3_cloud::cloud::CloudLink::new(
        descriptor,
        secrets,
        Arc::new(NoNetwork),
        Arc::new(t3_cloud::SystemClock),
    )
    .await
    .unwrap();
    link.install(t3_cloud::http::Sensitive::new(
        serde_json::from_value(json!({
            "relayUrl": "https://relay.test", "relayIssuer": "https://issuer.test",
            "cloudUserId": "user-one", "environmentCredential": "marker-environment-secret",
            "cloudMintPublicKey": relay_key.public_key,
            "endpointRuntime": {"providerKind":"cloudflare_tunnel","connectorToken":"marker-connector-secret","tunnelId":"tunnel-1","tunnelName":"t3-test"}
        }))
        .unwrap(),
    ))
    .await
    .unwrap();

    let device = DeviceKey(p256::ecdsa::SigningKey::from_slice(&[7u8; 32]).unwrap());
    let key = device.thumbprint();
    let unique = uuid::Uuid::new_v4().to_string();
    let proof = relay_key
        .sign(
            t3_cloud::crypto::MINT_REQUEST,
            &json!({
                "iss":"https://issuer.test","aud":format!("t3-env:{environment}"),"sub":"user-one",
                "jti":unique,"nonce":unique,"iat":now_s(),"exp":now_s()+120,
                "environmentId":environment,"scope":["environment:connect"],
                "clientProofKeyThumbprint":key,"cnf":{"jkt":key}
            }),
        )
        .unwrap();
    let minted = http
        .post(format!("{origin}/api/t3-connect/mint-credential"))
        .json(&json!({"proof": proof.expose()}))
        .send()
        .await
        .unwrap();
    assert_eq!(minted.status(), 200);
    assert_eq!(minted.headers()["cache-control"], "no-store");
    let minted: Value = minted.json().await.unwrap();
    let answer: t3_contracts::relay::RelayEnvironmentMintResponseProofPayload =
        t3_cloud::crypto::verify(
            &link.signing_key.public_key,
            &t3_cloud::Secret::new(minted["proof"].as_str().unwrap()),
            t3_cloud::crypto::MINT_RESPONSE,
            &format!("t3-env:{environment}"),
            "https://issuer.test",
            now_s(),
        )
        .unwrap();
    assert_eq!(answer.client_proof_key_thumbprint.0, key);
    assert_eq!(answer.credential.0, minted["credential"].as_str().unwrap());
    let credential = answer.credential.0.clone();

    let exchange = |dpop: Option<String>| {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair(
            "grant_type",
            "urn:ietf:params:oauth:grant-type:token-exchange",
        )
        .append_pair("subject_token", &credential)
        .append_pair(
            "subject_token_type",
            "urn:t3:params:oauth:token-type:environment-bootstrap",
        )
        .append_pair(
            "requested_token_type",
            "urn:ietf:params:oauth:token-type:access_token",
        );
        let mut request = http
            .post(format!("{origin}/oauth/token"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form.finish());
        if let Some(dpop) = dpop {
            request = request.header("dpop", dpop);
        }
        request.send()
    };
    // The device's own proof reaches the approval gate: pending, not a credential failure.
    let pending = exchange(Some(device.proof(&format!("{origin}/oauth/token"))))
        .await
        .unwrap();
    assert_eq!(pending.status(), 403);
    let body: Value = pending.json().await.unwrap();
    assert_eq!(
        body["_tag"], "EnvironmentConnectApprovalPendingError",
        "{body}"
    );
    assert_eq!(body["code"], "connect_approval_pending");
    // Without the bound key's proof the credential is refused outright.
    assert_eq!(exchange(None).await.unwrap().status(), 401);
    // A different key cannot use it either.
    let other = DeviceKey(p256::ecdsa::SigningKey::from_slice(&[8u8; 32]).unwrap());
    let stolen = exchange(Some(other.proof(&format!("{origin}/oauth/token"))))
        .await
        .unwrap();
    assert_ne!(stolen.status(), 200);
    server.shutdown().await.unwrap();
}
