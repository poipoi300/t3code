#![allow(dead_code, clippy::unwrap_used)]
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};
use t3_auth::{
    AuthConfig, EnvironmentAuth, Request, ServerSecretStore, clock::Clock, session::IssueSession,
};
use t3_contracts::{auth::*, base_schemas::TrimmedNonEmptyString};
use t3_persistence::Database;
use tokio::sync::watch;
pub const NOW: i64 = 1_767_225_600_000;
pub struct TestClock {
    now: AtomicI64,
    changed: watch::Sender<i64>,
    pub waiting: watch::Sender<Option<i64>>,
}
impl TestClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            now: AtomicI64::new(NOW),
            changed: watch::channel(NOW).0,
            waiting: watch::channel(None).0,
        })
    }
    pub fn advance(&self, ms: i64) {
        self.set(self.now_ms() + ms);
    }
    pub fn set(&self, ms: i64) {
        self.now.store(ms, Ordering::SeqCst);
        self.changed.send_replace(ms);
    }
}
impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }
    fn wait_until(&self, deadline: i64) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.waiting.send_replace(Some(deadline));
        Box::pin(async move {
            let mut rx = self.changed.subscribe();
            while self.now_ms() < deadline {
                if rx.changed().await.is_err() {
                    return;
                }
            }
        })
    }
}
pub struct Harness {
    pub auth: EnvironmentAuth,
    pub db: Database,
    pub clock: Arc<TestClock>,
    pub temp: tempfile::TempDir,
}
impl Harness {
    pub async fn new() -> Self {
        Self::config(AuthConfig::default()).await
    }
    pub async fn config(mut config: AuthConfig) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.sqlite");
        let db = tokio::task::spawn_blocking(move || Database::open(path))
            .await
            .unwrap()
            .unwrap();
        config.state_dir = temp.path().to_string_lossy().into_owned();
        config.environment_id = if config.environment_id.is_empty() {
            "test-environment".into()
        } else {
            config.environment_id
        };
        config.port = if config.port == 0 { 3773 } else { config.port };
        let secrets = ServerSecretStore::open(temp.path().join("secrets"))
            .await
            .unwrap();
        let clock = TestClock::new();
        let auth = EnvironmentAuth::new(db.clone(), secrets, config, None, clock.clone())
            .await
            .unwrap();
        Self {
            auth,
            db,
            clock,
            temp,
        }
    }
    pub async fn reopen(&self, config: AuthConfig) -> EnvironmentAuth {
        EnvironmentAuth::new(
            self.db.clone(),
            self.auth.secrets.clone(),
            config,
            None,
            self.clock.clone(),
        )
        .await
        .unwrap()
    }
    pub async fn issue(&self, subject: &str) -> t3_auth::session::IssuedSession {
        self.auth
            .sessions
            .issue(IssueSession {
                subject: Some(subject.into()),
                ..Default::default()
            })
            .await
            .unwrap()
    }
}
pub fn metadata() -> AuthClientMetadata {
    serde_json::from_value(serde_json::json!({"label":"Test laptop","deviceType":"desktop","os":"macOS","browser":"Chrome","ipAddress":"192.168.1.23"})).unwrap()
}
pub fn token_request(token: &str) -> Request {
    Request {
        method: "GET".into(),
        url: "/api/auth/session".into(),
        headers: [
            ("host".into(), "192.168.1.50:3773".into()),
            ("authorization".into(), format!("Bearer {token}")),
        ]
        .into(),
        ..Default::default()
    }
}
pub fn cookie_request(auth: &EnvironmentAuth, token: &str) -> Request {
    Request {
        method: "GET".into(),
        url: "/ws".into(),
        headers: [("host".into(), "192.168.1.50:3773".into())].into(),
        cookies: [(auth.sessions.cookie_name(), token.into())].into(),
        ..Default::default()
    }
}
pub fn credential_failure<T>(
    result: t3_auth::Result<T>,
    expected: t3_auth::error::CredentialFailure,
) {
    match result {
        Err(t3_auth::Error::Credential { kind, .. }) => assert_eq!(kind, expected),
        Err(e) => panic!("wrong error {e:?}"),
        Ok(_) => panic!("unexpected success"),
    }
}
pub fn pairing_input(
    scopes: Option<Vec<AuthEnvironmentScope>>,
    access: Option<AuthMcpClientAccess>,
) -> AuthCreatePairingCredentialInput {
    AuthCreatePairingCredentialInput {
        label: Some(TrimmedNonEmptyString("phone".into())),
        scopes: scopes.map(|s| {
            s.into_iter()
                .map(|s| serde_json::from_value(serde_json::to_value(s).unwrap()).unwrap())
                .collect()
        }),
        access,
    }
}

pub fn bootstrap_thumbprint() -> String {
    let (_, jwk) = bootstrap_key();
    t3_auth::dpop::thumbprint(&jwk)
}
fn bootstrap_key() -> (p256::ecdsa::SigningKey, t3_auth::dpop::PublicJwk) {
    let signer = p256::ecdsa::SigningKey::from_slice(&[1u8; 32]).unwrap();
    let point = signer.verifying_key().to_encoded_point(false);
    let jwk = t3_auth::dpop::PublicJwk {
        kty: "EC".into(),
        crv: "P-256".into(),
        x: t3_auth::utils::b64(point.x().unwrap()),
        y: t3_auth::utils::b64(point.y().unwrap()),
    };
    (signer, jwk)
}
pub fn bootstrap_request(key: Option<String>) -> Request {
    let mut request = Request {
        method: "POST".into(),
        url: "https://example.test/oauth/token".into(),
        ..Default::default()
    };
    if key.is_some() {
        use p256::ecdsa::signature::Signer;
        let (signer, jwk) = bootstrap_key();
        let header = t3_auth::utils::b64(
            serde_json::to_vec(&serde_json::json!({"typ":"dpop+jwt","alg":"ES256","jwk":jwk}))
                .unwrap(),
        );
        let payload = t3_auth::utils::b64(serde_json::to_vec(&serde_json::json!({"htm":"POST","htu":request.url,"iat":NOW/1000,"jti":t3_auth::utils::random_uuid().unwrap()})).unwrap());
        let body = format!("{header}.{payload}");
        let signature: p256::ecdsa::Signature = signer.sign(body.as_bytes());
        request.headers.insert(
            "DPoP".into(),
            format!("{body}.{}", t3_auth::utils::b64(signature.to_bytes())),
        );
    }
    request
}
